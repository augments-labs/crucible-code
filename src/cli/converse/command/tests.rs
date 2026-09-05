//! What a line is taken for, and what the list shows while it is being typed.
//!
//! The drawing is tested where the loop runs, because what a command answers
//! with only exists once there is a terminal to answer on. What is left here is
//! the deciding: which lines are commands, which command a line names, and what
//! is on the list at each point of typing one.

use super::*;
use crucible_core::RegistryRow;

/// The built-in registry, as a session starts with it.
fn commands() -> Commands {
    builtins()
        .expect("the built-in commands register")
        .snapshot()
}

/// What a list of them says, row by row.
fn art(rows: &[Row]) -> Vec<String> {
    rows.iter().map(Row::text).collect()
}

/// The names of what the menu shows for a line, in the order it shows them.
fn shown(line: &str) -> Vec<&'static str> {
    filtering(&commands(), line, Glyphs::Unicode)
        .into_iter()
        .map(|one| one.name)
        .collect()
}

#[test]
fn every_command_is_reached_by_the_name_it_is_listed_under() {
    // Also what says no two are typed the same way: a shared name would give
    // both rows the first command, and the second one's turn here would fail.
    for command in EVERY {
        let name = command.name();

        assert!(shaped(name), "{name} is not shaped like a command");
        assert_eq!(
            wanted(&commands(), name),
            Some(Wanted::Known { command, rest: "" }),
            "{name}"
        );
    }
}

#[test]
fn every_command_decides_what_it_can_do_mid_turn() {
    let live = [Command::Help, Command::Theme];
    let deferred = [Command::Model, Command::Mode];

    for command in EVERY {
        match command.mid_turn() {
            MidTurn::Live => assert!(live.contains(&command), "{command:?}"),
            MidTurn::Deferred => assert!(deferred.contains(&command), "{command:?}"),
            MidTurn::Refused(why) => {
                assert!(!live.contains(&command), "{command:?}");
                assert!(!deferred.contains(&command), "{command:?}");
                assert!(!why.trim().is_empty(), "{command:?}");
            }
        }
    }
}

#[test]
fn everything_the_list_offers_is_something_a_line_can_run() {
    // The menu, `/help` and the match that runs one walk the same array, and
    // this is what says so. A name on the list that no line names is a row
    // promising something pressing return would not do.
    for one in filtering(&commands(), "/", Glyphs::Unicode) {
        assert!(
            matches!(wanted(&commands(), one.name), Some(Wanted::Known { .. })),
            "{}",
            one.name
        );
    }
}

#[test]
fn what_follows_the_name_comes_back_with_it_and_nothing_else_does() {
    assert_eq!(
        wanted(&commands(), "  /mode allowEdits  "),
        Some(Wanted::Known {
            command: Command::Mode,
            rest: "allowEdits",
        })
    );
}

#[test]
fn a_word_shaped_like_a_command_that_names_none_is_said_back() {
    assert_eq!(wanted(&commands(), "/nope"), Some(Wanted::Unknown("/nope")));
    assert_eq!(
        wanted(&commands(), "/nope with a word"),
        Some(Wanted::Unknown("/nope"))
    );
}

#[test]
fn a_line_that_opens_with_a_path_is_a_prompt() {
    // The case the shape check is for. A sentence about a file, and a file, are
    // both things somebody types at a coding agent, and neither is a command
    // that happens not to exist.
    for said in [
        "/etc/hosts is wrong",
        "/Users/me/notes.md",
        "/usr/bin/env, but why",
    ] {
        assert_eq!(wanted(&commands(), said), None, "{said:?}");
    }
}

#[test]
fn a_line_that_is_not_a_command_at_all_is_none() {
    for said in ["", "   ", "hello", "why does /help exist", "//", "/mode2"] {
        assert_eq!(wanted(&commands(), said), None, "{said:?}");
    }
}

#[test]
fn a_bare_slash_opens_the_whole_list() {
    assert_eq!(shown("/").len(), EVERY.len());
}

#[test]
fn what_has_been_typed_is_what_is_left_on_the_list() {
    assert_eq!(shown("/m"), ["/model", "/mode"]);
    assert_eq!(shown("/mod"), ["/model", "/mode"]);
    assert_eq!(shown("/e"), ["/effort", "/exit"]);

    // A finished name that is also the start of a longer one keeps both. The
    // list says what pressing return would run *and* what one more character
    // would reach, and a name is not a reason to stop offering the other.
    assert_eq!(shown("/mode"), ["/model", "/mode"]);
    assert_eq!(shown("/model"), ["/model"]);
}

#[test]
fn a_line_that_could_still_be_a_command_and_names_none_closes_the_list() {
    assert!(shown("/modes").is_empty());
    assert!(shown("/z").is_empty());
}

#[test]
fn the_list_closes_the_moment_the_line_becomes_something_else() {
    // Every one of these is a line somebody is part way through typing, and on
    // none of them is a list of commands what they are looking at.
    for said in [
        "",
        "/mode      ",
        "/mode      allowEdits",
        "/etc/hosts",
        "hello",
        " /",
    ] {
        assert!(
            filtering(&commands(), said, Glyphs::Unicode).is_empty(),
            "{said:?}"
        );
    }
}

#[test]
fn a_prompt_puts_nothing_on_the_heap() {
    // The menu is rebuilt on every keystroke of every line, and all but a few
    // of those lines are prompts. `Vec::new` does not allocate; a filter that
    // ran and found nothing would have.
    assert_eq!(
        filtering(&commands(), "hello", Glyphs::Unicode).capacity(),
        0
    );
}

#[test]
fn help_answers_with_a_name_and_what_it_does() {
    assert_eq!(
        art(&listing(&commands(), 60, Glyphs::Unicode)),
        [
            "/help      what these are",
            "/model     pick which model answers",
            "/effort    pick how hard it thinks",
            "/login     sign in to a provider account",
            "/logout    remove a stored account or API key",
            "/mode      ask · allowEdits · fullAccess",
            "/theme     pick the colours crucible draws with",
            "/resume    pick up an earlier session here",
            "/cache     inspect or clean prompt-cache state",
            "/compact   replace what is behind you with notes on it",
            "/clear     start a new session, leaving this one",
            "/exit      leave",
        ]
    );
}

#[test]
fn a_terminal_without_the_marks_gets_the_ring_punctuated_for_it() {
    assert_eq!(
        art(&listing(&commands(), 60, Glyphs::Ascii)),
        [
            "/help      what these are",
            "/model     pick which model answers",
            "/effort    pick how hard it thinks",
            "/login     sign in to a provider account",
            "/logout    remove a stored account or API key",
            "/mode      ask, allowEdits, fullAccess",
            "/theme     pick the colours crucible draws with",
            "/resume    pick up an earlier session here",
            "/cache     inspect or clean prompt-cache state",
            "/compact   replace what is behind you with notes on it",
            "/clear     start a new session, leaving this one",
            "/exit      leave",
        ]
    );
}

#[test]
fn nothing_answered_is_wider_than_the_window_it_was_asked_in() {
    // A row over the width would wrap, and a wrapped row leaves the cursor a
    // row below where the next frame expects it.
    for columns in 1..=60 {
        for row in listing(&commands(), columns, Glyphs::Unicode) {
            assert!(row.columns() <= columns, "at {columns}: {row:?}");
        }
    }
}

#[test]
fn leaving_is_the_only_command_that_ends_the_session() {
    for command in EVERY {
        let wanted = Wanted::Known { command, rest: "" };

        assert_eq!(leaves(wanted), command == Command::Exit, "{command:?}");
    }

    assert!(!leaves(Wanted::Unknown("/nope")));
}

#[test]
fn the_mark_in_a_listing_row_comes_out_of_the_glyph_set() {
    // The listing is what a run with no keyboard is given in place of a panel,
    // and a piped run is exactly the run most likely to have the setting turned
    // down. A mark drawn from the set is a mark that arrives; one written into
    // the sentence is a question mark between the line and what it does.
    assert_eq!(
        about(
            "/login     openai",
            "a key from OPENAI_API_KEY",
            Glyphs::Unicode
        ),
        "/login     openai — a key from OPENAI_API_KEY"
    );
    assert_eq!(
        about(
            "/login     openai",
            "a key from OPENAI_API_KEY",
            Glyphs::Ascii
        ),
        "/login     openai -- a key from OPENAI_API_KEY"
    );
}

#[test]
fn the_registry_holds_every_command_once_in_the_order_help_lists_them() {
    let commands = commands();
    let names: Vec<&str> = commands.entries().iter().map(|one| one.id()).collect();
    assert_eq!(names, EVERY.map(Command::name));
    assert!(commands.shadows().is_empty());

    for one in commands.entries() {
        assert_eq!(one.provenance().kind(), SourceKind::Builtin, "{}", one.id());
        assert_eq!(one.provenance().id(), format!("crucible:{}", one.id()));
    }
}

#[test]
fn a_second_command_under_a_taken_name_is_refused_naming_both_sources() {
    let registry = builtins().expect("the built-in commands register");
    let mut staged = registry.stage();
    let again = Slash::builtin(Command::Help).expect("a constant name fits");

    let refused = staged.register(again).expect_err("the name is taken");
    let RegistryError::Duplicate { id, first, second } = refused else {
        panic!("refused for another reason: {refused}");
    };
    assert_eq!(&*id, "/help");
    assert_eq!(first.kind(), SourceKind::Builtin);
    assert_eq!(second.kind(), SourceKind::Builtin);
    assert_eq!(
        refused_sentence(&id, &first, &second),
        "/help is registered by both built-in /help command [builtin:crucible:/help] \
         and built-in /help command [builtin:crucible:/help]"
    );

    // What was in force stays in force: the staged change was never committed.
    assert_eq!(registry.snapshot().entries().len(), EVERY.len());
}

fn refused_sentence(id: &str, first: &Provenance, second: &Provenance) -> String {
    format!("{id} is registered by both {first} and {second}")
}

#[test]
fn what_is_read_is_the_generation_in_force_and_not_the_array_that_filled_it() {
    // A command taken out of the registry is not on the list, is not filtered
    // in, and a line naming it is a word that names none — which is the proof
    // that the three readers walk the registry, and not `EVERY` behind it.
    let registry = builtins().expect("the built-in commands register");
    let mut staged = registry.stage();
    staged.deregister("/cache").expect("/cache is registered");
    let without = registry.commit(staged).expect("the generation commits");

    assert!(
        !filtering(&without, "/c", Glyphs::Unicode)
            .iter()
            .any(|one| one.name == "/cache")
    );
    assert!(
        filtering(&without, "/c", Glyphs::Unicode)
            .iter()
            .any(|one| one.name == "/clear")
    );
    assert_eq!(wanted(&without, "/cache"), Some(Wanted::Unknown("/cache")));
    assert!(
        !art(&listing(&without, 60, Glyphs::Unicode))
            .iter()
            .any(|row| row.contains("/cache"))
    );

    // And the one that was in force before is still whole: a snapshot is a
    // generation, not a view of the registry as it is now.
    assert_eq!(commands().entries().len(), EVERY.len());
}

#[test]
fn the_report_of_what_is_registered_names_every_command_and_its_source() {
    let report = commands().inspect();
    let rows: Vec<&str> = report.registered().iter().map(RegistryRow::id).collect();
    assert_eq!(rows, EVERY.map(Command::name));
    assert!(report.shadowed().is_empty());
    for row in report.registered() {
        assert_eq!(row.source().kind(), SourceKind::Builtin);
    }
}

#[test]
fn what_is_said_back_folds_short_of_the_mark_it_will_hang_under() {
    // Every answer a command says is hung under the command afterwards, and
    // the mark and its space take two columns off the row. A sentence folded
    // to the whole width is two columns too wide once hung, and the window
    // clips it — in the middle of whatever word was last, which for a sentence
    // ending in a command name is the command name.
    let mut renderer = Renderer::new(crucible_tui::Recording::new(80, 24));
    let said = "! the key could not be saved — crucible cannot write its login store; try /login again after fixing the permissions";

    say(&mut renderer, said).expect("the terminal to be written");

    let rows = renderer.terminal().picture().said();
    assert!(
        rows.iter().all(|row| crucible_tui::columns(row) <= 78),
        "{rows:?}"
    );
    assert_eq!(rows.join(" "), said, "nothing of the sentence is lost");
}
