use std::fs;
use std::sync::mpsc;

use crucible_core::{
    Ask, Cancel, Delta, DeltaStream, Event, Message, Modalities, Modality, Provider, ProviderError,
    Remember, Request, Sensitivity, Steer, StopReason, ToolCall, Verdict, Workspace,
};
use crucible_runner::{Model, Runner, Session, Tools};

use crate::cli::fake::Script;
use crate::cli::sample::Sample;

use super::{Attaching, attaching};

/// The eight bytes every PNG starts with.
const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// The five a PDF starts with.
const PDF: &[u8] = b"%PDF-";

/// A provider that spells what it is told to and never sends anything.
///
/// It answers to a real provider's name because that name is what finds the
/// row in the generated table — the half of the intersection this stands in
/// for is the *other* one.
struct Spelling {
    named: &'static str,
    spells: Modalities,
}

impl Provider for Spelling {
    fn name(&self) -> &'static str {
        self.named
    }

    fn spells(&self) -> Modalities {
        self.spells
    }

    fn stream(
        &self,
        _request: Request<'_>,
        _cancel: &Cancel,
    ) -> Result<Box<dyn DeltaStream>, ProviderError> {
        panic!("nothing here sends a request")
    }
}

/// Text and pictures, which is what all three protocols spell today.
fn spelling(named: &'static str) -> Spelling {
    Spelling {
        named,
        spells: Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image),
    }
}

/// A workspace holding one file, and the prompt that names it.
fn holding(sample: &Sample, name: &str, bytes: &[u8]) -> Workspace {
    fs::write(sample.root().join(name), bytes).expect("a file in the workspace");
    sample.workspace()
}

#[test]
fn a_picture_named_at_the_prompt_is_attached() {
    let sample = Sample::new("attaching-a-picture");
    let workspace = holding(&sample, "holiday.png", PNG);

    let Attaching {
        attachments,
        refusals,
    } = attaching(
        &workspace,
        &spelling("anthropic"),
        "claude-opus-5",
        "what is in holiday.png",
    );

    assert!(refusals.is_empty(), "nothing was refused: {refusals:?}");
    assert_eq!(
        attachments.len(),
        1,
        "the one file named is the one attached"
    );

    let one = attachments.first().expect("the attachment just counted");
    assert_eq!(one.modality, Modality::Image);
    assert_eq!(one.media_type.as_ref(), "image/png");
    assert!(
        one.path.ends_with("holiday.png"),
        "the path is resolved against the workspace, not left as typed: {}",
        one.path,
    );
}

#[test]
fn a_prompt_naming_no_file_attaches_nothing_and_says_nothing() {
    let sample = Sample::new("attaching-nothing");
    let workspace = holding(&sample, "holiday.png", PNG);

    let Attaching {
        attachments,
        refusals,
    } = attaching(
        &workspace,
        &spelling("anthropic"),
        "claude-opus-5",
        "rename the field and run the tests",
    );

    assert!(attachments.is_empty(), "no file was named");
    assert!(refusals.is_empty(), "and so there was nothing to refuse");
}

#[test]
fn a_source_file_named_at_the_prompt_is_still_only_text() {
    let sample = Sample::new("attaching-source");
    let workspace = holding(&sample, "main.rs", b"fn main() {}");

    let Attaching {
        attachments,
        refusals,
    } = attaching(
        &workspace,
        &spelling("anthropic"),
        "claude-opus-5",
        "have a look at main.rs",
    );

    assert!(
        attachments.is_empty(),
        "a file the read tool can open is not an attachment",
    );
    assert!(refusals.is_empty(), "and refusing it would be noise");
}

#[test]
fn the_provider_half_of_the_intersection_names_the_protocol() {
    let sample = Sample::new("attaching-provider-half");
    let workspace = holding(&sample, "invoice.pdf", PDF);

    // The model reads a PDF; this protocol has no shape for one yet.
    let Attaching {
        attachments,
        refusals,
    } = attaching(
        &workspace,
        &spelling("anthropic"),
        "claude-opus-5",
        "read invoice.pdf",
    );

    assert!(attachments.is_empty());
    assert_eq!(
        refusals,
        vec![
            "invoice.pdf is not attached: crucible's anthropic requests have no shape for a pdf. \
             Nothing you type changes that — a later release adds the shape."
                .to_owned()
        ],
    );
}

#[test]
fn the_model_half_of_the_intersection_names_the_model() {
    let sample = Sample::new("attaching-model-half");
    let workspace = holding(&sample, "invoice.pdf", PDF);

    // This protocol has the shape; the model does not read one.
    let spelling = Spelling {
        named: "moonshot",
        spells: Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Pdf),
    };

    let Attaching {
        attachments,
        refusals,
    } = attaching(&workspace, &spelling, "k3", "read invoice.pdf");

    assert!(attachments.is_empty());
    assert_eq!(
        refusals,
        vec![
            "invoice.pdf is not attached: k3 does not read a pdf. /model picks one that does."
                .to_owned()
        ],
    );
}

#[test]
fn a_model_outside_the_table_neither_offers_the_file_nor_refuses_it() {
    let sample = Sample::new("attaching-unknown-model");
    let workspace = holding(&sample, "holiday.png", PNG);

    let Attaching {
        attachments,
        refusals,
    } = attaching(
        &workspace,
        &spelling("anthropic"),
        "claude-opus-9",
        "what is in holiday.png",
    );

    assert!(attachments.is_empty());
    assert_eq!(
        refusals,
        vec![
            "holiday.png is not attached: this build has no entry for claude-opus-9, so it does \
             not know what that model reads. That is not a refusal. /model names one it knows."
                .to_owned()
        ],
    );
}

#[test]
fn a_file_over_the_ceiling_is_refused_where_the_user_can_still_hear_it() {
    let sample = Sample::new("attaching-over-the-ceiling");
    let mut bytes = PNG.to_vec();
    bytes.resize(crucible_runner::attachments::CEILING + 1, 0);
    let workspace = holding(&sample, "huge.png", &bytes);

    let Attaching {
        attachments,
        refusals,
    } = attaching(
        &workspace,
        &spelling("anthropic"),
        "claude-opus-5",
        "what is in huge.png",
    );

    assert!(attachments.is_empty());
    assert_eq!(
        refusals,
        vec![
            "huge.png is larger than the 6 MB one attachment may be, so it is not attached. A \
             smaller copy of it would be."
                .to_owned()
        ],
    );
}

#[test]
fn a_png_that_is_not_a_png_is_refused_before_any_request() {
    let sample = Sample::new("attaching-a-liar");
    let workspace = holding(&sample, "holiday.png", b"GIF89a and not a png at all");

    let Attaching {
        attachments,
        refusals,
    } = attaching(
        &workspace,
        &spelling("anthropic"),
        "claude-opus-5",
        "what is in holiday.png",
    );

    assert!(attachments.is_empty());
    assert_eq!(
        refusals,
        vec![
            "holiday.png is not attached: it is named .png and its bytes are not a png. Rename it \
             to what it is."
                .to_owned()
        ],
    );
}

/// A runner over a script that answers once and yields, and the channel its
/// events go down.
///
/// The script is never asked what it was sent: what a request carries is the
/// runner's to decide and is proved where the ageing rule is. What is proved
/// here is the step before it — that what the prompt attached is what the
/// transcript ends up holding, for this turn and every one after it.
fn answering() -> (Runner, mpsc::Sender<Event>) {
    let (events, _seen) = mpsc::channel();

    (
        Runner::new(
            Box::new(Script::new(vec![vec![
                Delta::Text("looked".into()),
                Delta::Stopped(StopReason::Yielded),
            ]])),
            Tools::new(),
            Model {
                name: "script".into(),
                max_tokens: 64,
                window: None,
                system: None,
                effort: None,
            },
            Session::nowhere(),
        ),
        events,
    )
}

/// Nobody to ask, so nothing here can reach a question.
struct Nobody;

impl Ask for Nobody {
    fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
        (Verdict::Deny, Remember::Never)
    }
}

#[test]
fn what_the_prompt_attached_reaches_the_transcript() {
    let sample = Sample::new("attaching-into-a-turn");
    let workspace = holding(&sample, "holiday.png", PNG);
    let prompt = "what is in holiday.png";

    let Attaching { attachments, .. } =
        attaching(&workspace, &spelling("anthropic"), "claude-opus-5", prompt);

    let (mut runner, events) = answering();
    runner
        .turn(
            prompt,
            attachments,
            &mut Nobody,
            &events,
            &Cancel::new(),
            &Steer::new(),
        )
        .expect("the turn to finish");

    let Some(Message::User { text, attachments }) = runner.transcript().messages().first() else {
        panic!("the turn opens with what the user said");
    };

    assert_eq!(text.as_ref(), prompt);
    assert_eq!(
        attachments.len(),
        1,
        "the picture the prompt named is on the message it named it in",
    );
}

#[test]
fn a_prompt_naming_no_file_records_the_message_it_always_did() {
    let sample = Sample::new("attaching-into-a-plain-turn");
    let workspace = holding(&sample, "holiday.png", PNG);
    let prompt = "rename the field and run the tests";

    let Attaching { attachments, .. } =
        attaching(&workspace, &spelling("anthropic"), "claude-opus-5", prompt);

    let (mut runner, events) = answering();
    runner
        .turn(
            prompt,
            attachments,
            &mut Nobody,
            &events,
            &Cancel::new(),
            &Steer::new(),
        )
        .expect("the turn to finish");

    assert_eq!(
        runner.transcript().messages().first(),
        Some(&Message::said(prompt)),
        "a prompt that named no file is the message it was before one could",
    );
}
