//! Diagrams drawn as Kitty pictures: sent once and placed by their cells,
//! sent again where the terminal may have lost them, in chunks a frame can
//! carry, wrapped for tmux where it passes them through, and never where no
//! colour is written or nobody is looking. And as iTerm2 and Sixel images,
//! written over cells held blank for them, only while the band shows every
//! one of those cells.

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
///
/// A Sixel picture is those bytes as Sixel's characters, inside the one
/// device control string a Sixel payload is.
pub(in crate::render) struct Printed {
    across: u16,
    down: u16,
    bytes: usize,
    made: u8,
    /// Whether a Sixel picture carries an escape of its own in the middle.
    hostile: bool,
}

impl Printed {
    pub(in crate::render) fn new(across: u16, down: u16, bytes: usize) -> Self {
        Self {
            across,
            down,
            bytes,
            made: 0,
            hostile: false,
        }
    }

    /// Pictures whose Sixel ends its string part way and opens another.
    pub(in crate::render) fn hostile(across: u16, down: u16, bytes: usize) -> Self {
        Self {
            hostile: true,
            ..Self::new(across, down, bytes)
        }
    }

    fn payload(&mut self, key: &graphics::PictureKey) -> graphics::Payload {
        let start = self.made;
        self.made = self.made.wrapping_add(1);
        let bytes = match key.protocol {
            graphics::Protocol::Sixel if self.hostile => {
                let mut bytes = printed_sixel(start, self.bytes);
                let middle = bytes.len() / 2;
                bytes.splice(middle..middle, b"\x1b\\\x1b]52;c;aGk=\x07".iter().copied());
                bytes
            }
            graphics::Protocol::Sixel => printed_sixel(start, self.bytes),
            graphics::Protocol::Kitty | graphics::Protocol::Iterm2 => {
                printed_bytes(start, self.bytes)
            }
        };
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
    numbered_as_shipped(render, printed);
    render.record.numbers_from(1);
}

/// [`pictured_by`], with the numbers `render` was built to give out.
fn numbered_as_shipped<T: Terminal>(render: &mut Renderer<T>, printed: Printed) {
    render.wears(colourful());
    render.images(kitty_in(PICTURE_CELL.0, PICTURE_CELL.1));
    render.drawers(crate::drawing::Drawers::default().with(std::sync::Arc::new(Sketched)));
    render.pictures(Box::new(printed));
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

/// The Sixel [`Printed`] gives the `made`th picture it makes: `bytes` of
/// Sixel's own characters, counting up from `made`, in one string.
pub(in crate::render) fn printed_sixel(made: u8, bytes: usize) -> Vec<u8> {
    let mut sixel = b"\x1bPq".to_vec();
    sixel.extend(
        printed_bytes(made, bytes)
            .into_iter()
            .map(|byte| b'?' + byte % 63),
    );
    sixel.extend_from_slice(b"\x1b\\");
    sixel
}

/// The inline image iTerm2 is written for the `made`th picture [`Printed`]
/// makes, `across` by `down` cells.
pub(in crate::render) fn printed_iterm2(made: u8, bytes: usize, across: u16, down: u16) -> String {
    use base64::Engine as _;

    let data = base64::engine::general_purpose::STANDARD.encode(printed_bytes(made, bytes));
    format!("\x1b]1337;File=inline=1;width={across};height={down};preserveAspectRatio=1:{data}\x07")
}

/// `render` on a terminal that confirmed `protocol`, in colour, drawing
/// diagrams with [`Sketched`] and pictures with `printed`, numbering them
/// from one.
pub(in crate::render) fn pictured_in<T: Terminal>(
    render: &mut Renderer<T>,
    protocol: graphics::Protocol,
    printed: Printed,
) {
    pictured_by(render, printed);
    render.images(graphics::Capability::Graphics {
        protocol,
        cell: graphics::Cell {
            width: PICTURE_CELL.0,
            height: PICTURE_CELL.1,
        },
    });
}

/// Every iTerm2 or Sixel image `written` holds, whole, wrapped for tmux or
/// not, with what was written just before it.
pub(in crate::render) fn images(written: &str) -> Vec<(&str, &str)> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(start) = ["\x1b]1337;File=", "\x1bPq", "\x1bPtmux;"]
        .iter()
        .filter_map(|opens| written[at..].find(opens).map(|found| at + found))
        .min()
    {
        let rest = &written[start..];
        let end = if rest.starts_with("\x1b]") {
            rest.find('\x07').map(|end| end + 1)
        } else if rest.starts_with("\x1bPtmux;") {
            // Every escape inside is doubled, so the first one standing alone
            // before a backslash is the wrapper's own end.
            rest.match_indices("\x1b\\")
                .map(|(end, _)| end)
                .find(|&end| !rest[..end].ends_with('\x1b'))
                .map(|end| end + 2)
        } else {
            rest.find("\x1b\\").map(|end| end + 2)
        };
        let Some(end) = end else {
            break;
        };
        found.push((&written[..start], &rest[..end]));
        at = start + end;
    }
    found
}

/// The screen row and column the last cursor address in `before` puts the
/// cursor at, counted from nought; none where something else follows it.
pub(in crate::render) fn placed_at(before: &str) -> Option<(usize, usize)> {
    let start = before.rfind("\x1b[")?;
    let (row, column) = before[start + 2..].strip_suffix('H')?.split_once(';')?;
    Some((
        row.parse::<usize>().ok()? - 1,
        column.parse::<usize>().ok()? - 1,
    ))
}

/// A full screen this size on a terminal that confirmed `protocol`.
fn pictured_as(
    columns: usize,
    rows: usize,
    protocol: graphics::Protocol,
    printed: Printed,
) -> Drawn {
    let mut drawn = Drawn::new(columns, rows);
    pictured_in(&mut drawn.render, protocol, printed);
    drawn
}

/// The two protocols whose images are written over cells held for them.
const WRITTEN_OVER: [graphics::Protocol; 2] =
    [graphics::Protocol::Iterm2, graphics::Protocol::Sixel];

/// What a frame writes for the first picture [`Printed`] makes, three by two
/// cells of `bytes` bytes, in `protocol`.
fn first_image(protocol: graphics::Protocol, made: u8, bytes: usize) -> String {
    match protocol {
        graphics::Protocol::Sixel => String::from_utf8(printed_sixel(made, bytes)).unwrap(),
        graphics::Protocol::Iterm2 | graphics::Protocol::Kitty => printed_iterm2(made, bytes, 3, 2),
    }
}

#[test]
fn an_iterm2_or_sixel_diagram_is_written_over_the_cells_held_for_it() {
    for protocol in WRITTEN_OVER {
        let mut drawn = pictured_as(40, 12, protocol, Printed::new(3, 2, 300));
        drawn.answer(DIAGRAM);
        let written = drawn.take();

        let images = images(&written);
        assert_eq!(images.len(), 1, "{protocol:?}: {written:?}");
        let (before, image) = images[0];
        assert_eq!(image, first_image(protocol, 0, 300), "{protocol:?}");
        assert!(
            placed_at(before).is_some(),
            "{protocol:?}: not placed: {before:?}"
        );
        // No cell names it, and nothing is sent the Kitty way.
        assert!(!written.contains('\u{10EEEE}'), "{protocol:?}: {written:?}");
        assert!(!written.contains("\x1b_G"), "{protocol:?}: {written:?}");
        // The rows it stands on read as blank.
        assert!(
            !drawn.rows().iter().any(|row| row.contains("graph")),
            "{protocol:?}"
        );
    }
}

#[test]
fn an_iterm2_or_sixel_image_is_written_only_while_every_row_of_it_is_in_the_band() {
    // Scrolled down a row at a time from the top, the picture comes up
    // through the band's foot and leaves through its head. It is written in
    // every frame it is wholly inside, a row higher each time, and in none
    // where a row of it is past either edge.
    for protocol in WRITTEN_OVER {
        let mut drawn = pictured_as(40, 12, protocol, Printed::new(3, 2, 300));
        for line in 0..20 {
            drawn.commit(&format!("before {line}")).unwrap();
        }
        drawn.answer(DIAGRAM);
        for line in 0..20 {
            drawn.commit(&format!("after {line}")).unwrap();
        }
        drawn.scrolled(-1000).unwrap();
        drawn.take();
        let band = drawn.render.bands().transcript;

        let mut frames = Vec::new();
        for _ in 0..60 {
            drawn.scrolled(1).unwrap();
            let written = drawn.take();
            let images = images(&written);
            assert!(images.len() <= 1, "{protocol:?}: {written:?}");
            frames.push(images.first().map(|(before, image)| {
                assert_eq!(*image, first_image(protocol, 0, 300), "{protocol:?}");
                placed_at(before).unwrap_or_else(|| panic!("{protocol:?}: {before:?}"))
            }));
        }

        let first = frames.iter().position(Option::is_some);
        let Some(first) = first else {
            panic!("{protocol:?}: never written: {frames:?}");
        };
        let shown: Vec<(usize, usize)> = frames[first..].iter().map_while(|frame| *frame).collect();
        let tops: Vec<usize> = shown.iter().map(|(row, _)| *row).collect();
        let expected: Vec<usize> = (band.start..=band.end - 2).rev().collect();
        assert_eq!(tops, expected, "{protocol:?}: {frames:?}");
        assert!(shown.iter().all(|(_, column)| *column == 0), "{protocol:?}");
        assert!(
            frames[first + shown.len()..].iter().all(Option::is_none),
            "{protocol:?}: written again after it left: {frames:?}"
        );
    }
}

#[test]
fn an_iterm2_or_sixel_image_is_written_again_after_a_repaint_and_not_before() {
    for protocol in WRITTEN_OVER {
        let mut drawn = pictured_as(40, 12, protocol, Printed::new(3, 2, 300));
        drawn.answer(DIAGRAM);
        drawn.take();
        // Scrolled back to its top, so what comes after it leaves it alone.
        drawn.commit("one line under it").unwrap();
        drawn.scrolled(-1000).unwrap();
        drawn.take();
        drawn.commit("and another").unwrap();
        assert!(images(&drawn.take()).is_empty(), "{protocol:?}");

        drawn.repaint().unwrap();
        let again = drawn.take();
        assert_eq!(
            images(&again)
                .iter()
                .map(|(_, image)| *image)
                .collect::<Vec<_>>(),
            [first_image(protocol, 0, 300)],
            "{protocol:?}"
        );
    }
}

#[test]
fn a_theme_changed_writes_the_iterm2_or_sixel_image_again_as_it_was_encoded_in_it() {
    for protocol in WRITTEN_OVER {
        let mut drawn = pictured_as(40, 12, protocol, Printed::new(3, 2, 300));
        drawn.answer(DIAGRAM);
        drawn.take();

        drawn
            .render
            .wears(Palette::resolve(true, Theme::Light, None, &|name| {
                (name == "COLORTERM").then(|| "truecolor".to_owned())
            }));
        drawn.commit("after the change").unwrap();
        let after = drawn.take();

        assert_eq!(
            images(&after)
                .iter()
                .map(|(_, image)| *image)
                .collect::<Vec<_>>(),
            [first_image(protocol, 1, 300)],
            "{protocol:?}: {after:?}"
        );
    }
}

#[test]
fn a_sixel_image_out_of_view_is_encoded_for_a_new_width_only_once_it_is_in_view() {
    // Quantising is what costs, so a width change encodes again only the
    // Sixel pictures the band shows; iTerm2's PNG is encoded again at once,
    // as Kitty's is.
    for protocol in WRITTEN_OVER {
        let mut drawn = pictured_as(40, 12, protocol, Printed::new(3, 2, 300));
        drawn.answer(DIAGRAM);
        for line in 0..30 {
            drawn.commit(&format!("line {line}")).unwrap();
        }
        drawn.take();
        assert_eq!(drawn.render.record.picture_ids().collect::<Vec<_>>(), [1]);

        drawn.resize(100, 12);
        let resized = drawn.take();
        assert!(images(&resized).is_empty(), "{protocol:?}: {resized:?}");
        let encoded = match protocol {
            graphics::Protocol::Sixel => 1,
            graphics::Protocol::Iterm2 | graphics::Protocol::Kitty => 2,
        };
        assert_eq!(
            drawn.render.record.picture_ids().collect::<Vec<_>>(),
            [encoded],
            "{protocol:?}"
        );

        drawn.scrolled(-1000).unwrap();
        let back = drawn.take();
        assert_eq!(drawn.render.record.picture_ids().collect::<Vec<_>>(), [2]);
        assert_eq!(
            images(&back)
                .iter()
                .map(|(_, image)| *image)
                .collect::<Vec<_>>(),
            [first_image(protocol, 1, 300)],
            "{protocol:?}: {back:?}"
        );
    }
}

#[test]
fn a_sixel_payload_carrying_an_escape_of_its_own_is_never_written() {
    let mut drawn = pictured_as(
        40,
        12,
        graphics::Protocol::Sixel,
        Printed::hostile(3, 2, 300),
    );
    drawn.answer(DIAGRAM);
    let written = drawn.take();

    assert!(!written.contains("\x1bPq"), "{written:?}");
    assert!(!written.contains("\x1b]52"), "{written:?}");
}

#[test]
fn an_inline_image_a_model_wrote_never_reaches_the_terminal_beside_a_picture() {
    let hostile = "a \x1b]1337;File=inline=1:AAAA\x07 b \x1bPq#0;2;0;0;0~~\x1b\\ c\n\n";
    for protocol in WRITTEN_OVER {
        let mut drawn = pictured_as(40, 12, protocol, Printed::new(3, 2, 300));
        drawn.answer(&format!("{hostile}{DIAGRAM}{hostile}"));
        let written = drawn.take();

        assert_eq!(
            images(&written)
                .iter()
                .map(|(_, image)| *image)
                .collect::<Vec<_>>(),
            [first_image(protocol, 0, 300)],
            "{protocol:?}: {written:?}"
        );
    }
}

#[test]
fn inside_tmux_passing_sequences_through_an_image_is_wrapped_for_it() {
    for protocol in WRITTEN_OVER {
        let mut drawn = Drawn::new(40, 12);
        drawn.render.passes_through(graphics::Passthrough::On);
        pictured_in(&mut drawn.render, protocol, Printed::new(3, 2, 30));
        drawn.answer(DIAGRAM);
        let written = drawn.take();

        let wrapped = format!(
            "\x1bPtmux;{}\x1b\\",
            first_image(protocol, 0, 30).replace('\x1b', "\x1b\x1b")
        );
        assert_eq!(
            images(&written)
                .iter()
                .map(|(_, image)| *image)
                .collect::<Vec<_>>(),
            [wrapped.as_str()],
            "{protocol:?}: {written:?}"
        );
    }
}

#[test]
fn no_iterm2_or_sixel_image_is_written_where_colour_is_not_written_or_output_is_redirected() {
    for protocol in WRITTEN_OVER {
        for palette in [
            Palette::resolve(false, Theme::Dark, None, &|_| None),
            Palette::plain(),
        ] {
            let mut drawn = pictured_as(40, 12, protocol, Printed::new(3, 2, 300));
            drawn.render.wears(palette);
            drawn.answer(DIAGRAM);
            let rows = drawn.rows();
            let written = drawn.take();

            assert!(images(&written).is_empty(), "{protocol:?}: {written:?}");
            assert!(rows.iter().any(|row| row.contains("graph")), "{rows:#?}");
        }

        let mut drawn = Drawn {
            render: Renderer::new(Recording::redirected(40, 12)),
        };
        pictured_in(&mut drawn.render, protocol, Printed::new(3, 2, 300));
        drawn.answer(DIAGRAM);
        let written = drawn.take();
        assert!(images(&written).is_empty(), "{protocol:?}: {written:?}");
        assert!(written.contains("graph"), "{protocol:?}: {written:?}");
    }
}

/// Pictures refused as too wide for Sixel, counting how often they are
/// asked for.
struct Refusing(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl graphics::Pictures for Refusing {
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
        _: &graphics::PictureKey,
        _: &graphics::Form,
    ) -> Result<graphics::Payload, crate::chrome::Undrawn> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Err(crate::chrome::Undrawn::TooWide)
    }

    fn reencode(
        &mut self,
        _: &crate::KeptPicture,
        _: &graphics::PictureKey,
    ) -> Result<graphics::Payload, crate::chrome::Undrawn> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Err(crate::chrome::Undrawn::TooWide)
    }
}

#[test]
fn a_sixel_picture_refused_as_too_wide_is_text_and_is_not_asked_for_again_in_a_new_theme() {
    let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut drawn = pictured_as(40, 12, graphics::Protocol::Sixel, Printed::new(3, 2, 300));
    drawn
        .render
        .pictures(Box::new(Refusing(std::sync::Arc::clone(&asked))));
    drawn.answer(DIAGRAM);
    assert!(drawn.rows().iter().any(|row| row.contains("graph")));
    assert_eq!(asked.load(std::sync::atomic::Ordering::Relaxed), 1);

    drawn
        .render
        .wears(Palette::resolve(true, Theme::Light, None, &|name| {
            (name == "COLORTERM").then(|| "truecolor".to_owned())
        }));
    drawn.commit("after the change").unwrap();
    let rows = drawn.rows();
    let after = drawn.take();
    assert_eq!(asked.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert!(images(&after).is_empty(), "{after:?}");
    assert!(rows.iter().any(|row| row.contains("graph")), "{rows:#?}");
}

#[test]
fn two_renderers_built_as_shipped_do_not_both_number_their_pictures_from_one() {
    // The terminal's picture numbers are one namespace for every program
    // writing to it: two sessions in one window, or two panes under one
    // tmux, that both began at 1 would each replace the other's pictures.
    let first_sent = || {
        let mut drawn = Drawn::new(40, 12);
        numbered_as_shipped(&mut drawn.render, Printed::new(3, 2, 300));
        drawn.answer(DIAGRAM);
        let written = drawn.take();
        commands(&written)
            .iter()
            .find_map(|command| command.strip_prefix("a=t,f=100,i="))
            .and_then(|rest| rest.split_once(',').map(|(id, _)| id.parse::<u32>()))
            .and_then(Result::ok)
            .unwrap_or_else(|| panic!("no picture sent: {written:.80?}"))
    };
    let (one, two) = (first_sent(), first_sent());

    assert!(one != 1 || two != 1, "both numbered from 1");
    for id in [one, two] {
        assert!(
            (1..=0x00FF_FFFF).contains(&id),
            "{id} is not a 24-bit colour"
        );
    }
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
fn a_full_screen_closed_part_way_through_sending_a_picture_ends_it() {
    // Whatever writes to the terminal next would otherwise be read as more
    // of the picture.
    let mut drawn = pictured(40, 12, Printed::new(3, 2, 200 * 1024));
    drawn.answer(DIAGRAM);
    let mut written = drawn.take();
    assert_eq!(drawn.render.rests_in(), Some(Duration::ZERO), "sent whole");

    drawn.render.closes().unwrap();
    written.push_str(&drawn.take());

    let commands = commands(&written);
    assert!(
        commands
            .first()
            .is_some_and(|first| first.starts_with("a=t,f=100,i=1,q=2,m=1;"))
    );
    assert_eq!(
        commands.last(),
        Some(&"m=0,q=2;"),
        "left open: {commands:.32?}"
    );
    assert!(!commands.iter().any(|command| command.starts_with("a=p")));
    assert_eq!(drawn.render.rests_in(), None, "still owed after closing");
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
fn a_picture_encoded_again_has_the_terminal_delete_the_one_it_replaces() {
    // Nothing on a screen of crucible's own names the old number again, and
    // the terminal would otherwise keep its image to its own quota.
    let mut drawn = pictured(40, 12, Printed::new(3, 2, 300));
    drawn.answer(DIAGRAM);
    let before = drawn.take();
    assert!(!before.contains("a=d"), "{before:?}");

    drawn.resize(100, 12);
    let after = drawn.take();

    let commands = commands(&after);
    let deleted = commands
        .iter()
        .position(|command| *command == "a=d,d=I,i=1,q=2");
    let sent = commands
        .iter()
        .position(|command| command.starts_with("a=t,f=100,i=2,"));
    assert!(deleted.is_some() && sent.is_some(), "{commands:?}");
    assert!(
        deleted < sent,
        "deleted after the next was sent: {commands:?}"
    );
    assert_eq!(
        commands
            .iter()
            .filter(|command| command.starts_with("a=d"))
            .count(),
        1,
        "{commands:?}"
    );
}

#[test]
fn a_picture_drawn_as_text_again_has_the_terminal_delete_it_through_tmux() {
    let mut drawn = Drawn::new(40, 12);
    drawn.render.passes_through(graphics::Passthrough::On);
    pictured_on(&mut drawn, Printed::new(3, 2, 30));
    drawn.answer(DIAGRAM);
    drawn.take();

    drawn.render.diagrams(crate::drawing::Mode::Text);
    drawn.commit("after the change").unwrap();
    let after = drawn.take();

    assert!(
        after.contains("\x1bPtmux;\x1b\x1b_Ga=d,d=I,i=1,q=2\x1b\x1b\\\x1b\\"),
        "{after:?}"
    );
    assert_eq!(after.matches("\x1b_G").count(), 1, "{after:?}");
    assert!(!after.contains('\u{10EEEE}'), "{after:?}");
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
