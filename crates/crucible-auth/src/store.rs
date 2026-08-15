//! The file, and what reading it answers.
//!
//! One file for every provider rather than one each, because a directory
//! listing of `openai.json` beside `moonshot.json` says which providers this
//! user has logged in to without anybody being able to read either.
//!
//! Reading is the half that has to survive anything: a launch reads the store
//! before it knows whether it needs one, so every shape the file could be in —
//! absent, truncated, half-written, written by a version that does not exist
//! yet — resolves to a list of keys and at most one sentence, never to a stop.
//!
//! Writing is the same three steps every time: take the lock, read what is
//! there, rename a sibling temporary over the target. The lock is what stops
//! two crucibles logging in at once from each writing a file that has forgotten
//! the other's provider; the rename is what stops a full disk leaving half a
//! file where a whole one was.

mod document;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use crucible_core::ApiKey;

use crate::error::AuthError;
use crate::renewable::Tokens;

use self::document::Document;

/// What the file is called, inside the home directory.
const FILE: &str = "auth.json";

/// The greatest store this process will hold while parsing.
///
/// An auth store is a small map of provider names to keys. Sixty-four KiB is
/// hundreds of ordinary credentials and keeps a planted file from choosing
/// the launch's allocation.
const MAX_STORE: usize = 64 * 1024;

/// The lock beside it, and the temporary the write renames from.
///
/// Both are siblings of the file itself. The temporary has to be, or `rename`
/// is a cross-device copy that is not atomic and fails with `EXDEV` on the
/// machines where it matters — which is the failure "write it to the system
/// temporary directory" has every time.
const LOCK: &str = "auth.lock";
const PARTIAL: &str = "auth.json.new";

/// How long to wait for another crucible to finish writing.
///
/// The thing being waited for is not one rename. Nothing queues here — every
/// waiter wakes on the same pause and takes whatever it finds — so what a
/// crucible waits out is every other one that keeps winning ahead of it, each
/// syncing a few hundred bytes to a disk somebody else is using too. A budget
/// sized for the rename alone turns an ordinary handful of terminals into a
/// refusal, and a refusal here is a login nobody wrote down.
///
/// Five seconds, then: long enough for that queue on a machine under load,
/// short enough that a crucible which died holding the lock is a sentence
/// telling them to try again rather than something that looks like a hang.
const ATTEMPTS: u32 = 250;
const PAUSE: std::time::Duration = std::time::Duration::from_millis(20);

/// What this version of crucible writes, and the highest it can read.
///
/// A number rather than a guess at the shape, so a file from a version that
/// does not exist yet is a case this one can recognise and decline instead of
/// a parse failure it would report as damage.
const VERSION: u64 = 2;

/// Where the keys crucible was given are written down.
#[derive(Debug, Clone)]
pub struct Store {
    /// The file itself.
    path: PathBuf,
    /// The directory it lives in, which may not exist yet.
    home: PathBuf,
}

impl Store {
    /// The store inside `home`, whether or not anything is there yet.
    ///
    /// A path rather than a lookup: `crucible_config::Home` is the one place
    /// that answers where crucible's files are, and a second answer here is a
    /// bug that only shows up on somebody else's machine.
    #[must_use]
    pub fn in_home(home: &Path) -> Self {
        Self {
            path: home.join(FILE),
            home: home.to_path_buf(),
        }
    }

    /// Every key the store holds.
    ///
    /// Infallible on purpose. Absent, unreadable, or written by a version that
    /// does not exist yet all mean the same thing to a launch — no stored
    /// credential is available — and a usable environment key can still serve
    /// it. What could not be done comes back in
    /// [`StoredCredentials::trouble`] for the user to be told once.
    #[must_use]
    pub fn read(&self) -> StoredCredentials {
        let secured = match self.secure_existing() {
            Ok(Some(secured)) => secured,
            Ok(None) => return StoredCredentials::default(),
            Err(problem) => return StoredCredentials::nothing(&problem.to_string()),
        };

        let text = match self.read_text() {
            Ok(Some(text)) => text,
            Ok(None) => return StoredCredentials::default(),
            Err(problem) => return StoredCredentials::nothing(&problem.to_string()),
        };

        let mut keys = match document::parse(&text) {
            Ok(document) => StoredCredentials::from_document(document),
            Err(problem) => StoredCredentials::nothing(&problem.to_string()),
        };
        if let Some(said) = secured.warning {
            keys.also(&said);
        }

        keys
    }

    /// Writes `key` down as `provider`'s, replacing one already there.
    ///
    /// # Errors
    ///
    /// [`AuthError`] when the store cannot be read back, when another crucible
    /// holds the lock, or when the file cannot be written.
    pub fn keep(&self, provider: &str, key: &str) -> Result<(), AuthError> {
        self.change(|document| {
            let removed_subscription = document.subscriptions.remove(provider).is_some();
            let changed =
                document.keys.get(provider).is_none_or(|held| held != key) || removed_subscription;
            document.keys.insert(provider.to_owned(), key.to_owned());
            changed
        })
    }

    /// Writes a completed subscription login and removes an API key previously
    /// selected for the same provider.
    #[cfg(test)]
    pub(crate) fn keep_subscription(
        &self,
        provider: &str,
        tokens: Tokens,
    ) -> Result<(), AuthError> {
        self.change(|document| {
            document.keys.remove(provider);
            document.subscriptions.insert(provider.to_owned(), tokens);
            true
        })
    }

    /// Forgets `provider`'s key. `false` when there was none to forget.
    ///
    /// # Errors
    ///
    /// [`AuthError`] as [`Store::keep`].
    pub fn forget(&self, provider: &str) -> Result<bool, AuthError> {
        let mut had = false;
        self.change(|document| {
            had = document.keys.remove(provider).is_some()
                | document.subscriptions.remove(provider).is_some();
            had
        })?;

        Ok(had)
    }

    /// The read-modify-write, under the lock, once.
    ///
    /// `change` says whether anything moved: forgetting a provider that was
    /// never there rewrites nothing, which keeps the file's modification time
    /// honest about when somebody last logged in or out.
    fn change(&self, change: impl FnOnce(&mut Document) -> bool) -> Result<(), AuthError> {
        self.directory()?;
        let _held = Lock::take(&self.home.join(LOCK), &self.path)?;
        let _secured = self.secure_existing()?;

        let mut document = match self.read_text()? {
            Some(text) => document::parse(&text).map_err(|_| AuthError::Unreadable {
                path: self.path.clone(),
            })?,
            None => Document::default(),
        };

        if !change(&mut document) {
            return Ok(());
        }

        self.write(&document)
    }

    /// Replaces the complete protected document after its caller took the
    /// store lock.
    fn write(&self, document: &Document) -> Result<(), AuthError> {
        let partial = self.home.join(PARTIAL);
        write_private(&partial, &document::render(document))?;
        crucible_privacy::replace(&partial, &self.path)
            .map_err(|problem| AuthError::at(&self.path)(problem.into_io()))
    }

    /// The directory, created or tightened so only this user can enter it.
    ///
    /// The directory also holds configuration and sessions, all private user
    /// state. Tightening an older directory before creating the partial is what
    /// makes the file private from its first observable moment on Windows.
    fn directory(&self) -> Result<(), AuthError> {
        crucible_privacy::directory(&self.home)
            .map_err(|problem| AuthError::at(&self.home)(problem.into_io()))
    }

    /// An existing store, protected before any key is read from it.
    fn secure_existing(&self) -> Result<Option<Secured>, AuthError> {
        match fs::metadata(&self.home) {
            Ok(_) => self.directory()?,
            Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(problem) => return Err(AuthError::at(&self.home)(problem)),
        }

        match fs::symlink_metadata(&self.path) {
            Ok(_) => {}
            Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(problem) => return Err(AuthError::at(&self.path)(problem)),
        }

        let changed = crucible_privacy::tighten(&self.path)
            .map_err(|problem| AuthError::at(&self.path)(problem.into_io()))?;
        Ok(Some(Secured {
            warning: changed.then(|| {
                format!(
                    "{FILE} was readable by others and has been tightened to owner-only permissions"
                )
            }),
        }))
    }

    /// Reads no more than one bounded store from disk.
    fn read_text(&self) -> Result<Option<String>, AuthError> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(problem) => return Err(AuthError::at(&self.path)(problem)),
        };

        let mut text = String::new();
        file.take((MAX_STORE + 1) as u64)
            .read_to_string(&mut text)
            .map_err(AuthError::at(&self.path))?;
        if text.len() > MAX_STORE {
            return Err(AuthError::TooLarge {
                path: self.path.clone(),
                maximum: MAX_STORE,
            });
        }

        Ok(Some(text))
    }
}

/// Proof that an existing store was protected before it was read.
struct Secured {
    /// A successful repair the user should know happened.
    warning: Option<String>,
}

/// The keys the store held, and anything that has to be said about reading it.
#[derive(Default)]
pub struct StoredCredentials {
    /// Provider name to the key written down for it.
    keys: BTreeMap<String, String>,
    /// Provider name to its renewable subscription credential.
    subscriptions: BTreeMap<String, Tokens>,
    /// The union, retained once so listing never duplicates a malformed name.
    providers: BTreeSet<String>,
    /// What reading could not do, in a sentence for the user.
    trouble: Option<Box<str>>,
}

impl StoredCredentials {
    /// A complete parsed document.
    fn from_document(document: Document) -> Self {
        let providers = document
            .keys
            .keys()
            .chain(document.subscriptions.keys())
            .cloned()
            .collect();
        Self {
            keys: document.keys,
            subscriptions: document.subscriptions,
            providers,
            trouble: None,
        }
    }

    /// No keys, and a reason.
    fn nothing(said: &str) -> Self {
        Self {
            trouble: Some(said.into()),
            ..Self::default()
        }
    }

    /// Adds a second sentence, since a file can be both unreadable and too
    /// open and the user is told once either way.
    fn also(&mut self, said: &str) {
        self.trouble = Some(match self.trouble.take() {
            Some(first) => format!("{first}; {said}").into(),
            None => said.into(),
        });
    }

    /// `provider`'s key, as the type that can be applied and not read.
    #[must_use]
    pub fn get(&self, provider: &str) -> Option<ApiKey> {
        self.keys.get(provider).map(ApiKey::new)
    }

    /// Whether either supported credential kind is selected for `provider`.
    #[must_use]
    pub fn has(&self, provider: &str) -> bool {
        self.providers.contains(provider)
    }

    /// Whether `provider` has an API key in the protected store.
    #[must_use]
    pub fn has_key(&self, provider: &str) -> bool {
        self.keys.contains_key(provider)
    }

    /// Whether `provider` has a renewable account credential in the protected
    /// store.
    #[must_use]
    pub fn has_subscription(&self, provider: &str) -> bool {
        self.subscriptions.contains_key(provider)
    }

    /// Every provider with either stored credential kind, in name order.
    pub fn providers(&self) -> impl Iterator<Item = &str> {
        self.providers.iter().map(String::as_str)
    }

    /// What reading the store could not do, once, for the user to be told.
    #[must_use]
    pub fn trouble(&self) -> Option<&str> {
        self.trouble.as_deref()
    }
}

/// Written by hand: the derived one would print every key it holds.
impl std::fmt::Debug for StoredCredentials {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("StoredCredentials")
            .field("providers", &self.providers)
            .field("trouble", &self.trouble)
            .finish_non_exhaustive()
    }
}

/// Writes `text` to `path`, readable by nobody else, and does not return until
/// the bytes are on the disk.
///
/// A temporary left behind by a crucible that died mid-write is cleared first,
/// and `create_new` makes the file — which fails on a name already taken rather
/// than following it, a symlink aimed elsewhere being the one that matters, so
/// the gap between those two lines is not a way in. The mode is set at open
/// time rather than after, because the window between creating a file at the
/// umask's mode and tightening it is long enough to read a key out of. On
/// Windows the protected directory supplies that initial list by inheritance,
/// and the file is protected outright before this function receives it.
fn write_private(path: &Path, text: &str) -> Result<(), AuthError> {
    let _ = fs::remove_file(path);

    let mut file = crucible_privacy::create_write(path)
        .map_err(|problem| AuthError::at(path)(problem.into_io()))?;
    file.write_all(text.as_bytes())
        .map_err(AuthError::at(path))?;
    file.sync_all().map_err(AuthError::at(path))
}

/// The lock two crucibles logging in at once contend for.
///
/// Advisory, on a file of its own beside the store: locking the store itself
/// would not survive the rename that replaces it. Released by `Drop`, so an
/// error on any line between taking it and renaming leaves nothing held.
struct Lock {
    /// Held open because closing it releases the lock; read only to unlock.
    file: File,
}

impl Lock {
    /// Takes it, waiting briefly for whoever has it.
    ///
    /// `store` is only for the error: a sentence naming a lock file the user
    /// never created explains nothing, and the thing they are trying to write
    /// is the store.
    fn take(lock: &Path, store: &Path) -> Result<Self, AuthError> {
        let file = crucible_privacy::lock(lock)
            .map_err(|problem| AuthError::at(lock)(problem.into_io()))?;

        for _ in 0..ATTEMPTS {
            match file.try_lock() {
                Ok(()) => return Ok(Self { file }),
                Err(fs::TryLockError::WouldBlock) => std::thread::sleep(PAUSE),
                Err(fs::TryLockError::Error(trouble)) => return Err(AuthError::at(lock)(trouble)),
            }
        }

        Err(AuthError::Busy {
            path: store.to_path_buf(),
        })
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests;
