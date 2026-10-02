//! A provider that answers from a script, and a tool that answers from a field.
//!
//! The wiring's own tests need a whole turn to run — that is the only way the
//! thread, the two channels and the drain are exercised together — but they
//! must not need a network or a machine to run things on.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crucible_models::{
    Delta, DeltaStream, PromptCacheCapabilities, PromptCacheRoute, Provider, ProviderError, Request,
};
use crucible_runtime::BoxFuture;
use crucible_runtime::Cancel;
use crucible_tools::{
    Approved, Command, DescribeTool, Sensitivity, Summary, Target, Tool, ToolContext, ToolError,
    ToolOutput,
};
use crucible_types::{
    CredentialScopeId, Message, Modalities, Modality, PlanWindows, PromptCacheEncoding, ToolArgs,
};

/// Drives a future to its answer on a current-thread runtime of its own, the
/// way a test takes a turn on a runner it holds.
pub(crate) trait Awaited: std::future::Future + Sized {
    /// The future's answer, once it has one.
    fn awaited(self) -> Self::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a test runtime")
            .block_on(self)
    }
}

impl<F: std::future::Future> Awaited for F {}

/// The runtime a test's conversations wait for their turns on, standing where
/// the application's own would: built once for the whole test binary, with
/// its workers driving whatever a turn waits on.
pub(crate) fn runtime() -> tokio::runtime::Handle {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .build()
                .expect("a test runtime")
        })
        .handle()
        .clone()
}

/// How many requests a script has been given, readable after it has moved into
/// a runner.
pub(crate) type Asked = Arc<AtomicUsize>;

/// The instructions and typed context each request was asked under, in order.
///
/// Neither is shown on screen. Stable operator instructions occupy the system
/// field; session facts occupy typed transcript fragments. Keeping both makes
/// this fixture observe what the model actually read across that boundary.
pub(crate) type Under = Arc<Mutex<Vec<String>>>;

/// What every tool result in each request said, one entry per request, in
/// order: what the model was sent of what its tools returned.
pub(crate) type Sent = Arc<Mutex<Vec<Vec<String>>>>;

/// Answers each request with the next batch of deltas it was given.
#[derive(Debug)]
pub(crate) struct Script {
    credential_scope: CredentialScopeId,
    rounds: Mutex<std::vec::IntoIter<Vec<Delta>>>,
    asked: Asked,
    under: Under,
    sent: Sent,
    /// Refuses every request, for the path where the provider itself fails
    /// rather than the transcript going wrong inside a response.
    refusing: bool,
    /// The refusal of a used-up plan every request meets, where one does.
    used_up: Option<UsedUp>,
    /// What the plan answers when it is asked, where it keeps a source.
    answering: Option<PlanWindows>,
    /// How many times the plan was asked.
    limits_asked: Asked,
    /// Whether the plan, asked, never answers.
    stalling: bool,
}

/// A vendor's refusal of a used-up plan, and the windows its head reported,
/// where it reported any.
#[derive(Debug, Clone)]
struct UsedUp {
    reading: Option<PlanWindows>,
}

impl Script {
    pub(crate) fn new(rounds: Vec<Vec<Delta>>) -> Self {
        Self {
            credential_scope: CredentialScopeId::new(),
            rounds: Mutex::new(rounds.into_iter()),
            asked: Asked::default(),
            under: Under::default(),
            sent: Sent::default(),
            refusing: false,
            used_up: None,
            answering: None,
            limits_asked: Asked::default(),
            stalling: false,
        }
    }

    /// A provider whose plan keeps a source of its limits, and answers
    /// `windows` each time it is asked.
    pub(crate) fn answering(self, windows: PlanWindows) -> Self {
        Self {
            answering: Some(windows),
            ..self
        }
    }

    /// A provider whose plan keeps a source of its limits and, asked, never
    /// answers.
    pub(crate) fn stalling(self) -> Self {
        Self {
            answering: Some(PlanWindows::new(std::time::SystemTime::now())),
            stalling: true,
            ..self
        }
    }

    /// A handle on how many times the plan was asked, taken the same way as
    /// [`Self::asked`].
    pub(crate) fn limits_asked(&self) -> Asked {
        Arc::clone(&self.limits_asked)
    }

    /// A provider that will not answer at all.
    pub(crate) fn refusing() -> Self {
        Self {
            refusing: true,
            ..Self::new(Vec::new())
        }
    }

    /// A provider whose vendor refuses every request because the plan is used
    /// up until tomorrow, reporting `reading` on the refusal's head.
    pub(crate) fn used_up(reading: Option<PlanWindows>) -> Self {
        Self {
            used_up: Some(UsedUp { reading }),
            ..Self::new(Vec::new())
        }
    }

    /// A handle on the request count, taken before the script is handed over.
    pub(crate) fn asked(&self) -> Asked {
        Arc::clone(&self.asked)
    }

    /// A handle on what the requests were asked under, taken the same way and
    /// for the same reason: the script itself is inside the runner by the time
    /// there is anything to read.
    pub(crate) fn under(&self) -> Under {
        Arc::clone(&self.under)
    }

    /// A handle on what each request sent of its tools' results, taken the
    /// same way.
    pub(crate) fn sent(&self) -> Sent {
        Arc::clone(&self.sent)
    }
}

impl Provider for Script {
    fn name(&self) -> &'static str {
        "script"
    }

    /// A stand-in spells what every real provider here spells today.
    ///
    /// It is not a wire protocol, so it has nothing of its own to declare; what
    /// it must not do is differ, or a test would be exercising a capability no
    /// provider has.
    fn spells(&self) -> Modalities {
        Modalities::empty().insert(Modality::Text)
    }

    fn prompt_cache_capabilities(&self, _model: &str) -> PromptCacheCapabilities {
        PromptCacheCapabilities::unknown("cli-script-fixture-v1")
    }

    fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
        PromptCacheRoute {
            protocol: "script",
            endpoint: "script",
            custom_endpoint: true,
            credential_scope: self.credential_scope,
            account: None,
            project: None,
            request_shape_version: "cli-script-fixture-v1",
        }
    }

    fn prompt_cache_encoding(&self, _request: &Request<'_>) -> PromptCacheEncoding {
        PromptCacheEncoding::NoControlIntended
    }

    fn ask_limits(&self) -> Option<BoxFuture<'static, crucible_models::Asked>> {
        let windows = self.answering.clone()?;
        let asked = Arc::clone(&self.limits_asked);
        let stalling = self.stalling;
        Some(Box::pin(async move {
            asked.fetch_add(1, Ordering::Relaxed);
            if stalling {
                std::future::pending::<()>().await;
            }
            crucible_models::Asked::Answered(windows)
        }))
    }

    fn stream<'a>(
        &'a self,
        request: Request<'a>,
        _cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<Box<dyn DeltaStream>, ProviderError>> {
        Box::pin(async move {
            self.asked.fetch_add(1, Ordering::Relaxed);

            // A poisoned lock is a panic in another test's thread, which this one
            // cannot report better than by having nothing to assert on.
            if let Ok(mut under) = self.under.lock() {
                let mut assembled = request.system.unwrap_or_default().to_owned();
                for fragment in
                    request
                        .transcript
                        .messages()
                        .iter()
                        .filter_map(|message| match message {
                            Message::Context(fragment) => Some(fragment),
                            Message::User { .. }
                            | Message::Agent { .. }
                            | Message::ToolResults(_) => None,
                        })
                {
                    assembled.push_str("\n\n");
                    assembled.push_str(fragment.text());
                }
                under.push(assembled);
            }
            if let Ok(mut sent) = self.sent.lock() {
                sent.push(
                    request
                        .transcript
                        .messages()
                        .iter()
                        .filter_map(|message| match message {
                            Message::ToolResults(results) => Some(results),
                            _ => None,
                        })
                        .flatten()
                        .map(|result| result.output.text().to_owned())
                        .collect(),
                );
            }

            if self.refusing {
                return Err(ProviderError::Refused {
                    provider: "script",
                    status: 401,
                    message: "no".into(),
                });
            }

            if let Some(UsedUp { reading }) = self.used_up.clone() {
                return Err(ProviderError::PlanLimit {
                    provider: "script",
                    window: reading
                        .as_ref()
                        .and_then(|reading| reading.exhausted(request.model, SystemTime::now()))
                        .map(|(window, _)| window),
                    resets_at: Some(SystemTime::now() + Duration::from_hours(24)),
                    reading: reading.map(Box::new),
                });
            }

            let round = self
                .rounds
                .lock()
                .map_err(|_| ProviderError::Transport {
                    provider: "script",
                    problem: "poisoned".into(),
                })?
                .next()
                .unwrap_or_default();

            Ok(Box::new(Reading(round.into_iter())) as Box<dyn DeltaStream>)
        })
    }
}

/// The deltas of one round, handed over one at a time.
struct Reading(std::vec::IntoIter<Delta>);

impl DeltaStream for Reading {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<Delta, ProviderError>>> {
        Box::pin(async move { self.0.next().map(Ok) })
    }
}

/// A provider stream that stays quiet until cancellation reaches it.
#[derive(Debug)]
pub(crate) struct Stalling {
    escaped: Arc<AtomicBool>,
    credential_scope: CredentialScopeId,
}

impl Stalling {
    /// Makes the provider and a mark raised only by the test escape deadline.
    pub(crate) fn new() -> (Self, Arc<AtomicBool>) {
        let escaped = Arc::new(AtomicBool::new(false));
        (
            Self {
                escaped: Arc::clone(&escaped),
                credential_scope: CredentialScopeId::new(),
            },
            escaped,
        )
    }
}

impl Provider for Stalling {
    fn name(&self) -> &'static str {
        "stalling"
    }

    /// A stand-in spells what every real provider here spells today.
    ///
    /// It is not a wire protocol, so it has nothing of its own to declare; what
    /// it must not do is differ, or a test would be exercising a capability no
    /// provider has.
    fn spells(&self) -> Modalities {
        Modalities::empty().insert(Modality::Text)
    }

    fn prompt_cache_capabilities(&self, _model: &str) -> PromptCacheCapabilities {
        PromptCacheCapabilities::unknown("stalling-fixture-v1")
    }

    fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
        PromptCacheRoute {
            protocol: "stalling",
            endpoint: "stalling",
            custom_endpoint: true,
            credential_scope: self.credential_scope,
            account: None,
            project: None,
            request_shape_version: "stalling-fixture-v1",
        }
    }

    fn prompt_cache_encoding(&self, _request: &Request<'_>) -> PromptCacheEncoding {
        PromptCacheEncoding::NoControlIntended
    }

    fn stream<'a>(
        &'a self,
        _request: Request<'a>,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<Box<dyn DeltaStream>, ProviderError>> {
        Box::pin(async move {
            Ok(Box::new(Quiet {
                cancel: cancel.clone(),
                escaped: Arc::clone(&self.escaped),
            }) as Box<dyn DeltaStream>)
        })
    }
}

/// The live body of [`Stalling`].
struct Quiet {
    cancel: Cancel,
    escaped: Arc<AtomicBool>,
}

impl DeltaStream for Quiet {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<Delta, ProviderError>>> {
        Box::pin(async move {
            let escape = Instant::now() + Duration::from_millis(250);
            while !self.cancel.requested() {
                if Instant::now() >= escape {
                    self.escaped.store(true, Ordering::Release);
                    return Some(Err(ProviderError::Transport {
                        provider: "stalling",
                        problem: "test escape deadline elapsed".into(),
                    }));
                }
                std::thread::park_timeout(Duration::from_millis(1));
            }

            Some(Err(ProviderError::Cancelled("stalling")))
        })
    }
}

/// A tool that always produces the same thing, at a sensitivity it was told.
#[derive(Debug)]
pub(crate) struct Fixed {
    name: &'static str,
    answer: &'static str,
    sensitivity: Sensitivity,
}

impl Fixed {
    pub(crate) fn new(name: &'static str, sensitivity: Sensitivity) -> Self {
        Self {
            name,
            answer: "done",
            sensitivity,
        }
    }
}

impl DescribeTool for Fixed {
    fn name(&self) -> &str {
        self.name
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }
}

impl Tool for Fixed {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        self.sensitivity.clone()
    }

    /// The arguments as they arrived. A real tool names one field of them; what
    /// a test needs is to see that whatever the tool said reached the row, so
    /// this says something no other value could be mistaken for.
    fn summary(&self, args: &ToolArgs) -> Summary {
        Summary::new(args.as_str())
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        _context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move { Ok(ToolOutput::ok(self.answer)) })
    }
}

/// A change to a file: what every mode but `fullAccess` puts to the user.
///
/// The target is one nothing resolved, so no rule written about a path matches
/// it and what these tests exercise is the loop rather than the matcher.
pub(crate) fn changing() -> Sensitivity {
    Sensitivity::MutatesFile {
        target: Target::unresolved(),
    }
}

/// One program, run with nothing after it: the shape a rule can be minted from.
pub(crate) fn running(command: &str) -> Sensitivity {
    Sensitivity::SpawnsProcess {
        command: Command::Understood {
            sent: command.into(),
            parts: Box::from([Box::from(command)]),
        },
    }
}
