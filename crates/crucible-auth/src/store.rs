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
//! Every such read protects the store before it looks, which is itself a
//! write; the one read that is not is [`Store::inventory`], which reports what
//! is held, by name, as it finds it.
//!
//! Writing is the same three steps every time: take the lock, read what is
//! there, rename a sibling temporary over the target. The lock is what stops
//! two crucibles logging in at once from each writing a file that has forgotten
//! the other's provider; the rename is what stops a full disk leaving half a
//! file where a whole one was.

mod document;
mod names;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crucible_credentials::ApiKey;

use crate::error::AuthError;
use crate::oauth::{OAuthError, Tokens};

use self::document::Document;
pub use self::names::{Dropped, Held, Kind, Names, Settled, provider_of};

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
pub(crate) const WAIT: std::time::Duration = std::time::Duration::from_secs(5);
const PAUSE: std::time::Duration = std::time::Duration::from_millis(20);

/// What this version of crucible writes, and the highest it can read.
///
/// A number rather than a guess at the shape, so a file from a version that
/// does not exist yet is a case this one can recognise and decline instead of
/// a parse failure it would report as damage.
const VERSION: u64 = 2;

/// What is asked, under the store's lock, before a write takes a credential
/// out: its answer is written before the store is, and a refusal leaves the
/// store as it was. Handed every credential about to go.
pub type LettingGo = Arc<dyn Fn(&[Dropped]) -> Result<(), Box<str>> + Send + Sync>;

/// What is asked, under the store's lock, before a write moves the credential
/// a provider holds: a first one stored, one taken out, one replaced by
/// another row's. Handed each such provider by name; its answer is written
/// before the store is, and a refusal leaves the store as it was. A key
/// written again under its own name moves nothing.
pub type Moving = Arc<dyn Fn(&[&str]) -> Result<(), Box<str>> + Send + Sync>;

/// Where the keys crucible was given are written down.
#[derive(Clone)]
pub struct Store {
    /// The file itself.
    path: PathBuf,
    /// The directory it lives in, which may not exist yet.
    home: PathBuf,
    /// The names this build writes each provider's credential under.
    names: Names,
    /// What is asked before a credential is taken out, where anything is.
    letting_go: Option<LettingGo>,
    /// What is asked before a provider's credential moves, where anything is.
    moving: Option<Moving>,
}

impl fmt::Debug for Store {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Store")
            .field("path", &self.path)
            .field("names", &self.names)
            .field("letting_go", &self.letting_go.is_some())
            .field("moving", &self.moving.is_some())
            .finish_non_exhaustive()
    }
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
            names: Names::default(),
            letting_go: None,
            moving: None,
        }
    }

    /// The same store, asking `letting_go` before any write takes a
    /// credential out: a key or sign-in replaced by another row's, a provider
    /// forgotten, a second credential settled at a start.
    #[must_use]
    pub fn letting_go(mut self, letting_go: LettingGo) -> Self {
        self.letting_go = Some(letting_go);
        self
    }

    /// The same store, asking `moving` before any write moves the credential
    /// a provider holds.
    #[must_use]
    pub fn moving(mut self, moving: Moving) -> Self {
        self.moving = Some(moving);
        self
    }

    /// The same store, told the names this build writes each provider's
    /// credential under.
    #[must_use]
    pub fn naming(mut self, names: Names) -> Self {
        self.names = names;
        self
    }

    /// Takes out, in one locked write, every second credential a provider
    /// holds, keeping the one under its bare name.
    ///
    /// Only a write by 0.43.3, rolled back to, leaves two. The store is looked
    /// at without the lock first, so a start that finds one credential for
    /// each provider takes no lock and waits on no other crucible; only a
    /// start that finds two takes it, and looks again under it. What was found
    /// comes back whether or not the write could be made, so the start can
    /// say it either way; the next start tries again.
    ///
    /// Read the store first ([`Store::read`]): that read tightens a store left
    /// readable by others and says so, and this one finds it private.
    #[must_use]
    pub fn settle(&self) -> Settled {
        let names = self.names.clone();
        let found = match self.document() {
            Ok(Some(mut document)) => seconds(&mut document, &names),
            Ok(None) | Err(_) => Vec::new(),
        };
        if found.is_empty() {
            return Settled::Nothing;
        }

        let mut dropped = Vec::new();
        let written = self.change(|document| {
            dropped = seconds(document, &names);
            !dropped.is_empty()
        });
        match written {
            Err(why) => Settled::Stayed { found, why },
            Ok(()) if dropped.is_empty() => Settled::Nothing,
            Ok(()) => Settled::Removed(dropped),
        }
    }

    /// The document on disk, read without the lock, or `None` where there is
    /// none.
    fn document(&self) -> Result<Option<Document>, AuthError> {
        if self.secure_existing()?.is_none() {
            return Ok(None);
        }
        let Some(text) = self.read_text()? else {
            return Ok(None);
        };
        document::parse(&text)
            .map(Some)
            .map_err(|_| AuthError::Unreadable {
                path: self.path.clone(),
            })
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
            Ok(None) => return StoredCredentials::empty(self.clone()),
            Err(problem) => return StoredCredentials::nothing(self.clone(), &problem.to_string()),
        };

        let text = match self.read_text() {
            Ok(Some(text)) => text,
            Ok(None) => return StoredCredentials::empty(self.clone()),
            Err(problem) => return StoredCredentials::nothing(self.clone(), &problem.to_string()),
        };

        let mut keys = match document::parse(&text) {
            Ok(document) => StoredCredentials::from_document(self.clone(), document),
            Err(problem) => StoredCredentials::nothing(self.clone(), &problem.to_string()),
        };
        if let Some(said) = secured.warning {
            keys.also(&said);
        }

        keys
    }

    /// The credential each provider this build names holds, by its map and
    /// name: what a screen marking rows needs, and no key or token.
    ///
    /// # Errors
    ///
    /// [`AuthError`] where the store is there and cannot be read whole: the
    /// store a write would refuse to replace, said before anything is asked.
    pub fn holding(&self) -> Result<Vec<Held>, AuthError> {
        let Some(document) = self.document()? else {
            return Ok(Vec::new());
        };
        let stored = StoredCredentials::from_document(self.clone(), document);
        Ok(self
            .names
            .providers()
            .filter_map(|provider| stored.held(provider))
            .collect())
    }

    /// What the store holds, by name, and whether others could read it,
    /// found without changing a thing.
    ///
    /// Every other read here protects the store before it looks, which is a
    /// write: it makes the directory, tightens the file and says it did. A
    /// report about whether the store is private has to see it as it is, so
    /// this takes no lock, makes no directory, tightens nothing and follows no
    /// link, and keeps the names it read and no key or token. What could not
    /// be read is one sentence that names the store by its file name alone.
    #[must_use]
    pub fn inventory(&self) -> Inventory {
        let mut stock = Inventory {
            names: self.names.clone(),
            held: BTreeSet::new(),
            present: true,
            exposed: None,
            trouble: None,
        };
        let file = match crucible_privacy::open_read(&self.path) {
            Ok(file) => file,
            Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => {
                stock.present = false;
                return stock;
            }
            Err(problem) => {
                stock.trouble = Some(format!("{FILE} could not be opened: {problem}").into());
                return stock;
            }
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            stock.exposed = file
                .metadata()
                .ok()
                .map(|metadata| metadata.permissions().mode() & 0o077 != 0);
        }

        let mut bytes = Vec::new();
        let said = match file.take((MAX_STORE + 1) as u64).read_to_end(&mut bytes) {
            Err(problem) => Some(format!("{FILE} could not be read: {problem}")),
            Ok(_) if bytes.len() > MAX_STORE => Some(format!(
                "{FILE} is larger than {MAX_STORE} bytes, so no stored credential is used"
            )),
            Ok(_) => match String::from_utf8(bytes) {
                Err(_) => Some(format!("{FILE} is not text")),
                Ok(text) => match document::parse(&text) {
                    Ok(document) => {
                        stock.held = held_in(&document);
                        None
                    }
                    Err(problem) => Some(problem.to_string()),
                },
            },
        };
        stock.trouble = said.map(String::into_boxed_str);
        stock
    }

    /// Writes `key` under `name`, a row's stored name, replacing one already
    /// there.
    ///
    /// # Errors
    ///
    /// [`AuthError`] when the store cannot be read back, when another crucible
    /// holds the lock, or when the file cannot be written.
    ///
    /// Every other credential of the provider, under any name this build
    /// writes it under and in either map, goes in the same write: a provider
    /// holds one. What went comes back, so the caller can say which row it was.
    pub fn keep(&self, name: &str, key: &str) -> Result<Vec<Dropped>, AuthError> {
        let mut dropped = Vec::new();
        let names = self.names.clone();
        self.change(|document| {
            dropped = document.clear(&names, name, Kind::Key);
            let changed =
                document.keys.get(name).is_none_or(|held| held != key) || !dropped.is_empty();
            document.keys.insert(name.to_owned(), key.to_owned());
            changed
        })?;
        Ok(dropped)
    }

    /// Writes a completed sign-in under `name`, taking out every other
    /// credential its provider holds in the same write, and says what went.
    pub(crate) fn keep_subscription(
        &self,
        name: &str,
        tokens: Tokens,
    ) -> Result<Vec<Dropped>, AuthError> {
        self.subscribe(name, tokens, None)
    }

    /// [`Store::keep_subscription`], keeping `identity` as the installation
    /// identity `name`'s sign-ins present where none is kept yet, in the same
    /// write.
    ///
    /// Written with the tokens and not before them, so a sign-in that does not
    /// complete leaves the store as it found it. Where two first sign-ins
    /// complete at once, the identity the first wrote stays.
    pub(crate) fn keep_identified(
        &self,
        name: &str,
        tokens: Tokens,
        identity: &str,
    ) -> Result<Vec<Dropped>, AuthError> {
        self.subscribe(name, tokens, Some(identity))
    }

    fn subscribe(
        &self,
        name: &str,
        tokens: Tokens,
        identity: Option<&str>,
    ) -> Result<Vec<Dropped>, AuthError> {
        let mut dropped = Vec::new();
        let names = self.names.clone();
        self.change(|document| {
            dropped = document.clear(&names, name, Kind::Account);
            document.subscriptions.insert(name.to_owned(), tokens);
            if let Some(identity) = identity {
                document
                    .identities
                    .entry(name.to_owned())
                    .or_insert_with(|| identity.to_owned());
            }
            true
        })?;
        Ok(dropped)
    }

    /// The installation identity `name`'s sign-ins present, where one is
    /// kept. Reads and writes nothing else.
    pub(crate) fn identity(&self, name: &str) -> Result<Option<String>, AuthError> {
        Ok(self
            .document()?
            .and_then(|document| document.identities.get(name).cloned()))
    }

    /// Forgets `provider`'s key. `false` when there was none to forget.
    ///
    /// # Errors
    ///
    /// [`AuthError`] as [`Store::keep`].
    ///
    /// Every name this build writes the provider's credential under goes, so a
    /// provider left holding two by a roll back is left holding none.
    pub fn forget(&self, provider: &str) -> Result<bool, AuthError> {
        let mut had = false;
        let names = self.names.clone();
        self.change(|document| {
            for name in names.of(provider) {
                had |= !document.take(name, None).is_empty();
            }
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

        let before = held_in(&document);
        if !change(&mut document) {
            return Ok(());
        }

        // Asked with the file still as it was: whatever has to go with a
        // credential goes first, so a stop between the two leaves the
        // credential and not what went with it.
        let after = held_in(&document);
        if let Some(letting_go) = &self.letting_go {
            let going: Vec<Dropped> = before.difference(&after).cloned().collect();
            if !going.is_empty() {
                letting_go(&going).map_err(|why| AuthError::Unreleased { why })?;
            }
        }
        if let Some(moving) = &self.moving {
            let moved: BTreeSet<&str> = before
                .symmetric_difference(&after)
                .map(|held| provider_of(&held.name))
                .collect();
            if !moved.is_empty() {
                let moved: Vec<&str> = moved.into_iter().collect();
                moving(&moved).map_err(|why| AuthError::Unreleased { why })?;
            }
        }

        self.write(&document)
    }

    /// Takes the store lock for one provider's rotation and rereads the
    /// rotation inside it: the first half of a renewal.
    ///
    /// Reading the latest rotation after taking the lock is what prevents two
    /// processes from presenting the same one-use refresh token. A rotation no
    /// longer due — another process renewed it first — comes back with the
    /// lock released. One still due comes back holding the lock, which the
    /// caller keeps across its request and releases only by writing the new
    /// rotation ([`Rotating::persist`]) or dropping it: another process waits
    /// or fails visibly rather than invalidating the rotation in flight.
    ///
    /// Blocking: taking the lock waits up to 5 s, and the reread is file work.
    pub(crate) fn take_rotation(
        &self,
        provider: &str,
        needs_refresh: impl Fn(&Tokens, u64) -> bool,
    ) -> Result<Taken, OAuthError> {
        self.directory()?;
        let lock = Lock::take(&self.home.join(LOCK), &self.path)?;
        let _secured = self.secure_existing()?;
        let document = match self.read_text()? {
            Some(text) => document::parse(&text).map_err(|_| AuthError::Unreadable {
                path: self.path.clone(),
            })?,
            None => return Err(OAuthError::SignedOut),
        };
        let current = document
            .subscriptions
            .get(provider)
            .cloned()
            .ok_or(OAuthError::SignedOut)?;
        if !needs_refresh(&current, document::now()) {
            return Ok(Taken::Fresh(current));
        }

        Ok(Taken::Due(Box::new(Rotating {
            store: self.clone(),
            provider: provider.to_owned(),
            document,
            current,
            _lock: lock,
        })))
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

        // Bytes first, so a store past the limit is one too large wherever
        // the limit falls, even inside a character.
        let mut bytes = Vec::new();
        file.take((MAX_STORE + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(AuthError::at(&self.path))?;
        if bytes.len() > MAX_STORE {
            return Err(AuthError::TooLarge {
                path: self.path.clone(),
                maximum: MAX_STORE,
            });
        }
        // A file that opens and holds something other than text is there and
        // cannot be read, which is not a permissions matter.
        let text = String::from_utf8(bytes).map_err(|_| AuthError::Unreadable {
            path: self.path.clone(),
        })?;

        Ok(Some(text))
    }
}

/// Every credential `document` holds, by its map and name.
fn held_in(document: &Document) -> BTreeSet<Held> {
    document
        .keys
        .keys()
        .map(|name| Held::new(Kind::Key, name))
        .chain(
            document
                .subscriptions
                .keys()
                .map(|name| Held::new(Kind::Account, name)),
        )
        .collect()
}

/// Takes out of `document` every provider's second credential, where its bare
/// name holds one, and says what went.
fn seconds(document: &mut Document, names: &Names) -> Vec<Dropped> {
    let providers: BTreeSet<String> = document
        .keys
        .keys()
        .chain(document.subscriptions.keys())
        .map(|name| provider_of(name).to_owned())
        .collect();
    let mut dropped = Vec::new();
    for provider in providers {
        if document.holds(&provider).is_none() {
            continue;
        }
        for name in names.of(&provider) {
            if name != provider {
                dropped.extend(document.take(name, None));
            }
        }
    }
    dropped
}

/// What taking a rotation found.
pub(crate) enum Taken {
    /// The rotation the store holds is not due, and the lock is released.
    Fresh(Tokens),
    /// It is due, and the lock is held until it is replaced.
    Due(Box<Rotating>),
}

/// A rotation being replaced, holding the store lock until it is.
///
/// The lock is a file held open rather than a guard of a mutex, so holding it
/// across the request that renews the rotation blocks no thread; dropping it,
/// on any path, releases it.
pub(crate) struct Rotating {
    store: Store,
    provider: String,
    /// The whole document as it was read inside the lock, which nothing else
    /// can have changed since.
    document: Document,
    current: Tokens,
    _lock: Lock,
}

impl Rotating {
    /// The rotation being replaced.
    pub(crate) fn current(&self) -> &Tokens {
        &self.current
    }

    /// Writes `fresh` in place of the rotation, then releases the lock.
    ///
    /// Blocking: the write is synced to the disk before the rename.
    pub(crate) fn persist(mut self, fresh: Tokens) -> Result<Tokens, AuthError> {
        self.document
            .subscriptions
            .insert(self.provider.clone(), fresh.clone());
        self.store.write(&self.document)?;
        Ok(fresh)
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
    /// The store whose latest rotation a subscription credential renews.
    store: Option<Store>,
    /// What reading could not do, in a sentence for the user.
    trouble: Option<Box<str>>,
    /// Whether the store could not be read at all, so what it holds is not
    /// known: not an empty store, nor one read in full with a warning.
    unread: bool,
}

impl StoredCredentials {
    /// No credentials in an ordinary, readable store.
    fn empty(store: Store) -> Self {
        Self {
            store: Some(store),
            ..Self::default()
        }
    }

    /// A parsed document tied to the store it came from.
    fn from_document(store: Store, document: Document) -> Self {
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
            unread: false,
            store: Some(store),
        }
    }

    /// No keys, and a reason.
    fn nothing(store: Store, said: &str) -> Self {
        Self {
            trouble: Some(said.into()),
            unread: true,
            store: Some(store),
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

    /// The protected state an in-crate subscription implementation resolves.
    /// Tokens remain crate-private: callers receive only a [`Credential`]
    /// through [`crate::SubscriptionLogin::credential`].
    ///
    /// [`Credential`]: crucible_credentials::Credential
    pub(crate) fn subscription(&self, provider: &str) -> Option<(Store, Tokens)> {
        Some((
            self.store.as_ref()?.clone(),
            self.subscriptions.get(provider)?.clone(),
        ))
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

    /// Whether the store could not be read at all, so that holding nothing
    /// here says nothing about what it holds. A store read in full whose
    /// permissions had to be tightened is read.
    #[must_use]
    pub fn unread(&self) -> bool {
        self.unread
    }

    /// The one credential `provider` is served by, and the name it is under.
    ///
    /// Among the names this build writes for the provider only: a name it
    /// does not know is never used. Where two are held, which only a write by
    /// 0.43.3 can leave, the one under the bare name.
    #[must_use]
    pub fn held(&self, provider: &str) -> Option<Held> {
        let names = self
            .store
            .as_ref()
            .map_or_else(Names::default, |store| store.names.clone());
        serving(&names, provider, |held| match held.kind {
            Kind::Account => self.subscriptions.contains_key(&held.name),
            Kind::Key => self.keys.contains_key(&held.name),
        })
    }
}

/// What a store holds by name, and whether others could read it, as
/// [`Store::inventory`] found it without changing it.
///
/// No key or token is in here: only the map each credential sits in and the
/// name it is under, which is what a `/login` row is called.
pub struct Inventory {
    /// The names this build writes each provider's credential under.
    names: Names,
    /// Every credential the store holds, by map and name.
    held: BTreeSet<Held>,
    /// Whether there is a store at all.
    present: bool,
    /// Whether anyone but its owner may read or write the file, where that
    /// was looked at: on Unix, and of a store that opened.
    exposed: Option<bool>,
    /// What could not be read, in a sentence naming no path.
    trouble: Option<Box<str>>,
}

impl Inventory {
    /// Whether there is a store, readable or not.
    #[must_use]
    pub fn present(&self) -> bool {
        self.present
    }

    /// How many credentials it holds; none where it could not be read.
    #[must_use]
    pub fn count(&self) -> usize {
        self.held.len()
    }

    /// The one credential `provider` is served by, and the name it is under,
    /// chosen as [`StoredCredentials::held`] chooses it.
    #[must_use]
    pub fn held(&self, provider: &str) -> Option<Held> {
        serving(&self.names, provider, |held| self.held.contains(held))
    }

    /// Whether anyone but its owner may read or write the file: `None` where
    /// that was not looked at, because there is no store, it did not open, or
    /// this platform says it with something other than a mode.
    #[must_use]
    pub fn exposed(&self) -> Option<bool> {
        self.exposed
    }

    /// What could not be read, once, naming the store by its file name.
    #[must_use]
    pub fn trouble(&self) -> Option<&str> {
        self.trouble.as_deref()
    }
}

/// Written by hand, to say plainly that it holds names and nothing else.
impl fmt::Debug for Inventory {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Inventory")
            .field("held", &self.held)
            .field("present", &self.present)
            .field("exposed", &self.exposed)
            .field("trouble", &self.trouble)
            .finish_non_exhaustive()
    }
}

/// The one credential `provider` is served by among those `holds` says are
/// held: under the names this build writes for it only, the bare name first,
/// and an account before a key under the same name.
fn serving(names: &Names, provider: &str, holds: impl Fn(&Held) -> bool) -> Option<Held> {
    names.of(provider).into_iter().find_map(|name| {
        [Kind::Account, Kind::Key]
            .into_iter()
            .map(|kind| Held::new(kind, name))
            .find(|held| holds(held))
    })
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

        // Bounded by the clock, not by a count of pauses: a pause only
        // bounds how long a thread sleeps from below, and a kernel that
        // coalesces timers stretches each one, so the wait is measured on
        // the clock.
        let until = std::time::Instant::now().checked_add(WAIT);
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { file }),
                Err(fs::TryLockError::WouldBlock) => {}
                Err(fs::TryLockError::Error(trouble)) => return Err(AuthError::at(lock)(trouble)),
            }
            if until.is_none_or(|until| std::time::Instant::now() >= until) {
                return Err(AuthError::Busy {
                    path: store.to_path_buf(),
                });
            }
            std::thread::sleep(PAUSE);
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests;
