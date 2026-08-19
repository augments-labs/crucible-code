use super::*;

/// Everything a reading hands back, joined, so a test can check that nothing
/// was dropped or duplicated on the way through.
fn read(language: &str, line: &str) -> Vec<(Slot, String)> {
    let mut syntax = Syntax::of(language).expect("a language this build knows");
    let mut runs = Vec::new();

    syntax.read(line, &mut |slot, text| runs.push((slot, text.to_owned())));
    runs
}

/// The same, as one string.
fn said(runs: &[(Slot, String)]) -> String {
    runs.iter().map(|(_, text)| text.as_str()).collect()
}

#[test]
fn a_language_nothing_knows_is_not_read_at_all() {
    // The fallback the design calls correct rather than merely safe: the block
    // stays exactly as it is drawn today, quiet and whole.
    assert!(Syntax::of("").is_none());
    assert!(Syntax::of("wingdings").is_none());
    assert!(Syntax::of("not a language").is_none());
}

#[test]
fn the_languages_a_model_actually_writes_are_known() {
    // Not every language there is — the ones a model reaches for when it is
    // answering about code. A miss here is a block drawn quiet, which is a
    // disappointment rather than a defect, and this is the list worth keeping
    // true.
    let missing: Vec<&str> = [
        "rust",
        "python",
        "javascript",
        "typescript",
        "go",
        "c",
        "cpp",
        "java",
        "ruby",
        "shell",
        "bash",
        "sql",
        "json",
        "yaml",
        "html",
        "css",
        "markdown",
        "diff",
        "xml",
        "php",
        "perl",
        "lua",
        "haskell",
    ]
    .into_iter()
    .filter(|language| Syntax::of(language).is_none())
    .collect();

    assert!(missing.is_empty(), "not known: {missing:?}");
}

#[test]
fn typescript_is_read_as_javascript_and_that_is_written_down() {
    // The admission in `CALLED`, asserted so it stays an admission rather than
    // quietly becoming a surprise: there is no TypeScript in the set that
    // ships, and a block fenced `ts` is read as the JavaScript it extends.
    for spelled in ["typescript", "ts", "tsx", "jsx"] {
        assert!(Syntax::of(spelled).is_some(), "{spelled}");
    }

    // What it costs, stated: a type annotation is drawn as ordinary words.
    let runs = read("typescript", "const x: number = 1;\n");
    let typed = runs
        .iter()
        .find(|(_, text)| text.contains("number"))
        .map(|(slot, _)| *slot);

    assert_ne!(
        typed,
        Some(Slot::Name),
        "the cost has changed; re-read CALLED"
    );
}

#[test]
fn a_language_is_known_by_the_name_a_fence_spells_it_with() {
    // A fence says `rs` as often as `rust`, and `sh` as often as `shell`.
    for (short, long) in [
        ("rs", "rust"),
        ("py", "python"),
        ("js", "javascript"),
        ("yml", "yaml"),
    ] {
        assert!(Syntax::of(short).is_some(), "{short}");
        assert!(Syntax::of(long).is_some(), "{long}");
    }
}

#[test]
fn every_byte_of_a_line_comes_back_exactly_once() {
    // The property the whole thing rests on: a reading is a partition of the
    // line, not a rewrite of it. A byte dropped is code that silently changed
    // meaning on screen; a byte doubled is a row wider than it measured.
    for line in [
        "fn main() { let x = 1; }\n",
        "    // a comment with \"quotes\" and 42\n",
        "let s = \"日本語\"; // wide\n",
        "\n",
        "no_trailing_newline",
    ] {
        assert_eq!(said(&read("rust", line)), line, "{line:?}");
    }
}

#[test]
fn a_comment_a_string_and_a_keyword_are_told_apart() {
    let runs = read("rust", "let name = \"hi\"; // said\n");
    let slots: Vec<Slot> = runs.iter().map(|(slot, _)| *slot).collect();

    assert!(slots.contains(&Slot::Keyword), "no keyword: {runs:?}");
    assert!(slots.contains(&Slot::Str), "no string: {runs:?}");
    assert!(slots.contains(&Slot::Comment), "no comment: {runs:?}");
}

#[test]
fn a_number_is_its_own_run() {
    let runs = read("rust", "let x = 42;\n");

    assert!(
        runs.iter()
            .any(|(slot, text)| *slot == Slot::Number && text.contains("42")),
        "{runs:?}"
    );
}

#[test]
fn reading_one_line_carries_into_the_next() {
    // A block comment opened on one line and closed on another is one comment,
    // and a reader that started fresh每 line would draw the middle of it as code.
    let mut syntax = Syntax::of("rust").expect("rust");
    let mut opened = Vec::new();
    syntax.read("/* opened here\n", &mut |slot, text| {
        opened.push((slot, text.to_owned()));
    });

    let mut runs = Vec::new();
    syntax.read("still inside it\n", &mut |slot, text| {
        runs.push((slot, text.to_owned()));
    });

    assert!(
        runs.iter().all(|(slot, _)| *slot == Slot::Comment),
        "the second line left the comment: {runs:?}"
    );
}

#[test]
fn the_syntax_themes_people_name_are_the_ones_offered() {
    // A picker whose list nobody recognises is a picker nobody uses. These are
    // the names a reader already has an opinion about, and the reason the set
    // is not the seven base16-and-Solarized that the parser alone ships with.
    let every = every_theme();
    let missing: Vec<&str> = [
        "Monokai Extended",
        "GitHub",
        "Dracula",
        "Nord",
        "Solarized (dark)",
        "Solarized (light)",
        "gruvbox-dark",
        "gruvbox-light",
        "OneHalfDark",
        "OneHalfLight",
        "Coldark-Cold",
        "zenburn",
    ]
    .into_iter()
    .filter(|wanted| !every.iter().any(|named| named == wanted))
    .collect();

    assert!(
        missing.is_empty(),
        "not offered: {missing:?}\nhave: {every:?}"
    );
}

#[test]
fn the_one_drawn_unless_somebody_said_is_one_of_them() {
    assert!(every_theme().iter().any(|named| named == THEME_UNLESS_SAID));
}

#[test]
fn every_theme_offered_says_what_the_six_slots_are_worth() {
    // A theme in the list that cannot answer is a row somebody can take and
    // nothing happens.
    for named in every_theme() {
        assert!(colours(&named).is_some(), "{named}");
    }
}

#[test]
fn a_theme_says_something_different_for_at_least_a_comment_and_a_keyword() {
    // Six slots all the same colour is a theme that has been read wrongly
    // rather than a theme that is subtle.
    for named in every_theme() {
        let six = colours(&named).expect("a theme in the list to answer");
        let distinct: std::collections::BTreeSet<_> = six.iter().collect();

        assert!(distinct.len() >= 3, "{named} says only {distinct:?}");
    }
}
