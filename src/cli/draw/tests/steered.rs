//! What a line steered into a running turn leaves of that turn's formulas.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crucible_app::math::{MathPictures, Svg, Unmade};
use crucible_runtime::BoxFuture;
use crucible_tui::drawing::Mode;
use crucible_tui::graphics::{Capability, Cell, Protocol};
use crucible_tui::{Palette, ScreenMode, Theme};
use tokio::sync::Semaphore;

use super::*;
use crate::cli::math_engine::{Check, MOST_PER_TURN, MathEngine};
use crate::cli::pictures::Raster;

/// A stand-in engine that says what it was asked to make, in order.
#[derive(Default)]
struct Making(Mutex<Vec<String>>);

impl Making {
    fn made(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

impl MathPictures for Making {
    fn toolchain(&self) -> &'static str {
        "stand-in"
    }

    fn make(&self, formula: &str) -> BoxFuture<'_, Result<Svg, Unmade>> {
        self.0.lock().unwrap().push(formula.trim().to_owned());
        Box::pin(async { Ok(Svg::new(&b"<svg/>"[..])) })
    }
}

/// A full screen on a Kitty terminal whose display formulas go to `engine`.
fn math_session(engine: MathEngine<crucible_tui::graphics::Formula>) -> Renderer<Recording> {
    let truecolour = |name: &str| (name == "COLORTERM").then(|| "truecolor".to_owned());
    let mut renderer = Renderer::drawing(Recording::new(80, 40), ScreenMode::Fullscreen);
    renderer.drawers(drawers());
    renderer.pictures(Box::new(Raster));
    renderer.wears(Palette::resolve(true, Theme::Dark, None, &truecolour));
    renderer.images(Capability::Graphics {
        protocol: Protocol::Kitty,
        cell: Cell {
            width: 8,
            height: 16,
        },
    });
    renderer.math(Mode::Image);
    renderer.makes_formulas(Box::new(engine));
    renderer
}

/// The `n`th formula of the turn.
fn formula(n: usize) -> String {
    format!("x_{{{n}}}")
}

/// Streams the turn's formulas `from` to `to` in one answer, and settles it.
fn answers(renderer: &mut Renderer<Recording>, from: usize, to: usize) {
    for n in from..=to {
        renderer
            .stream(&format!("$$\n{}\n$$\n\n", formula(n)))
            .unwrap();
    }
    renderer.settle().unwrap();
}

#[test]
fn a_line_steered_into_a_turn_keeps_its_formulas_queued_and_its_sixteen_counted() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let making = Arc::new(Making::default());
    // The check waits until the test lets it through, so every formula the
    // turn tells before then waits in the queue.
    let gate = Arc::new(Semaphore::new(0));
    let check: Check = {
        let gate = Arc::clone(&gate);
        let engine: Arc<dyn MathPictures> = making.clone();
        Arc::new(move || {
            let gate = Arc::clone(&gate);
            let engine = Arc::clone(&engine);
            Box::pin(async move {
                gate.acquire().await.unwrap().forget();
                Ok(engine)
            })
        })
    };
    let mut renderer = math_session(MathEngine::start(runtime.handle(), check));
    let style = Style::plain();

    queued(&mut renderer, "first", style).unwrap();
    answers(&mut renderer, 1, 10);
    event(
        &mut renderer,
        Event::Steered {
            line: "and then".into(),
        },
        &here(),
        style,
        &mut Kept::default(),
    )
    .unwrap();
    answers(&mut renderer, 11, 17);
    gate.add_permits(1);

    let since = Instant::now();
    while making.made().len() < MOST_PER_TURN {
        assert!(
            since.elapsed() < Duration::from_secs(5),
            "made only {:?}",
            making.made()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    std::thread::sleep(Duration::from_millis(50));
    let every: Vec<String> = (1..=MOST_PER_TURN).map(formula).collect();
    assert_eq!(making.made(), every, "the seventeenth stays its text");
}
