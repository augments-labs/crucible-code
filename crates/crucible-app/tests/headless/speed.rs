//! The speed a conversation asks at, with no terminal: what asking for one
//! comes to, when the user's file is heeded, and when it goes back to standard.

use crucible_app::content_use::{Consent, Routes};
use crucible_app::speed::{self as speeds, Hastened};
use crucible_models::{Cost, FastForm, Served, Speed};

use super::*;

/// A provider whose every model has `form` for a fast form, which records the
/// speed each request asked at and, where it `refuses`, refuses fast.
struct Fastened {
    form: FastForm,
    refuses: bool,
    /// What each answer says about the speed it was served at.
    serves: Served,
    speeds: Arc<Mutex<Vec<Speed>>>,
    scope: CredentialScopeId,
}

impl Fastened {
    fn new(form: FastForm) -> Self {
        Self {
            form,
            refuses: false,
            serves: Served::Unsaid,
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
            Ok(Box::new(Answered {
                deltas: Reading(saying("answered").into_iter()),
                served: self.serves,
            }) as Box<dyn DeltaStream>)
        })
    }
}

/// One answer, and the speed it says it was served at.
struct Answered {
    deltas: Reading,
    served: Served,
}

impl DeltaStream for Answered {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<Delta, ProviderError>>> {
        self.deltas.next()
    }

    fn served(&self) -> Served {
        self.served
    }
}

const FIELD: FastForm = FastForm::Field(Cost {
    price: "2x the price",
    speed: None,
    caveat: None,
});

const OWN: FastForm = FastForm::Own(Cost {
    price: "6x the speed for 3x the quota",
    speed: None,
    caveat: None,
});

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
    for (form, answer) in [(FastForm::None, "unsupported"), (OWN, "own")] {
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
        (OWN, Speed::Standard),
    ] {
        let tree = Tree::new("speed-file")?;
        let (mut conversation, desk) = fastened(&tree, Fastened::new(form))?;
        remember::hastening(&desk.choosing, "openai", "script")?;

        conversation.hastened_as_kept(&desk.choosing);

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

/// A desk whose store takes a provider's speed out of the user's file when its
/// credential moves, as the terminal's does, and whose provider is set up as
/// one with a fast form.
struct Moving {
    desk: Desk,
    logins: Store,
    serving: Serving,
    /// The speed each request asked at, of every provider set up here.
    speeds: Arc<Mutex<Vec<Speed>>>,
}

impl Moving {
    fn new(tree: &Tree) -> Result<Self, Failed> {
        let desk = Desk::new(tree, &["openai"])?;
        let logins =
            Store::in_home(tree.home()?.path()).moving(speeds::moving(desk.choosing.clone()));
        let speeds: Arc<Mutex<Vec<Speed>>> = Arc::default();
        let shared = Arc::clone(&speeds);
        let serving: Serving = Box::new(move |_, _| {
            Ok(Resolved {
                provider: Box::new(Fastened {
                    speeds: Arc::clone(&shared),
                    ..Fastened::new(FIELD)
                }),
                source: CredentialSource::Environment("OPENAI_API_KEY".into()),
            })
        });
        Ok(Self {
            desk,
            logins,
            serving,
            speeds,
        })
    }

    fn with(&self) -> Switching<'_> {
        Switching {
            providers: &self.desk.providers,
            settings: &self.desk.settings,
            serving: &self.serving,
            sourcing: &self.desk.sourcing,
            logins: &self.logins,
            choosing: &self.desk.choosing,
        }
    }
}

#[test]
fn a_key_stored_where_the_environment_served_turns_fast_off() -> Result<(), Failed> {
    let tree = Tree::new("speed-stored")?;
    let moving = Moving::new(&tree)?;
    let (mut conversation, _) = fastened(&tree, Fastened::new(FIELD))?;
    let _ = conversation.hasten(Speed::Fast, &moving.with());

    moving.logins.keep("openai", "fabricated-stored-key")?;
    assert_eq!(written(&tree)?.speed("openai"), Speed::Standard);
    let openai = moving.desk.one("openai")?;
    let logged_in = runtime()?.block_on(conversation.logged_in(openai, &moving.with()));

    assert!(
        matches!(logged_in, LoggedIn::Serving { .. }),
        "{logged_in:?}"
    );
    assert_eq!(conversation.runner().speed(), Speed::Standard);
    turn(&mut conversation, "go")?;
    assert_eq!(
        *moving.speeds.lock().map_err(|_| "poisoned")?,
        [Speed::Standard]
    );
    Ok(())
}

#[test]
fn a_key_written_again_under_its_own_row_keeps_fast() -> Result<(), Failed> {
    let tree = Tree::new("speed-rekeyed")?;
    let moving = Moving::new(&tree)?;
    moving.logins.keep("openai", "fabricated-first-key")?;
    let (mut conversation, _) = fastened(&tree, Fastened::new(FIELD))?;
    let _ = conversation.hasten(Speed::Fast, &moving.with());

    moving.logins.keep("openai", "fabricated-second-key")?;
    let openai = moving.desk.one("openai")?;
    let _ = runtime()?.block_on(conversation.logged_in(openai, &moving.with()));

    assert_eq!(written(&tree)?.speed("openai"), Speed::Fast);
    assert_eq!(conversation.runner().speed(), Speed::Fast);
    Ok(())
}

#[test]
fn a_stored_key_forgotten_while_the_environment_still_serves_turns_fast_off() -> Result<(), Failed>
{
    let tree = Tree::new("speed-forgotten")?;
    let moving = Moving::new(&tree)?;
    moving.logins.keep("openai", "fabricated-stored-key")?;
    let (mut conversation, _) = fastened(&tree, Fastened::new(FIELD))?;
    let _ = conversation.hasten(Speed::Fast, &moving.with());

    let openai = moving.desk.one("openai")?;
    let logged_out = runtime()?.block_on(conversation.log_out(openai, &moving.with()));

    assert!(
        matches!(logged_out, LoggedOut::StillServed { .. }),
        "{logged_out:?}"
    );
    assert_eq!(written(&tree)?.speed("openai"), Speed::Standard);
    assert_eq!(conversation.runner().speed(), Speed::Standard);
    Ok(())
}

/// Every theme name a host in these tests reads: none.
fn reads(_: &str) -> bool {
    false
}

/// The release notes a host in these tests has: none asked for here.
fn notes(
    _: Option<&str>,
) -> Result<crucible_client_api::NotesOutcome, crucible_client_api::Refusal> {
    Err(crucible_client_api::ErrorCode::InvalidArgument.into())
}

#[test]
fn a_client_with_no_terminal_sets_the_speed_and_reads_what_was_served() -> Result<(), Failed> {
    use crucible_app::client;
    use crucible_client_api::{
        Capabilities, Command, Correlation, Outcome, Pace, Request, Response, Snapshot,
        SpeedOutcome,
    };

    let tree = Tree::new("speed-client")?;
    let provider = Fastened {
        serves: Served::Fast,
        ..Fastened::new(FIELD)
    };
    let (mut conversation, standing) = fastened(&tree, provider)?;
    let (workspace, sessions) = (tree.workspace()?, tree.sessions());
    let desk = client::Desk {
        switching: standing.with(),
        sessions: &sessions,
        workspace: &workspace,
        reads,
        notes,
    };
    let sent = Request::new(
        Capabilities::every(),
        Correlation::new(1),
        Command::SetSpeed(Pace::Fast),
    );
    let request = Request::decode(&sent.encode()?).map_err(|refused| refused.refusal)?;

    let performed = runtime()?.block_on(client::perform(&mut conversation, &request, &desk));

    let answered = Response::decode(&performed.response(&request).encode()?)?;
    assert_eq!(
        answered.outcome,
        Outcome::Speed(SpeedOutcome::Taken { unwritten: None })
    );
    let asked = Snapshot::decode(&client::snapshot(&conversation).encode()?)?;
    assert_eq!((asked.speed, asked.served), (Pace::Fast, None));

    turn(&mut conversation, "go")?;
    let served = Snapshot::decode(&client::snapshot(&conversation).encode()?)?;
    assert_eq!(
        (served.speed, served.served),
        (Pace::Fast, Some(Pace::Fast))
    );
    Ok(())
}

#[test]
fn a_refusal_of_fast_while_making_room_is_written_down_as_standard() -> Result<(), Failed> {
    let tree = Tree::new("speed-refused-compacting")?;
    let provider = Fastened::new(FIELD).refusing();
    let speeds = Arc::clone(&provider.speeds);
    let desk = Desk::new(&tree, &["openai"])?;
    let consent = Consent::new(Routes::production());
    consent.keeps_in(desk.choosing.clone());
    // A budget that keeps only the turn in hand, so two turns have a middle
    // for the compaction to replace, and it sends a request.
    let session = Arc::new(Session::start(&tree.sessions(), &tree.workspace()?, None)?);
    let agent = AgentBuilder::new(
        AgentId::new("test"),
        Model {
            name: "script".into(),
            max_tokens: 64,
            window: None,
            accepts: None,
            effort: None,
        },
    );
    let work = tree.0.join("work");
    let mut conversation = Conversation::recording(session, Some("openai"), |session| {
        Runner::new(
            Box::new(provider),
            Tools::new(),
            agent.build(),
            crucible_context::ContextInputs::new(work),
            session,
        )
        .under(crucible_runner::RunPolicy {
            compaction: crucible_runner::Compaction {
                keep_tokens: 1,
                ..crucible_runner::Compaction::default()
            },
            ..crucible_runner::RunPolicy::default()
        })
    })
    .consenting(consent);
    turn(&mut conversation, "something to make room from")?;
    turn(&mut conversation, "and a middle to replace")?;
    let _ = conversation.hasten(Speed::Fast, &desk.with());
    assert_eq!(written(&tree)?.speed("openai"), Speed::Fast);

    let (events, _reported) = mpsc::channel::<EventEnvelope>();
    let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
    let run = conversation
        .runner()
        .starting(&events, &cancel, &steer, &aside);
    let mut spent = crucible_types::Spend::default();
    let _ = runtime()?.block_on(conversation.compact(
        crucible_types::Compacting::Asked,
        &run,
        &mut spent,
    ));

    let asked = speeds.lock().map_err(|_| "poisoned")?.clone();
    assert_eq!(
        asked.get(2..),
        Some(&[Speed::Fast, Speed::Standard][..]),
        "{asked:?}"
    );
    assert_eq!(conversation.runner().speed(), Speed::Standard);
    assert_eq!(written(&tree)?.speed("openai"), Speed::Standard);
    Ok(())
}
