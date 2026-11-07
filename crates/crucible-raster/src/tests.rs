use super::*;
use resvg::usvg;
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
