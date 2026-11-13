//! What a diagram draws as, read back from the rows: the frame each kind is
//! drawn in, the refusals and what they name, the caps, what is taken out of
//! a label, the ASCII pass, and the pictures checked in beside the drawer.

use crucible_tui::chrome::Undrawn;
use crucible_tui::drawing::{Draws, Room};
use crucible_tui::testing::dump;
use crucible_tui::{Glyphs, Row};

use crate::{Caps, Diagrams, Kind, MOST_BYTES};

const UNICODE: Glyphs = Glyphs::Unicode;
const ASCII: Glyphs = Glyphs::Ascii;

fn block(source: &str, columns: usize, glyphs: Glyphs) -> Result<Vec<Row>, Undrawn> {
    Diagrams
        .block(source, &Room { columns, glyphs })
        .map(|laid| laid.rows)
}

fn texts(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|row| row.text().trim_end().to_owned())
        .collect()
}

const FLOWCHART: &str = "flowchart TD
    A[Start] --> B{Is it?}
    B -->|Yes| C[OK]
    B -- No --> D[Retry]
    D -.-> A
";

const STATE: &str = "stateDiagram-v2
    [*] --> Still
    Still --> Moving : push
    Moving --> Still : stop
    Moving --> Crash
    Crash --> [*]
";

const CLASS: &str = "classDiagram
    class Animal {
        +String name
        +eat()
    }
    class Duck
    Animal <|-- Duck : is a
";

const ENTITIES: &str = "erDiagram
    CUSTOMER ||--o{ ORDER : places
    ORDER ||--|{ LINE-ITEM : contains
    CUSTOMER {
        string name
        string email
    }
";

/// One diagram of each kind drawn as text, and the kind it is.
const KINDS: [(&str, &str, Kind); 3] = [
    ("flowchart", FLOWCHART, Kind::Flowchart),
    ("state", STATE, Kind::State),
    ("entities", ENTITIES, Kind::EntityRelationship),
];

#[test]
fn the_drawer_takes_the_mermaid_fence() {
    assert_eq!(Diagrams.names(), &["mermaid"]);
}

#[test]
fn the_status_row_is_told_the_kind_its_first_line_opens() {
    for (first, told) in [
        ("graph TD", "a mermaid flowchart"),
        ("flowchart LR", "a mermaid flowchart"),
        ("stateDiagram", "a mermaid state diagram"),
        ("stateDiagram-v2", "a mermaid state diagram"),
        ("classDiagram", "a mermaid class diagram"),
        ("erDiagram", "a mermaid er diagram"),
    ] {
        assert_eq!(Diagrams.label("mermaid", first), Some(told), "{first}");
    }
    for first in ["pie", "sequenceDiagram", "nonsense", "", "%%{init: {}}%%"] {
        assert_eq!(Diagrams.label("mermaid", first), None, "{first}");
    }
}

#[test]
fn each_kind_is_drawn_in_a_frame_that_names_it() {
    for (_, source, kind) in KINDS {
        for (glyphs, top) in [
            (UNICODE, format!("┌─ mermaid · {}", kind.name())),
            (ASCII, format!("+- mermaid - {}", kind.name())),
        ] {
            let rows = texts(&block(source, 80, glyphs).unwrap_or_default());
            assert!(
                rows.first().is_some_and(|row| row.starts_with(&top)),
                "{kind:?} {glyphs:?}: {rows:#?}"
            );
            assert!(rows.len() > 3, "{kind:?} {glyphs:?}: {rows:#?}");
        }
    }
}

#[test]
fn a_flowchart_draws_its_labels_and_none_of_its_syntax() {
    let rows = texts(&block(FLOWCHART, 80, UNICODE).unwrap_or_default()).join("\n");
    for label in ["Start", "Is it?", "OK", "Retry", "Yes", "No"] {
        assert!(rows.contains(label), "{label} in\n{rows}");
    }
    for syntax in ["-->", "|Yes|", "[Start]", "{Is"] {
        assert!(!rows.contains(syntax), "{syntax} in\n{rows}");
    }
}

#[test]
fn a_kind_not_drawn_as_text_is_refused_naming_it() {
    for (source, named) in [
        ("pie\n  \"a\" : 1\n", "pie"),
        ("sequenceDiagram\n  A->>B: hi\n", "sequence diagram"),
        ("gantt\n  title x\n", "gantt"),
        ("gitGraph\n  commit\n", "git graph"),
        ("C4Context\n  title x\n", "c4 diagram"),
        ("xychart-beta\n  title x\n", "xy chart"),
        ("%% a comment first\nmindmap\n  root\n", "mindmap"),
    ] {
        assert_eq!(
            block(source, 80, UNICODE),
            Err(Undrawn::Unsupported(named)),
            "{source}"
        );
    }
}

#[test]
fn a_class_diagram_is_named_and_shown_as_written() {
    assert_eq!(
        Diagrams.label("mermaid", "classDiagram"),
        Some("a mermaid class diagram")
    );
    for glyphs in [UNICODE, ASCII] {
        assert_eq!(
            block(CLASS, 160, glyphs),
            Err(Undrawn::Unsupported("class diagram")),
            "{glyphs:?}"
        );
    }
    assert_eq!(
        block("classDiagram\nclass List~T~\n", 160, UNICODE),
        Err(Undrawn::Unsupported("class diagram"))
    );
}

#[test]
fn a_source_with_no_diagram_in_it_is_not_one() {
    for source in ["", "\n  \n", "%% only a comment\n"] {
        assert_eq!(
            block(source, 80, UNICODE),
            Err(Undrawn::NotHere),
            "{source:?}"
        );
    }
    for source in [
        "nonsense\n",
        "---\ntitle: front matter\n---\ngraph TD\nA-->B\n",
        "graph sideways\nA-->B\n",
    ] {
        assert_eq!(
            block(source, 80, UNICODE),
            Err(Undrawn::NotText),
            "{source:?}"
        );
    }
}

#[test]
fn a_flowchart_with_a_group_is_shown_as_written_saying_groups_are_not_drawn() {
    for source in [
        "graph TD\nsubgraph one [a group]\nA-->B\nend\n",
        "flowchart LR\nA-->B\nsubgraph two\ndirection TB\nC-->D\nend\n",
        "graph TD\nA-->B\nsubgraph one\nC\n",
    ] {
        assert_eq!(
            block(source, 160, UNICODE),
            Err(Undrawn::Unsupported("groups")),
            "{source}"
        );
    }
    assert!(block("graph TD\nA[subgraph]-->B\n", 160, UNICODE).is_ok());
}

#[test]
fn a_statement_this_does_not_read_refuses_the_whole_diagram() {
    for source in [
        "graph TD\nA-->B\nthis is prose, not a statement\n",
        "graph TD\nA-->B\nend\n",
        "graph TD\nA-->B\ndirection LR\n",
        "graph TD\nA@{ shape: rect }\n",
        "stateDiagram-v2\n[*] --> A\nA : a description in place of its name\n",
        "stateDiagram-v2\n[*] --> A\nnote \"floating\" as N\n",
        "stateDiagram-v2\nstate A {\n[*] --> B\n",
        "erDiagram\nA ||--o{ B : has\n}\n",
        "erDiagram\nA ||--o{ B : \"cut short\n",
        "erDiagram\nA ||--o{ B : cut short\"\n",
        "erDiagram\nA ||--o{ B : \"one\" \"two\"\n",
    ] {
        assert_eq!(
            block(source, 80, UNICODE),
            Err(Undrawn::NotText),
            "{source}"
        );
    }
}

#[test]
fn a_line_the_engine_reads_otherwise_than_mermaid_is_refused() {
    for line in ["A ---> B", "A o--o B", "A x--x B", "A ~~~ B", "A--text-->B"] {
        assert_eq!(
            block(&format!("graph LR\n{line}\n"), 80, UNICODE),
            Err(Undrawn::NotText),
            "{line}"
        );
    }
}

#[test]
fn every_line_form_the_engine_reads_as_mermaid_is_drawn() {
    for line in [
        "A --> B",
        "A --- B",
        "A -.-> B",
        "A -.- B",
        "A ==> B",
        "A === B",
        "A --o B",
        "A --x B",
        "A <--> B",
        "A -->|label| B",
        "A ---|label| B",
        "A -- label --> B",
        "A -. label .-> B",
        "A == label ==> B",
        "A & C --> B",
        "A((round)) --> B([stadium]) --> C[[sub]] --> D[(db)]",
        "A{{hex}} --> B[/slant/] --> C[\\back\\] --> D>flag]",
        "A[\"quoted (label)\"] --> B:::styled",
    ] {
        let drawn = block(&format!("graph LR\n{line}\n"), 160, UNICODE);
        assert!(drawn.is_ok(), "{line}: {drawn:?}");
    }
}

/// The caps of a kind that is drawn.
fn caps_of(kind: Kind) -> Caps {
    kind.caps().expect("a kind that is drawn has caps")
}

/// For each kind drawn: its opening, and a statement of it that names a box
/// `{}` and a line, written `n` times with `{}` as 0 to `n - 1`.
const SHAPES: [(Kind, &str, &str, &str); 3] = [
    (Kind::Flowchart, "graph TD\n", "N{}\n", "A --> B\n"),
    (
        Kind::State,
        "stateDiagram-v2\n",
        "state S{}\n",
        "S0 --> S1\n",
    ),
    (
        Kind::EntityRelationship,
        "erDiagram\n",
        "E{}\n",
        "E0 ||--o{ E1 : has\n",
    ),
];

#[test]
fn a_source_past_its_kinds_byte_cap_is_refused() {
    for (kind, head, _, line) in SHAPES {
        let head = format!("{head}{line}");
        let caps = caps_of(kind);
        let at_cap = format!("{head}%%{}", "x".repeat(caps.bytes - head.len() - 2));
        assert!(block(&at_cap, 160, UNICODE).is_ok(), "{kind:?}");
        assert_eq!(
            block(&format!("{at_cap}x"), 160, UNICODE),
            Err(Undrawn::PastCap),
            "{kind:?}"
        );
    }
    let comment = format!("%%{}", "x".repeat(MOST_BYTES));
    assert_eq!(block(&comment, 160, UNICODE), Err(Undrawn::PastCap));
}

#[test]
fn boxes_past_each_kinds_cap_are_refused() {
    for (kind, head, named, _) in SHAPES {
        let caps = caps_of(kind);
        let boxes = |count: usize| -> String {
            let named: String = (0..count)
                .map(|at| named.replace("{}", &at.to_string()))
                .collect();
            format!("{head}{named}")
        };
        assert_ne!(
            block(&boxes(caps.nodes), 400, UNICODE),
            Err(Undrawn::PastCap),
            "{kind:?}"
        );
        assert_eq!(
            block(&boxes(caps.nodes + 1), 400, UNICODE),
            Err(Undrawn::PastCap),
            "{kind:?}"
        );
    }
    let mut states = String::from("stateDiagram-v2\n");
    for at in 0..Caps::STATE.nodes {
        states.push_str("state S");
        states.push_str(&at.to_string());
        states.push('\n');
    }
    states.push_str("note right of S0 : a note\n");
    assert_eq!(block(&states, 400, UNICODE), Err(Undrawn::PastCap));
}

#[test]
fn lines_past_each_kinds_cap_are_refused() {
    for (kind, head, _, line) in SHAPES {
        let caps = caps_of(kind);
        let lines = |count: usize| format!("{head}{}", line.repeat(count));
        assert_ne!(
            block(&lines(caps.edges), 400, UNICODE),
            Err(Undrawn::PastCap),
            "{kind:?}"
        );
        assert_eq!(
            block(&lines(caps.edges + 1), 400, UNICODE),
            Err(Undrawn::PastCap),
            "{kind:?}"
        );
    }
    // Every box on the left joined to every box on the right is a line each.
    let side = (1..=Caps::FLOWCHART.edges + 1)
        .find(|side| side * side > Caps::FLOWCHART.edges)
        .unwrap_or(1);
    let names = |at: &str| {
        (0..side)
            .map(|n| format!("{at}{n}"))
            .collect::<Vec<_>>()
            .join(" & ")
    };
    let crossed = format!("graph TD\n{} --> {}\n", names("A"), names("B"));
    assert_eq!(block(&crossed, 400, UNICODE), Err(Undrawn::PastCap));
}

/// Each place a label stands, by kind, with `{}` where it goes.
const LABELLED: [(Kind, &str); 9] = [
    (Kind::Flowchart, "graph TD\nA[\"{}\"] --> B\n"),
    (Kind::Flowchart, "graph TD\n{} --> B\n"),
    (Kind::Flowchart, "graph TD\nA -->|{}| B\n"),
    (Kind::Flowchart, "graph TD\nA -- {} --> B\n"),
    (
        Kind::State,
        "stateDiagram-v2\nstate \"{}\" as A\n[*] --> A\n",
    ),
    (Kind::State, "stateDiagram-v2\n[*] --> {}\n"),
    (Kind::State, "stateDiagram-v2\n[*] --> A : {}\n"),
    (
        Kind::State,
        "stateDiagram-v2\n[*] --> A\nnote right of A : {}\n",
    ),
    (
        Kind::State,
        "stateDiagram-v2\n[*] --> A\nnote right of A\nshort\n{}\nend note\n",
    ),
];

#[test]
fn a_label_past_its_kinds_cap_is_refused_counting_columns() {
    for (kind, template) in LABELLED {
        let most = caps_of(kind).label.expect("a capped label");
        for (at_cap, past) in [
            ("x".repeat(most), "x".repeat(most + 1)),
            ("漢".repeat(most / 2), "漢".repeat(most / 2 + 1)),
        ] {
            let drawn = block(&template.replace("{}", &at_cap), 400, UNICODE);
            assert!(drawn.is_ok(), "{template} {at_cap}: {drawn:?}");
            assert_eq!(
                block(&template.replace("{}", &past), 400, UNICODE),
                Err(Undrawn::PastCap),
                "{template} {past}"
            );
        }
    }
}

#[test]
fn an_entity_relationship_label_is_held_by_the_byte_cap_alone() {
    assert_eq!(Caps::ENTITIES.label, None);
    let source = format!("erDiagram\nA ||--o{{ B : \"{}\"\n", "x".repeat(64));
    let drawn = block(&source, 400, UNICODE);
    assert!(drawn.is_ok(), "{drawn:?}");
}

#[test]
fn a_click_draws_no_link_and_acts_on_nothing() {
    let rows = block(
        "graph TD\nA-->B\nclick A \"https://example.com/x\" \"a tip\"\nclick B call run()\n",
        80,
        UNICODE,
    )
    .map(|rows| texts(&rows).join("\n"));
    let rows = rows.unwrap_or_default();
    assert!(rows.contains('A') && rows.contains('B'), "{rows}");
    assert!(
        !rows.contains("example.com") && !rows.contains('\u{1b}'),
        "{rows}"
    );
}

#[test]
fn an_init_directive_and_comments_are_left_out() {
    let rows = block(
        "%%{init: {\"theme\": \"dark\", \"themeVariables\": {\"x\": \"https://example.com\"}}}%%\ngraph TD\n%% a comment\nA-->B\n",
        80,
        UNICODE,
    )
    .map(|rows| texts(&rows).join("\n"))
    .unwrap_or_default();
    assert!(rows.contains('A') && rows.contains('B'), "{rows}");
    assert!(
        !rows.contains("example") && !rows.contains("comment"),
        "{rows}"
    );
}

#[test]
fn what_a_terminal_would_act_on_is_taken_out_of_a_label() {
    let rows = block(
        "graph LR\nA[\"b\u{1b}[31mf\u{202e}a\u{200b}│e\"] --> B\n",
        80,
        UNICODE,
    )
    .map(|rows| texts(&rows).join("\n"))
    .unwrap_or_default();
    assert!(rows.contains("b[31mfae"), "{rows}");
    assert!(
        !rows
            .chars()
            .any(|character| character.is_control() && character != '\n'
                || matches!(character, '\u{202e}' | '\u{200b}')),
        "{rows:?}"
    );
}

#[test]
fn a_diagram_wider_than_its_room_is_refused_and_not_laid_out_again() {
    let wide =
        "graph LR\nA[first label] --> B[second label] --> C[third label] --> D[last label]\n";
    let natural = block(wide, 200, UNICODE).unwrap_or_default();
    let widest = natural.iter().map(Row::columns).max().unwrap_or(0);
    assert!(widest > 60, "{widest}");
    assert_eq!(
        block(wide, widest, UNICODE).map(|rows| rows == natural),
        Ok(true)
    );
    assert_eq!(block(wide, widest - 1, UNICODE), Err(Undrawn::TooWide));
    assert_eq!(block(wide, 60, UNICODE), Err(Undrawn::TooWide));
}

#[test]
fn under_ascii_every_character_is_ascii_or_written_in_a_label() {
    let labelled = "graph LR\nA[Ünïcödé 漢字] -->|→ ×| B((é)) -.-> C{ß}\n";
    for (_, source, kind) in KINDS
        .into_iter()
        .chain([("labelled", labelled, Kind::Flowchart)])
    {
        let rows = texts(&block(source, 80, ASCII).unwrap_or_default()).join("\n");
        assert!(!rows.is_empty(), "{kind:?}");
        let strays: String = rows
            .chars()
            .filter(|character| !character.is_ascii() && !source.contains(*character))
            .collect();
        assert!(strays.is_empty(), "{kind:?}: {strays:?} in\n{rows}");
    }
}

#[test]
fn a_state_diagram_draws_its_composite_states_notes_and_pseudostates() {
    let source = "stateDiagram-v2
    state \"Waiting key\" as Wait
    [*] --> Wait
    Wait --> Busy : key
    state Busy {
        [*] --> Reading
        Reading --> [*]
    }
    Busy --> [*]
    note right of Wait : idle
";
    let drawn = block(source, 120, UNICODE);
    assert!(drawn.is_ok(), "{drawn:?}");
    let rows = texts(&drawn.unwrap_or_default()).join("\n");
    for label in ["Waiting key", "Reading", "key", "idle"] {
        assert!(rows.contains(label), "{label} in\n{rows}");
    }
}

#[test]
fn pictures_of_each_kind() {
    for (name, source, _) in KINDS {
        for (set, glyphs) in [("unicode", UNICODE), ("ascii", ASCII)] {
            let rows = block(source, 80, glyphs).unwrap_or_default();
            insta::assert_snapshot!(format!("{name}_80_{set}"), dump(&rows, 80));
        }
    }
}
