//! What the hidden prompt makes of what is typed and pasted at it.

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

#[test]
fn a_key_longer_than_the_bound_is_refused_however_it_arrives_and_never_cut() {
    let key = "k".repeat(MAX_SECRET);
    let refused = Refused::Oversized.to_string();
    for (how, presses) in [
        (
            "pasted after whitespace",
            vec![
                Pressed::Pasted(format!("  {key}").into()),
                Pressed::Key(Key::Enter),
            ],
        ),
        (
            "typed after whitespace",
            "  ".chars()
                .chain(key.chars())
                .chain(['x'])
                .map(|one| Pressed::Key(Key::Char(one)))
                .chain([Pressed::Key(Key::Enter)])
                .collect(),
        ),
        (
            "pasted over what was typed",
            vec![
                Pressed::Key(Key::Char(' ')),
                Pressed::Pasted(format!("{key}x").into()),
                Pressed::Key(Key::Backspace),
                Pressed::Key(Key::Enter),
            ],
        ),
    ] {
        let outcome = typed(presses).and_then(|entered| {
            entered
                .map(|text| Secret::typed(&text).map_err(|problem| problem.to_string()))
                .transpose()
        });
        assert_eq!(
            outcome.as_ref().err(),
            Some(&refused),
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
