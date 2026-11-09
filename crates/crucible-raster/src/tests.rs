use super::*;
use resvg::usvg;
use std::fmt::Write as _;
use std::sync::{Mutex, MutexGuard, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

const SOON: Duration = Duration::from_secs(5);

/// The decode slot is the process's, and the tests of one binary share a
/// process: a test that holds it, or that times how long it is held, must not
/// run beside another that takes it.
fn alone() -> MutexGuard<'static, ()> {
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner)
}

#[test]
fn the_slot_is_free_when_nothing_holds_it() {
    let _alone = alone();
    let slot = Slot::take(Duration::ZERO);
    assert!(slot.is_ok(), "an idle slot is taken at once: {slot:?}");
}

#[test]
fn the_slot_lets_one_thread_hold_it_at_a_time() {
    let _alone = alone();
    let held = Slot::take(SOON).expect("the first take");
    let (told, heard) = mpsc::channel();
    let other = thread::spawn(move || {
        let slot = Slot::take(SOON).expect("taken once the first is dropped");
        told.send(()).expect("the test is listening");
        drop(slot);
    });
    assert!(
        heard.recv_timeout(Duration::from_millis(300)).is_err(),
        "a second thread took the slot while the first held it"
    );
    drop(held);
    heard
        .recv_timeout(SOON)
        .expect("the second thread takes it after the drop");
    other.join().expect("the thread ends");
}

#[test]
fn the_slot_is_released_when_its_holder_unwinds() {
    let _alone = alone();
    let holder = thread::spawn(|| {
        let _slot = Slot::take(SOON).expect("taken");
        panic!("planted: a holder that dies");
    });
    assert!(
        holder.join().is_err(),
        "the planted panic is the thread's end"
    );
    assert!(
        Slot::take(SOON).is_ok(),
        "a holder that unwound left the slot held"
    );
}

#[test]
fn a_thread_that_holds_the_slot_runs_again_without_taking_it() {
    let _alone = alone();
    let _held = Slot::take(SOON).expect("the first take");
    let again = Slot::take(Duration::ZERO);
    assert!(
        again.is_ok(),
        "re-entry on the holding thread waited on itself: {again:?}"
    );
    drop(again);
    let (told, heard) = mpsc::channel();
    let other = thread::spawn(move || told.send(Slot::take(Duration::from_millis(50)).is_ok()));
    assert!(
        !heard.recv_timeout(SOON).expect("the other thread answers"),
        "dropping the re-entered slot released the one the thread still holds"
    );
    other
        .join()
        .expect("the thread ends")
        .expect("the test is listening");
}

#[test]
fn the_wait_for_the_slot_is_bounded_and_says_over_time() {
    let _alone = alone();
    let _held = Slot::take(SOON).expect("the first take");
    let waited = thread::spawn(|| {
        let began = Instant::now();
        let slot = Slot::take(Duration::from_millis(150));
        (slot.map(|_| ()), began.elapsed())
    })
    .join()
    .expect("the thread ends");
    assert_eq!(waited.0, Err(Refused::OverTime));
    assert!(
        waited.1 >= Duration::from_millis(150),
        "gave up early: {:?}",
        waited.1
    );
    assert!(
        waited.1 < Duration::from_secs(3),
        "waited far past the bound: {:?}",
        waited.1
    );
}

/// Encoded files made on the spot, so a cap is tested at the byte it sits on.
mod made {
    use image::codecs::gif::GifEncoder;
    use image::codecs::jpeg::JpegEncoder;
    use image::codecs::png::PngEncoder;
    use image::codecs::webp::WebPEncoder;
    use image::{ExtendedColorType, Frame, ImageEncoder, RgbaImage};

    /// `width` by `height` of one opaque colour.
    pub(super) fn flat(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        std::iter::repeat_n(rgba, (width * height) as usize)
            .flatten()
            .collect()
    }

    pub(super) fn png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut out = Vec::new();
        PngEncoder::new(&mut out)
            .write_image(
                &flat(width, height, rgba),
                width,
                height,
                ExtendedColorType::Rgba8,
            )
            .expect("a PNG");
        out
    }

    pub(super) fn png16(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        PngEncoder::new(&mut out)
            .write_image(
                &vec![0; (width * height * 8) as usize],
                width,
                height,
                ExtendedColorType::Rgba16,
            )
            .expect("a 16-bit PNG");
        out
    }

    pub(super) fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let rgb: Vec<u8> = (0..width * height).flat_map(|_| [200, 30, 30]).collect();
        let mut out = Vec::new();
        JpegEncoder::new(&mut out)
            .write_image(&rgb, width, height, ExtendedColorType::Rgb8)
            .expect("a JPEG");
        out
    }

    pub(super) fn gif(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = GifEncoder::new(&mut out);
        let first = RgbaImage::from_raw(width, height, flat(width, height, rgba)).expect("pixels");
        let second = RgbaImage::from_raw(width, height, flat(width, height, [0, 0, 255, 255]))
            .expect("pixels");
        encoder.encode_frame(Frame::new(first)).expect("a frame");
        encoder
            .encode_frame(Frame::new(second))
            .expect("a second frame");
        drop(encoder);
        out
    }

    pub(super) fn webp(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        let mut out = Vec::new();
        WebPEncoder::new_lossless(&mut out)
            .write_image(
                &flat(width, height, rgba),
                width,
                height,
                ExtendedColorType::Rgba8,
            )
            .expect("a WebP");
        out
    }

    /// The same file, its header claiming another size: what a bomb is.
    pub(super) fn claiming(png: &[u8], width: u32, height: u32) -> Vec<u8> {
        let mut header = b"IHDR".to_vec();
        header.extend(width.to_be_bytes());
        header.extend(height.to_be_bytes());
        header.extend(png.get(24..29).expect("the rest of the header"));
        let mut crc = !0u32;
        for byte in &header {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        let mut out = png.get(..8).expect("the signature").to_vec();
        out.extend(png.get(8..12).expect("the header's length"));
        out.extend(header);
        out.extend((!crc).to_be_bytes());
        out.extend(png.get(33..).expect("the chunks after the header"));
        out
    }

    /// The first `len` bytes of a file.
    pub(super) fn cut(file: &[u8], len: usize) -> Vec<u8> {
        file.iter().copied().take(len).collect()
    }
}

/// The first pixel, and the last.
fn ends(pixels: &Pixels) -> ([u8; 4], [u8; 4]) {
    let first = pixels.rgba().first_chunk().expect("a pixel");
    let last = pixels.rgba().last_chunk().expect("a pixel");
    (*first, *last)
}

const RED: [u8; 4] = [255, 0, 0, 255];

#[test]
fn each_image_format_decodes_to_straight_rgba() {
    let _alone = alone();
    for (name, file, exact) in [
        ("png", made::png(16, 8, RED), true),
        ("gif", made::gif(16, 8, RED), true),
        ("webp", made::webp(16, 8, RED), true),
        ("jpeg", made::jpeg(16, 8), false),
    ] {
        let pixels = decode(&file).unwrap_or_else(|why| panic!("{name} was refused: {why}"));
        assert_eq!((pixels.width(), pixels.height()), (16, 8), "{name}");
        assert_eq!(pixels.rgba().len(), 16 * 8 * 4, "{name}");
        let first = ends(&pixels).0;
        if exact {
            assert_eq!(first, RED, "{name} lost its colour");
        } else {
            let [red, green, _, alpha] = first;
            assert!(red > 150 && green < 90 && alpha == 255, "{name}: {first:?}");
        }
    }
}

#[test]
fn an_animation_decodes_to_its_first_frame_only() {
    let _alone = alone();
    let pixels = decode(&made::gif(16, 8, RED)).expect("a GIF");
    assert_eq!(
        pixels.rgba().len(),
        16 * 8 * 4,
        "one frame's pixels, not the sequence's"
    );
    assert_eq!(
        ends(&pixels).1,
        RED,
        "the last pixel is still the first frame's"
    );
}

#[test]
fn bytes_that_are_no_image_are_not_an_image() {
    let _alone = alone();
    let svg = b"<svg xmlns='http://www.w3.org/2000/svg' width='4' height='4'/>";
    let cut = made::png(16, 8, RED);
    for (name, bytes) in [
        ("empty", Vec::new()),
        ("text", b"not a picture".to_vec()),
        ("an svg, which decode does not read", svg.to_vec()),
        ("zeros", vec![0; 4096]),
        ("a png cut short", made::cut(&cut, cut.len() / 2)),
        ("a jpeg cut short", made::cut(&made::jpeg(64, 64), 100)),
        ("a gif cut short", made::cut(&made::gif(16, 8, RED), 40)),
        ("a webp cut short", made::cut(&made::webp(16, 8, RED), 30)),
    ] {
        assert_eq!(
            decode(&bytes).map(|_| ()),
            Err(Refused::NotAnImage),
            "{name}"
        );
    }
}

#[test]
fn input_is_refused_one_byte_past_its_ceiling() {
    let _alone = alone();
    let mut at = made::png(16, 8, RED);
    at.resize(MOST_INPUT, 0);
    assert!(
        decode(&at).is_ok(),
        "a file of exactly {MOST_INPUT} bytes is read"
    );
    at.push(0);
    assert_eq!(decode(&at).map(|_| ()), Err(Refused::TooLarge));
}

#[test]
fn decoded_pixels_are_refused_one_byte_past_their_ceiling() {
    let _alone = alone();
    assert_eq!(MOST_DECODED, 1024 * 512 * 4);
    let at = decode(&made::png(1024, 512, RED)).expect("exactly the ceiling");
    assert_eq!(at.rgba().len(), MOST_DECODED);
    for (width, height) in [(1025, 512), (1024, 513), (524_289, 1)] {
        assert_eq!(
            decode(&made::claiming(&made::png(16, 8, RED), width, height)).map(|_| ()),
            Err(Refused::TooLarge),
            "{width}x{height} is judged from the header, with no pixels behind it"
        );
    }
    assert_eq!(
        decode(&made::png(1025, 512, RED)).map(|_| ()),
        Err(Refused::TooLarge)
    );
}

#[test]
fn a_header_claiming_four_billion_pixels_is_refused_not_allocated() {
    let _alone = alone();
    let bomb = made::claiming(&made::png(16, 8, RED), u32::MAX, u32::MAX);
    assert_eq!(decode(&bomb).map(|_| ()), Err(Refused::TooLarge));
}

#[test]
fn a_sixteen_bit_file_is_judged_by_what_the_decoder_holds_for_it() {
    let _alone = alone();
    // 1024 x 512 is the ceiling as RGBA8, but this file decodes through 4 MiB.
    assert_eq!(
        decode(&made::png16(1024, 512)).map(|_| ()),
        Err(Refused::TooLarge)
    );
    assert!(
        decode(&made::png16(512, 512)).is_ok(),
        "within the ceiling at 16 bits a channel"
    );
}

#[test]
fn decode_waits_for_the_slot_before_it_works() {
    let _alone = alone();
    let held = Slot::take(SOON).expect("the slot");
    let file = made::png(16, 8, RED);
    let (told, heard) = mpsc::channel();
    let worker = thread::spawn(move || told.send(decode(&file).is_ok()));
    assert!(
        heard.recv_timeout(Duration::from_millis(300)).is_err(),
        "decoded while another held the slot"
    );
    drop(held);
    assert!(
        heard
            .recv_timeout(SOON)
            .expect("it finishes once the slot is free")
    );
    worker
        .join()
        .expect("the thread ends")
        .expect("the test is listening");
}

const RECT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="50" height="50" fill="#ff0000"/></svg>"##;

/// The pixel at a column and row.
fn at(pixels: &Pixels, column: u32, row: u32) -> [u8; 4] {
    let index = ((row * pixels.width() + column) * 4) as usize;
    *pixels
        .rgba()
        .get(index..index + 4)
        .expect("inside the picture")
        .first_chunk()
        .expect("four bytes")
}

#[test]
fn an_svg_is_drawn_at_the_width_asked_with_nothing_behind_it() {
    let _alone = alone();
    let pixels = rasterise(RECT.as_bytes(), 200, &[]).expect("a rectangle");
    assert_eq!((pixels.width(), pixels.height()), (200, 100));
    assert_eq!(at(&pixels, 10, 10), RED, "inside the rectangle");
    assert_eq!(
        at(&pixels, 150, 10)[3],
        0,
        "outside it is transparent, not white"
    );
}

#[test]
fn the_height_follows_the_proportions_and_rounds_up_to_a_row() {
    let _alone = alone();
    let tall = r#"<svg xmlns="http://www.w3.org/2000/svg" width="3" height="10"><rect width="3" height="10"/></svg>"#;
    let flat = r#"<svg xmlns="http://www.w3.org/2000/svg" width="1000" height="1"><rect width="1000" height="1"/></svg>"#;
    assert_eq!(
        rasterise(tall.as_bytes(), 10, &[]).map(|p| p.height()),
        Ok(34),
        "33.3 rows is 34"
    );
    assert_eq!(
        rasterise(flat.as_bytes(), 10, &[]).map(|p| p.height()),
        Ok(1),
        "never a zero-row picture"
    );
}

#[test]
fn a_drawing_is_refused_one_byte_past_its_ceiling() {
    let _alone = alone();
    let square = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10"/></svg>"#;
    assert_eq!(
        rasterise(square.as_bytes(), 724, &[]).map(|p| p.rgba().len()),
        Ok(724 * 724 * 4)
    );
    assert_eq!(
        rasterise(square.as_bytes(), 725, &[]).map(|_| ()),
        Err(Refused::TooLarge)
    );
    let wide = r#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="1"><rect width="2" height="1"/></svg>"#;
    assert_eq!(
        rasterise(wide.as_bytes(), 1024, &[]).map(|p| p.rgba().len()),
        Ok(MOST_DECODED)
    );
    assert_eq!(
        rasterise(wide.as_bytes(), 1025, &[]).map(|_| ()),
        Err(Refused::TooLarge)
    );
    for width in [u32::MAX, 1 << 31, 100_000] {
        assert_eq!(
            rasterise(wide.as_bytes(), width, &[]).map(|_| ()),
            Err(Refused::TooLarge),
            "{width}"
        );
    }
}

#[test]
fn an_svg_is_refused_one_byte_past_its_ceiling() {
    let _alone = alone();
    let mut at = RECT.as_bytes().to_vec();
    at.resize(MOST_INPUT, b' ');
    assert!(
        rasterise(&at, 10, &[]).is_ok(),
        "an SVG of exactly {MOST_INPUT} bytes is read"
    );
    at.push(b' ');
    assert_eq!(rasterise(&at, 10, &[]).map(|_| ()), Err(Refused::TooLarge));
}

#[test]
fn bytes_that_are_no_svg_are_not_an_image() {
    let _alone = alone();
    for (name, bytes) in [
        ("empty", Vec::new()),
        ("text", b"not a picture".to_vec()),
        ("a png", made::png(4, 4, RED)),
        ("bytes that are not utf-8", vec![0xff, 0xfe, 0x3c, 0x73]),
        (
            "a compressed svg, which is not read",
            vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0],
        ),
        ("markup that is not svg", b"<html><body/></html>".to_vec()),
        (
            "an unclosed svg",
            b"<svg xmlns='http://www.w3.org/2000/svg' width='4' height='4'><g>".to_vec(),
        ),
    ] {
        assert_eq!(
            rasterise(&bytes, 10, &[]).map(|_| ()),
            Err(Refused::NotAnImage),
            "{name}"
        );
    }
    assert_eq!(
        rasterise(RECT.as_bytes(), 0, &[]).map(|_| ()),
        Err(Refused::NotAnImage),
        "no width"
    );
}

const INKED: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><g class="warm"><rect width="5" height="10" fill="currentColor"/></g><rect x="5" width="5" height="10" class="cool" fill="currentColor"/></svg>"#;

#[test]
fn the_classes_an_svg_names_are_coloured_by_the_inks_it_is_given() {
    let _alone = alone();
    let by_day = [("warm", [200, 40, 0]), ("cool", [0, 40, 200])];
    let by_night = [("warm", [255, 200, 120]), ("cool", [10, 10, 90])];
    let day = rasterise(INKED.as_bytes(), 10, &by_day).expect("by day");
    let night = rasterise(INKED.as_bytes(), 10, &by_night).expect("by night");
    assert_eq!(
        (at(&day, 1, 5), at(&day, 8, 5)),
        ([200, 40, 0, 255], [0, 40, 200, 255])
    );
    assert_eq!(
        (at(&night, 1, 5), at(&night, 8, 5)),
        ([255, 200, 120, 255], [10, 10, 90, 255])
    );
    let unlisted = rasterise(INKED.as_bytes(), 10, &[("warm", [200, 40, 0])]).expect("one ink");
    assert_eq!(
        at(&unlisted, 8, 5),
        [0, 0, 0, 255],
        "a class with no ink keeps the default colour"
    );
}

#[test]
fn a_picture_nested_as_deep_as_the_parser_allows_is_drawn_whatever_stack_the_caller_has() {
    let _alone = alone();
    let nested = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{}<rect width="10" height="10" fill="#ff0000"/>{}</svg>"##,
        "<g>".repeat(1000),
        "</g>".repeat(1000)
    );
    // A stack far smaller than the nesting needs, as a caller's thread may have.
    let caller = thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || rasterise(nested.as_bytes(), 10, &[]))
        .expect("a thread");
    let pixels = caller.join().expect("the drawing ends").expect("an svg");
    assert_eq!(at(&pixels, 5, 5), RED);
}

#[test]
fn a_name_that_is_not_a_class_is_not_let_into_the_style_sheet() {
    let _alone = alone();
    for name in [
        "",
        "a b",
        "a{color:red}b",
        "a,b",
        "a;b",
        "é",
        "a\nb",
        ".warm",
        "a/*",
        "a}",
    ] {
        assert_eq!(
            rasterise(INKED.as_bytes(), 10, &[(name, [1, 2, 3])]).map(|_| ()),
            Err(Refused::NotAnImage),
            "{name:?}"
        );
    }
    assert!(rasterise(INKED.as_bytes(), 10, &[("Slot_9-b", [1, 2, 3])]).is_ok());
}

const WORDS: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="30"><text x="2" y="22" font-size="20" font-family="FAMILY">Hello</text></svg>"#;

#[test]
fn text_is_drawn_with_the_font_the_crate_carries_whatever_family_is_named() {
    let _alone = alone();
    let drawn = |family: &str| {
        rasterise(WORDS.replace("FAMILY", family).as_bytes(), 120, &[]).expect("a text")
    };
    let reference = drawn("DejaVu Sans");
    assert!(
        reference
            .rgba()
            .chunks_exact(4)
            .any(|pixel| pixel.last().is_some_and(|alpha| *alpha > 0)),
        "no glyph reached the picture"
    );
    // Were any system font consulted, one of these names would find its own.
    for family in [
        "Times New Roman",
        "monospace",
        "serif",
        "Courier New",
        "Liberation Sans",
        "No Such Family",
    ] {
        assert_eq!(
            drawn(family),
            reference,
            "{family} was drawn in another font"
        );
    }
}

#[test]
fn the_font_database_holds_the_one_embedded_face_and_nothing_from_the_system() {
    let database = fonts();
    let faces: Vec<_> = database.faces().collect();
    assert_eq!(faces.len(), 1, "a second face came from somewhere");
    let face = faces.first().expect("the embedded face");
    assert_eq!(
        face.families.first().map(|(name, _)| name.as_str()),
        Some("DejaVu Sans")
    );
    assert!(
        matches!(face.source, usvg::fontdb::Source::Binary(_)),
        "the face was read from a file"
    );
}

#[test]
fn an_image_that_is_not_a_data_url_is_not_followed() {
    let _alone = alone();
    // An SVG file, because that is what a resolver that reads the file would
    // draw even where raster images are not.
    let file =
        std::env::temp_dir().join(format!("crucible-raster-href-{}.svg", std::process::id()));
    std::fs::write(
        &file,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#00ff00"/></svg>"##,
    )
    .expect("a picture to point at");
    let page = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="8" height="8"><image width="8" height="8" xlink:href="{}"/></svg>"#,
        file.display()
    );
    let pixels = rasterise(page.as_bytes(), 8, &[]).expect("an svg");
    std::fs::remove_file(&file).expect("clean up");
    assert!(
        pixels.rgba().iter().all(|byte| *byte == 0),
        "the file named by an href was read and drawn"
    );
}

#[test]
fn an_svg_inside_a_data_url_is_drawn_and_its_own_references_are_not_followed() {
    let _alone = alone();
    let inner = r##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect width="4" height="4" fill="#00ff00"/></svg>"##;
    let encoded: String = inner
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect();
    let outer = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="4" height="4"><image width="4" height="4" xlink:href="data:image/svg+xml;charset=utf8,{encoded}"/></svg>"#
    );
    let pixels = rasterise(outer.as_bytes(), 4, &[]).expect("a nested svg");
    assert_eq!(at(&pixels, 2, 2), [0, 255, 0, 255]);
}

#[test]
fn rasterise_waits_for_the_slot_before_it_works() {
    let _alone = alone();
    let held = Slot::take(SOON).expect("the slot");
    let (told, heard) = mpsc::channel();
    let worker = thread::spawn(move || told.send(rasterise(RECT.as_bytes(), 10, &[]).is_ok()));
    assert!(
        heard.recv_timeout(Duration::from_millis(300)).is_err(),
        "drew while another held the slot"
    );
    drop(held);
    assert!(
        heard
            .recv_timeout(SOON)
            .expect("it finishes once the slot is free")
    );
    worker
        .join()
        .expect("the thread ends")
        .expect("the test is listening");
}

/// Bytes nothing compresses, from a fixed seed.
fn noise(width: u32, height: u32) -> Pixels {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let rgba = (0..width * height * 4)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes().first().copied().unwrap_or(0)
        })
        .collect();
    Pixels::from_rgba(width, height, rgba).expect("pixels")
}

fn flat_pixels(width: u32, height: u32, rgba: [u8; 4]) -> Pixels {
    Pixels::from_rgba(width, height, made::flat(width, height, rgba)).expect("pixels")
}

#[test]
fn a_png_payload_decodes_back_to_the_pixels_it_was_made_from() {
    let _alone = alone();
    let pixels = noise(40, 30);
    let payload = png(&pixels).expect("a PNG");
    assert!(payload.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]));
    assert_eq!(
        decode(&payload),
        Ok(pixels),
        "straight alpha comes back as it went"
    );
}

#[test]
fn a_png_payload_is_refused_one_byte_past_its_ceiling() {
    let pixels = noise(64, 64);
    let whole = png_within(&pixels, usize::MAX).expect("no ceiling");
    assert!(whole.len() > 1000, "noise does not shrink");
    assert!(
        png_within(&pixels, whole.len()).is_ok_and(|exact| exact == whole),
        "exactly the ceiling"
    );
    assert_eq!(png_within(&pixels, whole.len() - 1), Err(Refused::TooLarge));
    assert_eq!(png_within(&pixels, 0), Err(Refused::TooLarge));
}

#[test]
fn the_ceiling_a_png_payload_is_held_to_is_one_mebibyte() {
    assert_eq!(MOST_PNG, 1024 * 1024);
    // 1024 x 512 of noise is 2 MiB before the encoder starts, and does not shrink.
    assert_eq!(png(&noise(1024, 512)), Err(Refused::TooLarge));
    assert!(png(&noise(256, 256)).is_ok_and(|payload| payload.len() <= MOST_PNG));
}

const PLAIN: SixelPalette = SixelPalette {
    ground: [0, 0, 0],
    colours: 256,
};

fn text(payload: Vec<u8>) -> String {
    String::from_utf8(payload).expect("a Sixel payload is ASCII")
}

#[test]
fn a_sixel_payload_is_a_device_control_string_defining_the_colours_it_draws() {
    let drawn = text(sixel(&flat_pixels(8, 12, RED), &PLAIN).expect("a payload"));
    assert!(drawn.starts_with("\u{1b}P"), "opens with DCS: {drawn:?}");
    assert!(drawn.ends_with("\u{1b}\\"), "closes with ST: {drawn:?}");
    assert!(
        drawn.contains(";2;100;0;0"),
        "red is defined, in percent: {drawn:?}"
    );
}

#[test]
fn a_sixel_payload_is_refused_wider_than_its_ceiling() {
    assert_eq!(MOST_SIXEL_WIDTH, 600);
    assert!(
        sixel(&flat_pixels(600, 6, RED), &PLAIN).is_ok(),
        "exactly the ceiling"
    );
    assert_eq!(
        sixel(&flat_pixels(601, 6, RED), &PLAIN),
        Err(Refused::TooLarge)
    );
    assert_eq!(
        sixel(&flat_pixels(4000, 1, RED), &PLAIN),
        Err(Refused::TooLarge)
    );
}

#[test]
fn what_is_transparent_shows_the_ground_it_is_laid_over() {
    let nothing = flat_pixels(8, 12, [255, 0, 0, 0]);
    let on_blue = text(
        sixel(
            &nothing,
            &SixelPalette {
                ground: [0, 0, 255],
                colours: 16,
            },
        )
        .expect("blue"),
    );
    let on_green = text(
        sixel(
            &nothing,
            &SixelPalette {
                ground: [0, 255, 0],
                colours: 16,
            },
        )
        .expect("green"),
    );
    assert!(on_blue.contains(";2;0;0;100"), "{on_blue:?}");
    assert!(on_green.contains(";2;0;100;0"), "{on_green:?}");
    assert!(
        !on_blue.contains(";2;100;0;0"),
        "the colour of a pixel with no alpha leaked: {on_blue:?}"
    );
    // Half way is half way between.
    let half = flat_pixels(8, 12, [255, 255, 255, 128]);
    let grey = text(sixel(&half, &PLAIN).expect("grey"));
    assert!(
        grey.contains(";2;50;50;50") || grey.contains(";2;51;51;51"),
        "{grey:?}"
    );
}

#[test]
fn the_payload_defines_no_more_colours_than_it_is_allowed() {
    let defined = |colours: u16| {
        let palette = SixelPalette {
            ground: [0, 0, 0],
            colours,
        };
        text(sixel(&noise(60, 60), &palette).expect("a payload"))
            .matches(";2;")
            .count()
    };
    assert!(defined(8) <= 8 && defined(8) > 2, "{}", defined(8));
    assert!(defined(256) > 100, "{}", defined(256));
    // A number outside 2 to 256 is the nearest end of it.
    assert_eq!(defined(0), defined(2));
    assert_eq!(defined(1), defined(2));
    assert_eq!(defined(1000), defined(256));
}

#[test]
fn the_widest_noisiest_sixel_payload_stays_within_its_measured_bound() {
    let pixels = noise(600, 873);
    assert_eq!(pixels.rgba().len(), 600 * 873 * 4);
    let payload = sixel(&pixels, &PLAIN).expect("a payload");
    // Measured at 2 694 084 bytes: about 1.3 bytes a byte of pixels.
    assert!(payload.len() < 4 * MOST_DECODED, "{} bytes", payload.len());
}

/// A document of `inner` in a page, with the namespaces a link needs.
fn page(inner: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="20">{inner}</svg>"#
    )
}

/// `count` elements of `make(index, next)`, each naming the next and the last
/// naming the first: a ring no shorter than three is where the parser's own
/// check for a reference that leads back stops looking.
fn ring(count: usize, make: impl Fn(usize, usize) -> String) -> String {
    (0..count).map(|at| make(at, (at + 1) % count)).collect()
}

/// `count` elements of `make(index)`, from 1, each of which names the one before.
fn line(count: usize, make: impl Fn(usize) -> String) -> String {
    (1..=count).map(make).collect()
}

#[test]
fn references_that_lead_back_to_where_they_started_are_refused_before_the_parser_follows_them() {
    let _alone = alone();
    // Each of these overflows the stack of the thread that draws it, however
    // large that stack is, in the parser the crate carries.
    let rings = [
        (
            "masks",
            page(&format!(
                r#"<defs>{}</defs><rect width="20" height="20" mask="url(#m0)"/>"#,
                ring(3, |at, next| format!(
                    r##"<mask id="m{at}"><rect width="20" height="20" fill="#fff" mask="url(#m{next})"/></mask>"##
                ))
            )),
        ),
        (
            "clip paths",
            page(&format!(
                r#"<defs>{}</defs><rect width="20" height="20" clip-path="url(#c0)"/>"#,
                ring(4, |at, next| format!(
                    r#"<clipPath id="c{at}"><rect width="9" height="9" clip-path="url(#c{next})"/></clipPath>"#
                ))
            )),
        ),
        (
            "patterns",
            page(&format!(
                r#"<defs>{}</defs><rect width="20" height="20" fill="url(#p0)"/>"#,
                ring(3, |at, next| format!(
                    r#"<pattern id="p{at}" width="5" height="5" patternUnits="userSpaceOnUse"><rect width="2" height="2" fill="url(#p{next})"/></pattern>"#
                ))
            )),
        ),
        (
            "copies of each other",
            page(&format!(
                r##"<defs>{}</defs><use href="#u0"/>"##,
                ring(3, |at, next| format!(
                    r##"<g id="u{at}"><use xlink:href="#u{next}"/></g>"##
                ))
            )),
        ),
        (
            "a style attribute",
            page(&format!(
                r#"<defs>{}</defs><rect width="20" height="20" style="mask: url( '#m0' )"/>"#,
                ring(3, |at, next| format!(
                    r#"<mask id="m{at}"><rect width="20" height="20" style="MASK:URL(#m{next})"/></mask>"#
                ))
            )),
        ),
        (
            "a style sheet",
            page(&format!(
                // Nothing in the elements themselves names another.
                r#"<style>{}</style><defs>{}</defs><rect width="20" height="20" class="r0"/>"#,
                ring(3, |at, next| format!(".r{at} {{ mask: url(#m{next}) }} ")),
                ring(3, |at, _| format!(
                    r#"<mask id="m{at}"><rect width="20" height="20" class="r{at}"/></mask>"#
                ))
            )),
        ),
        (
            "a mask that is its own ancestor",
            page(
                r#"<mask id="m0"><g><g><rect width="20" height="20" mask="url(#m0)"/></g></g></mask><rect width="20" height="20" mask="url(#m0)"/>"#,
            ),
        ),
    ];
    for (name, svg) in rings {
        assert_eq!(
            rasterise(svg.as_bytes(), 20, &[]),
            Err(Refused::NotAnImage),
            "{name}"
        );
    }
}

#[test]
fn references_that_share_a_target_without_leading_back_are_drawn() {
    let _alone = alone();
    let diamonds = [
        (
            "one mask on two elements",
            page(
                r##"<mask id="m"><rect width="10" height="20" fill="#fff"/></mask><rect width="20" height="9" mask="url(#m)"/><rect y="10" width="20" height="9" mask="url(#m)"/>"##,
            ),
        ),
        (
            "two paths to one symbol",
            page(
                r##"<symbol id="s"><rect width="4" height="4"/></symbol><g id="a"><use href="#s"/></g><g id="b"><use href="#s"/></g><g id="top"><use href="#a"/><use href="#b" x="8"/></g>"##,
            ),
        ),
        (
            "a gradient named by a style sheet",
            page(
                r##"<style>.fill { fill: url(#g) }</style><linearGradient id="g"><stop offset="0" stop-color="#f00"/><stop offset="1" stop-color="#00f"/></linearGradient><rect width="20" height="20" class="fill"/>"##,
            ),
        ),
        (
            "a link to the group it is in",
            page(r##"<g id="all"><a href="#all"><rect width="20" height="20"/></a></g>"##),
        ),
        (
            "a long line of groups",
            page(&format!(
                r##"<defs><rect id="u0" width="4" height="4"/>{}</defs><use href="#u200"/>"##,
                line(200, |at| format!(
                    r##"<g id="u{at}"><use href="#u{}"/></g>"##,
                    at - 1
                ))
            )),
        ),
    ];
    for (name, svg) in diamonds {
        assert!(rasterise(svg.as_bytes(), 20, &[]).is_ok(), "{name}");
    }
}

#[test]
fn a_ring_inside_a_nested_svg_is_dropped_and_the_page_around_it_is_drawn() {
    let _alone = alone();
    let inner = page(&format!(
        r#"<defs>{}</defs><rect width="20" height="20" mask="url(#m0)"/>"#,
        ring(3, |at, next| format!(
            r##"<mask id="m{at}"><rect width="20" height="20" fill="#fff" mask="url(#m{next})"/></mask>"##
        ))
    ));
    let encoded: String = inner
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect();
    let outer = page(&format!(
        r##"<rect width="20" height="20" fill="#00ff00"/><image width="20" height="20" xlink:href="data:image/svg+xml;charset=utf8,{encoded}"/>"##
    ));
    let pixels = rasterise(outer.as_bytes(), 20, &[]).expect("the page around it");
    assert_eq!(at(&pixels, 10, 10), [0, 255, 0, 255]);
}

#[test]
fn a_drawing_the_renderer_panics_on_is_refused_not_raised_to_the_caller() {
    let _alone = alone();
    // A stroke far wider than the picture, dashed, makes the renderer's
    // anti-aliasing index past its own rows. Found by generated input. Its
    // dashes are few, since a drawing cut into many would be refused before it
    // was drawn.
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="756.131" height="1e400" viewBox="0 0 3803.823 3803.823"><g transform="scale(2215.33)"><path d="M0 0 L3803.823 3803.823" stroke="#000" stroke-width="413156971" stroke-dasharray="10000" fill="none"/><circle cx="3803.823" cy="3803.823" r="756.131"/></g></svg>"##;
    assert_eq!(
        rasterise(svg.as_bytes(), 583, &[]),
        Err(Refused::NotAnImage)
    );
    // The slot is free again for the next picture.
    assert!(rasterise(RECT.as_bytes(), 100, &[]).is_ok());
}

// ---------------------------------------------------------------------------
// What a drawing would hold and compute, judged before it is drawn.

/// A page holding `count` rectangles `across` by `down` pixels in a row, each
/// drawn through the filter `primitives`, whose region is the rectangle.
fn filtered(primitives: &str, count: u32, across: u32, down: u32) -> String {
    let rectangles = (0..count).fold(String::new(), |mut all, at| {
        let _ = write!(
            all,
            r#"<rect x="{}" width="{across}" height="{down}" filter="url(#f)"/>"#,
            at * across
        );
        all
    });
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{down}"><filter id="f" x="0" y="0" width="1" height="1">{primitives}</filter>{rectangles}</svg>"#,
        count * across
    )
}

/// A square page `side` pixels across with one square the size of the page
/// drawn at half opacity: the renderer makes a layer for it beside the canvas.
fn faded(side: u32) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{side}" height="{side}"><rect width="{side}" height="{side}" opacity="0.5"/></svg>"#
    )
}

#[test]
fn a_filter_with_many_named_results_is_refused_before_it_is_drawn() {
    let _alone = alone();
    // Each result is a pixmap the size of the filter region, and the region is
    // allowed to be five canvases across and down: sixteen of them are
    // hundreds of megabytes from a file under a kilobyte.
    let results = line(16, |at| format!(r#"<feFlood result="r{at}"/>"#));
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="60"><filter id="f" x="-1000%" y="-1000%" width="3000%" height="3000%">{results}</filter><rect width="100" height="60" filter="url(#f)"/></svg>"#
    );
    assert!(svg.len() < 1024);
    assert_eq!(rasterise(svg.as_bytes(), 100, &[]), Err(Refused::TooLarge));
}

#[test]
fn a_layer_beside_the_canvas_is_counted_against_the_ceiling_one_byte_past_it() {
    let _alone = alone();
    // The canvas is 4 bytes a pixel and the layer has two pixels more on each
    // side, so the two together are 8 side² + 32 side + 64 bytes.
    let held = |side: u32| 8 * u64::from(side).pow(2) + 32 * u64::from(side) + 64;
    let side = (1..=u32::MAX)
        .take_while(|side| held(*side) <= MOST_DECODED as u64)
        .last()
        .expect("a side that fits");
    assert!(held(side) <= MOST_DECODED as u64 && held(side + 1) > MOST_DECODED as u64);
    let drawn = rasterise(faded(side).as_bytes(), side, &[]).expect("held at the ceiling");
    assert_eq!((drawn.width(), drawn.height()), (side, side));
    assert_eq!(
        rasterise(faded(side + 1).as_bytes(), side + 1, &[]),
        Err(Refused::TooLarge)
    );
    // The same canvas without the layer is well within its own ceiling.
    let plain = faded(side + 1).replace(r#" opacity="0.5""#, "");
    assert!(rasterise(plain.as_bytes(), side + 1, &[]).is_ok());
}

#[test]
fn a_small_filter_and_a_small_layer_are_drawn() {
    let _alone = alone();
    let blurred = filtered(r#"<feGaussianBlur stdDeviation="2"/>"#, 4, 10, 10);
    assert!(rasterise(blurred.as_bytes(), 40, &[]).is_ok());
    assert!(rasterise(faded(100).as_bytes(), 100, &[]).is_ok());
}

/// Ten octaves of turbulence over `count` rectangles `across` by `down`: ten
/// steps for each pixel.
fn turbulent(count: u32, across: u32, down: u32) -> String {
    filtered(
        r#"<feTurbulence baseFrequency="0.1" numOctaves="10"/>"#,
        count,
        across,
        down,
    )
}

#[test]
fn filter_work_is_refused_one_row_past_its_ceiling() {
    let _alone = alone();
    // The rectangle is painted too, at 100 pixels to 6.25 steps: 139 rows of
    // 10 octaves are 139 000 steps and 869 of paint, which is within the
    // ceiling, and 140 rows are 140 000 and 875, which is not.
    let at = turbulent(1, 100, 139);
    assert!(rasterise(at.as_bytes(), 100, &[]).is_ok());
    let past = turbulent(1, 100, 140);
    assert_eq!(rasterise(past.as_bytes(), 100, &[]), Err(Refused::TooLarge));
}

#[test]
fn filter_work_is_added_up_over_the_whole_drawing() {
    let _alone = alone();
    // Each rectangle is 80 000 steps and passes alone; two are past the ceiling.
    let one = turbulent(1, 100, 80);
    assert!(rasterise(one.as_bytes(), 100, &[]).is_ok());
    let two = turbulent(2, 100, 80);
    assert_eq!(rasterise(two.as_bytes(), 200, &[]), Err(Refused::TooLarge));
}

#[test]
fn a_morphology_and_a_convolution_are_counted_by_the_window_each_pixel_reads() {
    let _alone = alone();
    // A morphology radius 4 reads 9 by 9 pixels for each of 100.
    let small = filtered(r#"<feMorphology radius="4"/>"#, 1, 10, 10);
    assert!(rasterise(small.as_bytes(), 10, &[]).is_ok());
    // A radius no region can use is a window as wide as the region, and
    // twenty regions of it are past the ceiling.
    let wide = filtered(r#"<feMorphology radius="65830"/>"#, 20, 10, 10);
    assert_eq!(rasterise(wide.as_bytes(), 200, &[]), Err(Refused::TooLarge));
    let kernel = ["1"; 81].join(" ");
    let convolved = filtered(
        &format!(r#"<feConvolveMatrix order="9" kernelMatrix="{kernel}"/>"#),
        1,
        10,
        10,
    );
    assert!(rasterise(convolved.as_bytes(), 10, &[]).is_ok());
}

#[test]
fn a_merge_is_counted_by_the_inputs_it_draws() {
    let _alone = alone();
    // The renderer draws the whole region once for every input of a merge, and
    // the inputs are limited only by the size of the file. A region of 100
    // pixels is 100 steps for each, and 20 more for the rectangle painted:
    // 1 399 inputs are 139 920 steps, and one more is past the ceiling.
    let merged = |inputs: usize| {
        let nodes = r#"<feMergeNode in="SourceGraphic"/>"#.repeat(inputs);
        filtered(&format!("<feMerge>{nodes}</feMerge>"), 1, 10, 10)
    };
    assert!(rasterise(merged(1_399).as_bytes(), 10, &[]).is_ok());
    assert_eq!(
        rasterise(merged(1_400).as_bytes(), 10, &[]),
        Err(Refused::TooLarge)
    );
    // A merge with no input at all is still one step, not none.
    assert!(rasterise(merged(0).as_bytes(), 10, &[]).is_ok());
}

/// A page 100 by 140 pixels with `count` copies of `shape` on it, one after
/// the other: 14 000 pixels each when the shape covers the page, which is 875
/// steps of work.
fn covered(count: usize, shape: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="140">{}</svg>"#,
        shape.repeat(count)
    )
}

#[test]
fn a_shape_is_counted_by_the_pixels_it_covers_inside_the_canvas() {
    let _alone = alone();
    // One hundred and sixty shapes of 875 steps are 140 000 steps, and one
    // more is past it.
    let page = r#"<rect width="100" height="140"/>"#;
    assert!(rasterise(covered(160, page).as_bytes(), 100, &[]).is_ok());
    assert_eq!(
        rasterise(covered(161, page).as_bytes(), 100, &[]),
        Err(Refused::TooLarge)
    );
    // Only what is on the canvas counts: a shape far larger than the page
    // covers the same 14 000 pixels, and the ceiling is reached at the same
    // shape.
    let beyond = r#"<rect x="-5000" y="-5000" width="10000" height="10000"/>"#;
    assert!(rasterise(covered(160, beyond).as_bytes(), 100, &[]).is_ok());
    assert_eq!(
        rasterise(covered(161, beyond).as_bytes(), 100, &[]),
        Err(Refused::TooLarge)
    );
    // A shape off the canvas covers nothing, and costs only what any shape
    // does.
    let off = r#"<rect x="500" width="100" height="140"/>"#;
    assert!(rasterise(covered(1_000, off).as_bytes(), 100, &[]).is_ok());
    // A shape that reaches one pixel is one step by its pixels, and the 20
    // every shape costs.
    let dot = r#"<rect width="1" height="1"/>"#;
    let tree = usvg::Tree::from_str(&covered(100, dot), &usvg::Options::default())
        .expect("a page of dots");
    let cost = budget::cost(&tree, Transform::identity(), 100, 140).expect("within the ceilings");
    assert_eq!(cost.work, 100 * 20);
}

#[test]
fn a_stroke_and_a_fill_are_counted_apart() {
    let _alone = alone();
    // A stroke with no fill covers the pixels the stroke reaches.
    let stroked = r#"<path d="M0 70H100" fill="none" stroke="black" stroke-width="140"/>"#;
    assert!(rasterise(covered(160, stroked).as_bytes(), 100, &[]).is_ok());
    assert_eq!(
        rasterise(covered(161, stroked).as_bytes(), 100, &[]),
        Err(Refused::TooLarge)
    );
    // A shape with both is counted twice: eighty are 140 000 steps, and one
    // more is not.
    let both = r#"<rect width="100" height="140" stroke="black"/>"#;
    assert!(rasterise(covered(80, both).as_bytes(), 100, &[]).is_ok());
    assert_eq!(
        rasterise(covered(81, both).as_bytes(), 100, &[]),
        Err(Refused::TooLarge)
    );
}

/// A page 100 pixels square with one line across it, stroked `width` wide and
/// dashed by `array`, as long as `length`.
fn dashed(length: &str, width: u32, array: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><path d="M0 50H{length}" fill="none" stroke="black" stroke-width="{width}" stroke-dasharray="{array}"/></svg>"#
    )
}

/// The work of a drawing of `svg` on a canvas 100 pixels square, or why it is
/// refused.
fn work_of(svg: &str) -> Result<u64, Refused> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).expect("a drawing");
    budget::cost(&tree, Transform::identity(), 100, 100).map(|cost| cost.work)
}

#[test]
fn a_dashed_stroke_is_counted_by_the_dashes_it_makes() {
    let _alone = alone();
    // A line 100 long in dashes of 5 and gaps of 5 is ten periods of two
    // entries. Each costs 8 steps, and 32 more for the second pixel across the
    // stroke (2 wide), which is more than the 14 pixels one dash can cover is
    // worth, and the bounding box of the stroke is not counted as well.
    assert_eq!(work_of(&dashed("100", 2, "5 5")), Ok(10 * 2 * 40));
    // An odd list is repeated, so its period and its entries double.
    assert_eq!(work_of(&dashed("100", 2, "5")), Ok(10 * 2 * 40));
    // A stroke no wider than a pixel is the 8 steps and no more.
    assert_eq!(work_of(&dashed("100", 1, "5 5")), Ok(10 * 2 * 8));
    // The line is counted whole, though most of it is off the canvas: 1 750
    // periods are 140 000 steps, and one more is past the ceiling.
    let at = dashed("17500", 2, "5 5");
    assert_eq!(work_of(&at), Ok(1_750 * 2 * 40));
    assert!(rasterise(at.as_bytes(), 100, &[]).is_ok());
    let past = dashed("17501", 2, "5 5");
    assert_eq!(work_of(&past), Err(Refused::TooLarge));
    assert_eq!(rasterise(past.as_bytes(), 100, &[]), Err(Refused::TooLarge));
}

#[test]
fn a_thin_dash_is_counted_by_the_width_it_is_drawn_across() {
    let _alone = alone();
    // Dashes of 0.3 across a stroke 10 wide cover 103 pixels each, which is
    // 7 steps by their area, but the renderer works the whole width of each:
    // 334 entries at 8 steps and 32 for each of the 9 pixels beyond the first.
    assert_eq!(work_of(&dashed("100", 10, "0.3")), Ok(334 * 296));
    // A dash long enough to cover more than that is counted by its area: 602
    // by 2 pixels reaches the 10 000 of the canvas in 76 steps, not 40.
    assert_eq!(work_of(&dashed("100", 2, "600")), Ok(2 * 76));
}

#[test]
fn a_dash_count_no_number_can_hold_is_refused() {
    let _alone = alone();
    // Dashes of a thousand-millionth of a millionth along a line of a million
    // millions are more than any count can say, and the drawing is refused
    // rather than counted as nothing.
    assert_eq!(work_of(&dashed("1e30", 2, "1e-30")), Err(Refused::TooLarge));
    // A list that adds up to nothing is not a dash: the stroke is solid, and
    // is counted by what it covers, 13 steps, or by the 20 any shape costs.
    assert_eq!(work_of(&dashed("100", 2, "0 0")), Ok(20));
}

/// A page 100 pixels square clipped by `count` rectangles the size of the
/// page, with one rectangle drawn through the clip.
fn clipped(count: usize) -> String {
    let rectangles = r#"<rect width="100" height="100"/>"#.repeat(count);
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><clipPath id="c">{rectangles}</clipPath><g clip-path="url(#c)"><rect width="100" height="100" fill="#000"/></g></svg>"##
    )
}

#[test]
fn a_path_in_a_clip_is_counted_by_the_pixels_it_fills() {
    let _alone = alone();
    // The clip is drawn on a pixmap the size of the layer, two pixels wider
    // than the page each side, and each rectangle fills the 10 000 pixels of
    // the page on it: 625 steps, as the rectangle drawn through it is.
    let one = work_of(&clipped(1)).expect("one rectangle");
    let two = work_of(&clipped(2)).expect("two rectangles");
    assert_eq!(two - one, 625);
    // So 223 rectangles in the clip and the one drawn are 140 000 steps, and
    // one more is past the ceiling.
    assert!(rasterise(clipped(223).as_bytes(), 100, &[]).is_ok());
    assert_eq!(
        rasterise(clipped(224).as_bytes(), 100, &[]),
        Err(Refused::TooLarge)
    );
}

#[test]
fn every_shape_costs_its_overhead_whatever_it_covers() {
    let _alone = alone();
    // A shape off the canvas covers nothing and is still built and drawn:
    // 20 steps each.
    let off = r#"<rect x="500" width="100" height="100"/>"#.repeat(1_000);
    let page =
        format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">{off}</svg>"#);
    assert_eq!(work_of(&page), Ok(20_000));
    assert!(rasterise(page.as_bytes(), 100, &[]).is_ok());
    // Five levels of ten uses of the level below are 100 000 shapes off the
    // canvas from a file of 1 419 bytes.
    let nested = nested_uses(5);
    assert_eq!(nested.len(), 1_419);
    assert_eq!(
        rasterise(nested.as_bytes(), 100, &[]),
        Err(Refused::TooLarge)
    );
    // Two levels are 100 shapes, and drawn.
    assert!(rasterise(nested_uses(2).as_bytes(), 100, &[]).is_ok());
}

/// A page 100 pixels square whose `<defs>` hold a rectangle off the page and
/// `levels` groups, each ten uses of the one before, and which uses the
/// group of the level `used`.
fn nested_uses_of(levels: u32, used: u32) -> String {
    let mut defined = String::from(r#"<rect id="l0" x="-50" width="1" height="1"/>"#);
    for level in 1..=levels {
        let ten = format!(r##"<use xlink:href="#l{}"/>"##, level - 1).repeat(10);
        let _ = write!(defined, r#"<g id="l{level}">{ten}</g>"#);
    }
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="100" height="100"><defs>{defined}</defs><use xlink:href="#l{used}"/></svg>"##
    )
}

/// [`nested_uses_of`] using its last level.
fn nested_uses(levels: u32) -> String {
    nested_uses_of(levels, levels)
}

/// Longer than counting the copies of any file takes, and shorter than
/// making those of the files counted here: 100 ms.
const SOON_REFUSED: Duration = Duration::from_millis(100);

/// What drawing `svg` 100 wide comes to, and how long it took to say so.
fn refused_in(svg: &str) -> (Result<Pixels, Refused>, Duration) {
    let began = Instant::now();
    let drawn = rasterise(svg.as_bytes(), 100, &[]);
    (drawn, began.elapsed())
}

#[test]
fn the_copies_uses_would_make_are_counted_before_any_is_made() {
    let _alone = alone();
    // Five levels of ten uses are 111 110 copies, and six are 1 111 110, past
    // the parser's own count of what it will make: both are refused before a
    // copy is made, in a moment however many there would be.
    for levels in [5, 6] {
        let (drawn, took) = refused_in(&nested_uses(levels));
        assert_eq!(drawn, Err(Refused::TooLarge), "{levels} levels");
        assert!(
            took < SOON_REFUSED,
            "{levels} levels were copied before they were refused: {took:?}"
        );
    }
    // A use in the definitions is copied whether or not anything draws it:
    // five levels defined and the second used are refused too.
    let (drawn, took) = refused_in(&nested_uses_of(5, 2));
    assert_eq!(drawn, Err(Refused::TooLarge), "five defined, two used");
    assert!(took < SOON_REFUSED, "copied first: {took:?}");
    // Three levels are 1 110 copies, and drawn.
    assert!(rasterise(nested_uses(3).as_bytes(), 100, &[]).is_ok());
}

/// A page 100 pixels square that uses `copies` times, off the page, the one
/// element its `<defs>` hold.
fn copied(element: &str, copies: usize) -> String {
    let uses = r##"<use xlink:href="#copied" x="-200"/>"##.repeat(copies);
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="100" height="100"><defs>{element}</defs>{uses}</svg>"#
    )
}

#[test]
fn a_copy_is_counted_by_the_text_and_the_attributes_it_copies() {
    let _alone = alone();
    // A thousand letters are shaped again for every copy of them: a hundred
    // copies are refused, and one is drawn.
    let letters = format!(
        r#"<text id="copied" font-size="10">{}</text>"#,
        "x".repeat(1_000)
    );
    let (drawn, took) = refused_in(&copied(&letters, 100));
    assert_eq!(drawn, Err(Refused::TooLarge), "a hundred copies of text");
    assert!(took < SOON_REFUSED, "shaped first: {took:?}");
    assert!(rasterise(copied(&letters, 1).as_bytes(), 100, &[]).is_ok());
    // A path's data is read again for every copy of it: fifty copies of
    // 16 000 segments are refused, and one is drawn.
    let mut data = String::from("M0 0");
    for at in 0..16_000_u32 {
        let _ = write!(data, "L{} {}", at % 100, at * 7 % 100);
    }
    let path = format!(r#"<path id="copied" d="{data}"/>"#);
    let (drawn, took) = refused_in(&copied(&path, 50));
    assert_eq!(drawn, Err(Refused::TooLarge), "fifty copies of a path");
    assert!(took < SOON_REFUSED, "read first: {took:?}");
    assert!(rasterise(copied(&path, 1).as_bytes(), 100, &[]).is_ok());
}

/// A page `rows` pixels tall and 100 wide holding one filled path of `edges`
/// edges, each from the top of the page to the bottom or back.
fn zigzag(edges: usize, rows: u32) -> String {
    let mut data = String::from("M0 0");
    for at in 0..edges {
        #[allow(clippy::cast_precision_loss, reason = "a small count")]
        let x = at as f64 * 100.0 / edges as f64;
        let _ = write!(data, "L{x:.3} {}", if at % 2 == 0 { rows } else { 0 });
    }
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="{rows}" viewBox="0 0 100 {rows}"><path d="{data}"/></svg>"#
    )
}

#[test]
fn a_path_is_counted_by_the_rows_each_segment_crosses() {
    let _alone = alone();
    // Four segments, the move and the closing line among them, crossing 1,
    // 100, 100 and 1 rows: 4 times 300 ns and 202 times 100, in steps of
    // 140 ns, which is more than the 200 pixels it covers are worth.
    let one = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><path d="M0 0L1 100L2 0"/></svg>"#;
    assert_eq!(work_of(one), Ok(153));
    // Ten thousand edges along one row are drawn, and two thousand that each
    // cross a hundred rows are not, though they cover the same page.
    let flat = zigzag(10_000, 1);
    assert!(rasterise(flat.as_bytes(), 100, &[]).is_ok());
    let tall = zigzag(2_000, 100);
    assert_eq!(rasterise(tall.as_bytes(), 100, &[]), Err(Refused::TooLarge));
}

/// A page `width` pixels wide and one tall holding one filled path of `edges`
/// edges across it, each from the top of the row to the bottom or back.
fn along(edges: usize, width: u32) -> String {
    let mut data = String::from("M0 0");
    for at in 0..edges {
        #[allow(clippy::cast_precision_loss, reason = "a small count")]
        let x = at as f64 * f64::from(width) / edges as f64;
        let _ = write!(data, "L{x:.3} {}", u8::from(at % 2 == 0));
    }
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="1"><path d="{data}"/></svg>"#
    )
}

#[test]
fn edges_crowded_along_a_row_are_counted_by_the_pixels_each_can_walk() {
    let _alone = alone();
    // Edges closer together than a pixel make spans that each start the
    // renderer's walk along the row over again: 46 000 across a row 9 300
    // wide, and 30 000 across one 6 000 wide, are refused.
    let wide = along(46_000, 9_300);
    assert_eq!(wide.len(), 500_594);
    assert_eq!(
        rasterise(wide.as_bytes(), 9_300, &[]),
        Err(Refused::TooLarge)
    );
    let narrower = along(30_000, 6_000);
    assert_eq!(narrower.len(), 324_536);
    assert_eq!(
        rasterise(narrower.as_bytes(), 6_000, &[]),
        Err(Refused::TooLarge)
    );
    // A thousand across the wide row are drawn.
    assert!(rasterise(along(1_000, 9_300).as_bytes(), 9_300, &[]).is_ok());
}

#[test]
fn a_dashed_stroke_is_counted_by_its_segments_too() {
    let _alone = alone();
    // Ten thousand contours shorter than a pixel, stroked across the page,
    // cut into a dash each: few dashes by the length of the path, and every
    // segment worked across the rows of the stroke.
    let contours = "M50 0h.01".repeat(10_000);
    let solid = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><path d="{contours}" fill="none" stroke="#000" stroke-width="100"/></svg>"##
    );
    let dashed = solid.replace("/>", r#" stroke-dasharray="1000 1000"/>"#);
    assert_eq!(
        rasterise(dashed.as_bytes(), 100, &[]),
        Err(Refused::TooLarge)
    );
    assert_eq!(
        rasterise(solid.as_bytes(), 100, &[]),
        Err(Refused::TooLarge)
    );
}

/// A page `side` pixels square filled with a linear gradient of `count` stops.
fn graded(count: usize, side: u32) -> String {
    let stops = (0..count).fold(String::new(), |mut all, at| {
        #[allow(clippy::cast_precision_loss, reason = "a small count")]
        let offset = at as f64 / count as f64;
        let _ = write!(
            all,
            r##"<stop offset="{offset:.6}" stop-color="#{:06x}"/>"##,
            (at * 2_654_435_761) & 0x00ff_ffff
        );
        all
    });
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{side}" height="{side}"><linearGradient id="g">{stops}</linearGradient><rect width="{side}" height="{side}" fill="url(#g)"/></svg>"#
    )
}

#[test]
fn a_gradient_is_counted_by_its_stops_over_the_pixels_it_paints() {
    let _alone = alone();
    // The renderer works every stop at every pixel: 512 stops over the 10 000
    // pixels of the page are 10 000 steps, where the pixels alone are 625.
    assert_eq!(work_of(&graded(512, 100)), Ok(10_000));
    // A thousand stops over 500 by 500 pixels are refused, and two are drawn.
    assert_eq!(
        rasterise(graded(1_000, 500).as_bytes(), 500, &[]),
        Err(Refused::TooLarge)
    );
    assert!(rasterise(graded(2, 500).as_bytes(), 500, &[]).is_ok());
}
