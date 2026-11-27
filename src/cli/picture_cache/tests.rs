//! What the cache keeps, for how long, for whom, and what it refuses to read
//! back.

use super::*;
use crate::cli::pictures::Raster;
use crate::cli::sample::Sample;
use crucible_tui::drawing::{Bucket, Svg};
use crucible_tui::graphics::{Cell, Pictures as _};
use crucible_tui::markdown::Kind;
use crucible_tui::{Design, Palette, Theme};
use std::fs;
use std::sync::PoisonError;
use std::sync::atomic::Ordering;
use std::time::{Instant, SystemTime};

/// The cell every key here is fitted to.
const CELL: Cell = Cell {
    width: 8,
    height: 16,
};

/// What every block here was written as.
const SOURCE: &str = "```mermaid\ngraph TD\n  A --> B\n```\n";

/// A key for a Kitty terminal in the calm design, in truecolour, with no
/// ground reported, in cells of [`CELL`].
fn key() -> PictureKey {
    key_in(Design::Calm, true, None, CELL)
}

/// A key in `design`, in truecolour or else 256 colours, against `ground`, in
/// cells of `cell`.
fn key_in(
    design: Design,
    truecolour: bool,
    ground: Option<(u8, u8, u8)>,
    cell: Cell,
) -> PictureKey {
    let depth = move |name: &str| match (truecolour, name) {
        (true, "COLORTERM") => Some("truecolor".to_owned()),
        (false, "TERM") => Some("xterm-256color".to_owned()),
        _ => None,
    };
    PictureKey {
        kind: Kind::Mermaid,
        bucket: Bucket::of(80),
        palette: Palette::resolve(true, Theme::Dark, ground, &depth)
            .designing(design)
            .identity(),
        protocol: Protocol::Kitty,
        cell,
        rows: crucible_tui::MOST_PLACED,
    }
}

/// A picture `width` by `height` pixels.
fn form(width: u32, height: u32) -> Form {
    Form::Svg(
        Svg::new(format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\">\
             <rect class=\"Accent\" width=\"{width}\" height=\"{height}\" fill=\"currentColor\"/>\
             </svg>"
        ))
        .unwrap(),
    )
}

/// `form`'s picture under `key`, as the session draws it.
fn drawn(key: &PictureKey, form: &Form) -> Payload {
    Raster.make(SOURCE, key, form).unwrap()
}

/// A home for one test, with nothing in it yet, and the sample that holds
/// it.
fn home(name: &str) -> (Sample, PathBuf) {
    let sample = Sample::new(&format!("picture-cache-{name}"));
    let home = sample.root().join("home");
    fs::create_dir_all(&home).unwrap();
    (sample, home)
}

/// The entry for `form`'s picture of `source` under `key`.
fn entry(source: &str, key: &PictureKey, form: &Form) -> Entry {
    Entry::of(source, key, form, &drawn(key, form)).unwrap()
}

/// `N` entries of as many blocks, under one key.
fn several<const N: usize>() -> [Entry; N] {
    std::array::from_fn(|at| entry(&format!("{SOURCE}{at}"), &key(), &form(40, 20)))
}

/// The bytes the largest of `written` takes.
fn largest(written: &[Entry]) -> u64 {
    written.iter().map(Entry::length).max().unwrap()
}

/// The names of the entries in `cache`, sorted.
fn entries(cache: &Cache) -> Vec<String> {
    let Ok(listed) = fs::read_dir(&cache.directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = listed
        .map(|listed| listed.unwrap().file_name().into_string().unwrap())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names
}

/// The bytes the entries of `cache` hold together; a file of another name is
/// not one.
fn held(cache: &Cache) -> u64 {
    entries(cache)
        .iter()
        .filter(|name| is_entry(name))
        .map(|name| fs::metadata(cache.directory.join(name)).unwrap().len())
        .sum()
}

/// Says `name` in `cache` was written `ago`.
fn aged(cache: &Cache, name: &str, ago: Duration) {
    fs::File::options()
        .write(true)
        .open(cache.directory.join(name))
        .unwrap()
        .set_modified(SystemTime::now() - ago)
        .unwrap();
}

/// Says `name` in `cache` was written `by` from now, as a clock set wrong,
/// or a file planted, would.
fn ahead(cache: &Cache, name: &str, by: Duration) {
    fs::File::options()
        .write(true)
        .open(cache.directory.join(name))
        .unwrap()
        .set_modified(SystemTime::now() + by)
        .unwrap();
}

/// Holds the one decode slot for a test, which drawing and reading take.
fn turn() -> std::sync::MutexGuard<'static, ()> {
    crate::cli::pictures::tests::ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

#[test]
fn a_picture_put_is_got_back_with_the_form_it_was_drawn_from() {
    let _turn = turn();
    let (_sample, home) = home("whole");
    let cache = Cache::open(&home);
    for protocol in [Protocol::Kitty, Protocol::Iterm2, Protocol::Sixel] {
        let key = PictureKey { protocol, ..key() };
        let form = form(40, 20);
        let payload = drawn(&key, &form);
        cache.put(&Entry::of(SOURCE, &key, &form, &payload).unwrap());

        let (kept, again) = cache.get(SOURCE, &key).expect("the entry just put");
        let (Form::Svg(kept), Form::Svg(was)) = (&kept, &form) else {
            panic!("{protocol:?}: kept {kept:?}");
        };
        assert_eq!(kept.as_str(), was.as_str(), "{protocol:?}");
        assert_eq!(again.protocol(), protocol);
        assert_eq!(again.bytes(), payload.bytes(), "{protocol:?}");
        assert_eq!(again.pixels(), payload.pixels(), "{protocol:?}");
        assert_eq!(again.cells(), payload.cells(), "{protocol:?}");
    }
    assert_eq!(entries(&cache).len(), 3);
    // Another block's source is another entry, and not this one.
    assert!(cache.get("```mermaid\ngraph LR\n```\n", &key()).is_none());
}

/// Two blocks of one source under `one` and `other`, put in a cache of their
/// own: two entries, each read back as its own picture.
fn kept_apart(name: &str, one: &PictureKey, other: &PictureKey) {
    let _turn = turn();
    assert_ne!(one, other, "{name}: the keys are one key");
    let (_sample, home) = home(name);
    let cache = Cache::open(&home);
    let form = form(40, 20);
    let (first, second) = (drawn(one, &form), drawn(other, &form));
    cache.put(&Entry::of(SOURCE, one, &form, &first).unwrap());
    cache.put(&Entry::of(SOURCE, other, &form, &second).unwrap());

    assert_eq!(entries(&cache).len(), 2, "{name}: {:?}", entries(&cache));
    let (_, got) = cache.get(SOURCE, one).unwrap();
    assert_eq!(got.bytes(), first.bytes(), "{name}");
    let (_, got) = cache.get(SOURCE, other).unwrap();
    assert_eq!(got.bytes(), second.bytes(), "{name}");
}

#[test]
fn two_blocks_differing_only_in_design_are_two_entries() {
    kept_apart(
        "design",
        &key_in(Design::Calm, true, None, CELL),
        &key_in(Design::Rich, true, None, CELL),
    );
}

#[test]
fn two_blocks_differing_only_in_colour_depth_are_two_entries() {
    kept_apart(
        "depth",
        &key_in(Design::Calm, true, None, CELL),
        &key_in(Design::Calm, false, None, CELL),
    );
}

#[test]
fn two_blocks_differing_only_in_ground_are_two_entries() {
    kept_apart(
        "ground",
        &key_in(Design::Calm, true, Some((0, 0, 0)), CELL),
        &key_in(Design::Calm, true, Some((255, 255, 255)), CELL),
    );
}

#[test]
fn two_blocks_differing_only_in_cell_pixel_size_are_two_entries() {
    kept_apart(
        "cell",
        &key_in(Design::Calm, true, None, CELL),
        &key_in(
            Design::Calm,
            true,
            None,
            Cell {
                width: 10,
                height: 20,
            },
        ),
    );
}

#[test]
fn an_image_file_is_never_queued_or_kept() {
    let _turn = turn();
    let (_sample, home) = home("image");
    let cache = Cache::open(&home);
    let (queue, writer) = writing(cache.clone());
    let png = crucible_raster::png(
        &crucible_raster::rasterise(
            b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"8\" height=\"8\"/>",
            8,
            &[],
        )
        .unwrap(),
    )
    .unwrap();
    let image = Form::Png(png);
    let payload = Raster.make("plot.png", &key(), &image).unwrap();

    assert!(Entry::of("plot.png", &key(), &image, &payload).is_none());
    assert!(!queue.put("plot.png", &key(), &image, &payload));
    writer.drain();
    assert!(entries(&cache).is_empty(), "{:?}", entries(&cache));
}

#[test]
fn every_write_holds_the_cache_to_its_bound_oldest_out_first() {
    let _turn = turn();
    let (_sample, home) = home("bound");
    let mut cache = Cache::open(&home);
    let written: [Entry; 4] = several();
    let one = largest(&written);
    // Room for two and a half entries: a third makes room by taking the
    // oldest out before it lands.
    cache.bounds.held = one * 5 / 2;
    for (at, entry) in written.iter().enumerate() {
        cache.put(entry);
        assert!(held(&cache) <= cache.bounds.held, "past the bound at {at}");
        assert!(entries(&cache).contains(&entry.name), "{at} did not land");
        // Each older than the one after it, as writes some time apart are.
        aged(&cache, &entry.name, Duration::from_mins(10 - at as u64));
    }
    let [_, _, third, fourth] = &written;
    let mut newest = vec![third.name.clone(), fourth.name.clone()];
    newest.sort();
    assert_eq!(
        entries(&cache),
        newest,
        "the oldest were not the ones taken"
    );
}

#[test]
fn a_write_takes_out_every_entry_past_its_age() {
    let _turn = turn();
    let (_sample, home) = home("age");
    let cache = Cache::open(&home);
    let (old, fresh, new) = (
        entry("old", &key(), &form(40, 20)),
        entry("fresh", &key(), &form(40, 20)),
        entry("new", &key(), &form(40, 20)),
    );
    cache.put(&old);
    cache.put(&fresh);
    aged(&cache, &old.name, MOST_AGE + Duration::from_mins(1));
    aged(
        &cache,
        &fresh.name,
        MOST_AGE.saturating_sub(Duration::from_hours(1)),
    );
    cache.put(&new);

    let mut kept = vec![fresh.name.clone(), new.name.clone()];
    kept.sort();
    assert_eq!(entries(&cache), kept);
}

#[test]
fn a_write_takes_out_every_entry_dated_more_than_a_day_ahead() {
    let _turn = turn();
    let (_sample, home) = home("ahead");
    let cache = Cache::open(&home);
    let (planted, skewed, new) = (
        entry("planted", &key(), &form(40, 20)),
        entry("skewed", &key(), &form(40, 20)),
        entry("new", &key(), &form(40, 20)),
    );
    cache.put(&planted);
    cache.put(&skewed);
    // Dated a month ahead it would never age out; an hour ahead is a clock
    // a little out, and is kept.
    ahead(&cache, &planted.name, Duration::from_hours(30 * 24));
    ahead(&cache, &skewed.name, Duration::from_hours(1));
    cache.put(&new);

    let mut kept = vec![skewed.name.clone(), new.name.clone()];
    kept.sort();
    assert_eq!(entries(&cache), kept);
}

#[test]
fn an_entry_past_its_age_or_dated_more_than_a_day_ahead_is_a_miss() {
    let _turn = turn();
    let (_sample, home) = home("stale");
    let cache = Cache::open(&home);
    let form = form(40, 20);
    let case = |source: &str, date: &dyn Fn(&Cache, &str), hit: bool| {
        let written = entry(source, &key(), &form);
        cache.put(&written);
        date(&cache, &written.name);
        // No write between, so only the read can tell it is stale.
        assert_eq!(cache.get(source, &key()).is_some(), hit, "{source}");
    };
    case(
        "old",
        &|cache, name| aged(cache, name, MOST_AGE + Duration::from_mins(1)),
        false,
    );
    case(
        "ahead",
        &|cache, name| ahead(cache, name, Duration::from_hours(30 * 24)),
        false,
    );
    case(
        "aging",
        &|cache, name| {
            aged(
                cache,
                name,
                MOST_AGE.saturating_sub(Duration::from_hours(1)),
            );
        },
        true,
    );
    case(
        "skewed",
        &|cache, name| ahead(cache, name, Duration::from_hours(1)),
        true,
    );
}

#[test]
fn a_write_holds_no_more_names_than_its_bound_and_takes_the_oldest_of_them_out() {
    let _turn = turn();
    let (_sample, home) = home("names");
    let mut cache = Cache::open(&home);
    // Two hundred entries' names, a kilobyte each and each older than the
    // one before it, against a bound of 150 kB: fifty and more must go.
    let names: Vec<String> = (0..200_u32)
        .map(|at| format!("{:064x}", u64::from(at) + 1))
        .collect();
    for (at, name) in names.iter().enumerate() {
        written(&cache, name, &[0; 1000]);
        aged(&cache, name, Duration::from_mins(1000 - at as u64));
    }
    cache.bounds.held = 150_000;
    cache.bounds.names = 8;

    let new = entry("new", &key(), &form(40, 20));
    cache.put(&new);

    // Only the eight oldest were held, so only they were taken out, and the
    // next write takes out the next eight.
    let left = entries(&cache);
    let gone: Vec<&String> = names.iter().filter(|name| !left.contains(name)).collect();
    assert_eq!(gone, names.iter().take(8).collect::<Vec<_>>());
    assert!(left.contains(&new.name));
    cache.put(&entry("newer", &key(), &form(40, 20)));
    assert_eq!(entries(&cache).len(), 200 - 16 + 2);
}

#[test]
fn trim_at_start_takes_the_cache_back_within_its_bounds() {
    let _turn = turn();
    let (_sample, home) = home("trim");
    let mut cache = Cache::open(&home);
    let written: [Entry; 4] = several();
    for (at, entry) in written.iter().enumerate() {
        cache.put(entry);
        aged(&cache, &entry.name, Duration::from_mins(10 - at as u64));
    }
    let [first, _, third, fourth] = &written;
    // What another build, or another bound, left: every entry is over this
    // one's, and the oldest is past its age.
    aged(&cache, &first.name, MOST_AGE + Duration::from_secs(1));
    fs::write(cache.directory.join("kept by someone else"), b"x").unwrap();
    cache.bounds.held = largest(&written) * 2;

    cache.trim();

    assert!(held(&cache) <= cache.bounds.held);
    let mut newest = vec![third.name.clone(), fourth.name.clone()];
    newest.push("kept by someone else".to_owned());
    newest.sort();
    assert_eq!(entries(&cache), newest);
}

#[test]
fn trim_at_start_removes_the_drafts_a_write_left_half_done() {
    let _turn = turn();
    let (_sample, home) = home("drafts");
    let cache = Cache::open(&home);
    let kept = entry(SOURCE, &key(), &form(40, 20));
    cache.put(&kept);
    let draft = cache.directory.join(".crucible-0123456789abcdef.writing");
    fs::write(&draft, kept.bytes.get(..20).unwrap()).unwrap();

    cache.trim();

    assert!(!draft.exists(), "a draft was left");
    assert_eq!(entries(&cache), std::slice::from_ref(&kept.name));
    assert!(cache.get(SOURCE, &key()).is_some());
}

#[test]
fn two_processes_writing_at_once_each_land_at_most_one_entry_past_the_bound() {
    let _turn = turn();
    let (_sample, home) = home("two");
    let mut one = Cache::open(&home);
    let written: [Entry; 5] = several();
    let most = largest(&written);
    one.bounds.held = most * 3;
    let other = one.clone();
    let [first, second, third, fourth, fifth] = &written;
    for (at, entry) in [first, second].into_iter().enumerate() {
        one.put(entry);
        aged(&one, &entry.name, Duration::from_mins(10 - at as u64));
    }

    // Each makes room for its own entry before either lands, so neither saw
    // the other's: both land, one entry each past the bound.
    one.make_room(third);
    other.make_room(fourth);
    one.land(third);
    other.land(fourth);
    assert_eq!(entries(&one).len(), 4);
    assert!(held(&one) <= one.bounds.held + 2 * most);
    assert!(held(&one) > one.bounds.held, "neither overshot");

    // The next write takes it back within the bound.
    aged(&one, &third.name, Duration::from_mins(8));
    aged(&one, &fourth.name, Duration::from_mins(7));
    other.put(fifth);
    assert!(held(&one) <= one.bounds.held);
    assert!(entries(&one).contains(&fifth.name));
}

#[test]
fn two_writers_replacing_one_entry_leave_it_whole_for_each_read_and_a_prune_beside_them() {
    use std::sync::atomic::AtomicBool;
    let _turn = turn();
    let (_sample, home) = home("same");
    let one = Cache::open(&home);
    let other = one.clone();
    let reader = one.clone();
    let (wide, narrow) = (form(40, 20), form(30, 10));
    let (first, second) = (entry(SOURCE, &key(), &wide), entry(SOURCE, &key(), &narrow));
    assert_eq!(first.name, second.name, "one key, one name");
    let whole = [
        drawn(&key(), &wide).bytes().to_vec(),
        drawn(&key(), &narrow).bytes().to_vec(),
    ];
    one.put(&first);

    // Each write lands whole under the name, so a read while both replace it
    // finds one of the two whole, and never a mixture.
    let writing = AtomicBool::new(true);
    let mut read = (0_usize, 0_usize);
    std::thread::scope(|scope| {
        let writers = [(&one, &first), (&other, &second)].map(|(cache, entry)| {
            scope.spawn(move || {
                for _ in 0..200 {
                    cache.put(entry);
                }
            })
        });
        let watching = scope.spawn(|| {
            let (mut reads, mut hits) = (0_usize, 0_usize);
            loop {
                // A read that opened the entry a rename then replaced is a
                // miss, as anything not one whole entry is.
                if let Some((_, payload)) = reader.get(SOURCE, &key()) {
                    assert!(whole.iter().any(|bytes| bytes == payload.bytes()));
                    hits += 1;
                }
                reads += 1;
                if !writing.load(Ordering::Acquire) {
                    break (reads, hits);
                }
            }
        });
        for writer in writers {
            writer.join().unwrap();
        }
        writing.store(false, Ordering::Release);
        read = watching.join().unwrap();
    });
    assert!(
        read.1 > 0,
        "of {} reads while they wrote, none found it",
        read.0
    );
    assert_eq!(entries(&one), std::slice::from_ref(&first.name));
    let (_, payload) = one.get(SOURCE, &key()).unwrap();
    assert!(whole.iter().any(|bytes| bytes == payload.bytes()));

    // A write of another picture that takes it out to make room, racing the
    // rename that replaces it, leaves no draft and nothing past the bound
    // but the one entry a race may, and the entry is written whole again.
    let mut tight = one.clone();
    let crowding: [Entry; 4] = several();
    tight.bounds.held = largest(&crowding);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for _ in 0..50 {
                for entry in &crowding {
                    tight.put(entry);
                }
            }
        });
        scope.spawn(|| {
            for _ in 0..200 {
                one.put(&first);
            }
        });
    });
    assert!(
        fs::read_dir(&one.directory)
            .unwrap()
            .all(|found| !is_draft(&found.unwrap().file_name())),
        "a draft was left"
    );
    assert!(held(&one) <= tight.bounds.held + first.length().max(second.length()));
    one.put(&first);
    let (_, payload) = one.get(SOURCE, &key()).unwrap();
    assert_eq!(payload.bytes(), whole.first().unwrap().as_slice());
}

#[cfg(unix)]
#[test]
fn the_directory_is_its_owners_alone_and_so_is_each_entry() {
    use std::os::unix::fs::PermissionsExt as _;
    let _turn = turn();
    let (_sample, home) = home("modes");
    let cache = Cache::open(&home);
    let kept = entry(SOURCE, &key(), &form(40, 20));
    cache.put(&kept);

    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&cache.directory), 0o700);
    assert_eq!(mode(&cache.directory.join(&kept.name)), 0o600);
}

/// Only the half a second call would not catch is asserted, as the session
/// log's tests do: a list with the wrong access on it shuts this account out
/// of its own cache, and the call that wrote it does not say so.
#[cfg(windows)]
#[test]
fn an_entry_stays_readable_by_the_account_that_wrote_it() {
    let _turn = turn();
    let (_sample, home) = home("list");
    let cache = Cache::open(&home);
    let kept = entry(SOURCE, &key(), &form(40, 20));
    cache.put(&kept);

    assert_eq!(
        fs::read(cache.directory.join(&kept.name)).unwrap(),
        kept.bytes
    );
    assert!(cache.get(SOURCE, &key()).is_some());
    assert_eq!(entries(&cache), std::slice::from_ref(&kept.name));
}

#[test]
fn a_truncated_entry_is_a_miss_until_the_next_write_replaces_it() {
    let _turn = turn();
    let (_sample, home) = home("truncated");
    let cache = Cache::open(&home);
    let kept = entry(SOURCE, &key(), &form(40, 20));
    cache.put(&kept);
    for cut in [0, 7, HEADER, kept.bytes.len() - 1] {
        fs::write(
            cache.directory.join(&kept.name),
            kept.bytes.get(..cut).unwrap(),
        )
        .unwrap();
        assert!(cache.get(SOURCE, &key()).is_none(), "cut at {cut} was read");
    }
    cache.put(&kept);
    assert!(cache.get(SOURCE, &key()).is_some(), "not replaced");
}

#[test]
fn a_tampered_entry_is_a_miss_until_the_next_write_replaces_it() {
    let _turn = turn();
    let (_sample, home) = home("tampered");
    let cache = Cache::open(&home);
    let kept = entry(SOURCE, &key(), &form(40, 20));
    cache.put(&kept);
    for at in [0, MAGIC.len(), HEADER - 1, HEADER, kept.bytes.len() - 1] {
        let mut tampered = kept.bytes.clone();
        *tampered.get_mut(at).unwrap() ^= 0x20;
        fs::write(cache.directory.join(&kept.name), &tampered).unwrap();
        assert!(
            cache.get(SOURCE, &key()).is_none(),
            "a change at {at} was read"
        );
    }
    cache.put(&kept);
    assert!(cache.get(SOURCE, &key()).is_some(), "not replaced");
}

#[test]
fn an_entry_under_another_keys_name_is_a_miss() {
    let _turn = turn();
    let (_sample, home) = home("misnamed");
    let cache = Cache::open(&home);
    let dark = key();
    let rich = key_in(Design::Rich, true, None, CELL);
    let one = entry(SOURCE, &dark, &form(40, 20));
    let other = entry(SOURCE, &rich, &form(40, 20));
    cache.put(&one);
    fs::copy(
        cache.directory.join(&one.name),
        cache.directory.join(&other.name),
    )
    .unwrap();

    assert!(cache.get(SOURCE, &rich).is_none(), "read under another key");
    assert!(cache.get(SOURCE, &dark).is_some());
    cache.put(&other);
    assert!(cache.get(SOURCE, &rich).is_some(), "not replaced");
}

#[test]
fn an_entry_larger_than_the_queue_takes_is_refused_before_it_is_read() {
    let _turn = turn();
    let (_sample, home) = home("oversize");
    let cache = Cache::open(&home);
    let form = form(40, 20);
    let payload = drawn(&key(), &form);
    // A picture that decodes as it says, with bytes after its end that make
    // the entry larger than any the writer is handed, sealed as one.
    let mut padded = payload.bytes().to_vec();
    padded.resize(MOST_QUEUED + 1, 0);
    let Form::Svg(svg) = &form else { panic!() };
    let digest = keyed(SOURCE, &key());
    let bytes = sealed(
        &digest,
        Protocol::Kitty,
        payload.pixels(),
        svg.as_str().as_bytes(),
        &padded,
    )
    .unwrap();
    fs::create_dir_all(&cache.directory).unwrap();
    fs::write(cache.directory.join(named(&digest)), &bytes).unwrap();

    assert!(
        cache.get(SOURCE, &key()).is_none(),
        "an oversize entry was read"
    );
}

#[test]
fn a_payload_that_is_not_what_its_protocol_says_is_a_miss() {
    let _turn = turn();
    let (_sample, home) = home("mislabelled");
    let cache = Cache::open(&home);
    let form = form(40, 20);
    let Form::Svg(svg) = &form else { panic!() };
    let kitty = drawn(&key(), &form);
    let sixel_key = PictureKey {
        protocol: Protocol::Sixel,
        ..key()
    };
    // Sealed as an entry is, so only what the payload is tells them apart.
    for (key, payload) in [
        (key(), b"not a picture".to_vec()),
        (key(), b"\x1bPq#0~~\x1b\\".to_vec()),
        (sixel_key, kitty.bytes().to_vec()),
        (sixel_key, b"\x1bPq#0~\x1b]52;c;x\x07~\x1b\\".to_vec()),
    ] {
        let digest = keyed(SOURCE, &key);
        let bytes = sealed(
            &digest,
            key.protocol,
            kitty.pixels(),
            svg.as_str().as_bytes(),
            &payload,
        )
        .unwrap();
        written(&cache, &named(&digest), &bytes);
        assert!(
            cache.get(SOURCE, &key).is_none(),
            "{:?} took {} bytes it is not sent",
            key.protocol,
            payload.len()
        );
    }
    // Pixels other than the picture's are another picture's.
    let digest = keyed(SOURCE, &key());
    let bytes = sealed(
        &digest,
        Protocol::Kitty,
        (41, 20),
        svg.as_str().as_bytes(),
        kitty.bytes(),
    )
    .unwrap();
    written(&cache, &named(&digest), &bytes);
    assert!(
        cache.get(SOURCE, &key()).is_none(),
        "the wrong size was read"
    );
}

/// Sealed as an entry is, `payload` under a Sixel key for `pixels`, with the
/// picture `form` is: only what the payload is can make it a miss.
fn sixel_sealed(cache: &Cache, form: &Form, pixels: (u32, u32), payload: &[u8]) -> PictureKey {
    let Form::Svg(svg) = form else { panic!() };
    let key = PictureKey {
        protocol: Protocol::Sixel,
        ..key()
    };
    let digest = keyed(SOURCE, &key);
    let bytes = sealed(
        &digest,
        key.protocol,
        pixels,
        svg.as_str().as_bytes(),
        payload,
    )
    .unwrap();
    written(cache, &named(&digest), &bytes);
    key
}

/// `bytes` written under `name` in `cache` as the writer writes an entry,
/// owner-only, so only what the bytes are can make it a miss.
fn written(cache: &Cache, name: &str, bytes: &[u8]) {
    crucible_privacy::directory(&cache.directory).unwrap();
    crucible_privacy::replace_in(&cache.directory, std::ffi::OsStr::new(name), bytes).unwrap();
}

#[test]
fn a_sixel_payload_the_terminal_would_not_be_sent_or_that_paints_past_its_pixels_is_a_miss() {
    let _turn = turn();
    let (_sample, home) = home("sixel-gate");
    let cache = Cache::open(&home);
    let form = form(40, 20);
    // A band of 2000 columns, a hundred times over: 2000 by 600 pixels.
    let wide = format!(
        "\x1bP0;1;0q\"1;1;1;1#0;2;0;0;0{}\x1b\\",
        "#0!2000~-".repeat(100)
    );
    for (pixels, payload, what) in [
        (
            (1, 1),
            "\x1bP$qm\x1b\\".to_owned(),
            "a request for a setting",
        ),
        (
            (1, 1),
            "\x1bP+q544e\x1b\\".to_owned(),
            "a request for a capability",
        ),
        (
            (1, 1),
            wide.clone(),
            "a 1 by 1 picture painting 2000 by 600",
        ),
        (
            (2000, 600),
            wide.replace("\"1;1;1;1", ""),
            "no raster attributes",
        ),
        (
            (1, 1),
            "\x1bP0;1;0q\"1;1;2;1#0~~-\x1b\\".to_owned(),
            "raster wider than kept",
        ),
        (
            (1, 1),
            "\x1bP0;1;0q\"1;1;1;1#0~-#0~-\x1b\\".to_owned(),
            "a band past its height",
        ),
        (
            (4, 1),
            "\x1bP0;1;0q\"1;1;4;1#0!5~-\x1b\\".to_owned(),
            "a repeat past its width",
        ),
    ] {
        let key = sixel_sealed(&cache, &form, pixels, payload.as_bytes());
        assert!(cache.get(SOURCE, &key).is_none(), "{what} was taken");
    }
    // What the live encoder wrote is still read back, whatever it repeats.
    let key = PictureKey {
        protocol: Protocol::Sixel,
        ..key()
    };
    for (width, height) in [(40, 20), (300, 7), (1, 1)] {
        let form = self::form(width, height);
        let payload = drawn(&key, &form);
        sixel_sealed(&cache, &form, payload.pixels(), payload.bytes());
        let (_, again) = cache
            .get(SOURCE, &key)
            .expect("the live encoder's own payload");
        assert_eq!(again.bytes(), payload.bytes());
    }
}

#[cfg(unix)]
#[test]
fn a_linked_directory_is_neither_read_nor_written_through() {
    let _turn = turn();
    let (sample, home) = home("linked-directory");
    let elsewhere = sample.root().join("elsewhere");
    let real = Cache::open(&elsewhere);
    let kept = entry(SOURCE, &key(), &form(40, 20));
    real.put(&kept);
    std::os::unix::fs::symlink(&real.directory, home.join(DIRECTORY)).unwrap();
    let cache = Cache::open(&home);

    assert!(cache.get(SOURCE, &key()).is_none(), "read through a link");
    let other = entry("another", &key(), &form(40, 20));
    cache.put(&other);
    cache.trim();
    assert_eq!(
        entries(&real),
        std::slice::from_ref(&kept.name),
        "written through a link"
    );
}

#[cfg(unix)]
#[test]
fn a_linked_entry_is_a_miss_and_the_next_write_replaces_the_link() {
    let _turn = turn();
    let (sample, home) = home("linked-entry");
    let elsewhere = Cache::open(&sample.root().join("elsewhere"));
    let kept = entry(SOURCE, &key(), &form(40, 20));
    elsewhere.put(&kept);
    let cache = Cache::open(&home);
    cache.put(&entry("another", &key(), &form(40, 20)));
    let at = cache.directory.join(&kept.name);
    std::os::unix::fs::symlink(elsewhere.directory.join(&kept.name), &at).unwrap();

    assert!(cache.get(SOURCE, &key()).is_none(), "read through a link");
    cache.put(&kept);
    assert!(
        !fs::symlink_metadata(&at).unwrap().is_symlink(),
        "the link stayed"
    );
    assert!(cache.get(SOURCE, &key()).is_some());
    assert_eq!(
        fs::read(elsewhere.directory.join(&kept.name)).unwrap(),
        kept.bytes,
        "written through a link"
    );
}

#[cfg(unix)]
#[test]
fn an_entry_or_a_directory_others_may_reach_is_a_miss() {
    use std::os::unix::fs::PermissionsExt as _;
    let _turn = turn();
    let (_sample, home) = home("reached");
    let cache = Cache::open(&home);
    let kept = entry(SOURCE, &key(), &form(40, 20));
    cache.put(&kept);
    let at = cache.directory.join(&kept.name);
    let opened = |path: &Path, mode: u32| {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    };

    opened(&at, 0o644);
    assert!(cache.get(SOURCE, &key()).is_none(), "an entry others read");
    opened(&at, 0o600);
    opened(&cache.directory, 0o755);
    assert!(
        cache.get(SOURCE, &key()).is_none(),
        "a directory others read"
    );
    cache.put(&kept);
    assert!(cache.get(SOURCE, &key()).is_some(), "not narrowed again");
}

#[cfg(unix)]
#[test]
fn a_directory_an_earlier_build_left_open_is_narrowed_before_it_is_first_read() {
    use std::os::unix::fs::PermissionsExt as _;
    let _turn = turn();
    let (_sample, home) = home("left-open");
    let kept = entry(SOURCE, &key(), &form(40, 20));
    Cache::open(&home).put(&kept);
    let directory = home.join(DIRECTORY);
    // Opened up as a build before this one, or a hand, might have left it.
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();

    // A process's first read narrows it, then takes what it holds.
    let cache = Cache::open(&home);
    assert!(cache.get(SOURCE, &key()).is_some(), "not narrowed first");
    let mode = fs::metadata(&directory).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}

#[cfg(unix)]
#[test]
fn only_what_its_owner_alone_may_reach_is_taken() {
    assert!(kept_alone(0o100_600, 501, 501));
    assert!(kept_alone(0o040_700, 501, 501));
    assert!(!kept_alone(0o100_640, 501, 501), "the group may read it");
    assert!(!kept_alone(0o100_602, 501, 501), "anyone may write it");
    assert!(!kept_alone(0o100_600, 0, 501), "another account owns it");
}

#[test]
fn a_cache_that_cannot_be_written_draws_as_before_and_frees_its_queue() {
    let _turn = turn();
    let (_sample, home) = home("unwritable");
    // A file where its directory would be made, on every platform.
    let home = home.join("a file");
    fs::write(&home, b"").unwrap();
    let cache = Cache::open(&home);
    let (queue, writer) = writing(cache.clone());
    let mut pictures = crate::cli::pictures::Caching::new(cache.clone(), queue);
    let form = form(40, 20);

    for at in 0..3 {
        let source = format!("{SOURCE}{at}");
        let payload = pictures.make(&source, &key(), &form);
        assert!(payload.is_ok(), "{at}: {payload:?}");
        assert!(
            writer.queued.load(Ordering::SeqCst) > 0,
            "{at} was not queued"
        );
        writer.drain();
        assert_eq!(
            writer.queued.load(Ordering::SeqCst),
            0,
            "{at} held its room"
        );
    }
    pictures.resuming(true);
    assert!(pictures.cached(SOURCE, &key()).is_none());
}

#[test]
fn a_full_queue_drops_the_write_and_takes_more_once_the_writer_has_written() {
    let _turn = turn();
    let (_sample, home) = home("full");
    let cache = Cache::open(&home);
    let (queue, writer) = writing(cache.clone());
    let form = form(40, 20);
    let payload = drawn(&key(), &form);
    let one = Entry::of(SOURCE, &key(), &form, &payload)
        .unwrap()
        .bytes
        .len();
    let fits = MOST_QUEUED / one;

    for at in 0..fits {
        let source = format!("{SOURCE}{at}");
        assert!(
            queue.put(&source, &key(), &form, &payload),
            "{at} of {fits}"
        );
    }
    assert!(!queue.put("one too many", &key(), &form, &payload));
    assert!(queue.queued.load(Ordering::SeqCst) <= MOST_QUEUED);

    writer.drain();
    assert_eq!(entries(&cache).len(), fits);
    assert!(queue.put("one too many", &key(), &form, &payload));
}

#[test]
fn queueing_a_picture_touches_no_disk_until_the_writer_runs() {
    let _turn = turn();
    let (_sample, home) = home("queued");
    let cache = Cache::open(&home);
    let (queue, writer) = writing(cache.clone());
    let form = form(40, 20);
    let payload = drawn(&key(), &form);

    assert!(queue.put(SOURCE, &key(), &form, &payload));
    // Not made, so not scanned or trimmed: that is the writer's.
    assert!(
        !cache.directory.exists(),
        "the drawing side touched the disk"
    );
    assert!(queue.queued.load(Ordering::SeqCst) > 0);

    writer.drain();
    assert_eq!(entries(&cache).len(), 1);
}

#[test]
fn the_writer_thread_trims_first_and_then_writes_what_it_is_handed() {
    let _turn = turn();
    let (_sample, home) = home("thread");
    let cache = Cache::open(&home);
    fs::create_dir_all(&cache.directory).unwrap();
    let draft = cache.directory.join(".crucible-00000000000000aa.writing");
    fs::write(&draft, b"half").unwrap();
    let (queue, writer) = writing(cache.clone());
    let form = form(40, 20);
    let payload = drawn(&key(), &form);
    let thread = std::thread::spawn(move || writer.run());

    assert!(queue.put(SOURCE, &key(), &form, &payload));
    drop(queue);
    let started = Instant::now();
    while !thread.is_finished() && started.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(thread.is_finished(), "the writer outlived its queue");
    thread.join().unwrap();
    assert!(!draft.exists(), "not trimmed at start");
    assert!(cache.get(SOURCE, &key()).is_some(), "not written");
}
