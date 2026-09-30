//! The speed a conversation asks at, with no terminal: what asking for one
//! comes to, when the user's file is heeded, and when it goes back to standard.

use crucible_app::content_use::{Consent, Routes};
use crucible_app::speed::Hastened;
use crucible_models::{FastForm, Speed};

use super::*;

/// A provider whose every model has `form` for a fast form, which records the
/// speed each request asked at and, where it `refuses`, refuses fast.
struct Fastened {
    form: FastForm,
    refuses: bool,
    speeds: Arc<Mutex<Vec<Speed>>>,
    scope: CredentialScopeId,
}

impl Fastened {
    fn new(form: FastForm) -> Self {
        Self {
            form,
            refuses: false,
            speeds: Arc::default(),
            scope: CredentialScopeId::new(),
        }
    }

    fn refusing(self) -> Self {
        Self {
            refuses: true,
            ..self
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
        PromptCacheCapabilities::unknown("headless-fast-fixture-v1")
    }

    fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
        PromptCacheRoute {
            protocol: "script",
            endpoint: "script",
            custom_endpoint: true,
            credential_scope: self.scope,
            account: None,
            project: None,
            request_shape_version: "headless-fast-fixture-v1",
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
                    message: "your plan does not include fast".into(),
                });
            }
            Ok(Box::new(Reading(saying("answered").into_iter())) as Box<dyn DeltaStream>)
        })
    }
}

const FIELD: FastForm = FastForm::Field("2x the price");

/// A conversation asking `provider` as OpenAI, under a desk whose user file
/// is the one a yes would be written to.
fn fastened(tree: &Tree, provider: Fastened) -> Result<(Conversation, Desk), Failed> {
    let desk = Desk::new(tree, &["openai"])?;
    let consent = Consent::new(Routes::production());
    consent.keeps_in(desk.choosing.clone());
    let conversation =
        conversing(tree, provider, &Guard::Nothing, Some("openai"))?.consenting(consent);
    Ok((conversation, desk))
}

#[test]
fn fast_is_asked_where_the_model_has_a_fast_form_and_is_written_down() -> Result<(), Failed> {
    let tree = Tree::new("speed-fast")?;
    let provider = Fastened::new(FIELD);
    let speeds = Arc::clone(&provider.speeds);
    let (mut conversation, desk) = fastened(&tree, provider)?;

    let hastened = conversation.hasten(Speed::Fast, &desk.with());

    assert!(
        matches!(hastened, Hastened::Taken { unwritten: None }),
        "{hastened:?}"
    );
    assert_eq!(written(&tree)?.speed("openai"), Speed::Fast);
    turn(&mut conversation, "go")?;
    assert_eq!(*speeds.lock().map_err(|_| "poisoned")?, [Speed::Fast]);

    let standard = conversation.hasten(Speed::Standard, &desk.with());
    assert!(matches!(standard, Hastened::Taken { unwritten: None }));
    assert_eq!(written(&tree)?.speed("openai"), Speed::Standard);
    Ok(())
}

#[test]
fn a_model_with_no_fast_form_or_one_of_its_own_changes_nothing() -> Result<(), Failed> {
    for (form, answer) in [
        (FastForm::None, "unsupported"),
        (FastForm::Own("6x the speed for 3x the quota"), "own"),
    ] {
        let tree = Tree::new("speed-none")?;
        let (mut conversation, desk) = fastened(&tree, Fastened::new(form))?;

        let hastened = conversation.hasten(Speed::Fast, &desk.with());

        let said = match hastened {
            Hastened::Unsupported => "unsupported",
            Hastened::Own => "own",
            Hastened::Unasked | Hastened::Taken { .. } => "changed",
        };
        assert_eq!(said, answer);
        assert_eq!(conversation.runner().speed(), Speed::Standard);
        assert!(!desk.choosing.exists(), "nothing is written down");
    }
    Ok(())
}

#[test]
fn nobody_being_asked_is_nobody_to_ask_fast() -> Result<(), Failed> {
    let tree = Tree::new("speed-unasked")?;
    let desk = Desk::new(&tree, &["openai"])?;
    let mut conversation = conversing(&tree, Fastened::new(FIELD), &Guard::Nothing, None)?;

    let hastened = conversation.hasten(Speed::Fast, &desk.with());

    assert!(matches!(hastened, Hastened::Unasked), "{hastened:?}");
    assert_eq!(conversation.runner().speed(), Speed::Standard);
    Ok(())
}

#[test]
fn the_speed_in_the_file_is_asked_only_where_the_model_has_a_fast_form() -> Result<(), Failed> {
    for (form, asked) in [
        (FIELD, Speed::Fast),
        (FastForm::None, Speed::Standard),
        (
            FastForm::Own("6x the speed for 3x the quota"),
            Speed::Standard,
        ),
    ] {
        let tree = Tree::new("speed-file")?;
        let (mut conversation, desk) = fastened(&tree, Fastened::new(form))?;
        remember::hastening(&desk.choosing, "openai", Speed::Fast)?;

        conversation.hastened_as_written(&written(&tree)?);

        assert_eq!(conversation.runner().speed(), asked, "{form:?}");
    }
    Ok(())
}

#[test]
fn another_model_goes_back_to_standard_and_the_same_one_keeps_fast() -> Result<(), Failed> {
    let tree = Tree::new("speed-model")?;
    let (mut conversation, desk) = fastened(&tree, Fastened::new(FIELD))?;
    let _ = conversation.hasten(Speed::Fast, &desk.with());
    let openai = desk.one("openai")?;

    let again = runtime()?.block_on(conversation.ask_for(openai, "script", None, &desk.with()));
    assert!(
        matches!(
            again,
            Switched::Taken {
                unwritten: None,
                ..
            }
        ),
        "{again:?}"
    );
    assert_eq!(conversation.runner().speed(), Speed::Fast);
    assert_eq!(written(&tree)?.speed("openai"), Speed::Fast);

    let other =
        runtime()?.block_on(conversation.ask_for(openai, "gpt-5.6-sol", None, &desk.with()));
    assert!(
        matches!(
            other,
            Switched::Taken {
                unwritten: None,
                ..
            }
        ),
        "{other:?}"
    );
    assert_eq!(conversation.runner().speed(), Speed::Standard);
    assert_eq!(written(&tree)?.speed("openai"), Speed::Standard);
    Ok(())
}

#[test]
fn a_refusal_of_fast_is_written_down_as_standard() -> Result<(), Failed> {
    let tree = Tree::new("speed-refused")?;
    let provider = Fastened::new(FIELD).refusing();
    let speeds = Arc::clone(&provider.speeds);
    let (mut conversation, desk) = fastened(&tree, provider)?;
    let _ = conversation.hasten(Speed::Fast, &desk.with());
    assert_eq!(written(&tree)?.speed("openai"), Speed::Fast);

    let (_, said) = turn(&mut conversation, "go")?;

    assert!(said.contains("answered"), "{said}");
    assert_eq!(
        *speeds.lock().map_err(|_| "poisoned")?,
        [Speed::Fast, Speed::Standard]
    );
    assert_eq!(conversation.runner().speed(), Speed::Standard);
    assert_eq!(written(&tree)?.speed("openai"), Speed::Standard);
    Ok(())
}
