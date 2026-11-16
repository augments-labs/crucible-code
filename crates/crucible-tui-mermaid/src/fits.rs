//! The width rule held over every drawer in this crate, at every width and
//! room the components of `crucible-tui` are held to.
//!
//! A drawer named here is swept; one that is not, the repository gate finds:
//! every `impl Draws for` in this crate's sources must name its type in this
//! file. The fixtures are hostile on purpose: diagrams at their kind's line
//! cap, with labels at the label cap, wide characters and every shape, so
//! most rooms refuse them and the widest draw them.

use crucible_tui::testing::fits;

use crate::Diagrams;

/// A flowchart as hostile as its kind gets inside the caps: six lines, one
/// of them back to the first box, boxes of every shape, wide characters and
/// labels at the label cap.
const HOSTILE: &str = "flowchart TD
    A[\"字 is at the cap\"] --> B((漢字漢字漢字))
    B -.->|点線| C{選ぶ?}
    C -->|yes| D[[subroutine]]
    C -- no --> E[(store)]
    D ==> F>flag]
    F --x A
    G{{hexagon}}
    H[/slanted/]
    I([stadium])
    J(((double)))
";

/// The other kinds, each as hostile as it gets in a few lines.
const KINDS: [&str; 5] = [
    "stateDiagram-v2
    state \"很长的状态名\" as Long
    [*] --> Long
    Long --> Busy : 一个转换
    state Busy {
        [*] --> Reading
        Reading --> [*]
    }
    Busy --> [*]
    note right of Long : 注意
",
    "erDiagram
    顧客 ||--o{ ORDER : places a long label
    ORDER ||--|{ LINE-ITEM : contains
",
    "sequenceDiagram
    participant 客 as 很长的参与者名字
    participant B as A participant at the cap
    客->>B: 一个很长的消息标签
    B-->>客: an answer back
    B->>B: to itself
",
    "gantt
    title 很长的标题 of a gantt
    dateFormat YYYY-MM-DD
    section 第一
    漢字の仕事 :a1, 2026-01-01, 12d
    At the cap name :after a1, 3w
",
    "gitGraph
    commit id: \"漢字のコミット\"
    branch 枝
    commit tag: \"a long tag on a branch\"
    checkout main
    commit
    merge 枝
",
];

#[test]
fn a_flowchart_fits_every_room() {
    fits("a flowchart", &Diagrams, HOSTILE);
}

#[test]
fn every_other_kind_fits_every_room() {
    for source in KINDS {
        fits("a diagram", &Diagrams, source);
    }
}
