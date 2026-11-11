//! Links, addresses and forge references: what the reader draws for each, the
//! quiet address it adds after words that are not their address, and what it
//! leaves as written.

use super::*;

#[test]
fn a_link_is_its_words_carrying_the_address_and_a_quiet_address_after_them() {
    // The sentence reads as the sentence the answer wrote. The address rides
    // on the words, for a terminal that opens links, and a quiet copy of it
    // follows them for one that does not.
    let said = pointing("see [the guide](https://example.com/guide) for more");

    assert_eq!(wrote(&said), "see the guide (example.com/guide) for more");
    assert_eq!(
        said.iter().map(|(slot, ..)| *slot).collect::<Vec<_>>(),
        [Slot::Plain, Slot::Link, Slot::Quiet, Slot::Plain]
    );
    assert_eq!(points(&said), [("the guide", "https://example.com/guide")]);
}

#[test]
fn a_link_whose_words_are_its_address_says_it_once() {
    let said = whole("[https://example.com](https://example.com)");

    assert_eq!(drawn(&said), "https://example.com");
    assert_eq!(slots(&said), vec![Slot::Link]);
}

#[test]
fn a_link_with_no_words_is_its_address() {
    let said = whole("[](https://example.com)");

    assert_eq!(drawn(&said), "https://example.com");
    assert_eq!(slots(&said), vec![Slot::Link]);
}

#[test]
fn the_title_after_an_address_is_not_part_of_it() {
    let said = pointing("[a](<https://example.com> \"The title\")");

    assert_eq!(wrote(&said), "a (example.com)");
    assert_eq!(points(&said), [("a", "https://example.com")]);
}

#[test]
fn a_bracket_that_was_never_a_link_is_left_exactly_as_it_was() {
    for written in [
        "[TODO] fix this",
        "an array is arr[0] and no more",
        "[unclosed and then the line ends\n",
        "[half](and then the line ends\n",
    ] {
        assert_eq!(drawn(&ended(written)), written, "{written:?}");
    }
}

#[test]
fn a_bracket_inside_a_span_of_code_is_an_index() {
    let said = whole("`arr[0](x)` and arr[1](y)");

    assert_eq!(drawn(&said), "arr[0](x) and arr1 (y)");
}

#[test]
fn a_link_split_across_deltas_is_still_one_link() {
    // Every boundary a socket could put in the middle of one, one at a time.
    let whole_link = "see [the guide](https://example.com) now";
    for at in 0..whole_link.len() {
        if !whole_link.is_char_boundary(at) {
            continue;
        }

        let mut markdown = Markdown::default();
        let mut said = read(&mut markdown, &whole_link[..at]);
        said.extend(read(&mut markdown, &whole_link[at..]));
        markdown.finish_runs(ROOM, &mut |slot, text, _| {
            said.push((slot, text.to_owned()));
        });

        assert_eq!(
            drawn(&said),
            "see the guide (example.com) now",
            "split at {at}"
        );
    }
}

#[test]
fn a_message_that_ended_inside_a_link_keeps_what_it_wrote() {
    let mut markdown = Markdown::default();
    let mut said = read(&mut markdown, "see [the guide");
    markdown.finish_runs(ROOM, &mut |slot, text, _| {
        said.push((slot, text.to_owned()));
    });

    assert_eq!(drawn(&said), "see [the guide");
}

#[test]
fn a_link_in_a_fenced_block_is_code() {
    let said = whole("```\n[a](b)\n```\n");

    assert_eq!(drawn(&said), "┌─\n│ [a](b)\n└─\n");
}

#[test]
fn a_number_on_a_later_line_is_counted_against_the_same_repository() {
    // The repository and the checkout are the session's, not the line's: a
    // line break forgets what the line was doing and nothing about where the
    // session is.
    let said = counted("one\nlanded in #487 yesterday\n");

    assert_eq!(
        points(&said),
        [(
            "#487",
            "https://github.com/augments-labs/crucible-code/issues/487"
        )]
    );

    let opened = opened("one\nsee [a](src/a.rs:3)\n");
    assert_eq!(points(&opened), [("a", "file:///repo/src/a.rs#3")]);
}

/// What `answer` hands on, read against `forge`, with where each run points;
/// a quiet address as the record draws it, quiet and pointing nowhere.
fn linked(answer: &str, forge: Option<Forge>) -> Vec<(Slot, String, Option<String>)> {
    let mut markdown = Markdown::default().counting(forge);
    let mut said = Vec::new();
    let mut into = |slot, text: &str, link: Option<&str>| {
        said.push((slot, text.to_owned(), link.map(str::to_owned)));
    };
    markdown.read_runs(answer, ROOM, &mut into);
    markdown.finish_runs(ROOM, &mut into);
    said
}

/// The quiet runs of one of those answers.
fn quiet(said: &[(Slot, String, Option<String>)]) -> Vec<&str> {
    said.iter()
        .filter(|(slot, _, _)| *slot == Slot::Quiet)
        .map(|(_, text, _)| text.as_str())
        .collect()
}

#[test]
fn a_link_whose_words_are_not_its_address_is_followed_by_its_quiet_address() {
    let cases = [
        (
            "see [the guide](https://example.com/docs/guide/intro) now",
            "see the guide (example.com/docs/…) now",
            ("the guide", "https://example.com/docs/guide/intro"),
        ),
        (
            "see [the site](https://example.com) now",
            "see the site (example.com) now",
            ("the site", "https://example.com"),
        ),
        (
            "see [the docs](http://example.com/docs) now",
            "see the docs (example.com/docs) now",
            ("the docs", "http://example.com/docs"),
        ),
        (
            "see [the search](https://example.com/find?q=1) now",
            "see the search (example.com/find?q=1) now",
            ("the search", "https://example.com/find?q=1"),
        ),
        (
            "see [a](src/a.rs:3) now",
            "see a (src/a.rs:3) now",
            ("a", "src/a.rs:3"),
        ),
        (
            "see [write](mailto:someone@example.com) now",
            "see write (someone@example.com) now",
            ("write", "mailto:someone@example.com"),
        ),
    ];
    for (answer, drawn, pointed) in cases {
        let said = linked(answer, None);
        assert_eq!(wrote(&said), drawn, "{answer:?}");
        assert_eq!(points(&said), [pointed], "{answer:?}");
        let aside = drawn
            .strip_prefix(&format!("see {}", pointed.0))
            .and_then(|rest| rest.strip_suffix(" now"))
            .unwrap_or_default();
        assert_eq!(quiet(&said), [aside], "{answer:?}");
    }
}

#[test]
fn an_address_drawn_as_its_own_words_has_nothing_after_it() {
    let cases = [
        (
            "see https://example.com/a/b now",
            "see https://example.com/a/b now",
        ),
        (
            "see <https://example.com/a/b> now",
            "see https://example.com/a/b now",
        ),
        (
            "see <someone@example.com> now",
            "see someone@example.com now",
        ),
        ("see www.example.com/a/b now", "see www.example.com/a/b now"),
        (
            "see [https://example.com/a/b](https://example.com/a/b) now",
            "see https://example.com/a/b now",
        ),
        (
            "see [example.com/a/b](https://example.com/a/b) now",
            "see example.com/a/b now",
        ),
        (
            "see [someone@example.com](mailto:someone@example.com) now",
            "see someone@example.com now",
        ),
        (
            "see [](https://example.com/a/b) now",
            "see https://example.com/a/b now",
        ),
    ];
    for (answer, drawn) in cases {
        let said = linked(answer, None);
        assert_eq!(wrote(&said), drawn, "{answer:?}");
        assert_eq!(quiet(&said), Vec::<&str>::new(), "{answer:?}");
        assert_eq!(points(&said).len(), 1, "{answer:?}");
    }
}

#[test]
fn a_quiet_address_stops_at_twenty_four_columns() {
    // An address of twenty-four columns or fewer, once its scheme is gone, is
    // shown whole. A longer one keeps its host and first segment, and what
    // stands before the ellipsis is never wider than the bound: a host or
    // segment past it is cut there, and a wide character that would straddle
    // the bound is left out rather than cut in half.
    let host = |columns: usize| format!("{}.io", "h".repeat(columns - 3));
    let segment = |columns: usize| "s".repeat(columns);
    let cases = [
        (format!("https://{}", host(23)), format!(" ({})", host(23))),
        (format!("https://{}", host(24)), format!(" ({})", host(24))),
        (
            format!("https://{}", host(25)),
            format!(" ({}…)", "h".repeat(22) + ".i"),
        ),
        (
            format!("https://{}/a", host(22)),
            format!(" ({}/a)", host(22)),
        ),
        (
            format!("https://{}/a", host(23)),
            format!(" ({}/…)", host(23)),
        ),
        (
            format!("https://x/{}/z", segment(20)),
            format!(" (x/{}/z)", segment(20)),
        ),
        (
            format!("https://x/{}/z", segment(21)),
            format!(" (x/{}/…)", segment(21)),
        ),
        (
            format!("https://x/{}/z", segment(22)),
            format!(" (x/{}…)", segment(22)),
        ),
        (
            format!("https://x/{}", segment(22)),
            format!(" (x/{})", segment(22)),
        ),
        (
            format!("https://x/{}", segment(23)),
            format!(" (x/{}…)", segment(22)),
        ),
        (
            format!("https://{}漢/a", "h".repeat(23)),
            format!(" ({}…)", "h".repeat(23)),
        ),
        (
            "src/a/b/c/d/e/f/g/h/ij.rs".to_owned(),
            " (src/a/b/c/d/e/f/g/h/ij.r…)".to_owned(),
        ),
        (
            "src/a/b/c/d/e/f/g/h/i.rs".to_owned(),
            " (src/a/b/c/d/e/f/g/h/i.rs)".to_owned(),
        ),
        (
            "mailto:someone.with.a.long.name@example.com".to_owned(),
            " (someone.with.a.long.name…)".to_owned(),
        ),
    ];
    for (address, aside) in cases {
        let said = linked(&format!("[words]({address})"), None);
        assert_eq!(quiet(&said), [aside.as_str()], "{address:?}");
        assert_eq!(points(&said), [("words", address.as_str())], "{address:?}");
    }
}

#[test]
fn a_quiet_address_is_cut_with_the_glyph_sets_ellipsis() {
    let mut markdown = Markdown::new(Glyphs::Ascii);
    let mut said = Vec::new();
    markdown.read_runs(
        "[the guide](https://example.com/docs/guide/intro)",
        ROOM,
        &mut |slot, text, _| said.push((slot, text.to_owned())),
    );
    markdown.finish_runs(ROOM, &mut |slot, text, _| {
        said.push((slot, text.to_owned()));
    });

    assert_eq!(
        said.iter()
            .filter(|(slot, _)| *slot == Slot::Quiet)
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>(),
        [" (example.com/docs/...)"]
    );
}

#[test]
fn a_forge_address_drawn_as_its_own_words_is_drawn_as_its_reference() {
    let here = "https://github.com/augments-labs/crucible-code";
    let cases = [
        (
            format!("see {here}/pull/1324 now"),
            "#1324",
            format!("{here}/pull/1324"),
        ),
        (
            format!("see <{here}/issues/7> now"),
            "#7",
            format!("{here}/issues/7"),
        ),
        (
            format!("see [{here}/commit/abcdef1234]({here}/commit/abcdef1234) now"),
            "abcdef1",
            format!("{here}/commit/abcdef1234"),
        ),
        (
            "see https://github.com/someone/else/pull/12/ now".to_owned(),
            "someone/else#12",
            "https://github.com/someone/else/pull/12/".to_owned(),
        ),
        (
            "see https://github.com/someone/else/commit/abcdef1234 now".to_owned(),
            "someone/else@abcdef1",
            "https://github.com/someone/else/commit/abcdef1234".to_owned(),
        ),
        (
            "see https://gitlab.com/group/sub/repo/-/merge_requests/3 now".to_owned(),
            "group/sub/repo!3",
            "https://gitlab.com/group/sub/repo/-/merge_requests/3".to_owned(),
        ),
        (
            "see https://gitlab.com/group/repo/-/issues/3 now".to_owned(),
            "group/repo#3",
            "https://gitlab.com/group/repo/-/issues/3".to_owned(),
        ),
        (
            // The same repository on another host is another repository.
            "see https://gitlab.com/augments-labs/crucible-code/-/issues/5 now".to_owned(),
            "augments-labs/crucible-code#5",
            "https://gitlab.com/augments-labs/crucible-code/-/issues/5".to_owned(),
        ),
    ];
    for (answer, short, address) in cases {
        let said = linked(&answer, Some(forge()));
        assert_eq!(wrote(&said), format!("see {short} now"), "{answer:?}");
        assert_eq!(points(&said), [(short, address.as_str())], "{answer:?}");
        assert_eq!(quiet(&said), Vec::<&str>::new(), "{answer:?}");
    }

    // With no repository in hand, every reference names its own.
    let said = linked(&format!("see {here}/pull/1324 now"), None);
    assert_eq!(wrote(&said), "see augments-labs/crucible-code#1324 now");
}

#[test]
fn a_worded_forge_link_keeps_its_words_and_is_followed_by_its_reference() {
    let here = "https://github.com/augments-labs/crucible-code";
    let said = linked(
        &format!("see [the fix]({here}/pull/1324) now"),
        Some(forge()),
    );
    assert_eq!(wrote(&said), "see the fix (#1324) now");
    assert_eq!(
        points(&said),
        [("the fix", format!("{here}/pull/1324").as_str())]
    );
    assert_eq!(quiet(&said), [" (#1324)"]);

    let said = linked(
        "see [the fix](https://gitlab.com/group/repo/-/merge_requests/9) now",
        Some(forge()),
    );
    assert_eq!(wrote(&said), "see the fix (group/repo!9) now");
}

#[test]
fn an_address_past_the_item_or_off_https_is_an_ordinary_link() {
    let here = "https://github.com/augments-labs/crucible-code";
    for address in [
        format!("{here}/pull/1324/files"),
        format!("{here}/pull/1324?w=1"),
        format!("{here}/pull/1324#issuecomment-1"),
        "http://github.com/augments-labs/crucible-code/pull/1324".to_owned(),
        "https://example.com/augments-labs/crucible-code/pull/1324".to_owned(),
    ] {
        let said = linked(&format!("see {address} now"), Some(forge()));
        assert_eq!(wrote(&said), format!("see {address} now"), "{address:?}");
    }
}

/// What the reader told while `answer` was read, as `told` sets it out, the
/// whole addresses it drew shorter and nothing else.
fn shortened(answer: &str, forge: Option<Forge>) -> Vec<String> {
    let mut markdown = Markdown::default().counting(forge);
    let mut told = Vec::new();
    let mut say = |said: Said<'_>| {
        if let Said::Shortened(address) = said {
            told.push(address.to_owned());
        }
    };
    markdown.read(answer, ROOM, &mut say);
    markdown.finish(ROOM, &mut say);
    told
}

#[test]
fn every_address_drawn_shorter_than_written_is_told_whole_and_no_other() {
    let here = "https://github.com/augments-labs/crucible-code";
    let answer = format!(
        "a {here}/pull/1 b [w]({here}/issues/2) c [d](https://example.com/docs/guide/intro) \
         e [f](https://example.com/docs/guide) g #3 h https://example.com/x/y\n"
    );
    assert_eq!(
        shortened(&answer, Some(forge())),
        [
            format!("{here}/pull/1"),
            format!("{here}/issues/2"),
            "https://example.com/docs/guide/intro".to_owned(),
        ]
    );
}

#[test]
fn an_image_is_its_alt_words_in_brackets_and_its_quiet_address() {
    // Nothing is fetched: what a terminal can show of a picture is what the
    // answer said it was, and where it is.
    let cases = [
        (
            "see ![the screenshot](out/shot.png) now",
            "see [image: the screenshot] (out/shot.png) now",
            vec![("[image: the screenshot]", "out/shot.png")],
        ),
        (
            "see ![](https://example.com/a.png) now",
            "see [image] (example.com/a.png) now",
            vec![("[image]", "https://example.com/a.png")],
        ),
        (
            "see wow![a](b.png) now",
            "see wow[image: a] (b.png) now",
            vec![("[image: a]", "b.png")],
        ),
        // A bang that opens nothing is the bang it was.
        ("see ![a] now!", "see ![a] now!", vec![]),
        ("see ![a]\nnow", "see ![a]\nnow", vec![]),
        ("see ! [a](b) now", "see ! a (b) now", vec![("a", "b")]),
        ("see \\![a](b) now", "see \\!a (b) now", vec![("a", "b")]),
        ("see `![a](b)` now", "see ![a](b) now", vec![]),
    ];
    for (answer, drawn, pointed) in cases {
        let said = linked(answer, None);
        assert_eq!(wrote(&said), drawn, "{answer:?}");
        assert_eq!(points(&said), pointed, "{answer:?}");
    }

    // A picture in the checkout points at the file, as any link to a path.
    let said = opened("see ![the screenshot](out/shot.png) now");
    assert_eq!(
        points(&said),
        [("[image: the screenshot]", "file:///repo/out/shot.png")]
    );
}

#[test]
fn an_image_cut_anywhere_is_the_image_it_was() {
    for answer in [
        "see ![the screenshot](out/shot.png) now\n",
        "see ![](https://example.com/a.png) now\n",
        "wow! ![a](b) now!",
        "![a] b!",
    ] {
        let whole = linked(answer, None);
        for cut in (0..=answer.len()).filter(|&cut| answer.is_char_boundary(cut)) {
            let (head, tail) = answer.split_at(cut);
            let mut markdown = Markdown::default();
            let mut said = Vec::new();
            let mut into = |slot, text: &str, link: Option<&str>| {
                said.push((slot, text.to_owned(), link.map(str::to_owned)));
            };
            markdown.read_runs(head, ROOM, &mut into);
            markdown.read_runs(tail, ROOM, &mut into);
            markdown.finish_runs(ROOM, &mut into);
            assert_eq!(wrote(&said), wrote(&whole), "{answer:?} cut at {cut}");
            assert_eq!(points(&said), points(&whole), "{answer:?} cut at {cut}");
        }
    }
}

/// The one body row of a one-column table holding `written`, laid out against
/// `forge` as the record lays a table block.
fn celled(written: &str, forge: Option<Forge>) -> String {
    let block = Block {
        kind: Kind::Table,
        source: format!("| x |\n|---|\n| {written} |\n"),
        closed: true,
    };
    let links = Links { forge, files: None };
    lay(&block, ROOM, Glyphs::Unicode, &links)
        .last()
        .map(|row| row.text().trim_end().to_owned())
        .unwrap_or_default()
}

#[test]
fn a_cell_draws_what_a_paragraph_draws_but_the_quiet_address() {
    // One reader decides what a link is drawn as wherever it stands: a cell
    // draws the words a paragraph does, and only the quiet address after
    // them, which a column has no room to spare for, is left out.
    let here = "https://github.com/augments-labs/crucible-code";
    let pull = format!("{here}/pull/1324");
    let elsewhere = "https://github.com/someone/else/issues/12";
    let commit = format!("{here}/commit/abcdef1234567");
    let shapes = [
        pull.clone(),
        format!("<{pull}>"),
        format!("[the fix]({pull})"),
        elsewhere.to_owned(),
        commit,
        "[the guide](https://example.com/docs/guide/intro)".to_owned(),
        "https://example.com/docs/guide/intro".to_owned(),
        "![the screenshot](out/shot.png)".to_owned(),
        "[write](mailto:someone@example.com)".to_owned(),
    ];
    for forge in [Some(forge()), None] {
        for written in &shapes {
            let said = linked(written, forge.clone());
            let words: String = said
                .iter()
                .filter(|(slot, _, _)| *slot != Slot::Quiet)
                .map(|(_, text, _)| text.as_str())
                .collect();
            assert_eq!(
                celled(written, forge.clone()),
                words,
                "{written:?} against {forge:?}"
            );
        }
    }
}

#[test]
fn an_address_between_angle_brackets_is_drawn_as_the_bare_one_is() {
    let here = "https://github.com/augments-labs/crucible-code";
    for address in [
        format!("{here}/pull/1324"),
        format!("{here}/commit/abcdef1234567"),
        "https://github.com/someone/else/issues/12".to_owned(),
        "https://gitlab.com/group/sub/repo/-/merge_requests/3".to_owned(),
        "https://example.com/docs/guide/intro".to_owned(),
    ] {
        for forge in [Some(forge()), None] {
            let bare = linked(&format!("see {address} now"), forge.clone());
            let angled = linked(&format!("see <{address}> now"), forge.clone());
            assert_eq!(wrote(&angled), wrote(&bare), "{address:?}");
            assert_eq!(points(&angled), points(&bare), "{address:?}");
            assert_eq!(
                shortened(&format!("see <{address}> now"), forge.clone()),
                shortened(&format!("see {address} now"), forge.clone()),
                "{address:?}"
            );
        }
    }
}
