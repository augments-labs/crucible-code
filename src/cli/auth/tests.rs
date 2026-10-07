//! What the hidden prompt makes of what is typed and pasted at it, and what
//! the sign-in prompt shows of the page and code a vendor sends.

use super::*;

/// What `presses` type, each one read without trouble.
fn typed(presses: Vec<Pressed>) -> Result<Option<String>, String> {
    typing(presses.into_iter().map(Ok::<_, &str>))
}

#[test]
fn a_key_at_the_bound_is_taken_whole_with_the_whitespace_around_it() {
    let key = "k".repeat(MAX_SECRET.saturating_sub(1));
    let typed = typed(vec![
        Pressed::Pasted(format!("  {key}").into()),
        Pressed::Key(Key::Enter),
    ]);

    let secret = Secret::typed(&typed.expect("read").expect("entered"));
    assert!(secret.is_ok(), "a key under the bound, padded, is taken");
}

/// `key` with `before` and `after` spaces around it, as one paste and as one
/// press at a time, each ended with Enter.
fn arrivals(key: &str, before: usize, after: usize) -> [(&'static str, Vec<Pressed>); 2] {
    let text = format!("{}{key}{}", " ".repeat(before), " ".repeat(after));
    [
        (
            "pasted",
            vec![
                Pressed::Pasted(text.clone().into()),
                Pressed::Key(Key::Enter),
            ],
        ),
        (
            "typed",
            text.chars()
                .map(|one| Pressed::Key(Key::Char(one)))
                .chain([Pressed::Key(Key::Enter)])
                .collect(),
        ),
    ]
}

/// What Enter makes of `presses`: the key stored, or what is said instead.
fn entered(presses: Vec<Pressed>) -> Result<String, String> {
    match typed(presses)? {
        Some(text) => Secret::typed(&text)
            .map(|_| text.trim().to_owned())
            .map_err(|problem| problem.to_string()),
        None => Err("left the prompt".to_owned()),
    }
}

#[test]
fn a_key_of_exactly_the_bound_is_taken_with_the_whitespace_the_stdin_reader_allows() {
    let key = "k".repeat(MAX_SECRET);
    for (before, after) in [
        (0, 2),
        (SURROUNDING / 2, SURROUNDING / 2),
        (0, SURROUNDING),
        (SURROUNDING, 0),
    ] {
        for (how, presses) in arrivals(&key, before, after) {
            assert_eq!(
                entered(presses).map(|taken| taken == key),
                Ok(true),
                "{how} with {before} before and {after} after: the key itself fits"
            );
        }
    }
    let crlf = format!("{key}\r\n");
    assert!(
        Secret::read(&mut crlf.as_bytes()).is_ok(),
        "standard input takes the same key"
    );
    assert_eq!(
        entered(vec![Pressed::Pasted(crlf.into()), Pressed::Key(Key::Enter)])
            .map(|taken| taken == key),
        Ok(true),
        "pasted with a trailing line break"
    );
}

#[test]
fn a_key_longer_than_the_bound_is_refused_however_it_arrives_and_never_cut() {
    let refused = Refused::Oversized.to_string();
    let past = "k".repeat(MAX_SECRET.saturating_add(1));
    let mut cases = Vec::new();
    for (before, after) in [(0, 0), (2, 0), (0, 2), (SURROUNDING, 0)] {
        for (how, presses) in arrivals(&past, before, after) {
            cases.push((format!("one byte past, {how}, {before}+{after}"), presses));
        }
    }
    // Whitespace past what standard input allows refuses the same key.
    let key = "k".repeat(MAX_SECRET);
    for (before, after) in [(SURROUNDING + 1, 0), (0, SURROUNDING + 1), (SURROUNDING, 1)] {
        for (how, presses) in arrivals(&key, before, after) {
            cases.push((
                format!("whitespace past the room, {how}, {before}+{after}"),
                presses,
            ));
        }
    }
    // Backspace after an overflow leaves what would fit if the rest had been
    // cut to fit; it is refused, not the key typed.
    cases.push((
        "pasted over the room, then Backspace".to_owned(),
        vec![
            Pressed::Pasted(format!("{}{key}xx", " ".repeat(SURROUNDING)).into()),
            Pressed::Key(Key::Backspace),
            Pressed::Key(Key::Enter),
        ],
    ));
    for (how, presses) in cases {
        assert_eq!(
            entered(presses).as_ref(),
            Err(&refused),
            "{how}: a key cut to fit would have been stored"
        );
    }
}

#[test]
fn leaving_the_prompt_keeps_nothing() {
    for leaving in [
        Pressed::Escape,
        Pressed::Key(Key::Interrupt),
        Pressed::Key(Key::Eof),
    ] {
        let outcome = typed(vec![Pressed::Key(Key::Char('k')), leaving.clone()]);
        assert_eq!(outcome, Ok(None), "{leaving:?}");
    }
}

#[test]
fn a_prompt_that_cannot_be_hidden_is_refused_before_anything_is_asked_and_says_why() {
    let terminal = Ends {
        input: true,
        output: true,
        errors: true,
    };
    assert_eq!(terminal.unhidden(), None);
    assert_eq!(terminal.unsigned(), None);

    // Standard output redirected: the terminal is there, and the hidden
    // prompt still cannot be shown on it.
    let redirected = Ends {
        output: false,
        ..terminal
    };
    let why = redirected
        .unhidden()
        .expect("refused before the prompt, and before the choice that leads to it");
    assert!(why.contains("standard output"), "{why}");
    assert!(!why.contains("no terminal"), "a terminal is there: {why}");
    assert!(why.contains("--api-key-stdin"), "{why}");
    assert_eq!(
        redirected.unsigned(),
        None,
        "an account sign-in asks on standard input and error alone"
    );

    for unasked in [
        Ends {
            input: false,
            ..terminal
        },
        Ends {
            errors: false,
            ..terminal
        },
    ] {
        assert_eq!(unasked.unhidden(), Some(UNASKED), "{unasked:?}");
        assert_eq!(unasked.unsigned(), Some(UNSIGNED), "{unasked:?}");
    }
}

#[test]
fn a_sign_in_page_or_code_holding_a_line_break_cannot_add_a_line_to_the_prompt() {
    // As `said` writes it to standard error.
    let shown = crate::cli::visible(&visiting(
        "https://auth.example.test/device\nFinish signing in at https://forged.example.test",
        Some("ABCD-1234\n  and enter the code WXYZ-9876"),
    ));

    assert_eq!(shown.lines().count(), 2, "{shown:?}");
    let mut lines = shown.lines();
    assert_eq!(
        lines.next(),
        Some(
            r"Finish signing in at https://auth.example.test/device\nFinish signing in at https://forged.example.test"
        ),
        "{shown:?}"
    );
    assert_eq!(
        lines.next(),
        Some(r"  and enter the code ABCD-1234\n  and enter the code WXYZ-9876"),
        "{shown:?}"
    );
}
