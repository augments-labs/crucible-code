//! What a mode stepped to or a model picked over a running turn comes to once
//! that turn ends.
//!
//! A key pressed while a turn runs cannot be typed ahead: the keyboard is
//! crossterm's, and a pipe has none. What such a key does is fill a slot on the
//! terms from the drawing thread while the runner is on the worker, and the
//! answer to a question is the one line read then, so [`Meanwhile`] fills the
//! slot as that line is read.

use super::*;

/// Typed-ahead input that does one thing more as the line at `at` is read.
struct Meanwhile<'a> {
    typed: Cursor<Vec<u8>>,
    at: u64,
    pressed: Option<Box<dyn FnOnce() + 'a>>,
}

impl<'a> Meanwhile<'a> {
    fn new(typed: &str, at: &str, pressed: impl FnOnce() + 'a) -> Self {
        let at = typed.find(at).expect("the line the press is made at");
        Self {
            typed: Cursor::new(typed.as_bytes().to_vec()),
            at: u64::try_from(at).expect("an offset into a short line"),
            pressed: Some(Box::new(pressed)),
        }
    }

    fn press(&mut self) {
        if self.typed.position() == self.at
            && let Some(pressed) = self.pressed.take()
        {
            pressed();
        }
    }
}

impl io::Read for Meanwhile<'_> {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        self.press();
        io::Read::read(&mut self.typed, into)
    }
}

impl io::BufRead for Meanwhile<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.press();
        io::BufRead::fill_buf(&mut self.typed)
    }

    fn consume(&mut self, amount: usize) {
        io::BufRead::consume(&mut self.typed, amount);
    }
}

#[test]
fn a_mode_stepped_while_a_turn_runs_is_the_one_in_force_once_it_ends() {
    // Two steps over a running turn reach fullAccess, and the prompt after it
    // says so. A mode set there is then the one the next turn runs under: a
    // command in it is asked about, where a step still held would have let it
    // run under a mode the screen no longer showed. `/mode` stands in for the
    // key between turns, which a pipe cannot press either.
    let offered = {
        let mut offered = Tools::new();
        offered
            .add_builtin(Fixed::new("write", changing()))
            .expect("write registers");
        offered
            .add_builtin(Fixed::new("bash", running("ls")))
            .expect("bash registers");
        offered
    };
    let conversation = paired(Arc::new(Session::nowhere()), |session| {
        scripted(
            Script::new(vec![
                calling("write"),
                saying("changed it"),
                calling("bash"),
                saying("listed it"),
            ]),
            offered,
            session,
        )
    });
    let terms = plain();
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Meanwhile::new("edit it\ny\n/mode allowEdits\nlist it\nn\n", "y\n", || {
        terms.pending_mode.set(Some(Mode::FullAccess));
    });

    converse(
        conversation,
        &mut renderer,
        &terms,
        First {
            card: &opening(),
            arming: None,
        },
        &mut input,
    )
    .expect("the loop to finish");

    let written = renderer.terminal().written();
    let stepped = written
        .find("fullAccess › ")
        .expect("the step, once the turn ended");
    let named = written
        .find("allowEdits › ")
        .expect("the mode named after it");
    assert!(stepped < named, "{written}");
    assert!(written.contains("bash wants to run: ls"), "{written}");
}

#[test]
fn a_model_picked_while_a_turn_runs_is_the_one_in_force_once_it_ends() {
    // Taken as the turn ends rather than when another starts, so a session that
    // takes no other turn still switches, says so, and writes the pick down
    // for the next run.
    let sample = Sample::new("model-held");
    let terms = keeping(&sample);
    let anthropic = crucible_app::providers::offered(&terms.providers.snapshot())
        .find(|served| served.name == "anthropic")
        .expect("anthropic is offered");
    let conversation = paired(Arc::new(Session::nowhere()), |session| {
        scripted(
            Script::new(vec![calling("write"), saying("changed it")]),
            tools(Fixed::new("write", changing())),
            session,
        )
    });
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let mut input = Meanwhile::new("edit it\ny\n", "y\n", || {
        terms
            .pending_model
            .set(Some((anthropic, "claude-haiku-4-5".into())));
    });

    converse(
        conversation,
        &mut renderer,
        &terms,
        First {
            card: &opening(),
            arming: None,
        },
        &mut input,
    )
    .expect("the loop to finish");

    let written = renderer.terminal().written();
    assert!(
        written.contains("anthropic · claude-haiku-4-5"),
        "{written}"
    );

    let held = std::fs::read_to_string(sample.user_file()).expect("the file it said it wrote");
    assert!(held.contains("\"model\": \"claude-haiku-4-5\""), "{held}");
}
