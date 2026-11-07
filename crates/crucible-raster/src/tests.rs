use super::*;
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
