use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crucible_sandbox::SandboxViolation;
use tokio::sync::Semaphore;

use super::*;

/// How long a test waits for the task before it calls an answer lost.
const PATIENCE: Duration = Duration::from_secs(5);

/// A stand-in engine: what it was asked to make, in order, and what it says
/// for a formula, a picture of the formula's own bytes unless told otherwise.
#[derive(Default)]
struct StandIn {
    made: Mutex<Vec<String>>,
    says: Mutex<HashMap<String, Unmade>>,
}

impl StandIn {
    fn made(&self) -> Vec<String> {
        self.made.lock().unwrap().clone()
    }

    fn fails(&self, formula: &str, unmade: Unmade) {
        self.says.lock().unwrap().insert(formula.to_owned(), unmade);
    }
}

impl MathPictures for StandIn {
    fn toolchain(&self) -> &'static str {
        "stand-in"
    }

    fn make(&self, formula: &str) -> BoxFuture<'_, Result<Svg, Unmade>> {
        self.made.lock().unwrap().push(formula.to_owned());
        let said = self.says.lock().unwrap().get(formula).copied();
        let formula = formula.to_owned();
        Box::pin(async move {
            match said {
                Some(unmade) => Err(unmade),
                None => Ok(Svg::new(formula.into_bytes())),
            }
        })
    }
}

/// A check the test settles: each one made waits for the gate, then says
/// what `says` holds, counted as it starts.
struct Checking {
    gate: Arc<Semaphore>,
    says: Arc<Mutex<Result<(), Unavailable>>>,
    started: Arc<AtomicUsize>,
    engine: Arc<StandIn>,
}

impl Checking {
    /// A check that finds `engine` as soon as it is made.
    fn open(engine: &Arc<StandIn>) -> Self {
        let checking = Self::shut(engine);
        checking.gate.add_permits(1 << 20);
        checking
    }

    /// A check that waits for [`Checking::lets_one`].
    fn shut(engine: &Arc<StandIn>) -> Self {
        Self {
            gate: Arc::new(Semaphore::new(0)),
            says: Arc::new(Mutex::new(Ok(()))),
            started: Arc::new(AtomicUsize::new(0)),
            engine: Arc::clone(engine),
        }
    }

    fn lets_one(&self) {
        self.gate.add_permits(1);
    }

    fn says(&self, said: Result<(), Unavailable>) {
        *self.says.lock().unwrap() = said;
    }

    fn started(&self) -> usize {
        self.started.load(Ordering::SeqCst)
    }

    fn check(&self) -> Check {
        let gate = Arc::clone(&self.gate);
        let says = Arc::clone(&self.says);
        let started = Arc::clone(&self.started);
        let engine = Arc::clone(&self.engine);
        Arc::new(move || {
            started.fetch_add(1, Ordering::SeqCst);
            let gate = Arc::clone(&gate);
            let says = Arc::clone(&says);
            let engine: Arc<dyn MathPictures> = engine.clone();
            Box::pin(async move {
                gate.acquire().await.unwrap().forget();
                let said = *says.lock().unwrap();
                said.map(|()| engine)
            })
        })
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap()
}

/// Every answer until `count` have come, as each ticket and whether it
/// came with a picture.
fn answers(engine: &mut MathEngine<u32>, count: usize) -> Vec<(u32, Option<String>)> {
    let since = Instant::now();
    let mut answers = Vec::new();
    while answers.len() < count {
        assert!(since.elapsed() < PATIENCE, "answered only {answers:?}");
        match engine.answered() {
            Some((ticket, made)) => answers.push((
                ticket,
                made.map(|svg| String::from_utf8(svg.bytes().to_vec()).unwrap()),
            )),
            None => std::thread::sleep(Duration::from_millis(1)),
        }
    }
    answers
}

/// Waits for `holds` to come true.
fn until(holds: impl Fn() -> bool) {
    let since = Instant::now();
    while !holds() {
        assert!(since.elapsed() < PATIENCE, "never came");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Lets the task run a while, for a test asserting nothing more comes.
fn settle() {
    std::thread::sleep(Duration::from_millis(50));
}

#[test]
fn two_formulas_told_while_a_check_runs_start_one_check_and_are_made_in_order_once_it_passes() {
    let runtime = runtime();
    let stand_in = Arc::new(StandIn::default());
    let checking = Checking::shut(&stand_in);
    let mut engine = MathEngine::start(runtime.handle(), checking.check());

    engine.told(1, "a");
    until(|| checking.started() == 1);
    engine.told(2, "b");
    engine.told(3, "c");
    settle();
    assert_eq!(checking.started(), 1, "one check is in flight");
    checking.lets_one();

    assert_eq!(
        answers(&mut engine, 3),
        [
            (1, Some("a".to_owned())),
            (2, Some("b".to_owned())),
            (3, Some("c".to_owned()))
        ]
    );
    assert_eq!(stand_in.made(), ["a", "b", "c"]);
    assert_eq!(checking.started(), 1);
    assert!(!engine.pending());
}

#[test]
fn formulas_told_while_a_check_that_decides_nothing_runs_stay_text_and_the_next_asks_again() {
    let runtime = runtime();
    let stand_in = Arc::new(StandIn::default());
    let checking = Checking::shut(&stand_in);
    checking.says(Err(Unavailable::NotReached));
    let mut engine = MathEngine::start(runtime.handle(), checking.check());

    engine.told(1, "a");
    until(|| checking.started() == 1);
    engine.told(2, "b");
    checking.lets_one();

    assert_eq!(answers(&mut engine, 2), [(1, None), (2, None)]);
    settle();
    assert_eq!(checking.started(), 1, "neither started a check of its own");
    assert!(stand_in.made().is_empty());
    assert!(!engine.pending());

    // Each kept its place under the turn's cap: fourteen more are made, and
    // the one past them is not.
    checking.says(Ok(()));
    checking.lets_one();
    for ticket in 3..=17 {
        engine.told(ticket, &format!("f{ticket}"));
    }
    let made = answers(&mut engine, 14);
    assert_eq!(made.first(), Some(&(3, Some("f3".to_owned()))));
    assert_eq!(made.last(), Some(&(16, Some("f16".to_owned()))));
    assert_eq!(checking.started(), 2, "the next formula asked again");
    settle();
    assert!(engine.answered().is_none());
    assert!(!engine.pending());
}

#[test]
fn a_check_that_finds_no_engine_leaves_every_formula_text_and_is_not_made_again() {
    let runtime = runtime();
    let stand_in = Arc::new(StandIn::default());
    let checking = Checking::open(&stand_in);
    checking.says(Err(Unavailable::ProgramMissing));
    let mut engine = MathEngine::start(runtime.handle(), checking.check());

    engine.told(1, "a");
    assert_eq!(answers(&mut engine, 1), [(1, None)]);
    engine.told(2, "b");
    assert_eq!(answers(&mut engine, 1), [(2, None)]);
    assert_eq!(checking.started(), 1);
    assert!(stand_in.made().is_empty());
}

#[test]
fn when_a_turn_starts_an_earlier_turns_unstarted_formulas_stay_text() {
    let runtime = runtime();
    let stand_in = Arc::new(StandIn::default());
    let checking = Checking::shut(&stand_in);
    let mut engine = MathEngine::start(runtime.handle(), checking.check());

    engine.told(1, "a");
    until(|| checking.started() == 1);
    engine.told(2, "b");
    engine.told(3, "c");
    engine.turns();
    engine.told(4, "d");
    checking.lets_one();

    assert_eq!(
        answers(&mut engine, 2),
        [(1, Some("a".to_owned())), (4, Some("d".to_owned()))]
    );
    settle();
    assert!(engine.answered().is_none(), "b and c are never answered");
    assert!(!engine.pending());
    assert_eq!(stand_in.made(), ["a", "d"]);
}

#[test]
fn in_a_turn_of_seventeen_formulas_the_seventeenth_stays_text() {
    let runtime = runtime();
    let stand_in = Arc::new(StandIn::default());
    let checking = Checking::open(&stand_in);
    let mut engine = MathEngine::start(runtime.handle(), checking.check());

    for ticket in 1..=17 {
        engine.told(ticket, &format!("f{ticket}"));
    }
    let made = answers(&mut engine, MOST_PER_TURN);
    assert!(made.iter().all(|(_, svg)| svg.is_some()));
    settle();
    assert!(engine.answered().is_none());
    assert!(!engine.pending());
    assert_eq!(stand_in.made().len(), MOST_PER_TURN);
    assert!(!stand_in.made().contains(&"f17".to_owned()));

    // The next turn is given its own sixteen.
    engine.turns();
    engine.told(18, "f18");
    assert_eq!(answers(&mut engine, 1), [(18, Some("f18".to_owned()))]);
}

#[test]
fn a_formula_tex_could_not_make_is_not_run_again_and_one_that_never_reached_it_is() {
    let remembered = [
        Unmade::Rejected,
        Unmade::Limit(SandboxViolation::CommandTime),
        Unmade::Limit(SandboxViolation::Output),
        Unmade::Empty,
        Unmade::Timeout,
    ];
    let runtime = runtime();
    let stand_in = Arc::new(StandIn::default());
    let checking = Checking::open(&stand_in);
    let mut engine = MathEngine::start(runtime.handle(), checking.check());

    for (at, unmade) in remembered.iter().enumerate() {
        let formula = format!("bad{at}");
        stand_in.fails(&formula, *unmade);
        engine.told(1, &formula);
        engine.told(2, &formula);
        assert_eq!(answers(&mut engine, 2), [(1, None), (2, None)], "{unmade:?}");
        assert_eq!(
            stand_in.made().iter().filter(|made| **made == formula).count(),
            1,
            "{unmade:?} is remembered"
        );
        engine.turns();
    }

    stand_in.fails("busy", Unmade::NotReached);
    engine.told(1, "busy");
    engine.told(2, "busy");
    engine.told(3, "next");
    assert_eq!(
        answers(&mut engine, 3),
        [(1, None), (2, None), (3, Some("next".to_owned()))]
    );
    assert_eq!(
        stand_in.made().iter().filter(|made| *made == "busy").count(),
        2,
        "one that never reached TeX is run again"
    );
}

#[test]
fn the_failure_memory_drops_its_oldest_formula_first() {
    let runtime = runtime();
    let stand_in = Arc::new(StandIn::default());
    let checking = Checking::open(&stand_in);
    let mut engine = MathEngine::start(runtime.handle(), checking.check());
    let failing = |at: usize| format!("bad{at}");

    // One more than the memory holds, a turn's worth at a time.
    for at in 0..=MOST_REMEMBERED {
        stand_in.fails(&failing(at), Unmade::Rejected);
    }
    let all: Vec<usize> = (0..=MOST_REMEMBERED).collect();
    for turn in all.chunks(MOST_PER_TURN) {
        for &at in turn {
            engine.told(1, &failing(at));
        }
        answers(&mut engine, turn.len());
        engine.turns();
    }
    let runs = |formula: &str| stand_in.made().iter().filter(|made| *made == formula).count();

    engine.told(1, &failing(1));
    engine.told(2, &failing(0));
    answers(&mut engine, 2);
    assert_eq!(runs(&failing(1)), 1, "the second oldest is still remembered");
    assert_eq!(runs(&failing(0)), 2, "the oldest was dropped, so it ran again");
}

#[test]
fn the_task_ends_when_the_drawing_side_is_gone() {
    let runtime = runtime();
    let stand_in = Arc::new(StandIn::default());
    let checking = Checking::open(&stand_in);
    let engine: MathEngine<u32> = MathEngine::start(runtime.handle(), checking.check());
    let shared = Arc::downgrade(&engine.shared);

    drop(engine);
    until(|| shared.strong_count() == 0);
}
