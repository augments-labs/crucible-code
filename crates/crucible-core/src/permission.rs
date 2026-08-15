//! Deciding whether a call may run, and proving that it may.
//!
//! Everything a tool can do arrives here first. There is one route to running
//! a tool and it goes through [`Permission::decide`]; nothing else can mint the
//! token a tool needs, so a call that skipped this cannot be made to run by
//! forgetting a check.
//!
//! Two inputs settle a call. **Rules** are standing statements read from
//! configuration — three kinds, `deny` beating `ask` beating `allow` whatever
//! the patterns look like. A **mode** supplies the answer for the arm no rule
//! matched, and does nothing else: it never branches around this module, so
//! adding a mode adds one default and not a second way for a tool to run.
//!
//! One refusal precedes both: no tool may write the files this engine is
//! configured from. A single write there could allow everything from the next
//! start on, so that answer cannot be entrusted to the rules and modes it
//! would defeat. A tool that runs programs arrives here as a command rather
//! than as the paths it will touch, so the refusal cannot be spelled for it.
//! What stands in its place is that a command is put to the user in every mode
//! but `fullAccess`, so a line that would write one of those files is one
//! somebody was shown first.
//!
//! A refusal has two shapes on purpose. A rule is standing policy and stops one
//! call; the model meets the wall, is told, and gets on with something else. A
//! human saying no is about this moment, and ends the turn — otherwise a model
//! can reshape the same question until one of the shapes gets a yes.

use std::collections::HashSet;
use std::path::Path;

use crate::tool::ToolCall;

mod grant;
mod mode;
mod rule;
mod sensitivity;
#[cfg(test)]
mod tests;
mod verdict;

pub use grant::{Approved, Grant};
pub use mode::Mode;
pub use rule::mint::{Minted, narrowest};
pub use rule::{Disposition, RuleError, Rules};
pub use sensitivity::{Command, Sensitivity, Target};
pub use verdict::{Ask, Remember, Verdict};

/// The directory the permission configuration is read from, and the files
/// inside it that are read.
const DIRECTORY: &str = ".crucible";
const FILES: [&str; 2] = ["config.json", "config.local.json"];

/// Whether this resolved path is a file permission configuration is read from:
/// `config.json` or `config.local.json` directly inside a directory named
/// `.crucible`. Any such directory counts — the project's, the home one, or one
/// deeper in the tree, which is what a session started there would read.
///
/// Matched on whole components of the resolved path, so a directory that merely
/// ends in `.crucible` is somebody else's, and a symbolic link into the real
/// file does not slip past.
///
/// It is a check on the path and not on the file behind it, so a second name
/// for the same inode — a hard link, which resolving does not undo — is not
/// this file as far as this is concerned. Making one takes a program, and a
/// program is put to the user in every mode but `fullAccess`.
fn configuration(path: &Path) -> bool {
    let file = path.file_name().and_then(|name| name.to_str());
    let directory = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());

    directory == Some(DIRECTORY) && file.is_some_and(|file| FILES.contains(&file))
}

/// What the engine settled on for one call.
#[derive(Debug)]
pub enum Settled {
    /// It may run, and here is the proof.
    Approved(Approved),

    /// Standing policy forbids it — a rule, or the engine refusing to let any
    /// tool write its own configuration. The turn carries on and the model is
    /// told: policy costs nothing to hit twice, and ending the turn on one
    /// would let a single stray call throw away a piece of work.
    Forbidden,

    /// The user said no. The turn ends.
    Refused,
}

/// The permission engine for one session.
#[derive(Debug)]
pub struct Permission {
    mode: Mode,
    rules: Rules,

    /// What the user allowed for the rest of the session, by scope. Held in
    /// memory and never written down, so it dies with the process that earned
    /// it.
    remembered: HashSet<Box<str>>,
}

impl Permission {
    /// A session with no rules, asking about everything it would change or run.
    #[must_use]
    pub fn new() -> Self {
        Self::with(Mode::default(), Rules::new())
    }

    /// A session with the mode and rules configuration asked for.
    #[must_use]
    pub fn with(mode: Mode, rules: Rules) -> Self {
        Self {
            mode,
            rules,
            remembered: HashSet::new(),
        }
    }

    /// The mode in force, which the prompt line shows at all times.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Forgets what was allowed for the rest of the session, keeping the mode
    /// and the rules.
    ///
    /// For a process that has picked up a different session. "For the rest of
    /// this session" was answered about the session being left behind, and an
    /// allow that outlived the thing it was scoped to would be a question
    /// nobody was asked. A durable policy store, if an [`Ask`] implementation
    /// has one, is outside this engine and must build the rules for the session
    /// being entered.
    pub fn forget(&mut self) {
        self.remembered = HashSet::new();
    }

    /// Steps that mode on to the next of the ring, and says which it is now.
    ///
    /// The one thing about a session that changes after it has started. It
    /// changes nothing else: the rules are what they were, what was allowed for
    /// the session stays allowed, and the arm no rule matched is the only arm
    /// this reaches.
    ///
    /// Reachable while a prompt is being typed and at no other moment, which is
    /// what makes this need no lock: no turn is running then, so nothing is
    /// looking at the mode and no call can have the mode it was decided under
    /// change underneath it.
    pub fn cycle(&mut self) -> Mode {
        self.mode = self.mode.next();
        self.mode
    }

    /// Puts that mode where it is, for a user who named the one they want
    /// rather than stepping to it.
    ///
    /// The same change [`Permission::cycle`] makes and under the same
    /// conditions — a prompt is being typed, no turn is running, nothing else
    /// about the session moves. Naming one is not a second way to change the
    /// mode, it is a shorter way to reach the same one.
    pub fn switch(&mut self, mode: Mode) {
        self.mode = mode;
    }

    /// Decides one call, asking the user if that is what it comes to.
    pub fn decide(
        &mut self,
        call: &ToolCall,
        sensitivity: &Sensitivity,
        ask: &mut dyn Ask,
    ) -> Settled {
        match self.disposition(call, sensitivity) {
            Disposition::Allow => self.approve(call, Verdict::Allow),
            Disposition::Deny => Settled::Forbidden,
            Disposition::Ask => self.put(call, sensitivity, ask),
        }
    }

    /// What is to happen to this call before anybody is asked.
    fn disposition(&self, call: &ToolCall, sensitivity: &Sensitivity) -> Disposition {
        // Before the rules and the mode on purpose: this is the one answer
        // neither may reach differently. A write here could put an allow for
        // everything into the next start, so the refusal cannot be entrusted
        // to what the files say — the files are the thing being defended.
        // Writes only, and file tools only: reading configuration is how a
        // session begins, and a process is shown here as a command rather than
        // as the paths it will touch. What keeps that from being the way round
        // this is the mode itself — a command is put to the user in every mode
        // but `fullAccess`, so a line that would write one of these files is
        // one somebody was shown first.
        if let Sensitivity::MutatesFile { target } = sensitivity
            && target
                .absolute()
                .is_some_and(|absolute| configuration(Path::new(absolute)))
        {
            return Disposition::Deny;
        }

        let stated = self
            .rules
            .stated(call, sensitivity)
            .unwrap_or_else(|| self.mode.default_arm(sensitivity));

        // A read is never put to the user, so an `ask` about one has to become
        // something. It becomes a refusal: whoever wrote `ask read(secrets/**)`
        // asked not to have it go through unwatched, and refusing is the only
        // answer left that respects that. No mode produces this arm — every one
        // of them allows a read — so it is only ever somebody's own rule.
        //
        // Spelled out rather than closed with a wildcard. This is the last step
        // before a disposition becomes a verdict, so a sensitivity added later
        // has to be decided about here rather than inheriting whatever the
        // rules and the mode happened to say about it.
        match (sensitivity, stated) {
            (Sensitivity::ReadOnly { .. }, Disposition::Ask) => Disposition::Deny,
            (Sensitivity::ReadOnly { .. }, settled) => settled,
            (Sensitivity::MutatesFile { .. } | Sensitivity::SpawnsProcess { .. }, settled) => {
                settled
            }
        }
    }

    /// Asks, unless this scope was already allowed for the session.
    fn put(&mut self, call: &ToolCall, sensitivity: &Sensitivity, ask: &mut dyn Ask) -> Settled {
        let scope = Self::scope(call, sensitivity);
        if self.remembered.contains(&scope) {
            return self.approve(call, Verdict::Allow);
        }

        let (verdict, remember) = ask.ask(call, sensitivity);

        // Matched rather than compared, so a duration added later stops the
        // build here instead of being quietly read as "do not remember".
        let lasts = match remember {
            Remember::Never => false,
            Remember::Session | Remember::Always => true,
        };
        if verdict == Verdict::Allow && lasts {
            self.remembered.insert(scope);
        }

        // Nothing is remembered about a no. The turn ends on one, so there is
        // no next call in it to remember for, and the next turn is a fresh
        // instruction that deserves its own question.
        self.approve(call, verdict)
    }

    /// Mints the proof, or reports that the user said no.
    ///
    /// The rules that end a read travel with it. A tool that reaches further
    /// than the call it was decided about — a search walks a whole directory —
    /// has no way to ask this engine again from inside its own walk, and the
    /// alternative to carrying them is a second copy of the rules somewhere
    /// nothing keeps in step.
    fn approve(&self, call: &ToolCall, verdict: Verdict) -> Settled {
        match Grant::issue(verdict) {
            Some(grant) => Settled::Approved(Approved::new(
                call.clone(),
                grant,
                self.rules.denials(&call.name),
            )),
            None => Settled::Refused,
        }
    }

    /// What a session-long allow covers: the tool, and the one thing the
    /// question named it would do.
    ///
    /// Never the tool alone. Agreeing to `cargo test` is not agreeing to
    /// `curl`, and agreeing to change `src/a.rs` is not agreeing to change the
    /// hook git runs on every commit — the question showed one of them, so that
    /// is the whole of what an answer to it can cover.
    ///
    /// Spelled the way the question spelled it, which is also the way a durable
    /// rule is minted. The two scopes must agree: this is what stands for a
    /// persisted answer during the current session.
    fn scope(call: &ToolCall, sensitivity: &Sensitivity) -> Box<str> {
        match sensitivity {
            // A read never reaches here — an `ask` about one became a refusal
            // further up — and it is spelled anyway, because a scope that read
            // back as the bare tool name would be the widest one there is.
            Sensitivity::ReadOnly { target } | Sensitivity::MutatesFile { target } => {
                format!("{}:{target}", call.name).into()
            }
            Sensitivity::SpawnsProcess { command } => format!("{}:{command}", call.name).into(),
        }
    }
}

impl Default for Permission {
    fn default() -> Self {
        Self::new()
    }
}
