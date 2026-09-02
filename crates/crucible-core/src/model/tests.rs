//! What a model record refuses to be built from.

use super::{MODEL_NAME_BYTES, ModelCapabilities, ModelError, ModelLimits};
use crate::{Effort, Modalities, Modality};

/// The modalities every model these tests describe reads.
fn reads() -> Modalities {
    Modalities::empty()
        .insert(Modality::Text)
        .insert(Modality::Image)
}

/// The figures every model these tests describe is published with.
fn limits() -> ModelLimits {
    ModelLimits {
        window: 262_144,
        output: 32_768,
        accepts: reads(),
    }
}

/// One record, built the way a catalogue row builds it.
fn described(rungs: &[Effort]) -> Result<ModelCapabilities, ModelError> {
    ModelCapabilities::new("kimi-k3", "Kimi K3", limits(), rungs)
}

#[test]
fn a_record_answers_with_the_limits_it_was_described_with() {
    let one = described(&[Effort::Low, Effort::High]).expect("a described model");

    assert_eq!(one.name(), "kimi-k3");
    assert_eq!(one.shown(), "Kimi K3");
    assert_eq!(one.window(), 262_144);
    assert_eq!(one.output(), 32_768);
    assert!(one.accepts().contains(Modality::Image));
    assert_eq!(one.rungs(), [Effort::Low, Effort::High]);
}

#[test]
fn rungs_written_down_out_of_the_ladders_order_are_refused() {
    // The panel draws faster on the left and smarter on the right, and those
    // ends are a claim about the order of what is between them. A set written
    // down backwards draws a track whose ends are wrong, and nothing on screen
    // says so — so it is refused where it is described rather than where it is
    // walked.
    let refused = described(&[Effort::Max, Effort::Low]).expect_err("a backwards ladder");

    assert!(
        matches!(refused, ModelError::Rungs { .. }),
        "{refused:?} is not the ladder's order being refused"
    );
    assert!(refused.to_string().contains("max, low"), "{refused}");
}

#[test]
fn a_rung_written_down_twice_is_refused() {
    // A repeat is a rung that stands still under an arrow key: the panel moves
    // its cursor and the model beside it does not change.
    let refused = described(&[Effort::High, Effort::High]).expect_err("a repeated rung");

    assert!(
        matches!(refused, ModelError::Rungs { .. }),
        "{refused:?} is not a repeat being refused"
    );
}

#[test]
fn a_model_that_serves_no_rung_at_all_is_described_rather_than_refused() {
    // Several models serve none, and two vendors refuse the request outright
    // rather than ignoring the field. Empty is that fact; it is not the same as
    // a model nothing is known about, which has no record here at all.
    let none = described(&[]).expect("a model that serves no rung");

    assert!(none.rungs().is_empty());
}

#[test]
fn a_limit_stated_as_zero_is_refused_and_names_which_one() {
    // Zero is not a small window; it is a session that throws itself away on
    // the first turn. Nothing known is the honest answer, and that is said by
    // having no record rather than by a record full of zeroes.
    let window = ModelCapabilities::new(
        "kimi-k3",
        "Kimi K3",
        ModelLimits {
            window: 0,
            ..limits()
        },
        [],
    )
    .expect_err("a window of zero");
    assert_eq!(
        window.to_string(),
        "kimi-k3 states a context window of zero"
    );

    let output = ModelCapabilities::new(
        "kimi-k3",
        "Kimi K3",
        ModelLimits {
            output: 0,
            ..limits()
        },
        [],
    )
    .expect_err("an output ceiling of zero");
    assert_eq!(
        output.to_string(),
        "kimi-k3 states an output ceiling of zero"
    );
}

#[test]
fn a_name_that_is_empty_or_over_its_boundary_is_refused() {
    // A registered provider names its own models, so these spellings arrive
    // from outside this build. Both ways of being wrong are retained for the
    // life of the process if they are not refused here.
    let empty = ModelCapabilities::new("", "Kimi K3", limits(), []).expect_err("an empty name");
    assert_eq!(
        empty,
        ModelError::Empty {
            field: "model name"
        }
    );

    let long = "k".repeat(MODEL_NAME_BYTES + 1);
    let over = ModelCapabilities::new(long.clone(), "Kimi K3", limits(), [])
        .expect_err("a name over its boundary");
    assert_eq!(
        over,
        ModelError::TooLong {
            field: "model name",
            maximum: MODEL_NAME_BYTES,
            actual: long.len(),
        }
    );

    let shown =
        ModelCapabilities::new("kimi-k3", "", limits(), []).expect_err("an empty shown name");
    assert_eq!(
        shown,
        ModelError::Empty {
            field: "shown name"
        }
    );
}

#[test]
fn a_record_retains_its_two_spellings_and_its_rungs() {
    // What a registry adds up to decide whether one more generation fits.
    let one = described(&[Effort::Low, Effort::High]).expect("a described model");

    assert_eq!(
        one.retained_bytes(),
        "kimi-k3".len() + "Kimi K3".len() + 2 * size_of::<Effort>()
    );
}
