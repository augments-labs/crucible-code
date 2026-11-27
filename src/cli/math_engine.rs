//! What makes the display formulas of an answer into pictures, off the
//! drawing thread.
//!
//! The renderer tells each display formula that closes in a block it would
//! show as a picture; this queues it and hands it to one task on the
//! application's runtime, which makes one formula at a time and answers each
//! with its picture, or with none where it stays its text. The drawing thread
//! takes the answers when it next rests, so nothing it does waits on TeX.
//!
//! The queue holds a turn's first [`MOST_PER_TURN`] formulas, in the order
//! they closed. One told past them stays its text. When a turn starts, what
//! an earlier turn told and the task has not begun stays its text too, so a
//! new question is never kept waiting behind an old answer's formulas.
//!
//! The task finds the engine before its first formula with the session's
//! check. A check that decides nothing (the sandbox was busy, say) is made
//! again before the next formula, and only then: at most one is in flight,
//! since the task does one thing at a time, and a formula told while it runs
//! waits behind it in its place. If that check finds the engine, they are
//! all made, in order; if it decides nothing again, they all stay their text
//! and none starts a check of its own, and the next formula told asks again.
//!
//! A formula TeX could not make is remembered by its digest, the last
//! [`MOST_REMEMBERED`] of them, oldest out, and is not run again while it is:
//! a formula that failed once fails again. One that never reached TeX is not
//! remembered, since nothing was learnt of it, so it is tried again when a
//! later answer holds it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError, mpsc};

use crucible_app::math::{COMMAND_TIME, MathPictures, Svg, Unavailable, Unmade};
use crucible_runtime::BoxFuture;
use crucible_sandbox::SandboxService;
use sha2::{Digest, Sha256};
use tokio::runtime::Handle;
use tokio::sync::Notify;

/// The most formulas of one turn that are made into pictures.
pub(crate) const MOST_PER_TURN: usize = 16;

/// The most formulas remembered as ones TeX could not make.
pub(crate) const MOST_REMEMBERED: usize = 256;

/// What finds the engine: the session's check, made again while it decides
/// nothing.
pub(crate) type Check =
    Arc<dyn Fn() -> BoxFuture<'static, Result<Arc<dyn MathPictures>, Unavailable>> + Send + Sync>;

/// The session's check: the installed TeX on `sandbox`, given its
/// [`COMMAND_TIME`] to answer.
#[cfg(not(feature = "bench-math-stand-in"))]
pub(crate) fn checking(sandbox: Arc<dyn SandboxService>) -> Check {
    installed(sandbox)
}

/// The colour-on memory probe's check, in the build only
/// `scripts/sh/bench.sh` asks for with the `bench-math-stand-in` feature: it
/// checks nothing and finds [`stand_in`]'s engine. The installed TeX's check
/// is still built, so the selection a release makes is the one compiled.
#[cfg(feature = "bench-math-stand-in")]
pub(crate) fn checking(sandbox: Arc<dyn SandboxService>) -> Check {
    stand_in::instead(installed(sandbox))
}

/// The check of the TeX installed on `sandbox`.
fn installed(sandbox: Arc<dyn SandboxService>) -> Check {
    Arc::new(move || {
        let sandbox = Arc::clone(&sandbox);
        Box::pin(async move {
            crucible_app::math::available(sandbox, COMMAND_TIME)
                .await
                .map(|engine| -> Arc<dyn MathPictures> { Arc::new(engine) })
        })
    })
}

/// An engine that makes every formula into one picture without TeX or the
/// sandbox, for the colour-on memory probe to keep sixteen of.
///
/// The picture is what `dvisvgm` makes of `display-cases` through this
/// engine's own preamble, wrapper and options: the largest display formula
/// measured for math pictures that draws within the raster's caps, 9,856
/// bytes and about 294 by 100 pixels at 240 dpi.
#[cfg(feature = "bench-math-stand-in")]
mod stand_in {
    use std::sync::Arc;

    use crucible_app::math::{MathPictures, Svg, Unmade};
    use crucible_runtime::BoxFuture;

    use super::Check;

    /// The picture every formula is answered with.
    const PICTURE: &str = include_str!("math_engine/display-cases.svg");

    /// The engine: every formula is the one picture.
    struct StandIn;

    impl MathPictures for StandIn {
        fn toolchain(&self) -> &'static str {
            "stand-in"
        }

        fn make(&self, _formula: &str) -> BoxFuture<'_, Result<Svg, Unmade>> {
            Box::pin(async { Ok(Svg::new(PICTURE.as_bytes())) })
        }
    }

    /// A check that finds the stand-in, in place of `installed`.
    pub(super) fn instead(installed: Check) -> Check {
        drop(installed);
        Arc::new(|| Box::pin(async { Ok(Arc::new(StandIn) as Arc<dyn MathPictures>) }))
    }
}

/// The drawing thread's side: what it tells the task, and what the task
/// answers.
pub(crate) struct MathEngine<T> {
    /// The queue the task takes from.
    shared: Arc<Shared<T>>,
    /// Each formula handed to the task, answered once.
    answers: mpsc::Receiver<(T, Option<Svg>)>,
    /// Formulas handed to the task and not yet answered or dropped.
    owed: usize,
    /// Formulas this turn has queued.
    this_turn: usize,
}

impl<T> std::fmt::Debug for MathEngine<T> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("MathEngine")
            .field("owed", &self.owed)
            .field("this_turn", &self.this_turn)
            .finish_non_exhaustive()
    }
}

/// The queue between the drawing thread and the task.
struct Shared<T> {
    queue: Mutex<Queue<T>>,
    /// Woken when a formula is queued or the drawing side is gone.
    woken: Notify,
}

/// What waits for the task: never more than a turn's formulas.
struct Queue<T> {
    waiting: VecDeque<(T, Box<str>)>,
    /// Whether the drawing side is gone, so the task ends.
    closed: bool,
}

impl<T> Shared<T> {
    fn held(&self) -> std::sync::MutexGuard<'_, Queue<T>> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The next formula, waiting for one; `None` once the drawing side is
    /// gone.
    async fn next(&self) -> Option<(T, Box<str>)> {
        loop {
            let woken = self.woken.notified();
            {
                let mut queue = self.held();
                if queue.closed {
                    return None;
                }
                if let Some(next) = queue.waiting.pop_front() {
                    return Some(next);
                }
            }
            woken.await;
        }
    }

    /// Every formula queued now, in order.
    fn rest(&self) -> Vec<(T, Box<str>)> {
        self.held().waiting.drain(..).collect()
    }
}

impl<T: Send + 'static> MathEngine<T> {
    /// Starts the task on `runtime`, finding the engine with `check`.
    pub(crate) fn start(runtime: &Handle, check: Check) -> Self {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                waiting: VecDeque::new(),
                closed: false,
            }),
            woken: Notify::new(),
        });
        let (answer, answers) = mpsc::channel();
        runtime.spawn(making(Arc::clone(&shared), check, answer));
        Self {
            shared,
            answers,
            owed: 0,
            this_turn: 0,
        }
    }

    /// Queues `source`, answered under `ticket`; past the turn's
    /// [`MOST_PER_TURN`], it stays its text and is not answered.
    pub(crate) fn told(&mut self, ticket: T, source: &str) {
        if self.this_turn >= MOST_PER_TURN {
            return;
        }
        self.this_turn += 1;
        self.owed += 1;
        self.shared
            .held()
            .waiting
            .push_back((ticket, source.into()));
        self.shared.woken.notify_one();
    }

    /// Starts a turn: what an earlier one queued and the task has not begun
    /// stays its text, and is not answered.
    pub(crate) fn turns(&mut self) {
        let dropped = self.shared.rest().len();
        self.owed = self.owed.saturating_sub(dropped);
        self.this_turn = 0;
    }

    /// The next answer, if one has come.
    pub(crate) fn answered(&mut self) -> Option<(T, Option<Svg>)> {
        let answer = self.answers.try_recv().ok()?;
        self.owed = self.owed.saturating_sub(1);
        Some(answer)
    }

    /// Whether a formula queued has not been answered.
    pub(crate) const fn pending(&self) -> bool {
        self.owed > 0
    }
}

impl<T> Drop for MathEngine<T> {
    fn drop(&mut self) {
        self.shared.held().closed = true;
        self.shared.woken.notify_one();
    }
}

/// What the task knows of the engine.
enum Found {
    /// Not yet asked, or asked and decided nothing.
    Unsure,
    /// The engine.
    Engine(Arc<dyn MathPictures>),
    /// Asked, and it cannot be made here.
    Never,
}

/// The task: each formula queued, in order, made or left as text.
async fn making<T>(shared: Arc<Shared<T>>, check: Check, answer: mpsc::Sender<(T, Option<Svg>)>) {
    let mut found = Found::Unsure;
    let mut remembered: VecDeque<[u8; 32]> = VecDeque::new();
    while let Some((ticket, source)) = shared.next().await {
        if matches!(found, Found::Unsure) {
            found = match check().await {
                Ok(engine) => Found::Engine(engine),
                Err(unavailable) if unavailable.decides_nothing() => Found::Unsure,
                Err(_) => Found::Never,
            };
            // What was told while the check ran is settled by it, made in
            // order or left as text with no check of its own.
            if !matches!(found, Found::Engine(_)) {
                let _ = answer.send((ticket, None));
                for (ticket, _) in shared.rest() {
                    let _ = answer.send((ticket, None));
                }
                continue;
            }
        }
        let Found::Engine(engine) = &found else {
            let _ = answer.send((ticket, None));
            continue;
        };
        let digest: [u8; 32] = Sha256::digest(source.as_bytes()).into();
        if remembered.contains(&digest) {
            let _ = answer.send((ticket, None));
            continue;
        }
        let made = match engine.make(&source).await {
            Ok(svg) => Some(svg),
            Err(Unmade::NotReached) => None,
            Err(Unmade::Rejected | Unmade::Limit(_) | Unmade::Empty | Unmade::Timeout) => {
                if remembered.len() >= MOST_REMEMBERED {
                    remembered.pop_front();
                }
                remembered.push_back(digest);
                None
            }
        };
        if answer.send((ticket, made)).is_err() {
            return;
        }
    }
}

impl crucible_tui::graphics::Formulas for MathEngine<crucible_tui::graphics::Formula> {
    fn told(&mut self, formula: crucible_tui::graphics::Formula) {
        let source = formula.source().to_owned();
        MathEngine::told(self, formula, &source);
    }

    fn turns(&mut self) {
        MathEngine::turns(self);
    }

    fn answered(
        &mut self,
    ) -> Option<(
        crucible_tui::graphics::Formula,
        Option<crucible_tui::drawing::Svg>,
    )> {
        let (formula, made) = MathEngine::answered(self)?;
        Some((formula, made.as_ref().and_then(super::pictures::inked)))
    }

    fn pending(&self) -> bool {
        MathEngine::pending(self)
    }
}

#[cfg(test)]
mod tests;
