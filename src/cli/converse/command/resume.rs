//! `/resume`: what was worked on in this directory, and picking one of them
//! back up.
//!
//! A session is named by its id — the same word `--resume` takes and the
//! parting message prints — so an id given here is picked up directly, and
//! anything else stands the picker: a search line over the sessions recorded
//! in this workspace, with a window over the tail of whichever one is marked.
//! What the picker looks like is [`Picker`]'s; which sessions a query keeps
//! and what each key moves is `finding`'s; what is listed, previewed and
//! written down is decided here, where the sessions are. A run with no
//! keyboard has no picker to walk, so it is given the listing instead, each
//! row carrying the exact id `/resume` and `--resume` take.
//!
//! Picking one up leaves nothing behind. The session being left is closed
//! here, which is the last chance to say that its log stopped being written,
//! and what it was allowed for the rest of *its* run is forgotten by the
//! runner — see [`Runner::pick_up`]. The record of what has been read is
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
use std::path::Path;
use std::str::FromStr as _;
use std::time::SystemTime;

use crucible_core::{Compacting, SessionId, Workspace};
use crucible_runner::{Glimpse, Recorded, Runner, Session, SessionError, glimpse, recent, retitle};
use crucible_tui::{Editor, Glyphs, Kept, Picker, Renderer, Row, Slot, Terminal, clip};

use crate::cli::Fatal;
use crate::cli::draw::when;

use super::super::region::{self, Ended};
use super::super::{Held, finding, replaying};
use super::Terms;

/// How many sessions the picker is handed.
///
/// The search line is what reaches past the visible rows, so the ceiling is
/// about how far back a query looks rather than how tall a window is. What is
/// older than this in one directory is a directory listing, and the session
/// directory is already that.
const OFFERED: usize = 64;

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
const HINT: &str = "a title, or a branch";

/// What the preview pane says where the query left nothing to preview.
const NOVIEW: &str = "nothing to preview";

/// How many drawn rows of one session the pane keeps.
///
/// Enough to fill any pane several times over, so wheeling back through a
/// preview reaches further than a glance — and bounded, because these are kept
/// for every session the mark passes over and a long conversation draws into
/// thousands of rows.
const KEPT: usize = 256;

/// What Enter does, said under the marked session's metadata.
const TAKES: &str = "enter to resume";

/// What is said where the picker was left with nothing taken.
const LEFT: &str = "cancelled, no session picked up";

/// Runs it: the picker, or the session `said` picked up by id.
pub(super) fn run<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    held: &mut Held<'_>,
    terms: &Terms,
) -> Result<Option<Compacting>, Fatal> {
    let said = said.trim();
    if said.is_empty() {
        return offered(renderer, runner, held, terms);
    }

    // Anything at all can follow `/resume `, and a word that is not even
    // shaped like an id is refused the same way one nothing here answers to
    // is: neither names a session recorded in this workspace, and whether
    // that is spelling or absence is nothing the reader can act on
    // differently. What to try instead follows the refusal.
    let Ok(id) = SessionId::from_str(said) else {
        renderer.commit(&format!("! no session {said} in this workspace"))?;
        return offered(renderer, runner, held, terms);
    };

    picking(&id, renderer, runner, held, terms)
}

/// Picks the session `id` names back up.
fn picking<T: Terminal>(
    id: &SessionId,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    held: &mut Held<'_>,
    terms: &Terms,
) -> Result<Option<Compacting>, Fatal> {
    let columns = renderer.columns();

    // Answered before the log is opened. This session's own claim is on that
    // file, so continuing it would come back as "open in another crucible" —
    // which names the wrong crucible, and reads as a reason to go and close
    // something.
    if runner.session().id() == Some(id) {
        let rows = [Row::new().then(Slot::Quiet, clip("this is the session you are in", columns))];
        renderer.present(&rows)?;
        return Ok(None);
    }

    let (session, transcript) = match Session::reopen(&terms.sessions, &terms.workspace, id) {
        Ok(picked) => picked,

        // The one shape of failure the reader can act on from here: the id
        // names nothing recorded in this workspace, so what is recorded is
        // offered instead.
        Err(SessionError::Unknown { .. }) => {
            renderer.commit(&format!("! no session {} in this workspace", id.as_str()))?;
            return offered(renderer, runner, held, terms);
        }

        // A path is in every one of these, so it is committed rather than
        // presented. Nothing else changes: the session in hand is still
        // being recorded, and the loop carries on with it.
        Err(problem) => {
            renderer.commit(&format!("! {problem}"))?;
            return Ok(None);
        }
    };

    let left = runner.pick_up(session, transcript);

    // The files remembered were read by the session just left, and `write`
    // replaces a file on the strength of that record. The session picked up saw
    // none of them, however much of it comes back off the disk: what a log holds
    // is what was said, not what the tools of that run had looked at.
    terms.ledger.forget();

    // The plan the panel is drawn from goes the same way, and then comes back
    // as the session picked up left it — the same read a `--continue` does, so
    // resuming here or from the command line stands the same plan over the box.
    terms.plan.forget();
    crate::cli::startup::planned(&terms.plan, runner.transcript());

    // The tools looked up belong to the conversation that looked them up. Left
    // standing they would be advertised to a session that never asked.
    terms.revealed.forget();

    // The images pasted were named by markers in prompts of the session just
    // left, and the numbering starts over with the session. Held on, one would
    // ride the first prompt after the resume that says `[image 1]`.
    held.images.clear();

    // The last chance to say that the log of the session being left stopped
    // being written. After this there is no session to say it about.
    if let Some(problem) = left.finish() {
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
    super::super::replaying::replayed(
        renderer,
        runner,
        &terms.workspace,
        &mut held.kept,
        terms.style(),
    )?;
    super::super::resuming::asked(renderer, runner, terms, held.answers.keys)
}

/// Offers what was worked on here: the picker, or the listing for a run that
/// reads no keys.
fn offered<T: Terminal>(
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    held: &mut Held<'_>,
    terms: &Terms,
) -> Result<Option<Compacting>, Fatal> {
    let listed = recent(&terms.sessions, &terms.workspace, OFFERED);

    // Read once, here, rather than per row: a list drawn against several
    // instants is several lists, each dated from a different now.
    let now = SystemTime::now();
    let columns = renderer.columns();

    if listed.is_empty() {
        let rows = [Row::new().then(
            Slot::Quiet,
            clip("nothing has been worked on here yet", columns),
        )];
        renderer.present(&rows)?;
        return Ok(None);
    }

    if !held.answers.keys {
        renderer.present(&listing(shown(&listed), now, columns))?;
        return Ok(None);
    }

    stood(listed, renderer, runner, held, terms)
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
    /// The sessions offered, in the order the list shows them.
    listed: Vec<Recorded>,
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
    listed: Vec<Recorded>,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    held: &mut Held<'_>,
    terms: &Terms,
) -> Result<Option<Compacting>, Fatal> {
    let style = terms.style();
    let glyphs = style.glyphs();
    let now = SystemTime::now();
    let total = listed.len();
    let root = terms.workspace.root().display().to_string();
    let empty = nothing(glyphs);
    let (long, short) = keys(glyphs);

    let mut stood = Stood {
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
        cached: HashMap::new(),
        drawn: HashMap::new(),
        wide: None,
    };

    // What a previewed session is drawn against. The session being drawn is
    // not the one this runner is in: what the runner is asked for is what each
    // tool's name and arguments read as, which is a fact about this build
    // rather than about the log being previewed.
    let against = replaying::Replay {
        runner,
        workspace: &terms.workspace,
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
                if let Some(id) = renamed {
                    stood.listed = saved(&title, &id, &terms.sessions, &terms.workspace);
                }
            }

            let found: Vec<usize> = {
                let query = stood.standing.query.text();
                stood
                    .listed
                    .iter()
                    .enumerate()
                    .filter(|(_, session)| {
                        finding::matches(session.title(), session.branch(), query)
                    })
                    .map(|(at, _)| at)
                    .collect()
            };
            stood.standing.found = found;
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
            let kept: Vec<Kept<'_>> = stood
                .standing
                .found
                .iter()
                .filter_map(|&at| stood.listed.get(at))
                .zip(&ages)
                .map(|(session, when)| Kept {
                    title: session.title(),
                    when,
                    branch: session.branch().unwrap_or_default(),
                })
                .collect();

            let marked = stood
                .standing
                .found
                .get(stood.standing.marked)
                .and_then(|&at| stood.listed.get(at));

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
                    let looked = stood.cached.entry(named.clone()).or_insert_with(|| {
                        glimpse(&terms.sessions, &terms.workspace, session.id()).ok()
                    });
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

            stood.standing.over = full.len().saturating_sub(1);
            stood.standing.behind = stood.standing.behind.min(stood.standing.over);
            let end = full.len().saturating_sub(stood.standing.behind);
            let windowed = full.get(..end).unwrap_or_default();

            let heading = format!(
                "{} of {} sessions {} {}",
                stood.standing.found.len(),
                total,
                glyphs.dot(),
                root
            );

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
                preview: windowed,
                preview_meta: &meta_line,
                takes: TAKES,
                nothing: &empty,
                noview: NOVIEW,
                keys: (&long, &short),
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
        |arrived, stood| {
            // Owned before the keys move anything under it: the title a rename
            // opens over is the marked row's, and the mark is about to be the
            // key's business.
            let titled = stood
                .standing
                .found
                .get(stood.standing.marked)
                .and_then(|&at| stood.listed.get(at))
                .map(|session| session.title().to_owned());
            finding::sifting(arrived, &mut stood.standing, titled.as_deref())
        },
    )?;

    match ended {
        Ended::Took => {
            // Enter on an empty list is refused by the keys, so the mark
            // stands on a session — but the picker's answer is read back off
            // the list rather than assumed, the same way every taken mark is.
            let Some(id) = stood
                .standing
                .found
                .get(stood.standing.marked)
                .and_then(|&at| stood.listed.get(at))
                .map(|session| session.id().clone())
            else {
                return Ok(None);
            };
            picking(&id, renderer, runner, held, terms)
        }
        Ended::Left => {
            super::say(renderer, LEFT)?;
            Ok(None)
        }
        // No room to stand it. The listing needs one row a session and no
        // keys at all, which is exactly what a window this small has room for.
        Ended::Cramped => {
            renderer.present(&listing(shown(&stood.listed), now, renderer.columns()))?;
            Ok(None)
        }
    }
}

/// The first [`SHOWN`] of them, for the ways in that print rather than stand.
fn shown(listed: &[Recorded]) -> &[Recorded] {
    listed.get(..SHOWN).unwrap_or(listed)
}

/// The listing, one row a session: the id, the age, and the title.
///
/// The id leads because it is the row's handle — the exact word `/resume` and
/// `--resume` take, for the runs that have no picker to walk.
fn listing(listed: &[Recorded], now: SystemTime, columns: usize) -> Vec<Row> {
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
/// that exists nowhere.
fn saved(title: &str, id: &SessionId, directory: &Path, workspace: &Workspace) -> Vec<Recorded> {
    drop(retitle(directory, id, title));
    recent(directory, workspace, OFFERED)
}

/// The line under the preview: age, count, branch, and whether the session is
/// held open elsewhere.
///
/// The claim is said here, inline, rather than kept for a refusal: the reader
/// finds out while they are looking at the row, before Enter has closed the
/// picker over a session that would refuse to open.
fn meta(session: &Recorded, held: Option<&Glimpse>, now: SystemTime, glyphs: Glyphs) -> String {
    let count = session.messages();

    let mut parts = vec![
        when::ago(session.started(), now),
        format!("{count} message{}", if count == 1 { "" } else { "s" }),
    ];

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
        rows.insert(
            0,
            Row::new().then(Slot::Quiet, against.style.glyphs().ellipsis().to_owned()),
        );
    }

    rows
}

/// What the list says where the query left nothing on it.
///
/// The way out is named beside the fact, because an empty pane under a line
/// with words in it is the one place here where a reader can be stuck without
/// knowing which key gets them out. Built from the glyph set for the dash: a
/// terminal without one draws a hollow square in the middle of the sentence.
fn nothing(glyphs: Glyphs) -> String {
    format!("nothing matches {} backspace to widen it", glyphs.dash())
}

/// The keys row, long and short.
///
/// Built rather than written down, because the arrows in it are the setting's:
/// a terminal without them draws hollow squares on the one row that exists to
/// be read by somebody who does not yet know. The short form is what a window
/// with no room for the long one gets — the same keys, without the words
/// saying what each of them moves.
fn keys(glyphs: Glyphs) -> (String, String) {
    let (up, down) = glyphs.walking();
    let dot = glyphs.dot();

    (
        format!("{up}{down} session {dot} enter resumes {dot} ctrl+r renames {dot} esc to cancel"),
        format!("{up}{down} {dot} enter {dot} ctrl+r {dot} esc"),
    )
}

#[cfg(test)]
mod tests;
