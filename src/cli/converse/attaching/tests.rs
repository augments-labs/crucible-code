use std::fs;
use std::sync::mpsc;

use crucible_core::{
    AgentId, Aside, Ask, Cancel, CredentialScopeId, Delta, DeltaStream, EventEnvelope, Message,
    Modalities, Modality, PromptCacheCapabilities, PromptCacheEncoding, PromptCacheRoute, Provider,
    ProviderError, Remember, Request, Sensitivity, Steer, StopReason, ToolCall, Transcript,
    Verdict, Workspace, written,
};
use crucible_runner::{AgentSpec, Model, Pruned, Runner, Session, Tools};

use crucible_tui::{Glyphs, Recording, Renderer};

use crate::cli::draw;
use crate::cli::fake::Script;
use crate::cli::kept::Kept;
use crate::cli::sample::Sample;
use crate::cli::style::Style;

use super::super::replaying::{Replay, replayed};
use super::{Attaching, Named, Sent, attaching, beside, decide, marked, names, pictured};

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
    credential_scope: CredentialScopeId,
}

impl Provider for Spelling {
    fn name(&self) -> &'static str {
        self.named
    }

    fn spells(&self) -> Modalities {
        self.spells
    }

    fn prompt_cache_capabilities(&self, _model: &str) -> PromptCacheCapabilities {
        PromptCacheCapabilities::unknown("spelling-fixture-v1")
    }

    fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
        PromptCacheRoute {
            protocol: self.named,
            endpoint: self.named,
            custom_endpoint: true,
            credential_scope: self.credential_scope,
            account: None,
            project: None,
            request_shape_version: "spelling-fixture-v1",
        }
    }

    fn prompt_cache_encoding(&self, _request: &Request<'_>) -> PromptCacheEncoding {
        PromptCacheEncoding::NoControlIntended
    }

    fn stream(
        &self,
        _request: Request<'_>,
        _cancel: &Cancel,
    ) -> Result<Box<dyn DeltaStream>, ProviderError> {
        panic!("nothing here sends a request")
    }
}

/// Text and pictures, the common baseline used by tests not about video.
fn spelling(named: &'static str) -> Spelling {
    Spelling {
        named,
        credential_scope: CredentialScopeId::new(),
        spells: Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image),
    }
}

/// A Moonshot-shaped provider that can carry named MP4 videos.
fn videos() -> Spelling {
    Spelling {
        named: "moonshot",
        credential_scope: CredentialScopeId::new(),
        spells: Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Video),
    }
}

/// A minimal MP4 `ftyp` box accepted by the core attachment detector.
fn mp4() -> Vec<u8> {
    let mut bytes = Vec::from(&20_u32.to_be_bytes()[..]);
    bytes.extend_from_slice(b"ftypisom");
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    bytes.extend_from_slice(b"mp42");
    bytes
}

/// A workspace holding one file, and the prompt that names it.
fn holding(sample: &Sample, name: &str, bytes: &[u8]) -> Workspace {
    fs::write(sample.root().join(name), bytes).expect("a file in the workspace");
    sample.workspace()
}

#[test]
fn a_quoted_path_with_spaces_is_one_prompt_name() {
    assert_eq!(
        names("don't describe '/home/ada/Pictures/Screen Shot.png'?"),
        [
            "don't",
            "describe",
            "/home/ada/Pictures/Screen Shot.png",
            "?",
        ]
    );
}

#[test]
fn an_external_picture_is_imported_for_the_session() {
    let sample = Sample::new("attaching-an-external-picture");
    let workspace = sample.workspace();
    let outside = sample
        .root()
        .parent()
        .expect("the sample base")
        .join("Screen Shot.png");
    fs::write(&outside, PNG).expect("an external picture");
    let imported = sample.logs().join("attachments/session-one");

    let Named::Attached(one) = decide(
        &workspace,
        &spelling("anthropic"),
        "claude-opus-5",
        outside.to_str().expect("a text path"),
        Some(&imported),
    ) else {
        panic!("the user-selected external picture is attached")
    };

    assert!(one.path.starts_with(&written(&imported)));
    assert_eq!(
        fs::read(one.path.as_ref()).expect("the imported bytes"),
        PNG
    );
    fs::remove_file(&outside).expect("the source goes away");
    assert_eq!(fs::read(one.path.as_ref()).expect("the durable copy"), PNG);

    let copied = super::import(&imported, "png", one.hash, PNG).expect("the same import");
    assert_eq!(
        Some(written(&copied).as_str()),
        Some(one.path.as_ref()),
        "the bytes deduplicate"
    );
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
        Sent {
            prompt: "what is in holiday.png",
            images: &[],
        },
        None,
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
fn an_mp4_is_attached_only_when_provider_and_model_both_accept_video() {
    let sample = Sample::new("attaching-a-video");
    let workspace = holding(&sample, "demo.MP4", &mp4());

    let Attaching {
        attachments,
        refusals,
    } = attaching(
        &workspace,
        &videos(),
        "k3",
        Sent {
            prompt: "describe demo.MP4",
            images: &[],
        },
        None,
    );

    assert!(refusals.is_empty(), "nothing was refused: {refusals:?}");
    let one = attachments.first().expect("the MP4 is attached");
    assert_eq!(one.modality, Modality::Video);
    assert_eq!(one.media_type.as_ref(), "video/mp4");
}

#[test]
fn an_mp4_is_refused_when_the_provider_has_no_video_shape() {
    let sample = Sample::new("attaching-provider-refuses-video");
    let workspace = holding(&sample, "demo.mp4", &mp4());

    let Attaching {
        attachments,
        refusals,
    } = attaching(
        &workspace,
        &spelling("moonshot"),
        "k3",
        Sent {
            prompt: "describe demo.mp4",
            images: &[],
        },
        None,
    );

    assert!(attachments.is_empty());
    assert_eq!(
        refusals,
        [
            "demo.mp4 is not attached: crucible's moonshot requests have no shape for a video. Nothing you type changes that — a later release adds the shape."
        ]
    );
}

#[test]
fn an_mp4_name_with_non_mp4_bytes_is_refused() {
    let sample = Sample::new("attaching-a-video-liar");
    let workspace = holding(&sample, "demo.mp4", PNG);

    let Attaching {
        attachments,
        refusals,
    } = attaching(
        &workspace,
        &videos(),
        "k3",
        Sent {
            prompt: "describe demo.mp4",
            images: &[],
        },
        None,
    );

    assert!(attachments.is_empty());
    assert_eq!(
        refusals,
        [
            "demo.mp4 is not attached: it is named .mp4 and its bytes are not a mp4. Rename it to what it is."
        ]
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
        Sent {
            prompt: "rename the field and run the tests",
            images: &[],
        },
        None,
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
        Sent {
            prompt: "have a look at main.rs",
            images: &[],
        },
        None,
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

    // The model reads a PDF, and this stands for a protocol with no shape for
    // one -- which is what every one of them was until the release that wrote
    // the block, and what a fourth would be on the day it is added. What is
    // under test is which half of the intersection the sentence names, and
    // only a provider that refuses can name that half.
    let Attaching {
        attachments,
        refusals,
    } = attaching(
        &workspace,
        &spelling("anthropic"),
        "claude-opus-5",
        Sent {
            prompt: "read invoice.pdf",
            images: &[],
        },
        None,
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

/// The one protocol crucible speaks that carries no document, against the
/// models it serves -- none of which reads a PDF either. Both halves refuse,
/// and the provider's sentence is the one that reaches the reader because the
/// provider is asked first: `/model` is no fix here, and a sentence offering
/// it would send somebody around a menu that cannot help them.
#[test]
fn a_pdf_on_moonshot_is_refused_by_the_protocol_and_never_sent() {
    let sample = Sample::new("attaching-moonshot-pdf");
    let workspace = holding(&sample, "invoice.pdf", PDF);

    let Attaching {
        attachments,
        refusals,
    } = attaching(
        &workspace,
        &spelling("moonshot"),
        "k3",
        Sent {
            prompt: "read invoice.pdf",
            images: &[],
        },
        None,
    );

    assert!(attachments.is_empty(), "nothing goes with the prompt");
    assert_eq!(
        refusals,
        vec![
            "invoice.pdf is not attached: crucible's moonshot requests have no shape for a pdf. \
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
        credential_scope: CredentialScopeId::new(),
        spells: Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Pdf),
    };

    let Attaching {
        attachments,
        refusals,
    } = attaching(
        &workspace,
        &spelling,
        "k3",
        Sent {
            prompt: "read invoice.pdf",
            images: &[],
        },
        None,
    );

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
        Sent {
            prompt: "what is in holiday.png",
            images: &[],
        },
        None,
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
    bytes.resize(crucible_core::CEILING + 1, 0);
    let workspace = holding(&sample, "huge.png", &bytes);

    let Attaching {
        attachments,
        refusals,
    } = attaching(
        &workspace,
        &spelling("anthropic"),
        "claude-opus-5",
        Sent {
            prompt: "what is in huge.png",
            images: &[],
        },
        None,
    );

    assert!(attachments.is_empty());
    assert_eq!(
        refusals,
        vec![
            "huge.png is larger than the 4 MB one attachment may be, so it is not attached. A \
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
        Sent {
            prompt: "what is in holiday.png",
            images: &[],
        },
        None,
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
fn answering() -> (Runner, mpsc::Sender<EventEnvelope>) {
    let (events, _seen) = mpsc::channel();

    (
        Runner::new(
            Box::new(Script::new(vec![vec![
                Delta::Text("looked".into()),
                Delta::Stopped(StopReason::Yielded),
            ]])),
            Tools::new(),
            AgentSpec::new(
                AgentId::new("test"),
                Model {
                    name: "script".into(),
                    max_tokens: 64,
                    window: None,
                    accepts: None,
                    effort: None,
                },
            ),
            crucible_runner::ContextInputs::new(std::env::temp_dir()),
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

    let Attaching { attachments, .. } = attaching(
        &workspace,
        &spelling("anthropic"),
        "claude-opus-5",
        Sent {
            prompt,
            images: &[],
        },
        None,
    );

    let (mut runner, events) = answering();
    let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
    let run = runner.starting(&events, &cancel, &steer, &aside);
    runner
        .turn(prompt, attachments, &mut Nobody, &run)
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

    let Attaching { attachments, .. } = attaching(
        &workspace,
        &spelling("anthropic"),
        "claude-opus-5",
        Sent {
            prompt,
            images: &[],
        },
        None,
    );

    let (mut runner, events) = answering();
    let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
    let run = runner.starting(&events, &cancel, &steer, &aside);
    runner
        .turn(prompt, attachments, &mut Nobody, &run)
        .expect("the turn to finish");

    assert_eq!(
        runner.transcript().messages().first(),
        Some(&Message::said(prompt)),
        "a prompt that named no file is the message it was before one could",
    );
}

/// A runner whose provider answers to a real name and can spell a picture, so
/// the intersection lets one through. Nothing here sends a request.
fn sending() -> Runner {
    Runner::new(
        Box::new(spelling("anthropic")),
        Tools::new(),
        AgentSpec::new(
            AgentId::new("test"),
            Model {
                name: "claude-opus-5".into(),
                max_tokens: 64,
                window: None,
                accepts: None,
                effort: None,
            },
        ),
        crucible_runner::ContextInputs::new(std::env::temp_dir()),
        Session::nowhere(),
    )
}

#[test]
fn a_file_sent_with_a_prompt_is_marked_under_it_whichever_way_it_reached_the_screen() {
    let sample = Sample::new("attaching-on-both-paths");
    let workspace = holding(&sample, "holiday.png", PNG);
    let prompt = "what is in holiday.png";
    let style = Style::drawn(Glyphs::Unicode);

    // Live: the line goes down, and what went with it goes under it.
    let runner = sending();
    let mut live = Renderer::new(Recording::new(120, 24));
    live.wears(style.palette());
    draw::queued(&mut live, prompt, style).expect("a recording cannot fail");
    let attachments = beside(
        &mut live,
        &runner,
        &workspace,
        Sent {
            prompt,
            images: &[],
        },
        style,
    )
    .expect("a recording cannot fail");

    // Replayed: the same message, back out of a transcript.
    let mut transcript = Transcript::new();
    transcript.push(Message::User {
        text: prompt.into(),
        attachments,
    });
    let back = sending().resuming(transcript);
    let mut replay = Renderer::new(Recording::new(120, 24));
    replay.wears(style.palette());
    replayed(
        &mut replay,
        &Replay {
            runner: &back,
            pruned: &Pruned::default(),
            style,
        },
        &mut Kept::default(),
    )
    .expect("a recording cannot fail");

    // The row is the marker the prompt above it uses, and only that.
    let named = "[Image #1]";
    let live = live.terminal().written().to_string();
    let replayed = replay.terminal().written().to_string();

    assert!(
        live.contains(named),
        "the file went with the line and the line said so: {live:?}",
    );
    assert!(
        replayed.contains(named),
        "a transcript that remembers says it again on the way back: {replayed:?}",
    );

    // Wide enough here for the whole resolved path to have fitted, so its
    // absence is the row having stopped at the marker rather than the row
    // having been clipped. The prompt above names the file because the person
    // typing it did; the row under it is the marker and the end of the row,
    // which is what the marker is for.
    let root = written(&sample.root());
    let after = format!("{named} ");
    for said in [root.as_str(), after.as_str()] {
        assert!(
            !live.contains(said) && !replayed.contains(said),
            "a row spells out a path the marker stands for: {said:?}",
        );
    }
}

#[test]
fn a_marker_names_the_image_pasted_before_it() {
    let sample = Sample::new("attaching-a-marker");
    let workspace = sample.workspace();
    let outside = sample
        .root()
        .parent()
        .expect("the sample base")
        .join("pasted-marker.png");
    fs::write(&outside, PNG).expect("a pasted picture");
    let imported = sample.logs().join("attachments/session-one");
    let pasted = [written(&outside).into_boxed_str()];

    let Attaching {
        attachments,
        refusals,
    } = attaching(
        &workspace,
        &spelling("anthropic"),
        "claude-opus-5",
        Sent {
            prompt: "what is in [Image #1]",
            images: &pasted,
        },
        Some(&imported),
    );

    assert!(refusals.is_empty(), "nothing was refused: {refusals:?}");
    assert_eq!(attachments.len(), 1, "the marked image is the one attached");
    let one = attachments.first().expect("the attachment just counted");
    assert!(one.path.starts_with(&written(&imported)));
    assert_eq!(
        fs::read(one.path.as_ref()).expect("the imported bytes"),
        PNG
    );
}

#[test]
fn a_marker_with_nothing_pasted_behind_it_is_a_word() {
    let sample = Sample::new("attaching-a-bare-marker");
    let workspace = sample.workspace();

    let Attaching {
        attachments,
        refusals,
    } = attaching(
        &workspace,
        &spelling("anthropic"),
        "claude-opus-5",
        Sent {
            prompt: "the plan in [Image #3] step one",
            images: &[],
        },
        None,
    );

    assert!(attachments.is_empty(), "no paste stands behind the marker");
    assert!(refusals.is_empty(), "and a typed marker is not an error");
}

#[test]
fn a_marker_said_twice_attaches_the_image_once() {
    let sample = Sample::new("attaching-a-marker-twice");
    let workspace = sample.workspace();
    let outside = sample
        .root()
        .parent()
        .expect("the sample base")
        .join("pasted-twice.png");
    fs::write(&outside, PNG).expect("a pasted picture");
    let imported = sample.logs().join("attachments/session-one");
    let pasted = [written(&outside).into_boxed_str()];

    let Attaching {
        attachments,
        refusals,
    } = attaching(
        &workspace,
        &spelling("anthropic"),
        "claude-opus-5",
        Sent {
            prompt: "compare [Image #1] with [Image #1]",
            images: &pasted,
        },
        Some(&imported),
    );

    assert!(refusals.is_empty(), "nothing was refused: {refusals:?}");
    assert_eq!(attachments.len(), 1, "the same bytes go once");
}

#[test]
fn every_marker_in_a_prompt_is_read_in_order() {
    assert_eq!(
        marked("start [Image #1] middle [Image #12] end [Image #2]"),
        [1, 12, 2]
    );
    assert_eq!(
        marked("[Image] [Image #] [Image #x] [Image #1x] [image #2] Image #3]"),
        [] as [usize; 0],
        "a marker is the exact shape or it is words",
    );
}

#[test]
fn a_file_uri_on_the_clipboard_names_the_picture_it_points_at() {
    let sample = Sample::new("attaching-a-file-uri");
    let outside = sample
        .root()
        .parent()
        .expect("the sample base")
        .join("Screen Shot.png");
    fs::write(&outside, PNG).expect("a picture to point at");
    // A file manager spells the path with forward slashes behind an empty
    // authority, which on Windows puts the URI's own slash before the drive
    // letter: `file:///C:/…`.
    let slashed = written(&outside).replace('\\', "/").replace(' ', "%20");
    let uri = format!("file:///{}", slashed.trim_start_matches('/'));

    assert_eq!(pictured(&uri), Some(outside.clone()));
    assert_eq!(
        pictured(&format!("{uri}\n")),
        Some(outside.clone()),
        "a file manager ends the list with a newline",
    );
    assert_eq!(
        pictured(&written(&outside)),
        Some(outside),
        "a bare absolute path is one somebody copied out of a shell",
    );
}

#[test]
fn clipboard_words_name_no_picture() {
    let sample = Sample::new("attaching-clipboard-words");
    let source = sample.root().join("main.rs");
    fs::write(&source, b"fn main() {}").expect("a source file");

    assert_eq!(pictured("hello world"), None);
    assert_eq!(
        pictured("holiday.png"),
        None,
        "a relative path names nobody"
    );
    assert_eq!(
        pictured("/nowhere/at/all/holiday.png"),
        None,
        "a picture that is not there is not one",
    );
    assert_eq!(
        pictured(&written(&source)),
        None,
        "a file no model reads as an image is not one either",
    );
}
