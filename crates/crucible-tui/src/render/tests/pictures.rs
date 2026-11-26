//! Diagrams drawn as Kitty pictures: sent once and placed by their cells,
//! sent again where the terminal may have lost them, in chunks a frame can
//! carry, wrapped for tmux where it passes them through, and never where no
//! colour is written or nobody is looking.

use super::*;

/// A diagram drawer that lays a `mermaid` fence as its lines and draws it as
/// a picture of them.
pub(in crate::render) struct Sketched;

impl crate::drawing::Draws for Sketched {
    fn names(&self) -> &'static [&'static str] {
        &["mermaid"]
    }

    fn block(
        &self,
        source: &str,
        _: &crate::drawing::Room,
    ) -> Result<crate::drawing::Laid, crate::chrome::Undrawn> {
        let rows: Vec<Row> = source.lines().map(Row::plain).collect();
        Ok(crate::drawing::Laid {
            natural: (8, rows.len()),
            rows,
            narrow: None,
        })
    }

    fn picture(
        &self,
        source: &str,
        _: &crate::drawing::Room,
    ) -> Result<crate::drawing::Svg, crate::chrome::Undrawn> {
        crate::drawing::Svg::new(format!("<svg>{}</svg>", source.trim()))
    }
}

/// Pictures `across` by `down` cells, each `bytes` bytes that count up from
/// how many were made before it, so two pictures never carry the same bytes.
pub(in crate::render) struct Printed {
    across: u16,
    down: u16,
    bytes: usize,
    made: u8,
}

impl Printed {
    pub(in crate::render) fn new(across: u16, down: u16, bytes: usize) -> Self {
        Self {
            across,
            down,
            bytes,
            made: 0,
        }
    }

    fn payload(&mut self, key: &graphics::PictureKey) -> graphics::Payload {
        let start = self.made;
        self.made = self.made.wrapping_add(1);
        let bytes = (0..self.bytes)
            .map(|at| start.wrapping_add(u8::try_from(at % 251).unwrap_or(0)))
            .collect();
        graphics::Payload::new(
            key.protocol,
            bytes,
            (
                u32::from(self.across) * u32::from(key.cell.width),
                u32::from(self.down) * u32::from(key.cell.height),
            ),
            key.cell,
        )
    }
}

impl graphics::Pictures for Printed {
    fn cached(
        &mut self,
        _: &str,
        _: &graphics::PictureKey,
    ) -> Option<(graphics::Form, graphics::Payload)> {
        None
    }

    fn make(
        &mut self,
        _: &str,
        key: &graphics::PictureKey,
        _: &graphics::Form,
    ) -> Result<graphics::Payload, crate::chrome::Undrawn> {
        Ok(self.payload(key))
    }

    fn reencode(
        &mut self,
        _: &crate::KeptPicture,
        key: &graphics::PictureKey,
    ) -> Result<graphics::Payload, crate::chrome::Undrawn> {
        Ok(self.payload(key))
    }
}

/// The cell every picture test's window draws in.
pub(in crate::render) const PICTURE_CELL: (u16, u16) = (8, 16);

/// A diagram, in an answer.
pub(in crate::render) const DIAGRAM: &str = "```mermaid\ngraph\n```\n";

/// `render` on a terminal that confirmed Kitty, in colour, drawing diagrams
/// with [`Sketched`] and pictures with `printed`, numbering them from one.
pub(in crate::render) fn pictured_by<T: Terminal>(render: &mut Renderer<T>, printed: Printed) {
    render.wears(colourful());
    render.images(kitty_in(PICTURE_CELL.0, PICTURE_CELL.1));
    render.drawers(crate::drawing::Drawers::default().with(std::sync::Arc::new(Sketched)));
    render.pictures(Box::new(printed));
    render.record.numbers_from(1);
}

/// [`pictured_by`] the renderer `drawn` tests.
fn pictured_on(drawn: &mut Drawn, printed: Printed) {
    pictured_by(&mut drawn.render, printed);
}

/// [`pictured_on`] a full screen this size.
fn pictured(columns: usize, rows: usize, printed: Printed) -> Drawn {
    let mut drawn = Drawn::new(columns, rows);
    pictured_on(&mut drawn, printed);
    drawn
}

/// Every graphics command in `written`, as what is between its `G` and its
/// terminator.
pub(in crate::render) fn commands(written: &str) -> Vec<&str> {
    written
        .split("\x1b_G")
        .skip(1)
        .filter_map(|after| after.split_once("\x1b\\").map(|(command, _)| command))
        .collect()
}

/// How many bytes of graphics commands `written` holds, wrappers included.
pub(in crate::render) fn graphics_bytes(written: &str) -> usize {
    written
        .split("\x1b_G")
        .skip(1)
        .filter_map(|after| after.find("\x1b\\"))
        .map(|end| "\x1b_G".len() + end + "\x1b\\".len())
        .sum()
}

/// Each picture `commands` transmitted whole, by number, decoded.
pub(in crate::render) fn transmitted(commands: &[&str]) -> Vec<(u32, Vec<u8>)> {
    use base64::Engine as _;

    let mut whole = Vec::new();
    let mut loading: Option<(u32, Vec<u8>)> = None;
    for command in commands {
        let (keys, data) = command.split_once(';').unwrap_or((command, ""));
        let keys: Vec<(&str, &str)> = keys
            .split(',')
            .filter_map(|pair| pair.split_once('='))
            .collect();
        let key = |name: &str| {
            keys.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| *value)
        };
        if key("a") == Some("t") {
            let id = key("i").and_then(|id| id.parse().ok()).unwrap_or(0);
            loading = Some((id, Vec::new()));
        } else if key("a").is_some() {
            continue;
        }
        if let Some((_, bytes)) = loading.as_mut() {
            bytes.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap_or_default(),
            );
        }
        if key("m") == Some("0") {
            whole.extend(loading.take());
        }
    }
    whole
}

/// The cells of row `row` of picture `id`, `across` cells wide, as a frame
/// paints them.
pub(in crate::render) fn placeholders(id: u32, row: usize, across: usize) -> String {
    let [_, red, green, blue] = id.to_be_bytes();
    let mark = |at: usize| graphics::DIACRITICS.get(at).copied().unwrap();
    let row = mark(row);
    let first = mark(0);
    let rest = "\u{10EEEE}".repeat(across - 1);
    format!("\x1b[38;2;{red};{green};{blue}m\u{10EEEE}{row}{first}{rest}\x1b[39m")
}

/// The bytes [`Printed`] gives the `made`th picture it makes.
pub(in crate::render) fn printed_bytes(made: u8, bytes: usize) -> Vec<u8> {
    (0..bytes)
        .map(|at| made.wrapping_add(u8::try_from(at % 251).unwrap_or(0)))
        .collect()
}

#[test]
fn a_diagram_on_a_kitty_terminal_is_sent_once_and_placed_in_the_cells_it_covers() {
    use base64::Engine as _;

    let mut drawn = pictured(40, 12, Printed::new(3, 2, 300));
    drawn.answer(DIAGRAM);
    let written = drawn.take();

    let encoded = base64::engine::general_purpose::STANDARD.encode(printed_bytes(0, 300));
    assert_eq!(
        commands(&written),
        [
            format!("a=t,f=100,i=1,q=2,m=0;{encoded}").as_str(),
            "a=p,U=1,i=1,c=3,r=2,q=2",
        ],
        "{written:?}"
    );
    assert!(written.contains(&placeholders(1, 0, 3)), "{written:?}");
    assert!(written.contains(&placeholders(1, 1, 3)), "{written:?}");

    // Held by the terminal from then on: the frames after it send nothing.
    drawn.commit("after it").unwrap();
    assert!(commands(&drawn.take()).is_empty());
}

#[test]
fn a_picture_larger_than_a_frame_may_send_goes_out_in_chunks_over_the_frames_after_it() {
    let bytes = 200 * 1024;
    let mut drawn = pictured(40, 12, Printed::new(3, 2, bytes));
    drawn.answer(DIAGRAM);
    let mut written = drawn.take();
    assert!(
        drawn.rests_in() == Some(Duration::ZERO),
        "a picture still being sent did not wake the next wait"
    );
    for _ in 0..64 {
        if drawn.rests_in().is_none() {
            break;
        }
        drawn.render.repose().unwrap();
        written.push_str(&drawn.take());
    }
    assert_eq!(drawn.rests_in(), None, "the picture was never sent whole");

    for frame in written.split(frame::BEGIN_SYNC).skip(1) {
        assert!(
            graphics_bytes(frame) <= 64 * 1024,
            "a frame sent {} bytes of pictures",
            graphics_bytes(frame)
        );
    }
    let commands = commands(&written);
    for command in &commands {
        let data = command.split_once(';').map_or("", |(_, data)| data);
        assert!(data.len() <= 4096, "a chunk of {} bytes", data.len());
    }
    let [first, middle @ .., last, placement] = commands.as_slice() else {
        panic!("{} commands", commands.len());
    };
    assert!(first.starts_with("a=t,f=100,i=1,q=2,m=1;"));
    for command in middle {
        assert!(command.starts_with("m=1,q=2;"), "{command:.32}");
    }
    assert!(last.starts_with("m=0,q=2;"));
    assert_eq!(*placement, "a=p,U=1,i=1,c=3,r=2,q=2");
    assert_eq!(
        transmitted(&commands),
        [(1, printed_bytes(0, bytes))],
        "the picture did not arrive whole"
    );
}

#[test]
fn a_picture_moves_with_the_transcript_and_is_not_sent_again_for_it() {
    // Below a few lines, so that two more scroll it up whole.
    let mut drawn = pictured(40, 12, Printed::new(3, 2, 300));
    for line in 0..4 {
        drawn.commit(&format!("before {line}")).unwrap();
    }
    drawn.answer(DIAGRAM);
    let first = drawn.take();
    let at = |frame: &str| {
        frame
            .find(&placeholders(1, 0, 3))
            .and_then(|end| frame[..end].rfind("\x1b["))
            .map(|start| frame[start..].split_once('H').map(|(at, _)| at.to_owned()))
    };
    let was = at(&first).flatten();
    assert!(was.is_some(), "{first:?}");

    drawn.commit("one line under it").unwrap();
    drawn.commit("and another").unwrap();
    for line in 0..6 {
        drawn.commit(&format!("line {line}")).unwrap();
    }
    let moved = drawn.take();
    let now = at(&moved).flatten();
    assert!(
        now.is_some(),
        "the picture was not drawn where it went: {moved:?}"
    );
    assert_ne!(was, now, "the picture stayed where it was");
    assert!(
        commands(&moved)
            .iter()
            .all(|command| !command.starts_with("a=t"))
    );
}

#[test]
fn a_picture_scrolled_back_into_view_is_sent_where_the_terminal_lost_it() {
    // A frame sends what it shows: a picture the terminal lost while it was
    // out of view is sent again when it is scrolled back to, not before.
    let mut drawn = pictured(40, 6, Printed::new(3, 2, 300));
    drawn.answer(DIAGRAM);
    for line in 0..12 {
        drawn.commit(&format!("line {line}")).unwrap();
    }
    drawn.take();

    drawn.repaint().unwrap();
    assert!(
        commands(&drawn.take()).is_empty(),
        "a picture out of view was sent"
    );

    drawn.scrolled(-40).unwrap();
    let back = drawn.take();
    assert!(back.contains(&placeholders(1, 0, 3)), "{back:?}");
    assert_eq!(transmitted(&commands(&back)), [(1, printed_bytes(0, 300))]);
}

#[test]
fn a_repaint_sends_every_picture_on_screen_again() {
    // What the frame after a child had the terminal, or after the alternate
    // screen was given back and taken again, draws: the terminal kept no
    // picture this process can trust.
    let mut drawn = pictured(40, 12, Printed::new(3, 2, 300));
    drawn.answer(DIAGRAM);
    drawn.take();

    drawn.repaint().unwrap();
    let again = drawn.take();
    assert_eq!(transmitted(&commands(&again)), [(1, printed_bytes(0, 300))]);
    assert!(commands(&again).contains(&"a=p,U=1,i=1,c=3,r=2,q=2"));
    assert!(again.contains(&placeholders(1, 0, 3)), "{again:?}");
}

#[test]
fn a_resize_places_the_picture_again_at_the_new_width() {
    let mut drawn = pictured(40, 12, Printed::new(3, 2, 300));
    drawn.answer(DIAGRAM);
    let before = drawn.take();

    drawn.resize(100, 12);
    let after = drawn.take();

    // A width in another bucket is a new key: encoded again, numbered
    // again, sent and placed under its new number.
    assert_eq!(transmitted(&commands(&after)), [(2, printed_bytes(1, 300))]);
    assert!(commands(&after).contains(&"a=p,U=1,i=2,c=3,r=2,q=2"));
    assert!(after.contains(&placeholders(2, 0, 3)), "{after:?}");
    assert!(before.contains(&placeholders(1, 0, 3)));
}

#[test]
fn a_theme_changed_draws_the_picture_again_in_its_colours() {
    let mut drawn = pictured(40, 12, Printed::new(3, 2, 300));
    drawn.answer(DIAGRAM);
    drawn.take();

    drawn
        .render
        .wears(Palette::resolve(true, Theme::Light, None, &|name| {
            (name == "COLORTERM").then(|| "truecolor".to_owned())
        }));
    drawn.commit("after the change").unwrap();
    let after = drawn.take();

    assert_eq!(transmitted(&commands(&after)), [(2, printed_bytes(1, 300))]);
    assert!(after.contains(&placeholders(2, 0, 3)), "{after:?}");
    assert!(!after.contains(&placeholders(1, 0, 3)), "{after:?}");
}

#[test]
fn inside_tmux_passing_sequences_through_a_command_is_wrapped_for_it() {
    let mut drawn = Drawn::new(40, 12);
    drawn.render.passes_through(graphics::Passthrough::On);
    pictured_on(&mut drawn, Printed::new(3, 2, 30));
    drawn.answer(DIAGRAM);
    let written = drawn.take();

    assert!(
        written.contains("\x1bPtmux;\x1b\x1b_Ga=t,f=100,i=1,q=2,m=0;"),
        "{written:?}"
    );
    assert!(
        written.contains("\x1bPtmux;\x1b\x1b_Ga=p,U=1,i=1,c=3,r=2,q=2\x1b\x1b\\\x1b\\"),
        "{written:?}"
    );
    // The cells are text, which tmux draws itself.
    assert!(written.contains(&placeholders(1, 0, 3)), "{written:?}");
}

#[test]
fn no_picture_is_drawn_where_colour_is_not_written() {
    // `NO_COLOR` and `output.color never` both leave the palette writing
    // none; a terminal that confirmed Kitty is drawn text all the same.
    for palette in [
        Palette::resolve(false, Theme::Dark, None, &|_| None),
        Palette::plain(),
    ] {
        let mut drawn = pictured(40, 12, Printed::new(3, 2, 300));
        drawn.render.wears(palette);
        drawn.answer(DIAGRAM);
        let rows = drawn.rows();
        let written = drawn.take();

        assert!(!written.contains("\x1b_G"), "{written:?}");
        assert!(!written.contains('\u{10EEEE}'), "{written:?}");
        assert!(rows.iter().any(|row| row.contains("graph")), "{rows:#?}");
    }
}

#[test]
fn no_picture_is_drawn_where_output_is_redirected() {
    let mut drawn = Drawn {
        render: Renderer::new(Recording::redirected(40, 12)),
    };
    pictured_on(&mut drawn, Printed::new(3, 2, 300));
    drawn.answer(DIAGRAM);
    let written = drawn.take();

    assert!(!written.contains("\x1b_G"), "{written:?}");
    assert!(!written.contains('\u{10EEEE}'), "{written:?}");
}

#[test]
fn diagrams_drawn_as_text_or_as_written_never_draw_a_picture() {
    for mode in [crate::drawing::Mode::Text, crate::drawing::Mode::Written] {
        let mut drawn = pictured(40, 12, Printed::new(3, 2, 300));
        drawn.render.diagrams(mode);
        drawn.answer(DIAGRAM);
        let written = drawn.take();

        assert!(!written.contains("\x1b_G"), "{mode:?}: {written:?}");
        assert!(!written.contains('\u{10EEEE}'), "{mode:?}: {written:?}");
    }
}

#[test]
fn a_placeholder_a_model_wrote_never_reaches_the_terminal_beside_a_picture() {
    // The only placeholder cells written are the picture's own: the model's
    // are taken out with their marks, and its graphics command is a sequence
    // like any other.
    let hostile = "a \u{10EEEE}\u{0305}\u{030D} b \u{10EEEE} c \
                   \x1b_Ga=p,U=1,i=1,c=40,r=10\x1b\\ d\n\n";
    let mut drawn = pictured(40, 12, Printed::new(3, 2, 300));
    drawn.answer(&format!("{hostile}{DIAGRAM}{hostile}"));
    let rows = drawn.rows();
    let written = drawn.take();

    assert_eq!(
        written.matches('\u{10EEEE}').count(),
        2 * 3,
        "a placeholder other than the picture's: {written:?}"
    );
    assert_eq!(commands(&written).len(), 2, "{written:?}");
    assert!(rows.iter().any(|row| row.contains("a  b  c")), "{rows:#?}");
}
