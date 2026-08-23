//! `/resume`: what was worked on in this directory, and picking one of them
//! back up.
//!
//! A session is named by twenty characters nobody types, so the list is
//! numbered and a number is what is picked by. The numbers belong to the list
//! as it was printed: it is read again when one is chosen, and the answer names
//! the session that was picked up, so a list that changed underneath — another
//! crucible in the same directory, finishing a session while this one read — is
//! visible rather than silent.
//!
//! Picking one up leaves nothing behind. The session being left is closed here,
//! which is the last chance to say that its log stopped being written, and what
//! it was allowed for the rest of *its* run is forgotten by the runner — see
//! [`Runner::pick_up`]. The record of what has been read is emptied with it,
//! because it answers for the session being left rather than for this run.
//!
//! The screen goes the same way. What is put back is a different conversation
//! rather than the next thing that happened in the one on it, so the transcript
//! is emptied and the session picked up replaces it — and what was held behind
//! the rows of the old one is dropped with them, because a key that opens what
//! is behind a row nobody can see is worse than a row that offers nothing.

use std::time::SystemTime;

use crucible_core::Compacting;
use crucible_runner::{Recorded, Runner, Session, recent};
use crucible_tui::{Glyphs, Renderer, Row, Slot, Terminal, clip};

use crate::cli::Fatal;
use crate::cli::draw::when;

use super::super::Held;
use super::Terms;

/// How many sessions the list holds.
///
/// Nine, so every number on it is one character and the list is read in one
/// glance. What is older than the last nine sessions in one directory is a
/// directory listing, and the session directory is already that.
const SHOWN: usize = 9;

/// Runs it: the list, or the session `said` picked out of it.
pub(super) fn run<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    runner: &mut Runner,
    held: &mut Held<'_>,
    terms: &Terms,
) -> Result<Option<Compacting>, Fatal> {
    let listed = recent(&terms.sessions, &terms.workspace, SHOWN);

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

    if said.is_empty() {
        renderer.present(&listing(&listed, now, columns))?;
        return Ok(None);
    }

    let Some(picked) = chosen(said, &listed) else {
        // The word came off the line and was never shape-checked — anything at
        // all can follow `/resume ` — so it goes out the way arrived text does.
        renderer.commit(&format!("! {said} is not on the list"))?;
        renderer.present(&listing(&listed, now, columns))?;
        return Ok(None);
    };

    picking(picked, renderer, runner, held, terms)
}

/// Picks one up, having decided which.
fn picking<T: Terminal>(
    picked: &Recorded,
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
    if runner.session().id() == Some(picked.id()) {
        let rows = [Row::new().then(Slot::Quiet, clip("this is the session you are in", columns))];
        renderer.present(&rows)?;
        return Ok(None);
    }

    let (session, transcript) =
        match Session::reopen(&terms.sessions, &terms.workspace, picked.id()) {
            Ok(picked) => picked,
            // A path is in every one of these, so it is committed rather than
            // presented. Nothing else changes: the session in hand is still
            // being recorded, and the loop carries on with it.
            Err(problem) => {
                renderer.commit(&format!("! {problem}"))?;
                return Ok(None);
            }
        };

    let messages = transcript.len();
    let left = runner.pick_up(session, transcript);

    // The files remembered were read by the session just left, and `write`
    // replaces a file on the strength of that record. The session picked up saw
    // none of them, however much of it comes back off the disk: what a log holds
    // is what was said, not what the tools of that run had looked at.
    terms.ledger.forget();

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

    renderer.present(
        // Read here rather than carried in: one row is dated against one
        // instant however it was reached, unlike the list above, which is
        // several rows and has to share one.
        &picked_up(
            picked,
            messages,
            SystemTime::now(),
            columns,
            terms.style().glyphs(),
        ),
    )?;

    // Both of these are what a session picked up on the command line gets, and
    // for the same reason: what it already said, and what it costs to carry,
    // are facts about the session rather than about which way reached it.
    super::super::replaying::replayed(
        renderer,
        runner,
        &terms.workspace,
        &mut held.kept,
        terms.style(),
    )?;
    super::super::resuming::asked(renderer, runner, terms, held.answers.keys)
}

/// The rows that say what was picked up.
///
/// It names the session rather than the number that reached it, because the
/// list is read again between being printed and being picked from.
fn picked_up(
    picked: &Recorded,
    held: usize,
    now: SystemTime,
    columns: usize,
    glyphs: Glyphs,
) -> Vec<Row> {
    let said = match held {
        1 => "1 message".to_owned(),
        _ => format!("{held} messages"),
    };

    vec![
        Row::new().then(Slot::Plain, clip(picked.asked(), columns)),
        Row::new().then(
            Slot::Quiet,
            clip(
                &format!(
                    "{said} {} started {}",
                    glyphs.dot(),
                    when::ago(picked.started(), now)
                ),
                columns,
            ),
        ),
    ]
}

/// The list, numbered from one.
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
        .enumerate()
        .map(|(at, (session, age))| {
            let mut row = Row::new()
                .then(Slot::Accent, format!("{}  ", at + 1))
                .then(Slot::Quiet, format!("{age:widest$}  "));

            let room = columns.saturating_sub(row.columns());
            row.push(Slot::Plain, clip(session.asked(), room));
            row
        })
        .collect()
}

/// Which session `said` names, if it names one.
///
/// A number, and nothing else. Naming a session by its identifier would be a
/// second way in that nothing on screen ever offers, and the list is what the
/// numbers mean.
fn chosen<'a>(said: &str, listed: &'a [Recorded]) -> Option<&'a Recorded> {
    listed.get(said.parse::<usize>().ok()?.checked_sub(1)?)
}

#[cfg(test)]
mod tests;
