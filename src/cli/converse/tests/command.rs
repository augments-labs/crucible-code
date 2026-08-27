//! What a command does to the loop it was typed into.
//!
//! Which lines are commands at all is settled in [`super::super::command`],
//! over strings and without a terminal. What is left for here is everything
//! that needs the loop running: that a command costs the provider nothing, that
//! it answers on the screen, that `/mode` moves the mode the next prompt is
//! taken under, and that `/exit` ends the session with the lines after it
//! unread.

use std::cell::Cell;
use std::io::Cursor;
use std::sync::atomic::Ordering;

use crucible_auth::StoredCredentials;
use crucible_core::{
    Cancel, Delta, Message, Mode, Permission, Revealed, Rules, StopReason, ToolId, Workspace,
};
use crucible_runner::{Session, Tools};
use crucible_tools::Ledger;
use crucible_tui::{Recording, Renderer};

use crate::cli::converse::{Terms, converse};
use crate::cli::fake::Script;
use crate::cli::sample::Sample;

use super::{opening, over, plain, saying, scripted};

/// Terms recording to a tree of `sample`'s own, so a command that starts or
/// picks up a session has somewhere to do it — over the record the tools of
/// such a run were built with.
fn recording(sample: &Sample, ledger: &Ledger) -> Terms {
    Terms {
        ledger: ledger.clone(),
        revealed: Revealed::new(),
        sessions: sample.logs(),
        workspace: sample.workspace(),
        ..plain()
    }
}

/// One tool call, with the arguments written out.
///
/// [`super::calling`] sends none, because what it drives are tools that answer
/// from a field. These are the shipped ones and read what they were sent.
fn call(name: &str, args: &str) -> Vec<Delta> {
    vec![
        Delta::ToolStarted {
            id: ToolId::new("a"),
            name: name.into(),
        },
        Delta::ToolArgs(args.into()),
        Delta::Stopped(StopReason::WantsTools),
    ]
}

/// The whole loop over `offered`, in full access so a tool runs without a
/// question drawn: what the terminal ended up with, and how many requests the
/// script was given.
fn reaching(
    terms: &Terms,
    offered: Tools,
    rounds: Vec<Vec<Delta>>,
    typed: &str,
) -> (String, usize) {
    let script = Script::new(rounds);
    let asked = script.asked();
    let runner =
        scripted(script, offered).permitting(Permission::with(Mode::FullAccess, Rules::new()));

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(typed.as_bytes().to_vec());

    converse(runner, &mut renderer, terms, &opening(), &mut input).expect("the loop to finish");

    (
        renderer.terminal().written().to_string(),
        asked.load(Ordering::Relaxed),
    )
}

/// A workspace holding one file nobody has looked at, and the record the pair
/// that could replace it share.
///
/// The pair is the shipped `read` and `write` rather than anything written for
/// a test: what is being watched is the record the wiring hands them, and a
/// stand-in for either half would be a second opinion about the thing in
/// question. The concrete types are named here and go no further — this builds
/// a `Tools` and hands that over.
fn untouched(sample: &Sample, ledger: &Ledger) -> Tools {
    std::fs::write(sample.root().join("one.txt"), "work nobody looked at\n")
        .expect("a file in the workspace");

    let workspace: Workspace = sample.workspace();
    let mut offered = Tools::new();
    offered.add(Box::new(crucible_tools::Read::new(
        workspace.clone(),
        Cancel::new(),
        ledger.clone(),
    )));
    offered.add(Box::new(crucible_tools::Write::new(
        workspace,
        ledger.clone(),
    )));
    offered
}

/// The rounds that read that file and then replace it, one prompt each.
fn looking_then_replacing() -> Vec<Vec<Delta>> {
    vec![
        call("read", r#"{"path":"one.txt"}"#),
        saying("looked at it"),
        call("write", r#"{"path":"one.txt","content":"replaced\n"}"#),
        saying("replaced it"),
    ]
}

fn commanding(typed: &str) -> (String, usize) {
    over(Script::new(vec![saying("answered")]), Tools::new(), typed)
}

/// The same loop, over a named model of a named provider.
///
/// Which model is in force is the whole question `/effort` answers, and the one
/// [`scripted`] hands back is a name no vendor serves — which is what every
/// other test here wants and the one thing this one cannot use.
fn asking(provider: &'static str, model: &str, typed: &str) -> String {
    let terms = Terms {
        provider: Cell::new(Some(provider)),
        ..plain()
    };

    let mut runner = scripted(Script::new(vec![]), Tools::new());
    runner.ask(model, 8192, None, None);

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(typed.as_bytes().to_vec());

    converse(runner, &mut renderer, &terms, &opening(), &mut input).expect("the loop to finish");
    renderer.terminal().written().to_string()
}

#[test]
fn a_command_is_answered_here_rather_than_by_the_model() {
    let (written, asked) = commanding("/help\n");

    assert_eq!(asked, 0, "{written}");
    assert!(written.contains("/model"), "{written}");
    assert!(written.contains("pick which model answers"), "{written}");
}

#[test]
fn the_model_a_session_is_asking_is_the_one_it_was_built_with() {
    let (written, _) = commanding("/model\n");

    assert!(written.contains("script"), "{written}");
}

#[test]
fn model_down_a_pipe_lists_every_provider_beside_its_models() {
    // Naming none opens the panel where there is a keyboard to walk it with.
    // Down a pipe there is not, so the same line answers the question the panel
    // would have asked: under the model in force, the ones a name would reach
    // without anybody going to look up how the vendor spells them.
    let (written, asked) = commanding("/model\n");

    assert_eq!(asked, 0, "{written}");
    assert!(written.contains("script"), "{written}");
    for provider in crate::cli::PROVIDERS {
        for model in provider.models {
            assert!(
                written.contains(&format!("/model {}/{}", provider.name, model.name)),
                "{written}"
            );
        }
    }
}

#[test]
fn model_lists_providers_that_are_not_active_yet() {
    // A key says a provider can be reached; it does not choose one. A row can
    // be shown before its key exists — taking it then meets the missing key as
    // a missing key, rather than the catalog hiding half of itself.
    let (written, _) = commanding("/model\n");

    assert!(written.contains("openai/gpt-"), "{written}");
    assert!(written.contains("moonshot/kimi-"), "{written}");
}

#[test]
fn a_model_taken_mid_session_is_what_the_next_turn_is_told_it_is() {
    // The prompt says which model is answering and at which rung, because a
    // model can find out neither for itself. `/model` and `/effort` are
    // somebody changing exactly those two things — so a prompt written once at
    // startup would go on naming the model the session opened with, and every
    // turn after the first would be told something false about itself.
    let script = Script::new(vec![saying("answered")]);
    let under = script.under();
    let runner = scripted(script, Tools::new());

    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(b"/model claude-haiku-4-5\n/effort max\nwhat are you\n".to_vec());

    converse(runner, &mut renderer, &plain(), &opening(), &mut input).expect("the loop to finish");

    let under = under.lock().expect("what the turn was asked under");
    let said = under.last().expect("one turn was taken");

    assert!(said.contains("claude-haiku-4-5"), "{said}");
    assert!(said.contains("max effort"), "{said}");
    assert!(!said.contains("script"), "{said}");
}

#[test]
fn a_model_named_on_the_line_is_written_down_under_a_provider_and_beside_it() {
    // Both halves, because either one alone leaves the next run here asking a
    // question this command just answered: the model says what to ask for, and
    // the provider is the only key that says who to ask. A file holding the
    // first and not the second is a machine with two keys picking a vendor by
    // whichever it finds, and then asking it for somebody else's model.
    let sample = Sample::new("model-named");
    let choosing = sample.root().join("config.json");
    let terms = Terms {
        choosing: choosing.clone(),
        ..plain()
    };

    let runner = scripted(Script::new(vec![saying("answered")]), Tools::new());
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(b"/model claude-haiku-4-5\n".to_vec());

    converse(runner, &mut renderer, &terms, &opening(), &mut input).expect("the loop to finish");

    let written = renderer.terminal().written().to_string();
    assert!(written.contains("anthropic/claude-haiku-4-5"), "{written}");

    let held = std::fs::read_to_string(&choosing).expect("the file it said it wrote");
    assert!(held.contains("\"provider\": \"anthropic\""), "{held}");
    assert!(held.contains("\"model\": \"claude-haiku-4-5\""), "{held}");
}

#[test]
fn a_rung_named_on_the_line_is_asked_for_and_written_down() {
    // The whole of what `/effort <rung>` owes: the session asks for it from the
    // next turn on, and the file at home is what makes the next run ask too.
    let sample = Sample::new("effort-named");
    let choosing = sample.root().join("config.json");
    let terms = Terms {
        choosing: choosing.clone(),
        ..plain()
    };

    let runner = scripted(Script::new(vec![saying("answered")]), Tools::new());
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(b"/effort max\n".to_vec());

    converse(runner, &mut renderer, &terms, &opening(), &mut input).expect("the loop to finish");

    let written = renderer.terminal().written().to_string();
    assert!(written.contains("max effort"), "{written}");

    let held = std::fs::read_to_string(&choosing).expect("the file it said it wrote");
    assert!(held.contains("\"effort\""), "{held}");
    assert!(held.contains("\"max\""), "{held}");
    assert!(held.contains("anthropic"), "{held}");
}

#[test]
fn effort_down_a_pipe_lists_every_rung_under_the_one_in_force() {
    // Naming none opens the panel where there is a keyboard to walk it with.
    // Down a pipe there is not, so the same line answers the question the panel
    // would have asked. What is in force is named as the vendor's rather than
    // as a rung, because this session was told nothing and the vendor never
    // says which it picked.
    //
    // Every rung because the model in force here is a name the table does not
    // hold, which is the answer for every model released after a build. Nothing
    // is known about it either way, and withholding a rung it may well serve
    // would be this program deciding what a model it has never heard of can do.
    let (written, asked) = commanding("/effort\n");

    assert_eq!(asked, 0, "{written}");
    assert!(written.contains("the vendor's own default"), "{written}");
    for rung in ["low", "medium", "high", "xhigh", "max"] {
        assert!(written.contains(&format!("/effort {rung}")), "{written}");
    }
}

#[test]
fn a_model_that_serves_three_rungs_is_offered_three() {
    // The two it does not serve are missing rather than drawn and refused. A
    // rung offered is a rung asked for, and asking for one this model has never
    // been served is a refusal crucible walked somebody into one keystroke
    // after showing them the word that caused it.
    let written = asking("moonshot", "k3", "/effort\n");

    for rung in ["low", "high", "max"] {
        assert!(written.contains(&format!("/effort {rung}")), "{written}");
    }
    assert!(!written.contains("/effort medium"), "{written}");
    assert!(!written.contains("/effort xhigh"), "{written}");
}

#[test]
fn a_model_that_serves_no_rung_is_told_so_rather_than_offered_a_ladder() {
    // A ladder with nothing on it is a panel that cannot be answered, and a
    // ladder with five rungs on it is five ways to be refused. What is left to
    // say is which model this is and what would change it — the model by name,
    // because the name is the half that can be acted on.
    let written = asking("anthropic", "claude-haiku-4-5", "/effort\n");

    assert!(written.contains("claude-haiku-4-5"), "{written}");
    assert!(written.contains("takes no rung"), "{written}");
    assert!(!written.contains("/effort "), "{written}");
}

#[test]
fn a_word_that_is_not_a_rung_is_said_back_with_the_rungs_that_are() {
    // The same sentence `--effort` is refused with before the session starts.
    // One question asked twice deserves one answer, and nothing about it is a
    // turn: a mistyped command that reached the provider would be a request
    // paid for by a slip.
    let (written, asked) = commanding("/effort maximum\n");

    assert_eq!(asked, 0, "{written}");
    assert!(written.contains("! no effort called maximum"), "{written}");
    assert!(written.contains("/effort max"), "{written}");
}

#[test]
fn a_word_shaped_like_a_command_that_names_none_says_so_and_lists_what_there_is() {
    // Said back so it can be seen to be a typo, and the list under it so the
    // next thing typed is the right one. Nothing is a turn: a mistyped command
    // that reached the provider would be a request paid for by a slip.
    let (written, asked) = commanding("/hlep\n");

    assert_eq!(asked, 0, "{written}");
    assert!(written.contains("! no such command: /hlep"), "{written}");
    assert!(written.contains("what these are"), "{written}");
}

#[test]
fn login_says_where_every_provider_reads_its_key_from() {
    // Naming none opens the panel where there is a keyboard to walk it with.
    // Down a pipe there is not, so the same line answers the question the panel
    // would have asked: which names crucible knows, and which variable each of
    // them signs a request from.
    let (written, asked) = commanding("/login\n");

    assert_eq!(asked, 0, "{written}");
    for one in crate::cli::PROVIDERS {
        assert!(written.contains(one.name), "{written}");
        assert!(written.contains(one.key), "{written}");
    }
}

#[test]
fn login_down_a_pipe_names_the_variable_rather_than_opening_a_box() {
    // There is no keyboard, so a box asking for a key is a session that stops
    // and never comes back — this test hanging is that defect. What is left is
    // the other way in, and it is named rather than merely mentioned.
    let (written, asked) = commanding("/login openai\n");

    assert_eq!(asked, 0, "{written}");
    assert!(written.contains("OPENAI_API_KEY"), "{written}");
    assert!(!written.contains("ANTHROPIC_API_KEY"), "{written}");
}

#[test]
fn login_naming_a_provider_this_build_has_none_of_says_so() {
    // And goes on to say what it does have, which is the answer to the
    // question a misspelled name was asking.
    let (written, asked) = commanding("/login gemini\n");

    assert_eq!(asked, 0, "{written}");
    assert!(written.contains("! no provider called gemini"), "{written}");
    assert!(written.contains("OPENAI_API_KEY"), "{written}");
}

#[test]
fn logout_says_so_where_nothing_was_ever_written_down() {
    // These terms point at a store inside a tree nothing created, which is
    // every machine that has not logged in. There is no panel to stand and no
    // row to draw, so this is said before either is reached.
    let (written, asked) = commanding("/logout\n");

    assert_eq!(asked, 0, "{written}");
    assert!(
        written.contains("nothing is stored by Crucible"),
        "{written}"
    );
    assert!(
        written.contains("environment keys in the shell"),
        "{written}"
    );
}

/// Terms whose selected provider is authenticated by its ordinary environment
/// variable, without reading or retaining any secret in the test.
fn environmental(provider: &'static str) -> Terms {
    let mut terms = plain();
    terms.provider = Cell::new(Some(provider));
    terms.serving = Box::new(|named, _| {
        Ok(crate::cli::Resolved {
            provider: Box::new(crucible_provider::Unavailable::new(
                crate::cli::NOTHING_TO_ASK,
            )),
            source: crate::cli::CredentialSource::Environment(named.key.into()),
        })
    });
    terms
}

#[test]
fn logout_names_an_active_environment_credential_and_how_to_remove_it() {
    let terms = environmental("openai");
    let runner = scripted(Script::new(Vec::new()), Tools::new());
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(b"/logout\n".to_vec());

    converse(runner, &mut renderer, &terms, &opening(), &mut input).expect("the session to finish");

    let written = renderer.terminal().written();
    assert!(written.contains("OPENAI_API_KEY"), "{written}");
    assert!(written.contains("still uses OPENAI_API_KEY"), "{written}");
    assert!(written.contains("unset it in the shell"), "{written}");
    assert!(!written.contains("now signed out"), "{written}");
}

/// Drives the loop over a store of its own that was told `provider`'s key.
///
/// Returns what the terminal ended up with and what the store held afterwards.
/// The second half is why this exists: whether a key is gone is a question only
/// the file can answer, and a session that said it forgot one and did not is the
/// defect worth a temporary tree. `tree` keeps two of them apart, since these
/// run at once and each removes its own on the way out.
fn logging_out(tree: &str, provider: &str, typed: &str) -> (String, StoredCredentials) {
    let sample = Sample::new(&format!("logout-{tree}"));
    sample.stored(provider);

    let terms = Terms {
        logins: sample.store(),
        ..plain()
    };

    let runner = scripted(Script::new(vec![saying("answered")]), Tools::new());
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(typed.as_bytes().to_vec());

    converse(runner, &mut renderer, &terms, &opening(), &mut input).expect("the loop to finish");

    (
        renderer.terminal().written().to_string(),
        terms.logins.read(),
    )
}

#[test]
fn logout_naming_a_provider_forgets_its_key_and_says_what_it_left() {
    let (written, left) = logging_out("named", "openai", "/logout openai\n");

    assert!(
        written.contains("removed the stored credential for openai"),
        "{written}"
    );
    assert!(left.get("openai").is_none(), "{written}");

    // This was not the active provider, so removing it cannot silently switch
    // the provider serving the current session.
    assert!(
        written.contains("active provider is unchanged"),
        "{written}"
    );
}

#[test]
fn removing_the_active_stored_credential_exposes_an_environment_fallback() {
    let sample = Sample::new("logout-active-environment");
    sample.stored("openai");
    let mut terms = environmental("openai");
    terms.logins = sample.store();
    let runner = scripted(Script::new(Vec::new()), Tools::new());
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(b"/logout openai\n".to_vec());

    converse(runner, &mut renderer, &terms, &opening(), &mut input).expect("the session to finish");

    let written = renderer.terminal().written();
    assert!(
        written.contains("removed the stored credential"),
        "{written}"
    );
    assert!(written.contains("OPENAI_API_KEY"), "{written}");
    assert!(written.contains("unset it in the shell"), "{written}");
    assert!(terms.logins.read().get("openai").is_none(), "{written}");
}

#[test]
fn removing_the_only_active_credential_disables_the_current_session() {
    let sample = Sample::new("logout-active-only");
    sample.stored("openai");
    let mut terms = plain();
    terms.provider = Cell::new(Some("openai"));
    terms.logins = sample.store();
    let runner = scripted(Script::new(Vec::new()), Tools::new());
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Cursor::new(b"/logout openai\n".to_vec());

    converse(runner, &mut renderer, &terms, &opening(), &mut input).expect("the session to finish");

    let written = renderer.terminal().written();
    assert!(
        written.contains("active session is now signed out"),
        "{written}"
    );
    assert_eq!(terms.provider.get(), None, "{written}");
}

#[test]
fn logout_down_a_pipe_lists_what_is_logged_in_and_forgets_none_of_it() {
    // No keyboard, so the panel would be a session that stopped — this test
    // hanging is that defect. The names it would have offered are written
    // instead, and being asked what there is takes nothing away.
    let (written, left) = logging_out("piped", "moonshot", "/logout\n");

    assert!(written.contains("/logout moonshot"), "{written}");
    assert!(left.get("moonshot").is_some(), "{written}");
}

#[test]
fn logout_naming_a_provider_with_no_key_here_says_so_and_lists_what_has_one() {
    let (written, left) = logging_out("other", "openai", "/logout anthropic\n");

    assert!(
        written.contains("! no credential for anthropic is stored by Crucible"),
        "{written}"
    );
    assert!(written.contains("/logout openai"), "{written}");
    assert!(left.get("openai").is_some(), "{written}");
}

#[test]
fn a_line_that_opens_with_a_path_is_a_prompt_and_takes_a_turn() {
    let (written, asked) = commanding("/etc/hosts is wrong\n");

    assert_eq!(asked, 1, "{written}");
    assert!(written.contains("answered"), "{written}");
}

#[test]
fn naming_a_mode_puts_the_session_in_it() {
    // Read off the mark in front of the next line, which is where a session
    // with no box to type into says which mode it is in. The mode is the
    // engine's, so this is also what says the switch outlived the command.
    let (written, asked) = commanding("/mode allowEdits\n");

    assert_eq!(asked, 0, "{written}");
    assert!(written.contains("allow edits on"), "{written}");
    assert!(written.contains("allowEdits › "), "{written}");
}

#[test]
fn asking_which_mode_is_in_force_changes_none_of_it() {
    let (written, _) = commanding("/mode\n");

    assert!(written.contains("ask mode on"), "{written}");
    assert!(
        written.contains("ask · allowEdits · fullAccess"),
        "{written}"
    );
    assert!(!written.contains("allowEdits › "), "{written}");
}

#[test]
fn a_word_that_names_no_mode_leaves_the_session_where_it_was() {
    let (written, _) = commanding("/mode sideways\n");

    assert!(written.contains("! sideways is not a mode"), "{written}");
    assert!(
        written.contains("ask · allowEdits · fullAccess"),
        "{written}"
    );
    assert!(!written.contains("mode on"), "{written}");
}

#[test]
fn clearing_starts_a_session_and_leaves_the_loop_running() {
    // The turn before it is what gets left behind: a prompt and the answer to
    // it. The line after it is answered as normal, which is the difference
    // between starting a session and ending one.
    let sample = Sample::new("clear-in-the-loop");
    let (written, asked) = reaching(
        &recording(&sample, &Ledger::new()),
        Tools::new(),
        vec![saying("answered"), saying("answered again")],
        "hello\n/clear\nhello again\n",
    );

    assert_eq!(asked, 2, "{written}");
    assert!(
        !written.contains("started a new session"),
        "the screen after a clear is a fresh start, not an announcement: {written}"
    );
    assert!(written.contains("answered again"), "{written}");
}

#[test]
fn clearing_before_anything_was_said_says_there_was_nothing_to_leave() {
    let (written, asked) = commanding("/clear\n");

    assert_eq!(asked, 0, "{written}");
    assert!(written.contains("nothing had been said"), "{written}");
}

#[test]
fn a_file_read_before_a_clear_has_to_be_read_again_after_one() {
    // Both ends real, which is what makes this the proof: the `read` that
    // learns and the `write` that asks are the shipped tools over the record
    // the wiring hands them, and `/clear` is reached by typing it. A record
    // left standing across the command would let the session it started
    // replace a file that session has never seen.
    let sample = Sample::new("clear-forgets-reads");
    let ledger = Ledger::new();
    let offered = untouched(&sample, &ledger);

    let (written, _) = reaching(
        &recording(&sample, &ledger),
        offered,
        looking_then_replacing(),
        "look at it\n/clear\nreplace it\n",
    );

    let held = std::fs::read_to_string(sample.root().join("one.txt")).expect("the file");
    assert_eq!(held, "work nobody looked at\n", "{written}");
}

#[test]
fn a_file_read_before_a_resume_has_to_be_read_again_after_one() {
    // The record answers for a session, and `/resume` leaves the one those
    // files were read in. The session picked up saw none of them, however much
    // of it comes back off the disk — what a log holds is what was said, not
    // what the tools of that run had looked at.
    let sample = Sample::new("resume-forgets-reads");
    let ledger = Ledger::new();
    let offered = untouched(&sample, &ledger);

    // Closed before the loop starts, so its id names a session `/resume` can
    // reopen.
    let earlier = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let id = earlier
        .id()
        .expect("a recorded session has a name")
        .as_str()
        .to_owned();
    earlier.append(&Message::said("what was asked before"));
    drop(earlier);

    let (written, _) = reaching(
        &recording(&sample, &ledger),
        offered,
        looking_then_replacing(),
        &format!("look at it\n/resume {id}\nreplace it\n"),
    );

    let held = std::fs::read_to_string(sample.root().join("one.txt")).expect("the file");
    assert_eq!(held, "work nobody looked at\n", "{written}");
}

#[test]
fn asking_what_was_worked_on_here_costs_no_turn_and_leaves_the_loop_running() {
    // These terms are pointed at a directory nothing was ever recorded in, so
    // the list is the empty one — which is the answer the loop has to carry
    // just as far as any other. What the list says when there is something on
    // it, and what picking one off it does, is proved where the sessions are.
    let (written, asked) = commanding("/resume\nhello\n");

    assert_eq!(asked, 1, "{written}");
    assert!(
        written.contains("nothing has been worked on here yet"),
        "{written}"
    );
    assert!(written.contains("answered"), "{written}");
}

#[test]
fn leaving_ends_the_session_with_what_follows_it_unread() {
    let (written, asked) = commanding("/exit\nand this\n");

    assert_eq!(asked, 0, "{written}");
    assert!(!written.contains("answered"), "{written}");
}
