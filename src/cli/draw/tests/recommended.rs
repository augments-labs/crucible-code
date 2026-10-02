//! The label a recommended answer is drawn with in the line form.

use super::*;

/// The rows of the question, as the window `columns` wide shows them.
fn put_rows(question: &Question, columns: usize) -> Vec<String> {
    let mut renderer = Renderer::new(Recording::new(columns, 24));

    asking(&mut renderer, question, 0, 1, Style::plain()).expect("the question to commit");

    renderer.terminal().picture().said()
}

/// A question whose first answer is recommended, or not.
fn recommending(recommended: bool) -> Question {
    let first = crucible_types::Answer::new("Typed flag").saying("one field");
    let first = if recommended {
        first.recommending()
    } else {
        first
    };
    Question::new(
        "Design",
        "Which design?",
        [first, crucible_types::Answer::new("Enum value")],
    )
}

#[test]
fn the_line_form_draws_the_label_after_the_recommended_answers_name_only() {
    for columns in [WIDE, 80, 40] {
        let marked = put_rows(&recommending(true), columns);
        let joined = marked
            .iter()
            .map(|row| row.trim())
            .collect::<Vec<_>>()
            .join(" ");

        assert!(
            joined.contains("1. Typed flag (Recommended)"),
            "at {columns}: {marked:#?}"
        );
        assert_eq!(joined.matches("(Recommended)").count(), 1, "{marked:#?}");
        assert!(joined.contains("2. Enum value"), "{marked:#?}");

        // Nothing else moved: with the label taken out it is the plain form.
        let plain = put_rows(&recommending(false), columns);
        let flat = |rows: &[String]| -> Vec<String> {
            rows.join(" ")
                .replace("(Recommended)", "")
                .split_whitespace()
                .map(str::to_owned)
                .collect()
        };
        assert!(!plain.join(" ").contains("Recommended"), "{plain:#?}");
        assert_eq!(flat(&marked), flat(&plain), "at {columns}");
    }
}
