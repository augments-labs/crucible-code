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

/// What a sample is read as: a row for each of its lines, holding every run
/// of the line in order with the slot it came back as.
fn reading_of(spelled: &str, sample: &str) -> Vec<Vec<(Slot, String)>> {
    let mut syntax = Syntax::of(spelled).expect("a spelling this build knows");

    sample
        .split_inclusive('\n')
        .map(|line| {
            let mut runs = Vec::new();
            syntax.read(line, &mut |slot, text| runs.push((slot, text.to_owned())));
            runs
        })
        .collect()
}

const RUST: &str = r#"// A comment.
fn main() {
    let said = "words";
    let count = 42;
}
"#;

const PYTHON: &str = r#"# A comment.
import re

def find(text):
    count = 42
    said = "words"
    seen = re.compile(r"[a-z]+\d")
    rows = "SELECT name FROM people WHERE age > 30"
    return seen
"#;

const JAVASCRIPT: &str = r#"// A comment.
function find(text) {
  const count = 42;
  const said = "words";
  return /[a-z]+\d/.test(text);
}
"#;

const TYPESCRIPT: &str = r#"// A comment.
interface Found {
  count: number;
}
function find(text: string): Found {
  const said = "words";
  return { count: /[a-z]+\d/.test(text) ? 42 : 0 };
}
"#;

const GO: &str = r#"// A comment.
package main

func main() {
    count := 42
    said := "words"
    println(said, count)
}
"#;

const C: &str = r#"/* A comment. */
#include <stdio.h>

int main(void) {
    int count = 42;
    const char *said = "words";
    return count;
}
"#;

const CPP: &str = r#"// A comment.
#include <string>

class Greeter {
public:
    int count = 42;
    std::string said = "words";
    int find() { return count; }
};
"#;

const JAVA: &str = r#"/**
 * A comment with <b>markup</b>.
 * @param count how many
 */
class Greeter {
    int count = 42;
    String said = "words";
    int find() { return count; }
}
"#;

const RUBY: &str = r#"# A comment.
def find(text)
  count = 42
  said = "words"
  rows = <<-SQL
    SELECT name FROM people WHERE age > 30
  SQL
  page = <<-HTML
    <p class="said">words</p>
  HTML
  look = <<-CSS
    p.said { margin: 42px; }
  CSS
  code = <<-JS
    const count = 42;
  JS
  rows
end
"#;

const SHELL: &str = r#"# A comment.
count=42
said="words"
alias ll='ls -l'
find() {
  local total=$((count + 1))
  if [ "$count" -gt 1 ]; then
    echo "$said"
  fi
  cd /tmp
}
"#;

const SQL: &str = r"-- A comment.
SELECT name, 42 FROM people WHERE said = 'words';
CREATE TABLE people (name TEXT);
";

const JSON: &str = r#"// A comment.
{
  "said": "words",
  "count": 42,
  "kept": true,
  "none": null
}
"#;

const YAML: &str = r#"# A comment.
said: "words"
count: 42
kept: true
list:
  - one
"#;

const HTML: &str = r#"<!-- A comment. -->
<html>
<head>
<style>
p.said { color: red; margin: 42px; }
</style>
<script>
const count = 42; // counted
</script>
</head>
<body><p class="said" style="margin: 42px" onclick="find(42)">words</p></body>
</html>
"#;

const CSS: &str = r#"/* A comment. */
p.said {
  color: red;
  margin: 42px;
  content: "words";
}
@media screen { a { color: blue; } }
"#;

const MARKDOWN: &str = r#"# A heading

Some *words*, some `code` and a [link](https://example.com).

<!-- A comment. -->
<p class="said">words</p>

```html
<b>raw</b>
```

1. counted
"#;

const DIFF: &str = r"--- a/said.txt
+++ b/said.txt
@@ -1,3 +1,3 @@
 kept
-was
+now
";

const XML: &str = r#"<?xml version="1.0"?>
<!-- A comment. -->
<people count="42">
  <person name="words">text &amp; more</person>
</people>
"#;

const PHP: &str = r#"<p class="said">words</p>
<?php
// A comment.
function find($text) {
    $count = 42;
    $said = "words";
    $rows = "SELECT name FROM people WHERE age > 30";
    $page = <<<HTML
<b>words</b>
HTML;
    return $rows;
}
?>
<p>after</p>
"#;

const PERL: &str = r#"# A comment.
use strict;

sub find {
    my $count = 42;
    my $said = "words";
    my $rows = <<SQL;
SELECT name FROM people WHERE age > 30
SQL
    my $page = <<HTML;
<p class="said">words</p>
HTML
    return $rows;
}
"#;

const LUA: &str = r#"-- A comment.
local function find(text)
  local count = 42
  local said = "words"
  return said
end
"#;

const HASKELL: &str = r#"-- A comment.
module Main where

main :: IO ()
main = do
  let count = 42
  putStrLn "words"
"#;

/// Each language `the_languages_a_model_actually_writes_are_known` lists, in
/// its order, then each spelling of [`CALLED`] that is not one of them, with
/// the code it is read on: a comment, a string, a number, a keyword and a name
/// wherever its definition tells them apart, and a part for every other
/// definition its own one hands text to, which [`LENT`] lists.
const SAMPLES: [(&str, &str); 30] = [
    ("rust", RUST),
    ("python", PYTHON),
    ("javascript", JAVASCRIPT),
    ("typescript", TYPESCRIPT),
    ("go", GO),
    ("c", C),
    ("cpp", CPP),
    ("java", JAVA),
    ("ruby", RUBY),
    ("shell", SHELL),
    ("bash", SHELL),
    ("sql", SQL),
    ("json", JSON),
    ("yaml", YAML),
    ("html", HTML),
    ("css", CSS),
    ("markdown", MARKDOWN),
    ("diff", DIFF),
    ("xml", XML),
    ("php", PHP),
    ("perl", PERL),
    ("lua", LUA),
    ("haskell", HASKELL),
    ("ts", TYPESCRIPT),
    ("tsx", TYPESCRIPT),
    ("mts", TYPESCRIPT),
    ("cts", TYPESCRIPT),
    ("jsx", JAVASCRIPT),
    ("console", SHELL),
    ("golang", GO),
];

/// The parts of the samples that a definition other than the sample's own
/// reads: the sample, the line, one run of that line and the slot the other
/// definition gives it.
///
/// A definition loaded apart from the ones it leans on still reads its own
/// code and goes quiet exactly here, so each of these is a place where a
/// change to how definitions are loaded would show first.
const LENT: [(&str, &str, &str, Slot); 24] = [
    // A raw string is a regular expression, and a string that opens with a
    // statement is SQL.
    (
        PYTHON,
        r#"    seen = re.compile(r"[a-z]+\d")"#,
        "+",
        Slot::Operator,
    ),
    (PYTHON, "    rows = \"SELECT", "SELECT", Slot::Keyword),
    // A regular expression between slashes.
    (JAVASCRIPT, r"  return /[a-z]+\d/", "+", Slot::Operator),
    (
        TYPESCRIPT,
        r"  return { count: /[a-z]+\d/",
        "+",
        Slot::Operator,
    ),
    // Numbers, strings and comments are C's.
    (CPP, "    int count = 42;", "42", Slot::Number),
    (CPP, "    std::string said", "words", Slot::Str),
    // A documentation comment, and the markup inside one.
    (JAVA, " * @param count", "param", Slot::Keyword),
    (JAVA, " * A comment with <b>", "b", Slot::Name),
    // A document whose mark names a language.
    (RUBY, "    SELECT name", "SELECT", Slot::Keyword),
    (RUBY, "    <p class", "p", Slot::Name),
    (RUBY, "    p.said {", "px", Slot::Keyword),
    (RUBY, "    const count", "const", Slot::Keyword),
    // The commands a shell has built in.
    (SHELL, "alias ll=", "ll", Slot::Name),
    (SHELL, "  local total=", "local", Slot::Keyword),
    // A sheet, a script, and an attribute that holds either.
    (HTML, "p.said { color: red;", "px", Slot::Keyword),
    (HTML, "const count = 42; // counted", "const", Slot::Keyword),
    (HTML, "<body>", "42", Slot::Number),
    // Markup written straight into the prose.
    (MARKDOWN, "<p class=", "p", Slot::Name),
    // The page around the code, the code, a string that opens with a
    // statement, and a document whose mark names a language.
    (PHP, "<p class=", "p", Slot::Name),
    (PHP, "function find(", "function", Slot::Keyword),
    (PHP, "    $rows = \"SELECT", "SELECT", Slot::Keyword),
    (PHP, "<b>words</b>", "b", Slot::Name),
    // A document whose mark names a language.
    (PERL, "SELECT name", "SELECT", Slot::Keyword),
    (PERL, "<p class=", "p", Slot::Name),
];

#[test]
fn every_part_another_definition_reads_comes_back_as_what_it_is() {
    // A sample whose lent part was already read as plain words would hold
    // nothing about it: this is what makes each of those parts a live one.
    let mut quiet = Vec::new();
    for (sample, line, run, slot) in LENT {
        let at = sample
            .lines()
            .position(|whole| whole.starts_with(line))
            .expect("a line of the sample");
        for (spelled, _) in SAMPLES.iter().filter(|(_, sampled)| *sampled == sample) {
            let rows = reading_of(spelled, sample);
            let read = rows.get(at).expect("a row for the line");
            if !read.iter().any(|(was, text)| *was == slot && text == run) {
                quiet.push(format!(
                    "{spelled}: {run:?} of {line:?} is not {slot:?}: {read:?}"
                ));
            }
        }
    }

    assert!(quiet.is_empty(), "{quiet:#?}");
}

#[test]
fn every_theme_offered_is_worth_what_it_was() {
    // What a reader's code is coloured with, theme by theme, in the order the
    // picker lists them. A theme that goes missing, moves or changes a colour
    // changes what somebody's screen looks like, and this is where it shows.
    use std::fmt::Write as _;

    let mut said = String::new();
    for named in every_theme() {
        let six = colours(&named).expect("a theme in the list to answer");
        let six: Vec<String> = six
            .iter()
            .map(|(red, green, blue)| format!("#{red:02x}{green:02x}{blue:02x}"))
            .collect();
        let _ = writeln!(said, "{named}: {}", six.join(" "));
    }

    insta::assert_snapshot!(said);
}

#[test]
fn every_spelling_with_a_sample_reads_it_as_it_did() {
    // Every run of every line, with its slot: what a block in each language
    // looks like, held whole so that a different way of loading a definition
    // cannot move a single colour without saying so here.
    use std::fmt::Write as _;

    let unsampled: Vec<&str> = CALLED
        .iter()
        .map(|(spelled, _)| *spelled)
        .filter(|spelled| !SAMPLES.iter().any(|(sampled, _)| sampled == spelled))
        .collect();
    assert!(unsampled.is_empty(), "no sample for: {unsampled:?}");

    let mut said = String::new();
    for (spelled, sample) in SAMPLES {
        let definition = Syntax::of(spelled).map(|syntax| syntax.definition);
        let _ = writeln!(said, "== {spelled} ({definition:?})");
        for runs in reading_of(spelled, sample) {
            let runs: Vec<String> = runs
                .iter()
                .map(|(slot, text)| format!("{slot:?} {text:?}"))
                .collect();
            let _ = writeln!(said, "{}", runs.join(" | "));
        }
    }

    insta::assert_snapshot!(said);
}

#[test]
fn every_name_and_extension_a_definition_carries_resolves_as_it_did() {
    // A change is drawn in the language its file's extension names, or its
    // whole name where it has none, so any of these can arrive. Each is held
    // to the definition it reaches today, or to reaching none.
    use std::fmt::Write as _;

    let shipped = syntect::parsing::SyntaxSet::load_defaults_newlines();
    let resolved = |spelled: &str| Syntax::of(spelled).map(|syntax| syntax.definition);

    let mut said = String::new();
    for definition in shipped.syntaxes() {
        let _ = writeln!(said, "{}", definition.name);
        let _ = writeln!(
            said,
            "  name {:?} -> {:?}",
            definition.name,
            resolved(&definition.name)
        );
        for extension in &definition.file_extensions {
            let _ = writeln!(
                said,
                "  extension {extension:?} -> {:?}",
                resolved(extension)
            );
        }
    }

    insta::assert_snapshot!(said);
}
