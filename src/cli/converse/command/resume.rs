//! `/resume`: what was worked on in this directory, and picking one of them
//! back up.
//!
//! A session is named by its id — the same word `--resume` takes and the
//! parting message prints — so an id given here is picked up directly, and
//! anything else stands the picker: a search line over the sessions recorded
//! in this workspace — or in this repository's other checkouts, or in any
//! project, once a key asks for them — with a window over the tail of
//! whichever one is marked.
//! What the picker looks like is [`Picker`]'s; which sessions a query keeps
//! and what each key moves is `finding`'s; what is listed, previewed and
//! written down is decided here, where the sessions are. A run with no
//! keyboard has no picker to walk, so it is given the listing instead, each
//! row carrying the exact id `/resume` and `--resume` take.
//!
//! Three keys change which sessions the picker shows, and are read here for
//! that reason: Ctrl+A shows every directory's, Ctrl+W adds this repository's
//! other checkouts, and Ctrl+B keeps only the branch checked out here. Each
//! flips its own state, over one read of the index filtered again on every
//! key; the index is read again only after a rename, to show what it now
//! holds. A session from another directory is never picked up here, since it is
//! bound to the directory it was recorded in: Enter on one says the command
//! that resumes it there instead. Windows is told it as a row for cmd and a row
//! for PowerShell, because no one line runs the same in both; `windows_said`
//! says why.
//!
//! Picking one up leaves nothing behind. The session being left is closed
//! here, which is the last chance to say that its log stopped being written,
//! and what it was allowed for the rest of *its* run is forgotten by the
//! runner — see [`Conversation::resume`]. The record of what has been read is
//! emptied with it, because it answers for the session being left rather than
//! for this run — and so are the images pasted, the tools looked up and the
//! plan, which then comes back as the session picked up last wrote it, the
//! same read a `--continue` does.
//!
//! The screen goes the same way. What is put back is a different conversation
//! rather than the next thing that happened in the one on it, so the transcript
//! is emptied and the session picked up replaces it, under the opening card and
//! exactly as a launch would have drawn it — and what was held behind the rows
//! of the old one is dropped with them, because a key that opens what is behind
//! a row nobody can see is worse than a row that offers nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr as _;
use std::time::SystemTime;

use crucible_app::Conversation;
use crucible_app::client::{Performed, Resumed};
use crucible_client_api::Command;
use crucible_session::{Glimpse, Pruned, Reach, Recorded, Roots, glimpse, recent, retitle};
use crucible_tui::{
    Editor, Glyphs, Kept, Key, Picker, Pressed, Renderer, Row, Slot, Terminal, clip,
    columns as wide,
};
use crucible_types::{Compacting, SessionId};
use crucible_workspace::Workspace;

use crate::cli::Fatal;
use crate::cli::client::astray;
use crate::cli::draw::when;

use super::super::region::{self, Ended, Moved};
use super::super::{Held, finding, replaying};
use super::Terms;

/// How many sessions the picker is handed.
///
/// The search line is what reaches past the visible rows, so the ceiling is
/// about how far back a query looks rather than how tall a window is. A
/// hundred of however many the index names: past that, a session is found by
/// its id sooner than by walking to it.
const OFFERED: usize = 100;

/// How many sessions the keyboardless listing holds.
///
/// Nine, because without a search line to narrow it the listing is read in one
/// glance or not at all, and each row already carries a whole id.
const SHOWN: usize = 9;

/// What the search line says with nothing typed into it.
///
/// Both halves named, because the query is matched against both and nothing on
/// screen says which one a match came off. Somebody who only knew it searched
/// titles would never try a branch's name in it.
const HINT: &str = "a session, or a branch";

/// What the preview pane says where the query left nothing to preview.
const NOVIEW: &str = "nothing to show";

/// How many drawn rows of one session the pane keeps.
///
/// Enough to fill any pane several times over, so wheeling back through a
/// preview reaches further than a glance — and bounded, because these are kept
/// for every session the mark passes over and a long conversation draws into
/// thousands of rows.
const KEPT: usize = 256;

/// What Enter does and what Escape does, said under the marked session's
/// metadata.
///
/// Both keys, because this line is under the reader's eyes while they decide,
/// and the way out is half of that decision.
const TAKES: &str = "Enter to resume · Esc to cancel";

/// What Enter and Esc do, said under a session recorded in another directory.
///
/// Enter there does not resume: it says the command that does, in the
/// directory the session belongs to. Short enough to be whole in the preview
/// pane of an eighty-column window, where "Esc to cancel" is the half that
/// would be cut.
const SHOWS: &str = "Enter to see how to resume · Esc to cancel";

/// What a workspace nothing was ever recorded in says.
const NEVER: &str = "no earlier session for this workspace";

/// What the heading names the sessions of every directory as, under Ctrl+A.
const EVERYWHERE: &str = "all projects";

/// What it names this checkout's and this repository's others as, under
/// Ctrl+W.
const CHECKOUTS: &str = "this repository's worktrees";

/// What a preview says where the bounded read did not reach the whole log.
///
/// Words rather than a mark: what is missing is the earlier part of the
/// conversation, and a bare ellipsis over the first row leaves a reader to
/// guess whether it stands for that or for a sentence that was clipped.
const CUT: &str = "the rest could not be read";

/// What a rename Enter refused for having nothing in it says, under the row
/// the title was being typed on.
///
/// A session with no title falls back to its first prompt, so an empty rename
/// could not stick — and saying so where the reader's hands are beats keeping
/// the old name without a word.
const REFUSED: &str = "a title cannot be empty";

/// What is said where the picker was left with nothing taken.
const LEFT: &str = "cancelled, no session picked up";

/// Runs it: the picker, or the session `said` picked up by id.
pub(super) fn run<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    held: &mut Held<'_>,
    terms: &Terms,
) -> Result<Option<Compacting>, Fatal> {
    let said = said.trim();
    if said.is_empty() {
        return offered(renderer, conversation, held, terms);
    }

    // Anything at all can follow `/resume `, and a word that is not even
    // shaped like an id is refused the same way one nothing here answers to
    // is: neither names a session recorded in this workspace, and whether
    // that is spelling or absence is nothing the reader can act on
    // differently. What to try instead follows the refusal.
    let Ok(id) = SessionId::from_str(said) else {
        renderer.commit(&format!("! no session {said} in this workspace"))?;
        return offered(renderer, conversation, held, terms);
    };

    picking(&id, renderer, conversation, held, terms)
}

/// Picks the session `id` names back up.
fn picking<T: Terminal>(
    id: &SessionId,
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    held: &mut Held<'_>,
    terms: &Terms,
) -> Result<Option<Compacting>, Fatal> {
    let columns = renderer.transcript_columns();

    let unclosed = match terms.perform(conversation, Command::Resume(id.clone())) {
        // Answered before the log is opened. This session's own claim is on
        // that file, so continuing it would come back as "open in another
        // crucible" — which names the wrong crucible, and reads as a reason to
        // go and close something.
        Performed::Resumed(Resumed::Same) => {
            let said = clip("this is the session you are in", columns);
            renderer.present(&[Row::new().then(Slot::Quiet, said)])?;
            return Ok(None);
        }
        Performed::Resumed(Resumed::Picked { unclosed }) => unclosed,

        // The one shape of failure the reader can act on from here: the id
        // names nothing recorded in this workspace, so what is recorded is
        // offered instead.
        Performed::Resumed(Resumed::Unknown) => {
            renderer.commit(&format!("! no session {} in this workspace", id.as_str()))?;
            return offered(renderer, conversation, held, terms);
        }

        // A path is in every one of these, so it is committed rather than
        // presented. Nothing else changes: the session in hand is still
        // being recorded, and the loop carries on with it.
        Performed::Resumed(Resumed::Failed(problem)) => {
            renderer.commit(&format!("! {problem}"))?;
            return Ok(None);
        }
        other => {
            renderer.commit(&astray(&other))?;
            return Ok(None);
        }
    };

    // The conversation swapped its session and handed the runner the same one,
    // and everything this loop reads of a session it reads off the
    // conversation. The one being left was closed on the way, and what closing
    // it said about its log came back to be said below.

    // The files remembered were read by the session just left, and `write`
    // replaces a file on the strength of that record. The session picked up saw
    // none of them, however much of it comes back off the disk: what a log holds
    // is what was said, not what the tools of that run had looked at.
    terms.ledger.forget();

    // The plan the panel is drawn from goes the same way, and then comes back
    // as the session picked up left it — the same read a `--continue` does, so
    // resuming here or from the command line stands the same plan over the box.
    terms.plan.forget();
    crucible_app::startup::planned(&terms.plan, conversation.runner().transcript());

    // The tools looked up belong to the conversation that looked them up. Left
    // standing they would be advertised to a session that never asked.
    terms.revealed.forget();

    // The images pasted were named by markers in prompts of the session just
    // left, and the numbering starts over with the session. Held on, one would
    // ride the first prompt after the resume that says `[Image #1]`.
    held.images.clear();

    // The last chance to say that the log of the session being left stopped
    // being written. After this there is no session to say it about.
    if let Some(problem) = unclosed {
        renderer.commit(&format!("! {problem}"))?;
    }

    // The transcript on screen belonged to the session just closed, and what
    // follows is a different conversation rather than the next thing that
    // happened in that one. Left standing, the two would be joined at a point
    // nothing marks, and a reader scrolling back would walk out of the session
    // they picked up and into one they left without being told.
    //
    // What was held of the old session's results goes with the rows that offered
    // them: the offers are no longer on screen, and a key opening what is behind
    // a row nobody can see is the one thing worse than not offering at all.
    held.kept.forget();
    renderer.empties()?;

    // All three of these are what a session picked up on the command line
    // gets, in the same order and for the same reason: the card at the top,
    // what it already said under the card, and what it costs to carry are
    // facts about the session rather than about which way reached it — so a
    // reader scrolling back after a `/resume` finds exactly the screen a
    // launch would have drawn.
    held.opening.commit(renderer)?;
    let pruned = conversation.session().take_pruned();
    let against = replaying::Replay::of(conversation.runner(), terms, &pruned);
    super::super::replaying::replayed(renderer, &against, conversation.session(), &mut held.kept)?;
    drop(pruned);
    super::super::resuming::asked(
        renderer,
        conversation.runner(),
        conversation.session(),
        terms,
        held.answers.keys,
    )
}

/// Offers what was worked on here: the picker, or the listing for a run that
/// reads no keys.
fn offered<T: Terminal>(
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    held: &mut Held<'_>,
    terms: &Terms,
) -> Result<Option<Compacting>, Fatal> {
    let listed = scanned(&terms.sessions);
    let here = Here::of(terms.workspace.root());

    // Read once, here, rather than per row: a list drawn against several
    // instants is several lists, each dated from a different now.
    let now = SystemTime::now();
    let columns = renderer.transcript_columns();

    // What opens is this directory's list, even where it has nothing on it:
    // Ctrl+A is how a reader reaches a session recorded somewhere else, and a
    // line ending the command would leave them no key to press. The line is
    // for a home that holds no session at all, and for a run that reads no
    // keys, which has nothing but this directory's list to print.
    let first = scoped(&listed, Scope::default(), &here);
    if listed.is_empty() || (first.is_empty() && !held.answers.keys) {
        let rows = [Row::new().then(Slot::Quiet, clip(NEVER, columns))];
        renderer.present(&rows)?;
        return Ok(None);
    }

    if !held.answers.keys {
        renderer.present(&listing(&chosen(&listed, &first), now, columns))?;
        return Ok(None);
    }

    stood(
        Reached { listed, here },
        renderer,
        conversation,
        held,
        terms,
    )
}

/// What the picker opens over: every session the index names, and the
/// directory, checkouts and branch the keys narrow them to.
struct Reached {
    listed: Vec<Recorded>,
    here: Here,
}

/// Every session the index names, whichever directory it was recorded in.
///
/// The whole index rather than the handful the welcome screen opens: this is
/// read after somebody asked for it, never on the way to the first frame, so it
/// can afford to open every log the index holds. Every directory is admitted,
/// once, and the keys narrow what came back in memory — a key that read the
/// index again would be a read of every log per press.
fn scanned(directory: &Path) -> Vec<Recorded> {
    recent(directory, Roots::Any, Reach::Indexed, usize::MAX)
}

/// Which sessions the picker's three keys leave on the list.
///
/// Three flags rather than one choice of three, because each key flips its
/// own: Ctrl+A pressed again goes back to whatever was shown before it, with
/// or without the other checkouts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Scope {
    /// Ctrl+A: every directory a session was recorded in.
    all: bool,
    /// Ctrl+W: this repository's other checkouts beside this one.
    worktrees: bool,
    /// Ctrl+B: only what was recorded on the branch checked out here.
    branch: bool,
}

/// Where the picker stands: what each session's directory and branch are held
/// against, read once when it opens.
struct Here {
    /// This workspace's root, spelled as a session's header spells it.
    root: PathBuf,
    /// This repository's other checkouts, which Ctrl+W adds. Each is spelled
    /// as a header spells its workspace, because `branching::worktrees` opens
    /// every one as a workspace and keeps the root it canonicalised.
    others: Vec<PathBuf>,
    /// The branch checked out here. `None` leaves Ctrl+B nothing to keep, so
    /// the key does nothing and the keys row leaves it out.
    branch: Option<String>,
    /// The home directory, which a directory is written under as `~`.
    home: Option<PathBuf>,
}

impl Here {
    /// What `root` is, read off its git directory without running `git`.
    fn of(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            others: crucible_app::branching::worktrees(root),
            branch: crucible_app::branching::current(root),
            home: std::env::home_dir(),
        }
    }

    /// Where `session` was recorded, as its row says it: nothing for this
    /// directory, whose sessions every row would otherwise repeat.
    fn place(&self, session: &Recorded) -> String {
        if session.workspace() == self.root {
            String::new()
        } else {
            homed(session.workspace(), self.home.as_deref())
        }
    }
}

/// The sessions `scope` leaves of `listed`, as places in it, newest first and
/// no more than [`OFFERED`].
///
/// A log with nothing asked in it is not in `listed` at all, so no key shows
/// one: what is filtered here is only which directory and which branch.
fn scoped(listed: &[Recorded], scope: Scope, here: &Here) -> Vec<usize> {
    let branch = here.branch.as_deref().filter(|_| scope.branch);

    listed
        .iter()
        .enumerate()
        .filter(|(_, session)| {
            let place = session.workspace();
            scope.all
                || place == here.root
                || (scope.worktrees && here.others.iter().any(|other| other == place))
        })
        .filter(|(_, session)| branch.is_none_or(|branch| session.branch() == Some(branch)))
        .map(|(at, _)| at)
        .take(OFFERED)
        .collect()
}

/// The sessions at `places` in `listed`, for the ways in that print rather
/// than stand.
fn chosen<'a>(listed: &'a [Recorded], places: &[usize]) -> Vec<&'a Recorded> {
    places.iter().filter_map(|&at| listed.get(at)).collect()
}

/// Whether `query` names `session`: its title, its branch, and the directory
/// its row shows where it shows one — what is on screen is what a reader
/// searches by.
fn sought(session: &Recorded, place: &str, query: &str) -> bool {
    finding::matches(session.title(), session.branch(), query)
        || (!place.is_empty() && finding::matches(place, None, query))
}

/// `path` with the home directory written `~`, and nothing in it a terminal
/// would act on: it was read off a session's header, as untrusted as the
/// title beside it. Spelled as typed first, since the header keeps the
/// spelling resolving gave, which on Windows is under no home a person has.
pub(super) fn homed(path: &Path, home: Option<&Path>) -> String {
    let path = crucible_workspace::typed(path);
    let said = match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => path.display().to_string(),
    };
    crate::cli::draw::flattened(said)
}

/// The directory a `cd` is handed, in the spelling the shell it is pasted
/// into reads back as that directory: under `~`, which a POSIX shell expands.
#[cfg(not(windows))]
fn commanded(path: &Path, home: Option<&Path>) -> String {
    posix_quoted(&homed(path, home))
}

/// The directory Windows is told to change to, before each shell's row quotes
/// it: whole, because cmd reads `~` as a directory of that name rather than
/// the home directory.
#[cfg(windows)]
fn commanded(path: &Path, _home: Option<&Path>) -> String {
    homed(path, None)
}

/// `place` as cmd reads it back as the same directory, or nothing where cmd
/// cannot be handed it.
///
/// Left bare where every character is one cmd neither acts on nor splits at,
/// and otherwise in double quotes, inside which cmd acts on nothing but `%`
/// (and `!`, only where delayed expansion was turned on, which it is not by
/// default). A double quote cannot be in a Windows name and would end the
/// quoting, so it is not written. `%NAME%` is expanded even inside the quotes
/// and no quoting stops that, so a directory holding a `%` gets no cmd row.
///
/// Built on every platform, though only Windows says it, so that what it
/// writes is tested wherever the tests run.
fn cmd_quoted(place: &str) -> Option<String> {
    let place: String = place.chars().filter(|&c| c != '"').collect();
    if place.contains('%') {
        return None;
    }
    let bare = !place.is_empty()
        && place
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@+.-:\\".contains(c));
    Some(if bare { place } else { format!("\"{place}\"") })
}

/// `place` as PowerShell reads it back as the same directory, handed to
/// `-LiteralPath`, which reads no wildcard in it.
///
/// Left bare where every character is one PowerShell does nothing with, and
/// otherwise in single quotes, inside which nothing is expanded or escaped.
/// PowerShell ends single quotes at a typographic one as well as at `'`, so
/// each of those is doubled too, which it reads back as one.
///
/// Built on every platform, though only Windows says it, so that what it
/// writes is tested wherever the tests run.
fn powershell_quoted(place: &str) -> String {
    let bare = !place.is_empty()
        && place
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.-:\\".contains(c));
    if bare {
        return place.to_owned();
    }

    let mut written = String::with_capacity(place.len() + 2);
    written.push('\'');
    for c in place.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
            written.push(c);
        }
        written.push(c);
    }
    written.push('\'');
    written
}

/// `place` as a POSIX shell reads it back as the same directory.
///
/// Left bare where every character is one no shell treats specially, which is
/// most directories, and otherwise in single quotes, inside which nothing is
/// special but the quote itself. A leading `~/` stays outside the quotes,
/// because quoted it is a directory named `~` rather than the home directory.
///
/// Written for every platform, though Windows never calls it, so that what it
/// writes is tested wherever the tests run.
#[cfg_attr(
    all(windows, not(test)),
    expect(
        dead_code,
        reason = "Windows writes its directories for cmd and PowerShell"
    )
)]
fn posix_quoted(place: &str) -> String {
    let (tilde, rest) = match place.strip_prefix("~/") {
        Some(rest) => ("~/", rest),
        None if place == "~" => return place.to_owned(),
        None => ("", place),
    };

    let bare = !rest.is_empty()
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if bare {
        return format!("{tilde}{rest}");
    }

    format!("{tilde}'{}'", rest.replace('\'', "'\\''"))
}

/// What Enter on a session recorded somewhere else says: the command that
/// picks it up in its own directory, a row at a time across `columns`.
///
/// Said rather than done, because a session stays bound to the directory it
/// was recorded in — every path it holds is that checkout's. This never
/// shortens the id, because a command with part of an id resumes nothing;
/// where a row is too wide, it is the directory that gives way, from its
/// front, so the end that names the project stays — and the cut is marked,
/// because a directory with its front missing is not one to run.
fn elsewhere(
    session: &Recorded,
    home: Option<&Path>,
    columns: usize,
    glyphs: Glyphs,
) -> Vec<String> {
    let place = commanded(session.workspace(), home);
    let resume = format!("crucible --resume {}", session.id().as_str());
    if cfg!(windows) {
        windows_said(&place, &resume, columns, glyphs)
    } else {
        posix_said(&place, &resume, columns, glyphs)
    }
}

/// The command a POSIX shell is told, `place` already quoted for it.
///
/// One row where it fits; otherwise broken after the `&&`, where a shell reads
/// on to the next line, so a copy of both rows still runs. A window under 56
/// columns still clips the row it is drawn on, which is the picker's to say.
fn posix_said(place: &str, resume: &str, columns: usize, glyphs: Glyphs) -> Vec<String> {
    let room = columns.saturating_sub(2);
    let whole = format!("cd {place} && {resume}");
    if wide(&whole) <= room {
        return vec![whole];
    }

    let place = cut(place, room.saturating_sub(wide("cd  &&")), glyphs);
    vec![format!("cd {place} &&"), resume.to_owned()]
}

/// What Windows is told: a row changing to `place` for cmd, one for
/// PowerShell, and the command both then run, each labelled.
///
/// No one line is read the same by cmd and by either PowerShell. Windows
/// PowerShell 5.1, the one Windows ships, refuses `&&`, and cmd does not end a
/// command at `;`. cmd's `cd` stays on the current drive unless given `/d`,
/// which PowerShell's `cd` refuses, and PowerShell's `cd` reads `[` and `]` in
/// a name as a wildcard unless told `-LiteralPath`, which cmd has no word for.
/// So each shell gets its own way in — `pushd`, which changes drive and maps
/// a network share, and `Set-Location -LiteralPath` — and the resume is its
/// own row, said once, rather than joined to either by a separator one of
/// them would refuse. A directory cmd cannot be handed gets no cmd row.
///
/// The labels stand beside their rows where the resume fits that way, and on
/// rows of their own where it does not, so the id is whole on its row in any
/// window at least 56 columns wide.
///
/// Built on every platform, though only Windows says it, so that what it
/// writes is tested wherever the tests run.
fn windows_said(place: &str, resume: &str, columns: usize, glyphs: Glyphs) -> Vec<String> {
    const CMD: &str = "cmd:";
    const POWERSHELL: &str = "PowerShell:";
    const THEN: &str = "then:";

    let room = columns.saturating_sub(2);
    let mut steps: Vec<(&str, &str, String)> = Vec::with_capacity(2);
    if let Some(quoted) = cmd_quoted(place) {
        steps.push((CMD, "pushd ", quoted));
    }
    steps.push((
        POWERSHELL,
        "Set-Location -LiteralPath ",
        powershell_quoted(place),
    ));

    let label = wide(POWERSHELL) + 1;
    let beside = label + wide(resume) <= room;
    let mut rows = Vec::with_capacity(6);
    for (name, verb, quoted) in steps {
        if beside {
            let fits = room.saturating_sub(label + wide(verb));
            rows.push(format!(
                "{name:<label$}{verb}{}",
                cut(&quoted, fits, glyphs)
            ));
        } else {
            let fits = room.saturating_sub(wide(verb));
            rows.push(name.to_owned());
            rows.push(format!("{verb}{}", cut(&quoted, fits, glyphs)));
        }
    }
    if beside {
        rows.push(format!("{THEN:<label$}{resume}"));
    } else {
        rows.push(THEN.to_owned());
        rows.push(resume.to_owned());
    }
    rows
}

/// `place` whole where it is at most `columns` wide, and otherwise its longest
/// end that is, behind a mark saying its front was cut.
fn cut(place: &str, columns: usize, glyphs: Glyphs) -> String {
    if wide(place) <= columns {
        return place.to_owned();
    }
    let mark = glyphs.ellipsis();
    format!(
        "{mark}{}",
        ending(place, columns.saturating_sub(wide(mark)))
    )
}

/// The longest end of `text` at most `columns` wide.
fn ending(text: &str, columns: usize) -> &str {
    text.char_indices()
        .map(|(at, _)| at)
        .chain(std::iter::once(text.len()))
        .find_map(|at| text.get(at..).filter(|rest| wide(rest) <= columns))
        .unwrap_or_default()
}

/// What the picker keeps between frames, and the frames' own workings beside
/// it.
///
/// The list is here rather than borrowed because a rename replaces it: the
/// title is written into the index, and the honest list is the one read back
/// off the index afterwards. The tails are here because a glimpse is a read of
/// the whole log, and the mark walking a list must not reread a log per row it
/// passes over — what was looked at once is kept for as long as the picker
/// stands.
struct Stood {
    /// The query, the marks and the staging, as `finding` moves them.
    standing: finding::Standing,
    /// Every session read, from every directory, newest first: what the
    /// scope leaves of it is what the list shows.
    listed: Vec<Recorded>,
    /// Which of them the keys leave on the list.
    scope: Scope,
    /// The session Enter was pressed on that was recorded somewhere else,
    /// while the line saying how to pick it up there still stands.
    told: Option<SessionId>,
    /// The tail of every session already previewed, by id. `None` where the
    /// log could not be read, which the pane shows as nothing to preview.
    cached: HashMap<String, Option<Glimpse>>,
    /// What that tail was drawn into, by id, at the width [`Stood::wide`]
    /// names. Drawing a session is walking every message of it, which is far
    /// too much to do again for each key the reader presses.
    drawn: HashMap<String, Vec<Row>>,
    /// The room the drawn rows were laid out against — the preview pane's, or
    /// `None` in a window that folded the pane away.
    ///
    /// Rows keep only while it holds: a window pulled wider is a pane nobody
    /// has drawn for yet, and rows drawn for the old one would leave the
    /// session looking narrower than the pane it is now in.
    wide: Option<usize>,
}

/// Stands the picker over the whole window, and picks up what came off it.
///
/// The narrowing is done inside the frame rather than before it, because what
/// the list holds is decided by what has been typed, and that changes under
/// the keys. So the frame that narrows is the frame that writes down what the
/// keys will walk next — marks included, since a query that emptied the list
/// under the mark leaves it standing past the end.
fn stood<T: Terminal>(
    reached: Reached,
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    held: &mut Held<'_>,
    terms: &Terms,
) -> Result<Option<Compacting>, Fatal> {
    let style = terms.style();
    let glyphs = style.glyphs();
    let now = SystemTime::now();

    let Reached { listed, here } = reached;
    let here = &here;
    let mut stood = Stood::opened(listed);

    // What a previewed session is drawn against. The session being drawn is
    // not the one this runner is in: what the runner is asked for is what each
    // tool's name and arguments read as, which is a fact about this build
    // rather than about the log being previewed.
    // Nothing beside it, because a preview is drawn from the tail of a log
    // nobody has opened: no pruning has been replayed, so there is nothing a
    // pruning cleared to put back.
    let uncleared = Pruned::default();
    let against = replaying::Replay {
        runner: conversation.runner(),
        pruned: &uncleared,
        style,
    };

    let ended = region::stand(
        renderer,
        |_| style,
        &mut stood,
        |stood, columns, room| {
            // A title Enter accepted is written down first, so the rows this
            // frame draws are the rows the index now holds. A rename that
            // could not be written shows the old title back, which is the
            // honest answer to where the new one went.
            if let Some(title) = stood.standing.saving.take() {
                let renamed = stood
                    .standing
                    .found
                    .get(stood.standing.marked)
                    .and_then(|&at| stood.listed.get(at))
                    .map(|session| session.id().clone());
                // Read back whole, and narrowed by the same keys as before,
                // so a rename changes a title and never what is shown.
                if let Some(id) = renamed {
                    stood.listed = saved(&title, &id, &terms.sessions);
                }
            }

            // Narrowed again on every frame, from the one read: the keys and
            // the query both change what the list holds.
            let scoped = scoped(&stood.listed, stood.scope, here);
            let total = scoped.len();
            stood.standing.found = scoped
                .into_iter()
                .filter(|&at| {
                    stood.listed.get(at).is_some_and(|session| {
                        sought(session, &here.place(session), stood.standing.query.text())
                    })
                })
                .collect();
            stood.standing.marked = stood
                .standing
                .marked
                .min(stood.standing.found.len().saturating_sub(1));

            let ages: Vec<String> = stood
                .standing
                .found
                .iter()
                .filter_map(|&at| stood.listed.get(at))
                .map(|session| when::ago(session.started(), now))
                .collect();
            let places: Vec<String> = stood
                .standing
                .found
                .iter()
                .filter_map(|&at| stood.listed.get(at))
                .map(|session| here.place(session))
                .collect();
            let kept: Vec<Kept<'_>> = stood
                .standing
                .found
                .iter()
                .filter_map(|&at| stood.listed.get(at))
                .zip(ages.iter().zip(&places))
                .map(|(session, (when, place))| Kept {
                    title: session.title(),
                    when,
                    branch: session.branch().unwrap_or_default(),
                    place,
                })
                .collect();

            // Read field by field rather than through `Stood::marked`, because
            // the caches below are written while this is held.
            let marked = stood
                .standing
                .found
                .get(stood.standing.marked)
                .and_then(|&at| stood.listed.get(at));

            // Said for as long as the mark stays on the session it is about.
            let notice = marked
                .filter(|session| stood.told.as_ref() == Some(session.id()))
                .map(|session| elsewhere(session, here.home.as_deref(), columns, glyphs))
                .unwrap_or_default();
            let notice: Vec<&str> = notice.iter().map(String::as_str).collect();

            // The marked session's tail, read once and kept. The window over
            // it is handed to the picker as a shorter slice: the pane shows
            // the end of what it is given, so scrolling back is cutting the
            // slice off before the tail.
            let pane = Picker::previewing(columns);
            if stood.wide != pane {
                stood.drawn.clear();
                stood.wide = pane;
            }

            let (full, meta_line): (&[Row], String) = match marked {
                Some(session) => {
                    let named = session.id().as_str().to_owned();
                    let looked = stood
                        .cached
                        .entry(named.clone())
                        .or_insert_with(|| peeked(&terms.sessions, &terms.workspace, session));
                    let line = meta(session, looked.as_ref(), now, glyphs);

                    match (pane, looked.as_ref()) {
                        (Some(pane), Some(held)) => (
                            stood
                                .drawn
                                .entry(named)
                                .or_insert_with(|| previewed(held, &against, pane))
                                .as_slice(),
                            line,
                        ),
                        _ => (&[], line),
                    }
                }
                None => (&[], String::new()),
            };

            stood.standing.over = furthest(full.len(), room, notice.len());
            stood.standing.behind = stood.standing.behind.min(stood.standing.over);
            let end = full.len().saturating_sub(stood.standing.behind);
            let windowed = full.get(..end).unwrap_or_default();

            let heading = heading(stood.standing.found.len(), total, stood.scope, here, glyphs);
            let branch = here.branch.as_deref().filter(|_| stood.scope.branch);
            let empty = nothing(stood.standing.query.text(), branch);
            let forms = if stood.standing.renaming.is_some() {
                renaming(glyphs)
            } else {
                keys(
                    glyphs,
                    !stood.standing.found.is_empty(),
                    stood.scope,
                    here.branch.is_some(),
                )
            };

            let typed = stood
                .standing
                .renaming
                .as_ref()
                .map_or(stood.standing.query.column(), Editor::column);

            let picker = Picker {
                heading: &heading,
                query: stood.standing.query.text(),
                typed,
                hint: HINT,
                sessions: &kept,
                marked: stood.standing.marked,
                renaming: stood.standing.renaming.as_ref().map(Editor::text),
                refused: stood.standing.refused.then_some(REFUSED),
                preview: windowed,
                preview_meta: &meta_line,
                // What Enter does belongs to whichever thing is open. With a
                // title being typed it is the keys row's to say, and a foot
                // still offering to resume would be the second answer.
                takes: if stood.standing.renaming.is_some() {
                    ""
                } else {
                    marked.map_or(TAKES, |session| taken(session, &here.root))
                },
                nothing: &empty,
                noview: NOVIEW,
                keys: &forms.iter().map(String::as_str).collect::<Vec<_>>(),
                notice: &notice,
                pointer: stood.standing.pointer,
            };

            // What this frame found under the pointer, written down for the
            // click and the wheel to read back: which pane a place falls on is
            // a fact about the picture, and this is where the picture is.
            stood.standing.lit = Some(picker.resting(columns, room));

            (
                picker.within(columns, room, glyphs),
                Some(picker.caret(columns, room, glyphs)),
            )
        },
        |arrived, stood| pressed(arrived, stood, here),
    )?;

    match ended {
        Ended::Took => {
            // Enter on an empty list is refused by the keys, so the mark
            // stands on a session — but the picker's answer is read back off
            // the list rather than assumed, the same way every taken mark is.
            let Some(id) = stood.marked().map(|session| session.id().clone()) else {
                return Ok(None);
            };
            picking(&id, renderer, conversation, held, terms)
        }
        Ended::Left => {
            super::say(renderer, LEFT)?;
            Ok(None)
        }
        // No room to stand it. The listing needs one row a session and no
        // keys at all, which is exactly what a window this small has room for.
        // Listed as the keys had left it, which is what the reader was
        // looking at when the window closed in.
        Ended::Cramped => {
            let places = scoped(&stood.listed, stood.scope, here);
            let rows = listing(
                &chosen(&stood.listed, &places),
                now,
                renderer.transcript_columns(),
            );
            renderer.present(&rows)?;
            Ok(None)
        }
    }
}

impl Stood {
    /// A picker over `listed`, with nothing typed, nothing marked past the
    /// first row, and every key's scope at this directory.
    fn opened(listed: Vec<Recorded>) -> Self {
        Self {
            standing: finding::Standing {
                query: Editor::new(),
                renaming: None,
                refused: false,
                saving: None,
                found: Vec::new(),
                marked: 0,
                behind: 0,
                over: 0,
                pointer: None,
                lit: None,
            },
            listed,
            scope: Scope::default(),
            told: None,
            cached: HashMap::new(),
            drawn: HashMap::new(),
            wide: None,
        }
    }

    /// The session the mark stands on, where the list has one.
    fn marked(&self) -> Option<&Recorded> {
        self.standing
            .found
            .get(self.standing.marked)
            .and_then(|&at| self.listed.get(at))
    }
}

/// What one key does to the picker.
///
/// The three that change which sessions are shown are read here, where the
/// sessions are; every other key is `finding`'s. While a title is being typed
/// every key is the rename's, these three with it, so Ctrl+W still rubs a word
/// out of the title.
fn pressed(arrived: Pressed, stood: &mut Stood, here: &Here) -> Moved {
    if stood.standing.renaming.is_none() {
        let flipped = match arrived {
            Pressed::All => Some(&mut stood.scope.all),
            Pressed::Key(Key::WordErase) => Some(&mut stood.scope.worktrees),
            Pressed::Background if here.branch.is_some() => Some(&mut stood.scope.branch),
            _ => None,
        };

        // A different list, so the mark goes back to its top: the row it
        // stood on was the old list's.
        if let Some(flag) = flipped {
            *flag = !*flag;
            stood.standing.marked = 0;
            stood.standing.behind = 0;
            stood.told = None;
            return Moved::Redraw;
        }
    }

    // What Enter said stands until the reader does something else. The
    // pointer passing over and the window changing size are not that.
    if !matches!(arrived, Pressed::Hovered { .. } | Pressed::Resized) {
        stood.told = None;
    }

    // Owned before the keys move anything under it: the title a rename opens
    // over is the marked row's, and the mark is about to be the key's
    // business.
    let titled = stood.marked().map(|session| session.title().to_owned());
    let moved = finding::sifting(arrived, &mut stood.standing, titled.as_deref());
    if moved != Moved::Took {
        return moved;
    }

    // Taken, but recorded somewhere else: the picker stays, with the mark
    // where it was, and says how to pick it up there.
    match stood.marked() {
        Some(session) if session.workspace() != here.root => {
            stood.told = Some(session.id().clone());
            Moved::Redraw
        }
        _ => Moved::Took,
    }
}

/// What the preview's foot says Enter does on `session`, standing at `root`.
fn taken(session: &Recorded, root: &Path) -> &'static str {
    if session.workspace() == root {
        TAKES
    } else {
        SHOWS
    }
}

/// The first [`SHOWN`] of them, for the ways in that print rather than stand.
fn shown<'a, 'b>(listed: &'b [&'a Recorded]) -> &'b [&'a Recorded] {
    listed.get(..SHOWN).unwrap_or(listed)
}

/// The listing, one row a session: the id, the age, and the title.
///
/// The id leads because it is the row's handle — the exact word `/resume` and
/// `--resume` take, for the runs that have no picker to walk.
fn listing(listed: &[&Recorded], now: SystemTime, columns: usize) -> Vec<Row> {
    let listed = shown(listed);
    let ages: Vec<String> = listed
        .iter()
        .map(|session| when::ago(session.started(), now))
        .collect();

    // Measured with `len` rather than by display width, which every other
    // width in this program is measured by. These are this module's own words
    // and this module's own digits — ASCII, one column each — and the string
    // being padded is the one being measured. What arrived from a file is the
    // title, which is not padded and not measured.
    let widest = ages.iter().map(String::len).max().unwrap_or_default();

    listed
        .iter()
        .zip(&ages)
        .map(|(session, age)| {
            let mut row = Row::new()
                .then(Slot::Accent, format!("{}  ", session.id().as_str()))
                .then(Slot::Quiet, format!("{age:widest$}  "));

            let room = columns.saturating_sub(row.columns());
            row.push(Slot::Plain, clip(session.title(), room));
            row
        })
        .collect()
}

/// Writes `title` over the session `id` names and reads the list back.
///
/// The read-back is the point: the title is written into the index, and the
/// list the picker goes on showing is the one the index now holds — a rename
/// that could not be written shows the old title back rather than a new one
/// that exists nowhere. It is read back as [`scanned`] read it first, so the
/// keys narrow it the same way.
fn saved(title: &str, id: &SessionId, directory: &Path) -> Vec<Recorded> {
    drop(retitle(directory, id, title));
    scanned(directory)
}

/// The tail of `session`, read through the door of the directory it was
/// recorded in: this one's where it is this one's, and otherwise the one its
/// header names, which the keys put on the list and the pane has to be able to
/// show. `None` where that directory has gone, as for a log that will not open.
fn peeked(directory: &Path, workspace: &Workspace, session: &Recorded) -> Option<Glimpse> {
    if session.workspace() == workspace.root() {
        return glimpse(directory, workspace, session.id()).ok();
    }
    let there = Workspace::open(session.workspace()).ok()?;
    glimpse(directory, &there, session.id()).ok()
}

/// The line under the preview: age, count, branch, and whether the session is
/// held open elsewhere.
///
/// The claim is said here, inline, rather than kept for a refusal: the reader
/// finds out while they are looking at the row, before Enter has closed the
/// picker over a session that would refuse to open.
fn meta(session: &Recorded, held: Option<&Glimpse>, now: SystemTime, glyphs: Glyphs) -> String {
    let count = session.messages();

    let mut parts = vec![when::ago(session.started(), now)];

    // Nought is what the index holds for a session that has not ended since
    // there were counts to hold, rather than a session nobody said anything
    // in — and a count of none over a pane full of conversation is the one
    // part of this line that could be wrong. Left out instead.
    if count > 0 {
        parts.push(format!(
            "{count} message{}",
            if count == 1 { "" } else { "s" }
        ));
    }

    if let Some(branch) = session.branch() {
        parts.push(branch.to_owned());
    }

    if held.is_some_and(Glimpse::busy) {
        parts.push("in use elsewhere".to_owned());
    }

    parts.join(&format!(" {} ", glyphs.dot()))
}

/// The tail of a session, drawn into `room` columns the way resuming it would
/// draw it.
///
/// Not spelled out here: this is the replay walk on a screen nobody sees, so
/// the prompt marks, the call lines, the rows results came back on and the
/// model's prose are the ones the transcript would hold. What the pane shows is
/// then what Enter would leave the reader looking at.
///
/// A tail the glimpse cut short opens on the mark that says so, so the first
/// words on the pane are not mistaken for the first words of the session.
fn previewed(held: &Glimpse, against: &replaying::Replay<'_>, room: usize) -> Vec<Row> {
    // A recording takes every write, so nothing here can fail to be drawn —
    // and an empty pane is what an unreadable log already shows.
    let mut rows = replaying::glimpsed(held.messages(), against, room, KEPT).unwrap_or_default();

    if held.cut() {
        rows.insert(0, Row::new().then(Slot::Quiet, clip(CUT, room)));
    }

    rows
}

/// How far back the preview window may stand over a tail of `rows`, in a
/// window `room` rows tall.
///
/// The pane shows the end of the slice it is handed, so scrolling back is
/// handing it a shorter one — and a slice shorter than the pane is a pane
/// standing half empty rather than one scrolled back. The floor is therefore
/// the pane's own count, which it is the pane's to say.
fn furthest(rows: usize, room: usize, notice: usize) -> usize {
    rows.saturating_sub(Picker::previews(room, notice))
}

/// The line over the panes: what this is, how much of what the keys left the
/// query left, and where the sessions were recorded — this directory, every
/// directory, or this repository's checkouts — with the branch while Ctrl+B
/// keeps only it. The branch goes before the place, because a directory can be
/// longer than the row and it is the end of the row a narrow window cuts.
///
/// The lead says what the screen is for, because a picker that opens on a
/// count alone reads as a report on sessions rather than as a way into one.
fn heading(found: usize, total: usize, scope: Scope, here: &Here, glyphs: Glyphs) -> String {
    let dot = glyphs.dot();
    let place = if scope.all {
        EVERYWHERE.to_owned()
    } else if scope.worktrees {
        CHECKOUTS.to_owned()
    } else {
        homed(&here.root, here.home.as_deref())
    };

    let mut said = format!("Resume a session {dot} {found} of {total} {dot} ");
    if let Some(branch) = here.branch.as_deref().filter(|_| scope.branch) {
        said.push_str(&crate::cli::draw::flattened(branch));
        said.push(' ');
        said.push_str(dot);
        said.push(' ');
    }
    said.push_str(&place);
    said
}

/// What the list says where nothing is left on it.
///
/// The query is quoted back rather than described, because what a reader
/// checks first is whether the thing they typed is the thing they meant to
/// type. With nothing typed, what emptied the list is the branch Ctrl+B keeps,
/// the one key that can leave this directory's list with nothing on it — or
/// this directory never recorded one, and the picker opened anyway because
/// another did. The branch is flattened as the heading flattens it, so the two
/// rows never spell one branch two ways.
fn nothing(query: &str, branch: Option<&str>) -> String {
    match branch {
        _ if !query.is_empty() => format!("no session holds \"{query}\""),
        Some(branch) => format!("no session on {}", crate::cli::draw::flattened(branch)),
        None => NEVER.to_owned(),
    }
}

/// Every form of the keys row, longest first, for a list with something on it
/// or without: the picker draws the first the window has room for.
///
/// Built rather than written down, because the arrows in it are the setting's:
/// a terminal without them draws hollow squares on the one row that exists to
/// be read by somebody who does not yet know. Between the long form and the
/// short come the middle ones, which an eighty-column window gets: each toggle
/// named by what it does next in a word or two, with the keys that explain
/// themselves — the arrows and typing — dropped first and Ctrl+R next. The
/// short form is what a window with no room for those gets — the same keys,
/// without the words saying what each of them moves.
///
/// A list the query left empty is offered neither, because there is nothing to
/// walk to and nothing to rename: what is left to do is narrow the query, change
/// what the keys show, or leave, and a row naming keys that do nothing is worse
/// than a shorter one.
///
/// Each of the three keys that change what is shown is named by what pressing
/// it does next, since that is the question a reader brings to it. Ctrl+B is
/// left out where no branch is checked out here, because it then does nothing.
fn keys(glyphs: Glyphs, listed: bool, scope: Scope, branched: bool) -> Vec<String> {
    let (up, down) = glyphs.walking();
    let dot = glyphs.dot();

    let (all, all_next) = if scope.all {
        ("ctrl+a to show this project", "ctrl+a this project")
    } else {
        ("ctrl+a to show all projects", "ctrl+a all projects")
    };
    let (branch, branch_next) = if scope.branch {
        ("ctrl+b to show all branches", "ctrl+b all branches")
    } else {
        ("ctrl+b to only show this branch", "ctrl+b this branch")
    };
    let (worktrees, worktrees_next) = if scope.worktrees {
        ("ctrl+w to hide other worktrees", "ctrl+w hide worktrees")
    } else {
        ("ctrl+w to show all worktrees", "ctrl+w worktrees")
    };

    let mut long: Vec<String> = Vec::new();
    let mut toggles: Vec<&str> = Vec::new();
    let mut short: Vec<String> = Vec::new();
    if listed {
        long.extend([format!("{up}{down} to walk"), "ctrl+r to rename".to_owned()]);
        short.extend([
            format!("{up}{down}"),
            "enter".to_owned(),
            "ctrl+r".to_owned(),
        ]);
    } else {
        long.push("type to narrow".to_owned());
        short.push("type to narrow".to_owned());
    }

    long.push(all.to_owned());
    toggles.push(all_next);
    short.push("ctrl+a".to_owned());
    if branched {
        long.push(branch.to_owned());
        toggles.push(branch_next);
        short.push("ctrl+b".to_owned());
    }
    long.push(worktrees.to_owned());
    toggles.push(worktrees_next);
    short.push("ctrl+w".to_owned());

    if listed {
        long.push("type to search".to_owned());
    }
    long.push("esc to cancel".to_owned());
    toggles.push("esc");
    short.push("esc".to_owned());

    let joint = format!(" {dot} ");
    let mut forms = vec![long.join(&joint)];
    if listed {
        forms.push(format!("ctrl+r rename{joint}{}", toggles.join(&joint)));
    }
    forms.push(toggles.join(&joint));
    forms.push(short.join(&joint));
    forms
}

/// The keys row while a title is being renamed.
///
/// Its own row rather than the walking one with a word changed: with a title
/// open none of walking, renaming or searching is what a key does, and the row
/// that says what the keys do is the one place a reader finds out which mode
/// they are in.
fn renaming(glyphs: Glyphs) -> Vec<String> {
    let dot = glyphs.dot();

    vec![
        format!("enter to save {dot} esc to cancel"),
        format!("enter {dot} esc"),
    ]
}

#[cfg(test)]
mod tests;
