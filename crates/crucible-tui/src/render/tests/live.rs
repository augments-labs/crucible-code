//! The region a prompt is typed into: drawn where it stands, redrawn as it
//! changes, and taken off the screen whole.

use super::rewind;
use crate::color::Palette;
use crate::render::{Caret, Renderer};
use crate::row::Row;
use crate::terminal::Recording;

/// A prompt-shaped region and where its cursor goes: two rows, typed on the
/// first, so one row is left parked below the cursor.
fn region() -> (Vec<Row>, Caret) {
    let rows = vec![Row::plain("› ask"), Row::plain("ask before edits")];
    (rows, Caret { row: 0, column: 6 })
}

#[test]
fn a_live_region_is_redrawn_from_its_own_top_row() {
    // The cursor is parked on the first of two rows, so the way back up is no
    // rows at all. Rewinding by the height of the region would reach over a
    // row the last frame left below the cursor.
    let mut render = Renderer::new(Recording::new(80, 24));
    let (rows, caret) = region();
    render.live(&rows, caret, Palette::plain()).unwrap();
    render.terminal.take();

    render.live(&rows, caret, Palette::plain()).unwrap();

    assert!(
        render.terminal.written().starts_with(&rewind(1)),
        "{:?}",
        render.terminal.written()
    );
}

#[test]
fn a_live_region_never_lands_on_top_of_what_was_streamed() {
    // What the last turn left live belongs above the prompt, and it has to be
    // written down before the prompt reaches over it.
    let mut render = Renderer::new(Recording::new(80, 24));
    render.stream("answer").unwrap();
    render.terminal.take();

    let (rows, caret) = region();
    render.live(&rows, caret, Palette::plain()).unwrap();

    let written = render.terminal.written();
    assert!(written.contains("answer\r\n"), "{written:?}");
}

#[test]
fn ending_a_live_region_takes_every_row_of_it_off_the_screen() {
    // Including the rows below the cursor, which the region's own arithmetic
    // is what keeps track of.
    let mut render = Renderer::new(Recording::new(80, 24));
    let (rows, caret) = region();
    render.live(&rows, caret, Palette::plain()).unwrap();
    render.terminal.take();

    render.settle().unwrap();

    assert_eq!(render.terminal.written(), rewind(1));
    assert_eq!(render.drawn, 0);
    assert_eq!(render.parked, 0);
}

#[test]
fn a_redirected_run_draws_no_live_region_at_all() {
    // There is no cursor to park in a pipe, and the escapes that would park
    // one end up in whatever kept the output.
    let mut render = Renderer::new(Recording::redirected(80, 24));
    let (rows, caret) = region();

    render.live(&rows, caret, Palette::plain()).unwrap();

    assert_eq!(render.terminal.written(), "");
    assert_eq!(render.terminal.flushes(), 0);
}
