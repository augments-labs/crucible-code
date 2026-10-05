//! `/fast` in a whole conversation: what it answers for each kind of model,
//! what the next request asks at, the label, and a refusal of fast.

use crucible_models::{
    Cost, DeltaStream, FastForm, PromptCacheCapabilities, PromptCacheRoute, Provider,
    ProviderError, Request, Served, Speed,
};
use crucible_runner::Runner;
use crucible_runtime::{BoxFuture, Cancel};
use crucible_types::{CredentialScopeId, Modalities, Modality, PromptCacheEncoding};

use super::*;

/// A provider whose model has `form` for a fast form: it records the speed
/// each request asked at, says each answer was served at `serves`, and where it
/// `refuses`, refuses fast.
pub(super) struct Fastened {
    form: FastForm,
    refuses: bool,
    serves: Served,
    speeds: Arc<Mutex<Vec<Speed>>>,
    scope: CredentialScopeId,
}

impl Fastened {
    pub(super) fn new(form: FastForm) -> Self {
        Self {
            form,
            refuses: false,
            serves: Served::Unsaid,
            speeds: Arc::default(),
            scope: CredentialScopeId::new(),
        }
    }
}

impl Provider for Fastened {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn spells(&self) -> Modalities {
        Modalities::empty().insert(Modality::Text)
    }

    fn prompt_cache_capabilities(&self, _model: &str) -> PromptCacheCapabilities {
        PromptCacheCapabilities::unknown("cli-fast-fixture-v1")
    }

    fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
        PromptCacheRoute {
            protocol: "script",
            endpoint: "script",
            custom_endpoint: true,
            credential_scope: self.scope,
            account: None,
            project: None,
            request_shape_version: "cli-fast-fixture-v1",
        }
    }

    fn prompt_cache_encoding(&self, _request: &Request<'_>) -> PromptCacheEncoding {
        PromptCacheEncoding::NoControlIntended
    }

    fn fast(&self, _model: &str) -> FastForm {
        self.form
    }

    fn stream<'a>(
        &'a self,
        request: Request<'a>,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<Box<dyn DeltaStream>, ProviderError>> {
        self.stream_at(request, Speed::Standard, cancel)
    }

    fn stream_at<'a>(
        &'a self,
        _request: Request<'a>,
        speed: Speed,
        _cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<Box<dyn DeltaStream>, ProviderError>> {
        Box::pin(async move {
            if let Ok(mut speeds) = self.speeds.lock() {
                speeds.push(speed);
            }
            if self.refuses && speed == Speed::Fast {
                return Err(ProviderError::FastRefused {
                    provider: "openai",
                    message: "your plan does not include gpt-6-sol fast".into(),
                });
            }
            Ok(Box::new(Answered {
                deltas: saying("answered").into_iter(),
                served: self.serves,
            }) as Box<dyn DeltaStream>)
        })
    }
}

/// One answer, and the speed it says it was served at.
struct Answered {
    deltas: std::vec::IntoIter<Delta>,
    served: Served,
}

impl DeltaStream for Answered {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<Delta, ProviderError>>> {
        Box::pin(async move { self.deltas.next().map(Ok) })
    }

    fn served(&self) -> Served {
        self.served
    }
}

pub(super) const FIELD: FastForm = FastForm::Field(Cost {
    price: "2x the price",
    speed: None,
    caveat: Some("OpenAI may serve a fast request at standard speed."),
});

/// What the terminal holds after `typed` is run over `provider`, and the speed
/// each request asked at.
fn fasting(provider: Fastened, typed: &str) -> (String, Vec<Speed>) {
    fasting_on(provider, "gpt-6-sol", typed)
}

/// The same, over `model`, the empty name where none is chosen.
fn fasting_on(provider: Fastened, model: &str, typed: &str) -> (String, Vec<Speed>) {
    let speeds = Arc::clone(&provider.speeds);
    let conversation =
        Conversation::recording(Arc::new(Session::nowhere()), Some("openai"), |session| {
            Runner::new(
                Box::new(provider),
                Tools::new(),
                Agent::new(
                    AgentId::new("test"),
                    Model {
                        name: model.into(),
                        max_tokens: 64,
                        window: None,
                        accepts: None,
                        effort: None,
                    },
                ),
                crucible_context::ContextInputs::new(std::env::temp_dir()),
                session,
            )
        });
    let mut renderer = Renderer::new(Recording::new(200, 24));
    let mut input = Cursor::new(typed.as_bytes().to_vec());

    converse(
        conversation,
        &mut renderer,
        &plain(),
        First {
            card: &opening(),
            arming: None,
        },
        &mut input,
    )
    .expect("the loop to finish");
    let speeds = speeds
        .lock()
        .map(|speeds| speeds.clone())
        .unwrap_or_default();
    (renderer.terminal().written().to_string(), speeds)
}

#[test]
fn a_model_with_no_fast_form_is_told_so_and_asked_at_standard() {
    let (written, speeds) = fasting(Fastened::new(FastForm::None), "/fast on\nhello\n");

    assert!(
        written.contains("openai · gpt-6-sol has no fast form"),
        "{written}"
    );
    assert_eq!(speeds, [Speed::Standard]);
}

#[test]
fn a_model_that_is_fast_of_its_own_points_at_the_others() {
    let own = FastForm::Own(Cost {
        price: "6x the speed for 3x the quota",
        speed: None,
        caveat: None,
    });
    let (written, _) = fasting(Fastened::new(own), "/fast\n");

    assert!(
        written.contains("is a fast model of its own; /model lists the others"),
        "{written}"
    );
}

#[test]
fn fast_turned_on_is_what_the_next_request_asks_at() {
    let (written, speeds) = fasting(Fastened::new(FIELD), "/fast on\nhello\n/fast off\nagain\n");

    assert!(written.contains("openai · gpt-6-sol · fast"), "{written}");
    // Standard is the label with no speed part, not a speed of its own.
    assert!(written.contains("openai · gpt-6-sol,"), "{written}");
    assert!(!written.contains("· standard"), "{written}");
    assert_eq!(speeds, [Speed::Fast, Speed::Standard]);
}

#[test]
fn with_no_keyboard_the_speed_in_force_and_the_lines_to_type_are_written() {
    let (written, _) = fasting(Fastened::new(FIELD), "/fast\n");

    assert!(written.contains("standard"), "{written}");
    let dash = crucible_tui::Glyphs::Unicode.dash();
    assert!(
        written.contains(&format!("/fast on {dash} 2x the price")),
        "{written}"
    );
    assert!(
        written.contains(&format!("/fast off {dash} The standard price and speed")),
        "{written}"
    );
}

#[test]
fn a_word_that_is_neither_on_nor_off_changes_nothing() {
    let (written, speeds) = fasting(Fastened::new(FIELD), "/fast faster\nhello\n");

    assert!(written.contains("! /fast takes on or off"), "{written}");
    assert_eq!(speeds, [Speed::Standard]);
}

/// Every status row drawn: the words after the mode, up to the next escape.
fn status_rows(written: &str) -> Vec<&str> {
    written
        .split("(shift+tab to cycle)")
        .skip(1)
        .map(|after| after.split('\x1b').next().unwrap_or_default().trim())
        .collect()
}

#[test]
fn the_label_says_fast_only_after_an_answer_served_fast() {
    let labels = |serves| {
        let provider = Fastened {
            serves,
            ..Fastened::new(FIELD)
        };
        let (written, _) = fasting(provider, "/fast on\nhello\nagain\n");
        status_rows(&written)
            .iter()
            .map(|row| (*row).to_owned())
            .collect::<Vec<_>>()
    };

    let standard = labels(Served::Standard);
    let fast = labels(Served::Fast);

    assert!(!standard.is_empty(), "no status row was drawn");
    assert!(
        standard.iter().all(|row| !row.ends_with("· fast")),
        "{standard:?}"
    );
    assert!(
        fast.iter()
            .any(|row| row.ends_with("openai · gpt-6-sol · fast")),
        "{fast:?}"
    );
}

#[test]
fn a_refusal_of_fast_is_one_line_and_one_more_request_at_standard() {
    let provider = Fastened {
        refuses: true,
        ..Fastened::new(FIELD)
    };
    let (written, speeds) = fasting(provider, "/fast on\nhello\n");

    assert_eq!(speeds, [Speed::Fast, Speed::Standard]);
    assert!(
        written.contains(
            "openai refused fast: your plan does not include gpt-6-sol fast. \
             Sent again at standard speed; fast is off."
        ),
        "{written}"
    );
    assert!(written.contains("answered"), "{written}");
}

#[test]
fn with_no_model_chosen_it_says_what_it_says_for_any_model_command() {
    let (written, _) = fasting_on(Fastened::new(FIELD), "", "/fast\n");

    assert!(written.contains("No model selected"), "{written}");
    assert!(!written.contains("enter to choose"), "{written}");
}
