//! What making room changes, preserves, and reports.

use super::*;

/// A response that reports it carried `carried` tokens and then calls a tool.
fn carrying(carried: u64, id: &str) -> Vec<Delta> {
    vec![
        Delta::Carried(Carried::new(carried)),
        Delta::ToolStarted {
            id: ToolId::new(id),
            name: "missing".into(),
        },
        Delta::ToolArgs("{}".into()),
        Delta::Stopped(StopReason::WantsTools),
    ]
}

/// Compaction settings whose budget keeps only the current turn, so a
/// two-turn session has a middle. At the uncalibrated three bytes to the token,
/// a one-token budget keeps nothing before it.
fn keeping_one() -> Compaction {
    Compaction {
        keep_tokens: 1,
        ..Compaction::default()
    }
}

#[test]
fn a_compaction_posts_the_rebuilt_window_reading_immediately() {
    let script = Script::new(vec![
        vec![
            Delta::Carried(Carried::new(20_000)),
            Delta::Text("first".into()),
            Delta::Stopped(StopReason::Yielded),
        ],
        vec![
            Delta::Carried(Carried::new(30_000)),
            Delta::Text("second".into()),
            Delta::Stopped(StopReason::Yielded),
        ],
        recap("notes to self"),
    ]);
    let mut scripted = Scripted::within(script, 200_000, keeping_one());
    scripted.turn("first").expect("a turn to compact from");
    scripted.turn("second").expect("a middle to replace");
    let _ = scripted.left();

    scripted
        .runner
        .compact(
            Compacting::Asked,
            &scripted.events,
            &scripted.cancel,
            &mut Spend::default(),
        )
        .expect("a structured recap");

    let events = scripted.events();
    let finished = events
        .iter()
        .position(|event| matches!(event, Event::Compacting { part: 100, .. }))
        .expect("a final complete progress event");
    let compacted = events
        .iter()
        .position(|event| matches!(event, Event::Compacted { .. }))
        .expect("a completed compaction event");
    assert!(finished < compacted, "{events:?}");
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                Event::Carried { left } => Some(*left),
                _ => None,
            })
            .collect::<Vec<_>>(),
        [Some(99)]
    );
    assert_eq!(scripted.runner.left(), Some(99));
}

#[test]
fn the_structured_recap_uses_its_configured_ceiling_capped_by_the_model() {
    let script = Script::new(vec![
        saying("first"),
        saying("second"),
        recap("notes to self"),
    ]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);
    scripted.runner.policy.compaction = keeping_one();
    scripted.runner.policy.compaction.recap_tokens = 10_240;
    scripted.runner.spec.model.max_tokens = 12_000;
    scripted.turn("first").expect("a turn to compact from");
    scripted.turn("second").expect("a middle to replace");

    scripted
        .runner
        .compact(
            Compacting::Asked,
            &scripted.events,
            &scripted.cancel,
            &mut Spend::default(),
        )
        .expect("a structured recap");

    assert_eq!(
        scripted.sent.lock().unwrap().last().unwrap().max_tokens,
        10_240
    );

    let script = Script::new(vec![
        saying("first"),
        saying("second"),
        recap("notes to self"),
    ]);
    let mut capped = Scripted::new(script, Tools::new(), Verdict::Allow);
    capped.runner.policy.compaction = keeping_one();
    capped.runner.policy.compaction.recap_tokens = 10_240;
    capped.runner.spec.model.max_tokens = 8_000;
    capped.turn("first").expect("a turn to compact from");
    capped.turn("second").expect("a middle to replace");
    capped
        .runner
        .compact(
            Compacting::Asked,
            &capped.events,
            &capped.cancel,
            &mut Spend::default(),
        )
        .expect("a model-capped recap");

    assert_eq!(
        capped.sent.lock().unwrap().last().unwrap().max_tokens,
        8_000
    );
}

#[test]
fn a_recap_cut_off_at_its_token_ceiling_replaces_nothing() {
    let script = Script::new(vec![
        saying("first"),
        saying("second"),
        vec![
            Delta::Text("## Goal\nhalf a checkpoint".into()),
            Delta::Stopped(StopReason::OutOfTokens),
        ],
    ]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);
    scripted.runner.policy.compaction = keeping_one();
    scripted.turn("first").expect("a turn to compact from");
    scripted.turn("second").expect("a middle to replace");
    let before = scripted.runner.transcript().messages().to_vec();

    let problem = scripted
        .runner
        .compact(
            Compacting::Asked,
            &scripted.events,
            &scripted.cancel,
            &mut Spend::default(),
        )
        .expect_err("a truncated recap must not replace context");

    assert!(matches!(problem, TurnError::RecapIncomplete));
    assert_eq!(scripted.runner.transcript().messages(), before.as_slice());
}

#[test]
fn a_cleanly_stopped_but_malformed_recap_replaces_nothing() {
    let script = Script::new(vec![
        saying("first"),
        saying("second"),
        saying("notes without sections"),
    ]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);
    scripted.runner.policy.compaction = keeping_one();
    scripted.turn("first").expect("a turn to compact from");
    scripted.turn("second").expect("a middle to replace");
    let before = scripted.runner.transcript().messages().to_vec();

    let problem = scripted
        .runner
        .compact(
            Compacting::Asked,
            &scripted.events,
            &scripted.cancel,
            &mut Spend::default(),
        )
        .expect_err("a malformed recap must not replace context");

    assert!(matches!(problem, TurnError::RecapIncomplete));
    assert_eq!(scripted.runner.transcript().messages(), before.as_slice());
}

#[test]
fn a_recap_past_the_response_ceiling_replaces_nothing() {
    // The recap stream is the one response of a turn that does not pass
    // through the loop's own bounds, so it holds this ceiling itself. A
    // provider that streams without end would otherwise grow `said` without
    // limit — and a recap too large to be notes is not notes.
    let note = "x".repeat(3 * 1024 * 1024);
    let script = Script::new(vec![saying("first"), saying("second"), recap(&note)]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);
    scripted.runner.policy.compaction = keeping_one();
    scripted.turn("first").expect("a turn to compact from");
    scripted.turn("second").expect("a middle to replace");
    let before = scripted.runner.transcript().messages().to_vec();

    let problem = scripted
        .runner
        .compact(
            Compacting::Asked,
            &scripted.events,
            &scripted.cancel,
            &mut Spend::default(),
        )
        .expect_err("an unbounded recap must not replace context");

    assert!(matches!(problem, TurnError::RecapIncomplete), "{problem:?}");
    assert_eq!(scripted.runner.transcript().messages(), before.as_slice());
}

#[test]
fn a_recap_stopped_part_way_replaces_nothing() {
    // Escape, while the notes are being written. What has arrived by then is
    // half a memory of the session, and standing it in place of the messages it
    // was meant to replace would lose the rest of them for good — the log still
    // holds them, but nothing the model is sent ever would again. So a recap
    // that did not finish makes no room at all.
    let script = Script::new(vec![
        vec![
            Delta::Text("first".into()),
            Delta::Stopped(StopReason::Yielded),
        ],
        vec![
            Delta::Text("second".into()),
            Delta::Stopped(StopReason::Yielded),
        ],
        // What the recap request is answered with: notes that stop half way
        // through a word, which is what a stream cut off looks like.
        vec![
            Delta::Text("half the not".into()),
            Delta::Stopped(StopReason::Cancelled),
        ],
    ]);

    let mut scripted = Scripted::within(script, 10_000, keeping_one());
    scripted.turn("first").expect("a turn to compact from");
    scripted.turn("second").expect("a middle to replace");
    let before = scripted.runner.transcript().messages().to_vec();

    let made = scripted
        .runner
        .compact(
            Compacting::Asked,
            &scripted.events,
            &scripted.cancel,
            &mut Spend::default(),
        )
        .expect("a recap somebody stopped is not a failure");

    assert_eq!(made, Room::Stopped, "a stopped recap made room");
    assert_eq!(
        scripted.runner.transcript().messages(),
        before.as_slice(),
        "a stopped recap changed the transcript"
    );
}

#[test]
fn a_recap_whose_connection_broke_says_so_and_replaces_nothing() {
    // The failure is the provider's, and the reason handed on is the
    // provider's own — a recap that broke off reading as "incomplete" sends
    // the reader looking at the model's notes when the thing that went wrong
    // was the wire. The transcript stands exactly as it was either way.
    let script = Script::breaking(vec![vec![Delta::Text("## Goal\nhalf the".into())]]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);
    scripted.runner.policy.compaction = keeping_one();

    let mut earlier = Transcript::new();
    earlier.push(Message::said("first"));
    earlier.push(Message::Agent {
        text: "one".into(),
        calls: Vec::new(),
        stop: Some(StopReason::Yielded),
    });
    earlier.push(Message::said("second"));
    earlier.push(Message::Agent {
        text: "two".into(),
        calls: Vec::new(),
        stop: Some(StopReason::Yielded),
    });
    scripted.runner = scripted.runner.resuming(earlier);
    let before = scripted.runner.transcript().messages().to_vec();

    let problem = scripted
        .runner
        .compact(
            Compacting::Asked,
            &scripted.events,
            &scripted.cancel,
            &mut Spend::default(),
        )
        .expect_err("a broken connection is a failure, not a recap");

    assert!(
        matches!(
            problem,
            TurnError::Provider(ProviderError::Transport { .. })
        ),
        "{problem:?}"
    );
    assert_eq!(scripted.runner.transcript().messages(), before.as_slice());
}

#[test]
fn a_turn_whose_recap_was_stopped_ends_rather_than_asking_again() {
    // Escape, while a turn was making room for itself. The session is exactly
    // as it was, and the one thing that must not happen next is the request
    // going out again: somebody has just said they did not want to pay for it.
    let script = Script::new(vec![
        vec![
            Delta::Text("first".into()),
            Delta::Stopped(StopReason::Yielded),
        ],
        // The provider refuses the second turn for want of room, which is what
        // sends the loop to make some.
        vec![Delta::Stopped(StopReason::WindowExceeded)],
        vec![
            Delta::Text("half the not".into()),
            Delta::Stopped(StopReason::Cancelled),
        ],
        // Never reached, and that is the assertion: a fourth round here would
        // be the question going out after the key that stopped it.
        vec![
            Delta::Text("asked again".into()),
            Delta::Stopped(StopReason::Yielded),
        ],
    ]);

    let mut scripted = Scripted::within(script, 20_000, keeping_one());
    scripted.turn("first").expect("a turn to compact from");

    let stop = scripted
        .turn("go")
        .expect("a stopped recap is not a failure");

    assert_eq!(stop, StopReason::Cancelled);
    assert!(
        !scripted.said().contains("asked again"),
        "the turn asked again after the recap was stopped"
    );
}

#[test]
fn a_full_window_is_answered_by_making_room_and_the_turn_carries_on() {
    // A first turn to have something behind, then a second whose response says
    // the request carried nearly the whole window. The loop compacts before
    // asking again — mid-turn, without ending it — and the turn reaches its own
    // ending afterwards.
    let script = Script::new(vec![
        vec![
            Delta::Text("x".repeat(15_000).into()),
            Delta::Stopped(StopReason::Yielded),
        ],
        carrying(12_000, "a"),
        // What the recap request is answered with.
        recap("notes to self"),
        vec![
            Delta::Text("carried on".into()),
            Delta::Stopped(StopReason::Yielded),
        ],
    ]);

    let mut scripted = Scripted::within(script, 20_000, keeping_one());
    scripted.turn("first").expect("a turn to compact from");

    let stop = scripted
        .turn("go")
        .expect("the turn ended instead of making room");

    assert_eq!(stop, StopReason::Yielded);
    assert!(
        scripted.said().contains("carried on"),
        "the turn did not carry on after making room"
    );
}

#[test]
fn an_answer_cut_off_by_the_window_is_recorded_before_room_is_made() {
    // The provider streamed half an answer and then ran out of room. Making
    // room and asking again is the remedy, but the half that arrived was
    // produced and paid for — dropping it silently ends that stream
    // mid-sentence with no record, which is the truncation this crate promises
    // never to write.
    let script = Script::new(vec![
        vec![
            Delta::Text("x".repeat(15_000).into()),
            Delta::Stopped(StopReason::Yielded),
        ],
        vec![
            Delta::Text("half an answer".into()),
            Delta::Stopped(StopReason::WindowExceeded),
        ],
        recap("notes to self"),
        vec![
            Delta::Text("asked again".into()),
            Delta::Stopped(StopReason::Yielded),
        ],
    ]);

    let mut scripted = Scripted::within(script, 20_000, keeping_one());
    scripted.turn("first").expect("a turn to compact from");

    let stop = scripted.turn("go").expect("the turn to carry on");

    assert_eq!(stop, StopReason::Yielded);
    assert!(
        scripted
            .runner
            .transcript()
            .messages()
            .contains(&Message::Agent {
                text: "half an answer".into(),
                calls: Vec::new(),
                stop: Some(StopReason::WindowExceeded),
            }),
        "the cut-off answer left no record"
    );
}

#[test]
fn an_answer_cut_off_by_the_window_is_recorded_when_room_is_not_made() {
    // The same stop with compaction turned off: the turn ends instead of
    // making room, and what arrived before the cut still belongs to the
    // transcript, marked with the reason it stopped.
    let script = Script::new(vec![vec![
        Delta::Text("half".into()),
        Delta::Stopped(StopReason::WindowExceeded),
    ]]);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Allow);
    scripted.runner.policy.compaction = Compaction {
        automatic: false,
        ..Compaction::default()
    };

    let stop = scripted.turn("go").expect("a refusal is not a failure");

    assert_eq!(stop, StopReason::WindowExceeded);
    assert_eq!(
        scripted.runner.transcript().messages(),
        [
            Message::said("go"),
            Message::Agent {
                text: "half".into(),
                calls: Vec::new(),
                stop: Some(StopReason::WindowExceeded),
            },
        ]
    );
}

#[test]
fn what_the_recap_request_spent_joins_the_turns_spend() {
    // The recap is a response of the turn like any other: the turn asked for
    // it, the provider produced tokens for it, and a spend reading that skipped
    // it would leave the ceiling and the row both counting a turn cheaper than
    // it ran. Across responses readings add, so the recap's 500 lands on the 90
    // already spent, and the final response's 30 lands on both.
    let mut notes = recap("notes to self");
    notes.insert(notes.len() - 1, Delta::Spent(Spend::new(500)));
    let script = Script::new(vec![
        vec![
            Delta::Text("x".repeat(15_000).into()),
            Delta::Stopped(StopReason::Yielded),
        ],
        vec![
            Delta::Carried(Carried::new(12_000)),
            Delta::Spent(Spend::new(90)),
            Delta::ToolStarted {
                id: ToolId::new("a"),
                name: "missing".into(),
            },
            Delta::ToolArgs("{}".into()),
            Delta::Stopped(StopReason::WantsTools),
        ],
        notes,
        vec![
            Delta::Text("carried on".into()),
            Delta::Spent(Spend::new(30)),
            Delta::Stopped(StopReason::Yielded),
        ],
    ]);
    let mut scripted = Scripted::within(script, 20_000, keeping_one());
    scripted.turn("first").expect("a turn to compact from");
    let _ = scripted.spent();

    scripted.turn("go").expect("the turn to carry on");

    assert_eq!(scripted.spent(), [90, 590, 620]);
}

#[test]
fn a_full_window_prunes_tool_output_from_the_active_turn_and_carries_on() {
    // The failure that used to end with NoRoom: one turn asks for enough tool
    // output to fill its window, so there is no older turn for a recap to
    // replace. The newest result remains available, older results become
    // placeholders, and the next request is still made.
    let script = Script::new(vec![
        calling("a", "read", "{}"),
        calling("b", "read", "{}"),
        calling("c", "read", "{}"),
        saying("carried on"),
    ]);
    let output = "x".repeat(90_000);
    let sample = Sample::new("runner-active-prune-history");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let mut scripted = Scripted::recording(
        script,
        tools([Fixed::new("read").answering(&output)]),
        Verdict::Allow,
        session,
    );
    scripted.runner.spec.model.window = Some(80_000);
    scripted.runner.policy.compaction = Compaction {
        reserve: Some(1),
        ..Compaction::default()
    };

    let stop = scripted
        .turn("go")
        .expect("the active turn made room instead of failing");

    let events: Vec<Event> = scripted.seen.try_iter().collect();
    assert_eq!(stop, StopReason::Yielded);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Delta { text } if text.contains("carried on")))
    );
    assert_eq!(scripted.asked(), [1, 3, 5, 7]);
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                Event::Carried { left } => Some(*left),
                _ => None,
            })
            .collect::<Vec<_>>(),
        [Some(62), Some(24), Some(0), Some(0), Some(62)]
    );

    let sizes: Vec<usize> = scripted
        .runner
        .transcript()
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults(results) => Some(
                results
                    .iter()
                    .map(|result| result.output.text().len())
                    .collect::<Vec<_>>(),
            ),
            Message::User { .. } | Message::Agent { .. } => None,
        })
        .flatten()
        .collect();
    let [older @ .., newest] = sizes.as_slice() else {
        panic!("expected three call results, got {}", sizes.len());
    };
    assert_eq!(older.len(), 2, "all three call results still have a place");
    assert!(
        older.iter().all(|size| *size < output.len()),
        "older active-turn results were not pruned: {sizes:?}"
    );
    assert_eq!(*newest, output.len(), "the newest result was pruned");

    let path = scripted.runner.session().path().to_path_buf();
    drop(scripted);
    let log = std::fs::read_to_string(path).expect("the session log");
    assert_eq!(
        log.matches(&output).count(),
        3,
        "pruning rewrote or dropped original tool output from the durable log"
    );
}

#[test]
fn a_full_window_recaps_a_complete_active_turn_when_pruning_cannot_help() {
    // Provider prose cannot be pruned. Once the response that produced it has
    // completed and the turn is between passes, recapping that complete active
    // turn is the only remaining way forward; keeping it whole forever is the
    // dead end that emitted NoRoom.
    let original = format!("original-active-pass:{}", "x".repeat(18_000));
    let script = Script::new(vec![
        vec![
            Delta::Text(original.clone().into()),
            // Enough exact output spend to cross the 16 000-token request
            // boundary without relying on the same prose being estimated too.
            Delta::Spent(Spend::new(17_000)),
            Delta::ToolStarted {
                id: ToolId::new("a"),
                name: "read".into(),
            },
            Delta::ToolArgs("{}".into()),
            Delta::Stopped(StopReason::WantsTools),
        ],
        recap("notes to self"),
        saying("carried on"),
    ]);
    let sample = Sample::new("runner-active-recap-history");
    let session = Session::start(&sample.logs(), &sample.workspace(), None).expect("a new session");
    let mut scripted =
        Scripted::recording(script, tools([Fixed::new("read")]), Verdict::Allow, session);
    scripted.runner.spec.model.window = Some(30_000);
    scripted.runner.policy.compaction = Compaction {
        reserve: Some(14_000),
        ..Compaction::default()
    };

    let stop = scripted
        .turn("go")
        .expect("the complete active turn was recapped instead of failing");

    assert_eq!(stop, StopReason::Yielded);
    assert!(scripted.said().contains("carried on"));
    assert!(
        scripted.runner.transcript().messages().iter().any(
            |message| matches!(message, Message::User { text: said, .. } if said.contains("notes to self"))
        ),
        "the active turn did not become a recap"
    );
    assert!(
        !scripted.runner.transcript().messages().iter().any(
            |message| matches!(message, Message::Agent { text, .. } if text.as_ref() == original)
        ),
        "the model-facing transcript still carried the recapped active prose"
    );

    let path = scripted.runner.session().path().to_path_buf();
    drop(scripted);
    let log = std::fs::read_to_string(path).expect("the session log");
    assert!(
        log.contains(&original),
        "active-turn compaction dropped the original pass from the durable log"
    );
    assert!(log.contains("notes to self"), "the recap was not logged");
}

#[test]
fn a_request_the_provider_would_not_take_is_asked_again_once_there_is_room() {
    // The reactive rail. Nothing said the window was full — the provider did,
    // by refusing — and the same question goes back once the session is
    // smaller.
    let script = Script::new(vec![
        vec![Delta::Stopped(StopReason::WindowExceeded)],
        recap("notes to self"),
        vec![
            Delta::Text("asked again".into()),
            Delta::Stopped(StopReason::Yielded),
        ],
    ]);

    let mut scripted = Scripted::within(script, 10_000, keeping_one());
    scripted.turn("first").expect("a turn to compact from");

    let stop = scripted.turn("go").expect("the refusal ended the turn");

    assert_eq!(stop, StopReason::Yielded);
    assert!(scripted.said().contains("asked again"));
}

#[test]
fn a_session_told_never_to_compact_fails_rather_than_making_room() {
    // Somebody who said never meant it. The turn ends with the reason, which is
    // the one thing that tells them why.
    let script = Script::new(vec![vec![Delta::Stopped(StopReason::WindowExceeded)]]);
    let compacting = Compaction {
        automatic: false,
        ..Compaction::default()
    };

    let mut scripted = Scripted::within(script, 10_000, compacting);

    assert_eq!(scripted.turn("go").unwrap(), StopReason::WindowExceeded);
}

#[test]
fn the_recap_stands_where_the_messages_it_replaced_were() {
    // The split the whole design turns on: what the model is sent is compacted,
    // and the notes stand in the transcript in the model's place.
    let script = Script::new(vec![
        vec![
            Delta::Text("x".repeat(15_000).into()),
            Delta::Stopped(StopReason::Yielded),
        ],
        carrying(12_000, "a"),
        recap("notes to self"),
        vec![
            Delta::Text("carried on".into()),
            Delta::Stopped(StopReason::Yielded),
        ],
    ]);

    let mut scripted = Scripted::within(script, 20_000, keeping_one());
    scripted.turn("first").expect("a turn");
    scripted.turn("go").expect("a turn");

    assert!(
        scripted.runner.transcript().messages().iter().any(
            |message| matches!(message, Message::User { text: said, .. } if said.contains("notes to self"))
        ),
        "the recap is not standing in the transcript"
    );
}

#[test]
fn a_window_the_provider_disproves_stops_being_claimed_at_all() {
    // The failure this answers: a table one generation out of date says a model
    // takes far less than it does, and the reading pins to nothing while the
    // session goes on working perfectly. The provider reading the request is
    // the only authority on what fits, and it has just read one larger than
    // anybody wrote down — which disproves the figure without supplying
    // another, so nothing is claimed rather than a number being invented.
    let script = Script::new(vec![vec![
        Delta::Carried(Carried::new(500_000)),
        Delta::Text("done".into()),
        Delta::Stopped(StopReason::Yielded),
    ]]);

    let mut scripted = Scripted::within(script, 200_000, Compaction::default());
    scripted.turn("go").expect("a turn");

    // Disproved, and not replaced: the vendor showed this much fits and
    // nothing about how much more would have, so claiming the window is
    // exactly the size of the thing that just fitted would pin the reading at
    // nothing all over again.
    assert_eq!(
        scripted.runner.spec.model.window, None,
        "a figure the provider disproved is still being claimed"
    );
    assert_eq!(
        scripted.runner.left(),
        None,
        "a reading is drawn against a window nobody knows"
    );
}

#[test]
fn a_request_smaller_than_the_window_says_nothing_about_how_much_larger_it_is() {
    // Only ever upwards. A short request is not evidence of a small window, and
    // treating it as one would shrink the window on every quiet turn until the
    // session compacted itself to nothing.
    let script = Script::new(vec![vec![
        Delta::Carried(Carried::new(1_000)),
        Delta::Text("done".into()),
        Delta::Stopped(StopReason::Yielded),
    ]]);

    let mut scripted = Scripted::within(script, 200_000, Compaction::default());
    scripted.turn("go").expect("a turn");

    assert_eq!(scripted.runner.spec.model.window, Some(200_000));
}

#[test]
fn a_compaction_clears_the_bulk_of_old_tool_output_before_the_recap() {
    // The two-phase shape: tool output is the bulkiest thing in a session, and
    // clearing it costs no request, so it goes before the recap runs. The call
    // and the answer's prose stay; only the result's bulk is gone, and only
    // from what the model is sent.
    //
    // Three results, newest protected first. The newest two fall inside the
    // sixty-thousand-byte protected window — each is kept because the running
    // count is still under it when they are reached — and the oldest is past it
    // and crosses the savings floor, so it is the one that goes.
    // Four reads. The first is old enough to be the middle the recap
    // replaces; the three after it are the kept tail the clearing runs over.
    let script = Script::new(vec![
        calling("early", "read", "{}"),
        saying("read early"),
        calling("a", "read", "{}"),
        saying("read a"),
        calling("b", "read", "{}"),
        saying("read b"),
        calling("c", "read", "{}"),
        saying("read c"),
        // The recap request.
        recap("notes to self"),
    ]);

    // One tool, and every call to it returns a ninety-thousand-byte result —
    // the four ids above each get one, which is what the clearing then tells
    // apart by age.
    let mut scripted = Scripted::new(
        script,
        tools([Fixed::new("read").answering(&"x".repeat(90_000))]),
        Verdict::Allow,
    );
    // Keep the three recent read turns whole — about sixty thousand tokens at
    // the uncalibrated three bytes to the token — so their results survive the
    // recap and the clearing is what the test reads. The first turn is the
    // middle that gets replaced.
    scripted.runner.policy.compaction = Compaction {
        keep_tokens: 70_000,
        ..Compaction::default()
    };

    scripted.turn("first").expect("a turn");
    scripted.turn("second").expect("a turn");
    scripted.turn("third").expect("a turn");
    scripted.turn("fourth").expect("a turn");

    let room = scripted
        .runner
        .compact(
            Compacting::Asked,
            &scripted.events,
            &scripted.cancel,
            &mut Spend::default(),
        )
        .expect("a recap");

    // The clearing freed real room, so the compaction has to say so: `before`
    // is measured before the pruning, and a `before` taken after it would read
    // a working prune as a compaction that freed nothing — which is the loop
    // the caller stops a turn for, and the false NoRoom this guards against.
    let Room::Made(compacted) = room else {
        panic!("a compaction that pruned old results made room");
    };
    assert!(
        compacted.after < compacted.before,
        "freeing room reads as progress: {} not below {}",
        compacted.after,
        compacted.before
    );

    let cleared: Vec<usize> = scripted
        .runner
        .transcript()
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults(results) => Some(
                results
                    .iter()
                    .map(|result| result.output.text().len())
                    .collect(),
            ),
            _ => None,
        })
        .collect::<Vec<Vec<usize>>>()
        .into_iter()
        .flatten()
        .collect();

    // Three results stand in the kept tail, oldest first. The newest sits
    // inside the sixty-thousand-byte protected window and keeps its ninety
    // thousand bytes; the two older ones are past it and cross the savings
    // floor, so they are placeholders of a few words.
    let [older @ .., newest] = cleared.as_slice() else {
        panic!("expected three results standing, got {}", cleared.len());
    };
    assert_eq!(cleared.len(), 3, "the kept tail: {cleared:?}");
    assert_eq!(
        *newest, 90_000,
        "the newest result was cleared: {cleared:?}"
    );
    assert!(
        older.iter().all(|size| *size < 90_000),
        "an old result kept its bulk: {cleared:?}"
    );
}

#[test]
fn a_turn_that_outweighs_the_budget_is_not_kept_whole_for_being_recent() {
    // The failure the token bound answers: a turn that is mostly one enormous
    // tool result, kept whole because it was one of the last two turns. Bounded
    // in tokens instead, it is replaced — the kept tail is what has to fit the
    // window beside the recap, and a count of turns never promised that.
    let big = "x".repeat(6_000);
    let script = Script::new(vec![
        saying("small"),
        calling("a", "read", "{}"),
        saying("after the big read"),
        saying("small again"),
        // The recap request.
        recap("notes to self"),
    ]);

    // A ten-token budget. At the uncalibrated three bytes to the token, the six
    // thousand bytes of the middle turn are about two thousand tokens — far over
    // it — while the two small turns on either side are a handful.
    let budget = Compaction {
        keep_tokens: 10,
        ..Compaction::default()
    };
    let mut scripted = Scripted::new(
        script,
        tools([Fixed::new("read").answering(&big)]),
        Verdict::Allow,
    );
    scripted.runner.policy.compaction = budget;

    scripted.turn("first").expect("a turn");
    scripted.turn("read the file").expect("a turn");
    scripted.turn("third").expect("a turn");

    scripted
        .runner
        .compact(
            Compacting::Asked,
            &scripted.events,
            &scripted.cancel,
            &mut Spend::default(),
        )
        .expect("a recap");

    let standing = scripted.runner.transcript().messages();

    // The big turn is gone: nothing standing still carries its six thousand
    // bytes. Under a count of turns it would have been the most recent but one
    // and kept whole.
    assert!(
        !standing.iter().any(|message| matches!(message,
            Message::ToolResults(results)
                if results.iter().any(|result| result.output.text().len() >= 6_000))),
        "the enormous turn was kept whole for being recent"
    );

    // And the cut still landed on a user prompt: the recap is followed by a
    // whole turn, so no call is parted from the result that answers it.
    let recap = standing
        .iter()
        .position(
            |message| matches!(message, Message::User { text: said, .. } if said.contains("notes to self")),
        )
        .expect("the recap is standing");
    assert!(
        matches!(standing.get(recap + 1), Some(Message::User { .. })),
        "what follows the recap does not open a turn"
    );
}

#[test]
fn the_recap_request_carries_no_system_prompt_so_a_standing_note_cannot_become_a_permanent_one() {
    // Two things are called notes. The standing prompt carries what happened
    // between two turns, taken from a queue that empties as it is read; a recap
    // carries what the pruned span of the transcript said, and stays in the
    // transcript for the rest of the session. They must not become each other.
    //
    // Sending the ordinary prompt on this one request is what would join them:
    // the model would be summarising a transcript with a note about a finished
    // background command standing over it, and a one-turn note it read there
    // could land in the recap and be carried on every turn afterwards. So this
    // request sends none, and the fake records whether it did, because nothing
    // else about it looks different from an ordinary turn.
    let script = Script::new(vec![
        vec![
            Delta::Carried(Carried::new(20_000)),
            Delta::Text("first".into()),
            Delta::Stopped(StopReason::Yielded),
        ],
        vec![
            Delta::Carried(Carried::new(30_000)),
            Delta::Text("second".into()),
            Delta::Stopped(StopReason::Yielded),
        ],
        recap("notes to self"),
    ]);
    let sent = script.sent();
    let mut scripted = Scripted::within(script, 200_000, keeping_one());

    // The session has one, standing note and all. Without this the runner sends
    // no prompt on any request and the assertion below would hold over a
    // session that never had one to leave off.
    scripted
        .runner
        .telling("You are an expert in coding.\n\n## Since your last turn\n\ncrucible, not the developer: commands you left running have ended.");

    scripted.turn("first").expect("a turn to compact from");
    scripted.turn("second").expect("a middle to replace");
    let _ = scripted.left();

    scripted
        .runner
        .compact(
            Compacting::Asked,
            &scripted.events,
            &scripted.cancel,
            &mut Spend::default(),
        )
        .expect("a structured recap");

    let sent = sent.lock().unwrap();
    let (asked, turns) = sent.split_last().expect("the recap request");

    assert!(!asked.had_system, "the recap request carried a prompt");

    // And the ordinary turns did carry one, so the assertion above is about
    // this request rather than about a runner that never sends a prompt at all.
    assert!(!turns.is_empty(), "no ordinary turn to compare against");
    assert!(
        turns.iter().all(|one| one.had_system),
        "an ordinary turn went without a prompt"
    );
}
