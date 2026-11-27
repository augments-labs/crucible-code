//! The images a turn named: which are asked for, in what order, and what the
//! rest are named as.

use std::cell::Cell;
use std::io::Cursor;
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use crucible_runner::Tools;
use crucible_session::Session;
use crucible_tui::chrome::Undrawn;
use crucible_tui::graphics::{self, Capability, Protocol};
use crucible_tui::{Recording, Renderer};
use crucible_types::{Attachment, Modality, ToolId};

use super::super::{First, Named, Naming, PER_TURN, Terms, converse, shown_images, turn_end};
use super::{opening, paired, plain, saying, scripted};
use crate::cli::fake::Script;
use crate::cli::sample::Sample;
use crate::cli::style::Style;

/// A PNG `width` by `height`, as a file in a workspace holds one.
fn png(width: u32, height: u32) -> Vec<u8> {
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">\
         <rect width=\"{width}\" height=\"{height}\" fill=\"#3366cc\"/></svg>"
    );
    let pixels = crucible_raster::rasterise(svg.as_bytes(), width, &[]).unwrap();
    crucible_raster::png(&pixels).unwrap()
}

/// What a terminal that confirmed Kitty is left holding once a turn whose
/// answer is `answer` has run in a workspace holding `files`, with the rows
/// it shows.
fn after_a_turn(name: &str, answer: &str, files: &[(&str, &[u8])]) -> (String, Vec<String>) {
    let script = Script::new(vec![saying(answer)]);
    opened(name, files, b"show me\n", |session, _| {
        scripted(script, Tools::new(), session)
    })
}

/// The same, for a session whose runner `build` makes and a reader who
/// types `typed`.
fn opened(
    name: &str,
    files: &[(&str, &[u8])],
    typed: &[u8],
    build: impl FnOnce(Arc<Session>, crucible_workspace::Workspace) -> crucible_runner::Runner,
) -> (String, Vec<String>) {
    ruled(name, files, None, typed, build)
}

/// The same, under the configuration `config` where one is written: the
/// file the person keeps, read as a start reads it.
fn ruled(
    name: &str,
    files: &[(&str, &[u8])],
    config: Option<&str>,
    typed: &[u8],
    build: impl FnOnce(Arc<Session>, crucible_workspace::Workspace) -> crucible_runner::Runner,
) -> (String, Vec<String>) {
    let _turn = crate::cli::pictures::tests::ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let sample = Sample::new(name);
    for (file, bytes) in files {
        let file = sample.root().join(file);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(file, bytes).unwrap();
    }
    let settings = config.map_or_else(crucible_config::Settings::default, |config| {
        let home = sample.root().join(".home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("config.json"), config).unwrap();
        let home = crucible_config::Home::find(&|name: &str| {
            (name == crucible_config::HOME).then(|| home.clone().into_os_string())
        })
        .unwrap();
        crucible_config::Settings::read(&home, &sample.root()).unwrap()
    });
    let terms = Terms {
        workspace: sample.workspace(),
        style: Cell::new(Style::coloured()),
        settings,
        ..plain()
    };
    seated(
        &sample,
        &terms,
        Box::new(crate::cli::pictures::Raster),
        typed,
        build,
    )
}

/// The loop over `terms` in `sample`, drawing pictures with `pictures` on a
/// terminal that confirmed Kitty, once `typed` is read: what it was left
/// holding, and the rows it shows.
fn seated(
    sample: &Sample,
    terms: &Terms,
    pictures: Box<dyn graphics::Pictures>,
    typed: &[u8],
    build: impl FnOnce(Arc<Session>, crucible_workspace::Workspace) -> crucible_runner::Runner,
) -> (String, Vec<String>) {
    let workspace = sample.workspace();
    let conversation = paired(Arc::new(Session::nowhere()), |session| {
        build(session, workspace)
    });
    let mut renderer = crate::cli::furnished(
        Recording::new(80, 60),
        &crucible_config::Settings::default(),
        pictures,
    );
    renderer.wears(Style::coloured().palette());
    renderer.images(Capability::Graphics {
        protocol: Protocol::Kitty,
        cell: graphics::Cell {
            width: 10,
            height: 20,
        },
    });
    let mut input = Cursor::new(typed.to_vec());
    converse(
        conversation,
        &mut renderer,
        terms,
        First {
            card: &opening(),
            arming: None,
        },
        &mut input,
    )
    .expect("the loop to finish");
    let written = renderer.terminal().written().to_string();
    let rows = crucible_tui::Picture::of(&written, 80, 60)
        .rows()
        .iter()
        .map(|row| row.trim_end().to_owned())
        .collect();
    (written, rows)
}

#[test]
fn an_image_the_answer_named_in_the_workspace_is_drawn_under_its_edge_once_the_turn_ends() {
    let (written, rows) = after_a_turn(
        "images-shown",
        "Here is the plot.\n\n![plot](plot.png)\n",
        &[("plot.png", &png(40, 20))],
    );
    assert!(
        rows.iter().any(|row| row == "┌─ image · plot.png"),
        "{rows:#?}"
    );
    assert!(
        written.contains("\x1b_Ga=t,f=100,i="),
        "no picture was sent: {written:?}"
    );
}

#[test]
fn an_image_outside_the_workspace_or_missing_is_its_edge_saying_not_shown_and_nothing_is_sent() {
    let (written, rows) = after_a_turn(
        "images-refused",
        "![out](../../etc/passwd.png) and ![gone](gone.png)\n",
        &[],
    );
    for edge in [
        "┌─ image · ../../etc/passwd.png · not shown",
        "┌─ image · gone.png · not shown",
    ] {
        assert!(rows.iter().any(|row| row == edge), "{edge}: {rows:#?}");
    }
    assert!(
        !written.contains("\x1b_Ga=t"),
        "a picture was sent: {written:?}"
    );
}

#[test]
fn a_turn_draws_eight_images_and_names_the_ninth_as_past_them() {
    let picture = png(8, 8);
    let names: Vec<String> = (0..=PER_TURN).map(|at| format!("{at}.png")).collect();
    let files: Vec<(&str, &[u8])> = names
        .iter()
        .map(|name| (name.as_str(), &picture[..]))
        .collect();
    let answer: String = names
        .iter()
        .map(|name| format!("![i]({name})\n\n"))
        .collect::<Vec<_>>()
        .concat();
    let (written, rows) = after_a_turn("images-past", &answer, &files);
    assert!(
        rows.iter()
            .any(|row| row == "┌─ image · 8.png · past the eighth image in this turn"),
        "{rows:#?}"
    );
    assert_eq!(
        written.matches("\x1b_Ga=t,f=100,i=").count(),
        PER_TURN,
        "{written:?}"
    );
}

/// A renderer that reads an answer's markdown, which a plain one writes as
/// it came and names no image in.
fn reading() -> Renderer<Recording> {
    let mut renderer = Renderer::new(Recording::new(80, 24));
    renderer.wears(Style::coloured().palette());
    renderer
}

/// An answer naming `count` images, `0.png` on.
fn answered(count: usize) -> Vec<Named> {
    let mut renderer = reading();
    let text: String = (0..count)
        .map(|at| format!("![{at}]({at}.png)\n\n"))
        .collect::<Vec<_>>()
        .concat();
    renderer.stream(&text).unwrap();
    renderer.settle().unwrap();
    Naming::default().taken(&mut renderer)
}

/// A picture the read tool took from `path`, whose bytes hash to `hash`.
fn picture(path: &str, hash: u8) -> Attachment {
    Attachment {
        path: path.into(),
        modality: Modality::Image,
        media_type: "image/png".into(),
        hash: [hash; 32],
    }
}

#[test]
fn a_turn_asks_for_its_first_eight_images_in_order_and_names_the_rest_as_past_them() {
    let named = answered(PER_TURN + 3);
    assert_eq!(named.len(), PER_TURN + 3);

    let mut asked = Vec::new();
    let mut drawn = Vec::new();
    shown_images::<()>(
        &named,
        turn_end(|| Duration::ZERO),
        |one| {
            asked.push(one.path.clone());
            Ok(vec![1])
        },
        |one, png| {
            drawn.push((one.path.clone(), png.err()));
            Ok(())
        },
    )
    .unwrap();

    let first: Vec<Box<str>> = (0..PER_TURN).map(|at| format!("{at}.png").into()).collect();
    assert_eq!(asked, first, "asked for more than eight, or out of order");
    let past: Vec<_> = drawn
        .iter()
        .skip(PER_TURN)
        .map(|(path, why)| (&**path, *why))
        .collect();
    assert_eq!(
        past,
        [
            ("8.png", Some(Undrawn::PastTurnImages)),
            ("9.png", Some(Undrawn::PastTurnImages)),
            ("10.png", Some(Undrawn::PastTurnImages)),
        ]
    );
    assert!(drawn.iter().take(PER_TURN).all(|(_, why)| why.is_none()));
}

#[test]
fn a_turn_asks_for_no_image_once_its_two_seconds_are_spent_and_names_the_rest_as_late() {
    // Each request moves the clock on by most of a second, as a slow mount
    // would, so the third ends past the turn's two seconds: the rest of the
    // first eight are late, and the ones past eight are past them still.
    let spent = Cell::new(Duration::ZERO);
    let mut asked = 0;
    let mut whys = Vec::new();
    shown_images::<()>(
        &answered(PER_TURN + 2),
        turn_end(|| spent.get()),
        |_| {
            asked += 1;
            spent.set(spent.get().saturating_add(Duration::from_millis(700)));
            Ok(vec![1])
        },
        |one, png| {
            whys.push((one.path.to_string(), png.err()));
            Ok(())
        },
    )
    .unwrap();

    assert_eq!(asked, 3);
    let late = Some(Undrawn::NotReadInTime);
    let past = Some(Undrawn::PastTurnImages);
    let expected: Vec<_> = (0..PER_TURN + 2)
        .map(|at| {
            let why = match at {
                0..3 => None,
                _ if at < PER_TURN => late,
                _ => past,
            };
            (format!("{at}.png"), why)
        })
        .collect();
    assert_eq!(whys, expected);
}

#[test]
fn once_the_time_for_images_is_spent_none_after_is_asked_for() {
    let mut asked = 0;
    let mut whys = Vec::new();
    shown_images::<()>(
        &answered(4),
        // A budget that ran out at the second, and a clock that would say
        // otherwise later: what is past is past.
        |at| (at == 1).then_some(Undrawn::NotRedrawnOnResume),
        |_| {
            asked += 1;
            Ok(vec![1])
        },
        |_, png| {
            whys.push(png.err());
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(asked, 1);
    assert_eq!(
        whys,
        [
            None,
            Some(Undrawn::NotRedrawnOnResume),
            Some(Undrawn::NotRedrawnOnResume),
            Some(Undrawn::NotRedrawnOnResume),
        ]
    );
}

#[test]
fn a_tool_picture_is_named_after_what_the_answer_named_before_it_by_its_tool_and_hash() {
    let mut renderer = reading();
    let mut naming = Naming::default();
    let call = ToolId::new("call-1");

    renderer
        .stream("Here is ![the plot](out/plot.png).\n\n")
        .unwrap();
    naming.calling(&call, "read");
    renderer.settle().unwrap();
    naming.came_back(&mut renderer, &call, &[picture("/work/shot.png", 7)]);
    // A call this turn never saw go out names nothing it came back with.
    naming.came_back(
        &mut renderer,
        &ToolId::new("unknown"),
        &[picture("/work/x.png", 1)],
    );
    renderer.stream("And ![again](after.png)\n\n").unwrap();
    renderer.settle().unwrap();

    let named = naming.taken(&mut renderer);
    let read: Vec<(&str, Option<(&str, u8)>)> = named
        .iter()
        .map(|one| {
            let tool = one.tool.as_ref().map(|(tool, hash)| (&**tool, hash[0]));
            (&*one.path, tool)
        })
        .collect();
    assert_eq!(
        read,
        [
            ("out/plot.png", None),
            ("/work/shot.png", Some(("read", 7))),
            ("after.png", None),
        ]
    );
    assert!(naming.taken(&mut renderer).is_empty(), "taken twice");
}

#[test]
fn a_session_put_back_keeps_its_most_recent_images_and_a_turn_its_first() {
    let mut recent = Naming::recent();
    let mut first = Naming::default();
    let call = ToolId::new("call");
    for at in 0..100_u8 {
        let mut renderer = Renderer::new(Recording::new(80, 24));
        for naming in [&mut recent, &mut first] {
            naming.calling(&call, "read");
            naming.came_back(&mut renderer, &call, &[picture(&format!("{at}.png"), at)]);
        }
    }
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let recent = recent.taken(&mut renderer);
    let first = first.taken(&mut renderer);
    assert_eq!(recent.len(), crucible_tui::MOST_KEPT_IMAGES);
    assert_eq!(recent.first().map(|one| &*one.path), Some("84.png"));
    assert_eq!(first.len(), 64);
    assert_eq!(first.last().map(|one| &*one.path), Some("63.png"));
}

#[test]
fn a_session_put_back_draws_the_images_its_turns_named_once_it_is_on_the_screen() {
    let mut transcript = crucible_types::Transcript::new();
    transcript
        .push(crucible_types::Message::said("plot it"))
        .unwrap();
    transcript
        .push(crucible_types::Message::Agent {
            continuation: None,
            text: "![plot](plot.png)".into(),
            calls: Vec::new(),
            stop: Some(crucible_types::StopReason::Yielded),
        })
        .unwrap();
    let (written, rows) = opened(
        "images-resumed",
        &[("plot.png", &png(40, 20))],
        b"",
        |session, _| scripted(Script::new(Vec::new()), Tools::new(), session).resuming(transcript),
    );
    assert!(
        rows.iter().any(|row| row == "┌─ image · plot.png"),
        "{rows:#?}"
    );
    assert!(
        written.contains("\x1b_Ga=t,f=100,i="),
        "no picture was sent: {written:?}"
    );
}

#[test]
fn an_image_the_read_tool_took_is_drawn_after_the_turn_named_by_the_tool() {
    let read = |workspace: crucible_workspace::Workspace| {
        let mut offered = Tools::new();
        offered
            .add_builtin(crucible_builtins::Read::new(
                workspace,
                crucible_builtins::Ledger::default(),
            ))
            .unwrap();
        offered
    };
    let calls = vec![
        crucible_models::Delta::ToolStarted {
            id: ToolId::new("r-1"),
            name: "read".into(),
        },
        crucible_models::Delta::ToolArgs(r#"{"path":"plot.png"}"#.into()),
        crucible_models::Delta::Stopped(crucible_types::StopReason::WantsTools),
    ];
    let (written, rows) = opened(
        "images-read",
        &[("plot.png", &png(40, 20))],
        b"look\n",
        |session, workspace| {
            scripted(
                Script::new(vec![calls, saying("It is a blue square.")]),
                read(workspace),
                session,
            )
        },
    );
    assert!(
        rows.iter()
            .any(|row| row == "┌─ image · plot.png · read by read"),
        "{rows:#?}"
    );
    assert!(
        written.contains("\x1b_Ga=t,f=100,i="),
        "no picture was sent: {written:?}"
    );
}

#[test]
fn an_image_under_a_rule_refusing_its_read_is_not_shown_and_one_beside_it_is() {
    let picture = png(8, 8);
    let script = Script::new(vec![saying("![a](secret/a.png) and ![b](open.png)\n")]);
    let (written, rows) = ruled(
        "images-ruled",
        &[("secret/a.png", &picture), ("open.png", &picture)],
        Some(r#"{"permissions": {"deny": ["read(secret/**)"]}}"#),
        b"show me\n",
        |session, _| scripted(script, Tools::new(), session),
    );
    assert!(
        rows.iter()
            .any(|row| row == "┌─ image · secret/a.png · not shown"),
        "{rows:#?}"
    );
    assert!(
        rows.iter().any(|row| row == "┌─ image · open.png"),
        "{rows:#?}"
    );
    assert_eq!(
        written.matches("\x1b_Ga=t,f=100,i=").count(),
        1,
        "{written:?}"
    );
}

/// A session recorded in `sample` before the loop starts, its turns asking
/// for a plot and answering with it: the id `/resume` picks it up by.
fn recorded_plot(sample: &Sample) -> String {
    let earlier = Session::start(&sample.logs(), &sample.workspace(), None).unwrap();
    let id = earlier.id().unwrap().as_str().to_owned();
    earlier.append(&crucible_types::Message::said("plot it"));
    earlier.append(&crucible_types::Message::Agent {
        continuation: None,
        text: "![plot](plot.png)".into(),
        calls: Vec::new(),
        stop: Some(crucible_types::StopReason::Yielded),
    });
    drop(earlier);
    id
}

#[test]
fn a_session_picked_up_by_resume_draws_the_images_its_turns_named() {
    let _turn = crate::cli::pictures::tests::ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let sample = Sample::new("images-resume-command");
    std::fs::write(sample.root().join("plot.png"), png(40, 20)).unwrap();
    let id = recorded_plot(&sample);
    let terms = Terms {
        sessions: sample.logs(),
        workspace: sample.workspace(),
        style: Cell::new(Style::coloured()),
        ..plain()
    };

    let (written, rows) = seated(
        &sample,
        &terms,
        Box::new(crate::cli::pictures::Raster),
        format!("/resume {id}\n").as_bytes(),
        |session, _| scripted(Script::new(Vec::new()), Tools::new(), session),
    );

    assert!(
        rows.iter().any(|row| row == "┌─ image · plot.png"),
        "{rows:#?}"
    );
    assert!(
        written.contains("\x1b_Ga=t,f=100,i="),
        "no picture was sent: {written:?}"
    );
}

/// Pictures that keep what they draw under `home`, and the one writer that
/// lands it, run where a test says.
fn caching(
    home: &std::path::Path,
) -> (
    Box<dyn graphics::Pictures>,
    crate::cli::picture_cache::Writer,
) {
    let cache = crate::cli::picture_cache::Cache::open(home);
    let (queue, writer) = crate::cli::picture_cache::writing(cache.clone());
    (
        Box::new(crate::cli::pictures::Caching::new(
            cache,
            queue,
            crate::cli::math_engine::Outcome::default(),
        )),
        writer,
    )
}

/// The entries the cache under `home` holds: its own names only.
fn cached_entries(home: &std::path::Path) -> Vec<String> {
    let Ok(listed) = std::fs::read_dir(home.join("pictures")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = listed
        .map(|listed| listed.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .collect();
    names.sort();
    names
}

/// A diagram as an answer writes one, its node named `node`.
fn diagram(node: &str) -> String {
    format!("```mermaid\ngraph TD\n  {node} --> B\n```\n")
}

/// A session whose one turn was answered with `answer`, put back.
fn answered_with(answer: &str) -> crucible_types::Transcript {
    let mut transcript = crucible_types::Transcript::new();
    transcript
        .push(crucible_types::Message::said("draw it"))
        .unwrap();
    transcript
        .push(crucible_types::Message::Agent {
            continuation: None,
            text: answer.into(),
            calls: Vec::new(),
            stop: Some(crucible_types::StopReason::Yielded),
        })
        .unwrap();
    transcript
}

/// What the loop leaves when `pictures` draw in a workspace holding `files`,
/// over a runner `build` makes, once `typed` is read.
fn pictured(
    sample: &Sample,
    files: &[(&str, &[u8])],
    pictures: Box<dyn graphics::Pictures>,
    typed: &[u8],
    build: impl FnOnce(Arc<Session>, crucible_workspace::Workspace) -> crucible_runner::Runner,
) -> (String, Vec<String>) {
    for (file, bytes) in files {
        std::fs::write(sample.root().join(file), bytes).unwrap();
    }
    let terms = Terms {
        workspace: sample.workspace(),
        style: Cell::new(Style::coloured()),
        ..plain()
    };
    seated(sample, &terms, pictures, typed, build)
}

/// How many pictures `written` sent.
fn sent(written: &str) -> usize {
    written.matches("\x1b_Ga=t,f=100,i=").count()
}

#[test]
fn an_image_file_drawn_live_or_put_back_leaves_nothing_in_the_picture_cache() {
    let _turn = crate::cli::pictures::tests::ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let sample = Sample::new("images-never-cached");
    let home = sample.root().join(".home");
    let files: &[(&str, &[u8])] = &[("plot.png", &png(40, 20))];
    let read = |workspace: crucible_workspace::Workspace| {
        let mut offered = Tools::new();
        offered
            .add_builtin(crucible_builtins::Read::new(
                workspace,
                crucible_builtins::Ledger::default(),
            ))
            .unwrap();
        offered
    };
    let calls = vec![
        crucible_models::Delta::ToolStarted {
            id: ToolId::new("r-1"),
            name: "read".into(),
        },
        crucible_models::Delta::ToolArgs(r#"{"path":"plot.png"}"#.into()),
        crucible_models::Delta::Stopped(crucible_types::StopReason::WantsTools),
    ];

    // Put back naming the file, then read by a tool live, beside a diagram
    // that shows the cache is being written at all.
    let (pictures, writer) = caching(&home);
    let (written, rows) = pictured(&sample, files, pictures, b"look\n", |session, workspace| {
        scripted(
            Script::new(vec![
                calls,
                saying(&format!("A square.\n\n{}", diagram("A"))),
            ]),
            read(workspace),
            session,
        )
        .resuming(answered_with("![plot](plot.png)"))
    });
    writer.drain();

    assert!(
        rows.iter().any(|row| row == "┌─ image · plot.png"),
        "{rows:#?}"
    );
    assert!(
        rows.iter()
            .any(|row| row == "┌─ image · plot.png · read by read"),
        "{rows:#?}"
    );
    assert_eq!(sent(&written), 3, "{written:?}");
    assert_eq!(cached_entries(&home).len(), 1, "only the diagram is kept");
}

#[test]
fn a_session_put_back_draws_its_diagrams_from_what_an_earlier_run_kept() {
    let _turn = crate::cli::pictures::tests::ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let sample = Sample::new("images-resumed-cached");
    let home = sample.root().join(".home");
    let (pictures, writer) = caching(&home);
    let (written, _) = pictured(&sample, &[], pictures, b"draw\n", |session, _| {
        scripted(
            Script::new(vec![saying(&diagram("A"))]),
            Tools::new(),
            session,
        )
    });
    writer.drain();
    assert_eq!(sent(&written), 1, "the live run drew it: {written:?}");
    let kept = cached_entries(&home);
    assert_eq!(kept.len(), 1);

    let (pictures, writer) = caching(&home);
    let (written, _) = pictured(&sample, &[], pictures, b"", |session, _| {
        scripted(Script::new(Vec::new()), Tools::new(), session)
            .resuming(answered_with(&diagram("A")))
    });
    writer.drain();

    assert_eq!(sent(&written), 1, "put back as a picture: {written:?}");
    assert_eq!(cached_entries(&home), kept, "nothing drawn afresh");
}

#[test]
fn a_session_put_back_with_nothing_kept_draws_its_diagrams_as_text_and_says_nothing() {
    let _turn = crate::cli::pictures::tests::ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let sample = Sample::new("images-resumed-empty");
    let home = sample.root().join(".home");
    let (pictures, writer) = caching(&home);
    let (written, rows) = pictured(&sample, &[], pictures, b"", |session, _| {
        scripted(Script::new(Vec::new()), Tools::new(), session)
            .resuming(answered_with(&diagram("A")))
    });
    writer.drain();

    assert_eq!(sent(&written), 0, "{written:?}");
    assert!(cached_entries(&home).is_empty(), "no drawer ran");
    // Drawn as text, as a terminal that shows no pictures draws it.
    let (_, text) = after_a_turn_shown_as_text(&diagram("A"));
    let at = rows.iter().position(|row| row == "› draw it").unwrap();
    let after = rows.get(at + 1..).unwrap();
    for row in text.iter().filter(|row| !row.is_empty()) {
        assert!(after.contains(row), "{row:?} missing from {rows:#?}");
    }
    assert!(
        !rows
            .iter()
            .any(|row| row.contains("not drawn") || row.starts_with('!')),
        "{rows:#?}"
    );
}

/// The rows a turn answered with `answer` leaves on a terminal that shows no
/// pictures, under the line that asked.
fn after_a_turn_shown_as_text(answer: &str) -> (String, Vec<String>) {
    let sample = Sample::new("images-as-text");
    let terms = Terms {
        workspace: sample.workspace(),
        style: Cell::new(Style::coloured()),
        ..plain()
    };
    let conversation = paired(Arc::new(Session::nowhere()), |session| {
        scripted(Script::new(Vec::new()), Tools::new(), session).resuming(answered_with(answer))
    });
    let mut renderer = crate::cli::furnished(
        Recording::new(80, 60),
        &crucible_config::Settings::default(),
        Box::new(crate::cli::pictures::Raster),
    );
    renderer.wears(Style::coloured().palette());
    let mut input = Cursor::new(Vec::new());
    converse(
        conversation,
        &mut renderer,
        &terms,
        First {
            card: &opening(),
            arming: None,
        },
        &mut input,
    )
    .unwrap();
    let written = renderer.terminal().written().to_string();
    let rows: Vec<String> = crucible_tui::Picture::of(&written, 80, 60)
        .rows()
        .iter()
        .map(|row| row.trim_end().to_owned())
        .collect();
    let at = rows.iter().position(|row| row == "› draw it").unwrap();
    (written, rows.get(at + 1..).unwrap().to_vec())
}

#[test]
fn a_long_session_put_back_draws_every_kept_diagram_and_its_images_within_their_time() {
    let _turn = crate::cli::pictures::tests::ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let sample = Sample::new("images-resumed-many");
    let home = sample.root().join(".home");
    let nodes: Vec<String> = (0..10).map(|node| format!("N{node}")).collect();
    let files: Vec<(String, Vec<u8>)> = (0..12)
        .map(|file| (format!("{file}.png"), png(16, 8)))
        .collect();
    let files: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    let diagrams: String = nodes.iter().map(|node| diagram(node) + "\n").collect();
    let images: String = (0..12).fold(String::new(), |images, file| {
        images + &format!("![{file}]({file}.png)\n")
    });

    let (pictures, writer) = caching(&home);
    let (written, _) = pictured(&sample, &files, pictures, b"draw\n", |session, _| {
        scripted(Script::new(vec![saying(&diagrams)]), Tools::new(), session)
    });
    writer.drain();
    assert!(sent(&written) > 0, "{written:?}");
    let kept = cached_entries(&home);
    assert_eq!(kept.len(), 10, "each diagram is kept, in view or not");

    let started = std::time::Instant::now();
    let (pictures, writer) = caching(&home);
    let (written, rows) = pictured(&sample, &files, pictures, b"", |session, _| {
        scripted(Script::new(Vec::new()), Tools::new(), session)
            .resuming(answered_with(&(diagrams.clone() + &images)))
    });
    writer.drain();

    assert!(sent(&written) > 12, "{written:?}");
    // The diagrams in view are pictures, and none is its text.
    let images = rows
        .iter()
        .position(|row| row == "[image: 0] (0.png)")
        .unwrap();
    let above = rows.get(..images).unwrap();
    assert!(
        above.iter().any(|row| row.starts_with('\u{10eeee}')),
        "{rows:#?}"
    );
    assert!(!above.iter().any(|row| row.contains("N9")), "{rows:#?}");
    for file in 0..12 {
        let edge = format!("┌─ image · {file}.png");
        assert!(rows.contains(&edge), "{edge} in {rows:#?}");
    }
    assert_eq!(cached_entries(&home), kept, "nothing drawn afresh");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}
