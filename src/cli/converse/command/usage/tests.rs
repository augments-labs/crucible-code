use super::*;

use crucible_client_api::{Limit, LimitGroup, Limits, Model, Name, Percent, Used};
use jiff::tz::Offset;

use super::super::{Command, MidTurn};
use crate::cli::converse::tests::{asking_plan, plain, stalled_plan};
use crucible_tui::Recording;
use std::sync::atomic::Ordering;

/// Thursday 22 October 2026, noon UTC.
const NOW: i64 = 1_792_670_400;
/// 15:40 the same day.
const TODAY: u64 = 1_792_683_600;
/// Monday 26 October, 09:00.
const MONDAY: u64 = 1_793_005_200;
/// Sunday 1 November, 09:00.
const NOVEMBER: u64 = 1_793_523_600;

fn clock() -> Clock {
    Clock::at(NOW, TimeZone::UTC)
}

fn limit(window: Window, used: u8, resets_at: u64) -> Limit {
    Limit {
        window,
        reading: Reading::Percent(Percent::new(used).unwrap()),
        resets_at: Some(resets_at),
    }
}

/// `limits`, plan-wide.
fn plan(limits: Vec<Limit>) -> Limits {
    Limits {
        groups: vec![LimitGroup {
            model: None,
            limits,
        }],
        more: false,
    }
}

/// The session the mockup draws: an unpriced model, a weekly window only.
fn weekly() -> api::Usage {
    api::Usage {
        used: Used {
            cost: Cost::NotPriced,
            api_ms: 252_000,
            wall_ms: 2_285_000,
            added: 212,
            removed: 87,
            input: 1_420_000,
            output: 38_100,
            cache_read: 1_210_000,
            cache_write: 0,
        },
        context: api::Context {
            model: Model::new("gpt-6-sol"),
            window: Some(872_000),
            left: Percent::new(62),
            system: 3_100,
            instructions: 9_800,
            tools: 4_200,
            mcp: 0,
            messages: 184_600,
            reserve: 128_000,
            free: 542_300,
        },
        limits: plan(vec![limit(Window::Weekly, 31, MONDAY)]),
    }
}

/// The same session with all three windows reported.
fn every_window() -> api::Usage {
    api::Usage {
        limits: plan(vec![
            limit(Window::FiveHour, 12, TODAY),
            limit(Window::Weekly, 31, MONDAY),
            limit(Window::Monthly, 9, NOVEMBER),
        ]),
        ..weekly()
    }
}

/// A group of windows a plan keeps for the model `name`.
fn group(name: &str, limits: Vec<Limit>) -> LimitGroup {
    LimitGroup {
        model: Some(Name::new(name).unwrap()),
        limits,
    }
}

/// `used` of `total` in `window`, resetting at 15:40.
fn counted(window: Window, used: u64, total: u64) -> Limit {
    Limit {
        window,
        reading: Reading::Counted { used, total },
        resets_at: Some(TODAY),
    }
}

/// The weekly session with a model the plan limits on its own, as a `ChatGPT`
/// sign-in reports one.
fn with_a_group() -> api::Usage {
    let mut usage = weekly();
    usage.limits.groups.push(group(
        "GPT-5.3-Codex-Spark",
        vec![
            limit(Window::FiveHour, 12, TODAY),
            limit(Window::Weekly, 4, MONDAY),
        ],
    ));
    usage
}

/// The session with a model group, where the vendor reported more limits
/// than the reading kept.
fn with_more() -> api::Usage {
    let mut usage = with_a_group();
    usage.limits.more = true;
    usage
}

/// A plan that limits only per model and counts requests, one window of it
/// unlimited and one model the plan does not include.
fn counted_only() -> api::Usage {
    api::Usage {
        limits: Limits {
            groups: vec![
                group("MiniMax-M2.7", vec![counted(Window::FiveHour, 412, 1_500)]),
                group(
                    "speech-2.8-hd",
                    vec![
                        counted(Window::FiveHour, 0, 9_000),
                        Limit {
                            window: Window::Weekly,
                            reading: Reading::Unlimited,
                            resets_at: None,
                        },
                    ],
                ),
                group("image-01", vec![counted(Window::Daily, 0, 0)]),
            ],
            more: false,
        },
        ..weekly()
    }
}

/// The rows from `Plan limits` down.
fn plan_limits(rows: &[Row]) -> Vec<String> {
    let art = art(rows);
    let at = art
        .iter()
        .position(|row| row == "Plan limits")
        .expect("a heading over the windows");
    art.get(at..).unwrap().to_vec()
}

/// An API key on a priced model: no windows.
fn keyed() -> api::Usage {
    let weekly = weekly();
    api::Usage {
        used: Used {
            cost: Cost::Priced {
                currency: Name::new("USD").unwrap(),
                micros: 1_840_000,
            },
            cache_write: 92_400,
            ..weekly.used
        },
        limits: Limits::default(),
        ..weekly
    }
}

/// The keyed session after an answer was stopped before it said what it
/// cost: what is known is a floor.
fn stopped() -> api::Usage {
    let keyed = keyed();
    api::Usage {
        used: Used {
            cost: Cost::AtLeast {
                currency: Name::new("USD").unwrap(),
                micros: 400_000,
            },
            ..keyed.used
        },
        ..keyed
    }
}

/// The panel's rows at `columns` where the whole of it stands: a rule, the
/// body, and how to close it.
// What is drawn, whose plan is being asked, and how wide, in which glyphs and
// against which clock, are independent inputs, as for `body`.
#[allow(clippy::too_many_arguments)]
fn panel(
    heading: &str,
    usage: &api::Usage,
    asking: Option<&str>,
    columns: usize,
    glyphs: Glyphs,
    clock: &Clock,
) -> Vec<Row> {
    let rows = body(heading, usage, asking, columns, glyphs, clock);
    framed(rows, columns, glyphs, "esc to close".to_owned())
}

fn art(rows: &[Row]) -> Vec<String> {
    rows.iter().map(Row::text).collect()
}

/// The tone of the first cell of `row` that shows `text`.
fn tone(row: &Row, text: &str) -> Option<Slot> {
    row.spans()
        .find(|(_, said)| said.contains(text))
        .map(|(slot, _)| slot)
}

fn bar(used: usize, cells: usize) -> String {
    format!("{}{}", "█".repeat(used), "░".repeat(cells - used))
}

#[test]
fn usage_with_a_weekly_window_is_the_mockup_at_80_columns() {
    let rows = panel(
        "openai · ChatGPT sign-in",
        &weekly(),
        None,
        80,
        Glyphs::Unicode,
        &clock(),
    );

    let context = format!("  {}  38% used", bar(25, 66));
    let window = format!("  Weekly window    {}  31% used", bar(15, 49));
    assert_eq!(
        art(&rows),
        [
            "─".repeat(80).as_str(),
            "",
            "Usage · openai · ChatGPT sign-in",
            "",
            "Session",
            "  Total cost       not priced",
            "  API time         4m 12s",
            "  Wall time        38m 05s",
            "  Lines changed    +212 −87",
            "  Tokens           1.42M in · 38.1k out · 1.21M cache read · 0 cache write",
            "",
            "Context",
            &context,
            "",
            "Plan limits",
            &window,
            "                   resets 26 Oct 09:00",
            "",
            "esc to close",
        ]
    );
    let row = |n: usize| rows.get(n).unwrap();
    assert_eq!(tone(row(2), "Usage"), Some(Slot::Strong));
    assert_eq!(tone(row(5), "not priced"), Some(Slot::Quiet));
    assert_eq!(tone(row(12), "█"), Some(Slot::Accent));
    assert_eq!(tone(row(12), "░"), Some(Slot::Quiet));
    assert_eq!(tone(row(15), "31% used"), Some(Slot::Plain));
    assert_eq!(tone(row(16), "resets"), Some(Slot::Quiet));
    assert_eq!(tone(row(18), "esc to close"), Some(Slot::Quiet));
}

#[test]
fn usage_stands_every_reported_window_in_the_fixed_order() {
    let rows = body("", &every_window(), None, 80, Glyphs::Unicode, &clock());
    let art = art(&rows);
    let limits = art
        .iter()
        .position(|row| row == "Plan limits")
        .expect("a heading over the windows");

    assert_eq!(
        art.get(limits..).unwrap(),
        [
            "Plan limits".to_owned(),
            format!("  5-hour window    {}  12% used", bar(6, 49)),
            "                   resets 22 Oct 15:40".to_owned(),
            String::new(),
            format!("  Weekly window    {}  31% used", bar(15, 49)),
            "                   resets 26 Oct 09:00".to_owned(),
            String::new(),
            format!("  Monthly window   {}   9% used", bar(4, 49)),
            "                   resets 1 Nov 09:00".to_owned(),
        ]
    );
    // With nothing to say who answers, the title is the panel's name alone.
    assert_eq!(art.first().map(String::as_str), Some("Usage"));
}

#[test]
fn usage_with_no_windows_reported_says_so_and_a_price_is_a_sum() {
    let rows = body(
        "anthropic · API key",
        &keyed(),
        None,
        80,
        Glyphs::Unicode,
        &clock(),
    );

    let art = art(&rows);
    assert_eq!(
        art,
        [
            "Usage · anthropic · API key",
            "",
            "Session",
            "  Total cost       $1.84",
            "  API time         4m 12s",
            "  Wall time        38m 05s",
            "  Lines changed    +212 −87",
            "  Tokens           1.42M in · 38.1k out · 1.21M cache read · 92.4k cache write",
            "",
            "Context",
            &format!("  {}  38% used", bar(25, 66)),
            "",
            "Plan limits",
            "  limits not reported",
        ]
    );
    let last = rows.last().unwrap();
    assert_eq!(tone(last, "limits not reported"), Some(Slot::Quiet));
    assert_eq!(tone(rows.get(3).unwrap(), "$1.84"), Some(Slot::Plain));
}

#[test]
fn usage_at_40_columns_stands_each_label_over_or_before_a_narrower_value() {
    let rows = panel(
        "openai · ChatGPT sign-in",
        &weekly(),
        None,
        40,
        Glyphs::Unicode,
        &clock(),
    );

    let context = format!("  {}  38% used", bar(10, 26));
    let window = format!("  {}  31% used", bar(8, 26));
    assert_eq!(
        art(&rows),
        [
            "─".repeat(40).as_str(),
            "",
            "Usage · openai · ChatGPT sign-in",
            "",
            "Session",
            "  Total cost     not priced",
            "  API time       4m 12s",
            "  Wall time      38m 05s",
            "  Lines changed  +212 −87",
            "  Tokens         1.42M in · 38.1k out",
            "                 1.21M cache read",
            "                 0 cache write",
            "",
            "Context",
            &context,
            "",
            "Plan limits",
            "  Weekly window",
            &window,
            "  resets 26 Oct 09:00",
            "",
            "esc to close",
        ]
    );
}

#[test]
fn usage_fits_every_width_down_to_one_column() {
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        for usage in [
            weekly(),
            every_window(),
            keyed(),
            stopped(),
            with_a_group(),
            with_more(),
            counted_only(),
        ] {
            for columns in 1..=120 {
                for row in panel(
                    "openai · ChatGPT sign-in",
                    &usage,
                    Some("openai"),
                    columns,
                    glyphs,
                    &clock(),
                ) {
                    assert!(
                        row.columns() <= columns,
                        "{:?} is wider than {columns} columns",
                        row.text()
                    );
                }
            }
        }
    }
}

#[test]
fn usage_at_40_columns_says_a_floor_on_its_cost_row() {
    let rows = panel(
        "anthropic · API key",
        &stopped(),
        None,
        40,
        Glyphs::Unicode,
        &clock(),
    );

    let row = rows
        .iter()
        .find(|row| row.text().starts_with("  Total cost"))
        .unwrap();
    assert_eq!(row.text(), "  Total cost     at least $0.40");
    assert_eq!(tone(row, "at least $0.40"), Some(Slot::Plain));
}

#[test]
fn usage_draws_with_ascii_where_the_font_has_no_blocks() {
    let rows = body(
        "openai - ChatGPT sign-in",
        &weekly(),
        None,
        80,
        Glyphs::Ascii,
        &clock(),
    );

    let art = art(&rows);
    assert!(art.contains(&"  Lines changed    +212 -87".to_owned()));
    assert!(
        art.iter().all(|row| row.is_ascii()),
        "every row is ASCII: {art:#?}"
    );
}

#[test]
fn usage_says_a_cost_as_what_is_known_of_it() {
    let priced = |currency: &str, micros| Cost::Priced {
        currency: Name::new(currency).unwrap(),
        micros,
    };
    assert_eq!(
        cost(&Cost::Unspent),
        (Slot::Quiet, "nothing asked yet".to_owned())
    );
    assert_eq!(
        cost(&Cost::NotPriced),
        (Slot::Quiet, "not priced".to_owned())
    );
    assert_eq!(cost(&priced("USD", 0)), (Slot::Plain, "$0.00".to_owned()));
    // Spent, but less than a cent: never drawn as nothing.
    assert_eq!(
        cost(&priced("USD", 3_000)),
        (Slot::Plain, "<$0.01".to_owned())
    );
    assert_eq!(
        cost(&priced("USD", 1_835_000)),
        (Slot::Plain, "$1.84".to_owned())
    );
    assert_eq!(
        cost(&priced("EUR", 1_840_000)),
        (Slot::Plain, "1.84 EUR".to_owned())
    );

    // An answer stopped before the provider said what it cost: the figure
    // is a floor, in the same form as a sum.
    let at_least = |currency: &str, micros| Cost::AtLeast {
        currency: Name::new(currency).unwrap(),
        micros,
    };
    assert_eq!(
        cost(&at_least("USD", 400_000)),
        (Slot::Plain, "at least $0.40".to_owned())
    );
    assert_eq!(
        cost(&at_least("USD", 0)),
        (Slot::Plain, "at least $0.00".to_owned())
    );
    assert_eq!(
        cost(&at_least("USD", 3_000)),
        (Slot::Plain, "at least <$0.01".to_owned())
    );
    assert_eq!(
        cost(&at_least("EUR", 1_840_000)),
        (Slot::Plain, "at least 1.84 EUR".to_owned())
    );
}

#[test]
fn usage_spans_and_counts_read_to_three_figures() {
    assert_eq!(took(0), "0s");
    assert_eq!(took(12_999), "12s");
    assert_eq!(took(252_000), "4m 12s");
    assert_eq!(took(3_900_000), "1h 05m");
    assert_eq!(count(0), "0");
    assert_eq!(count(999), "999");
    assert_eq!(count(38_100), "38.1k");
    assert_eq!(count(1_420_000), "1.42M");
}

#[test]
fn usage_reset_times_are_the_reader_wall_clock() {
    // Two hours east of UTC, 09:00 UTC on a Monday is 11:00 there.
    let east = Clock::at(NOW, TimeZone::fixed(Offset::constant(2)));
    assert_eq!(east.dated(MONDAY).as_deref(), Some("26 Oct 11:00"));
    assert_eq!(east.dated(TODAY).as_deref(), Some("22 Oct 17:40"));

    // A machine whose zone could not be read is shown UTC, and says so.
    let guessed = Clock {
        guessed: true,
        ..clock()
    };
    assert_eq!(guessed.dated(TODAY).as_deref(), Some("22 Oct 15:40 UTC"));
}

#[test]
fn a_reset_already_past_says_the_window_started_again_since_the_reading() {
    // Three hours before the clock's noon, and the Monday before it.
    let earlier_today = TODAY - 6 * 3_600 - 40 * 60;
    let last_monday = MONDAY - 7 * 86_400;
    assert_eq!(
        clock().resets(earlier_today).as_deref(),
        Some("reset 22 Oct 09:00, since passed")
    );
    assert_eq!(
        clock().resets(last_monday).as_deref(),
        Some("reset 19 Oct 09:00, since passed")
    );
    // One still to come is dated the same way, later today too.
    assert_eq!(
        clock().resets(TODAY).as_deref(),
        Some("resets 22 Oct 15:40")
    );
    let guessed = Clock {
        guessed: true,
        ..clock()
    };
    assert_eq!(
        guessed.resets(earlier_today).as_deref(),
        Some("reset 22 Oct 09:00 UTC, since passed")
    );

    // The figure stays what the vendor reported, and the line fits the
    // narrowest panel whole.
    let mut usage = weekly();
    usage.limits = plan(vec![limit(Window::Weekly, 31, last_monday)]);
    for columns in [40, 80] {
        let rows = art(&body("", &usage, None, columns, Glyphs::Unicode, &guessed));
        let line = rows
            .iter()
            .find(|row| row.contains("since passed"))
            .unwrap_or_else(|| panic!("no passed reset at {columns}: {rows:#?}"));
        assert!(
            line.ends_with("reset 19 Oct 09:00 UTC, since passed"),
            "{line}"
        );
        assert!(rows.iter().any(|row| row.contains("31% used")), "{rows:#?}");
    }
}

#[test]
fn usage_with_no_window_known_says_so_in_place_of_the_bar() {
    let mut usage = keyed();
    usage.context.left = None;
    usage.context.window = None;
    let rows = body("", &usage, None, 80, Glyphs::Unicode, &clock());

    assert!(art(&rows).contains(&"  window not known".to_owned()));
}

#[test]
fn usage_stands_over_a_running_turn() {
    // It reads and changes nothing, so it stands over the running turn with
    // the figures that turn last reported.
    assert_eq!(Command::Usage.mid_turn(), MidTurn::Live);
}

/// Windows has no zoneinfo directory: the zone its registry names is read from
/// the copy of the database built in, or every reset time would be UTC.
#[cfg(windows)]
#[test]
fn a_windows_machine_reads_its_zone_from_the_database_built_in() {
    assert!(TimeZone::get("Europe/Paris").is_ok());
    let clock = Clock::system();
    assert!(!clock.guessed, "{:?}", clock.zone);
}

/// Set in the copy of this binary a zone test starts, to run its body there.
#[cfg(unix)]
const ZONE_BODY: &str = "CRUCIBLE_TEST_ZONE_BODY";

/// A `TZ` naming a pipe nobody writes to is never opened: opening one waits
/// for a writer, and the clock is read on the drawing thread. The panel shows
/// UTC, and says so, instead.
#[cfg(unix)]
#[test]
fn a_zone_named_by_a_pipe_is_never_waited_on() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    if std::env::var_os(ZONE_BODY).is_some() {
        let clock = Clock::system();
        assert!(clock.guessed, "{:?}", clock.zone);
        return;
    }
    let pipe = std::env::temp_dir().join(format!("crucible-zone-pipe-{}", std::process::id()));
    let _ = std::fs::remove_file(&pipe);
    let made = std::process::Command::new("mkfifo")
        .arg(&pipe)
        .status()
        .expect("mkfifo");
    assert!(made.success(), "mkfifo: {made}");
    let test = module_path!()
        .split_once("::")
        .map_or(module_path!(), |(_, path)| path);
    let mut copy = std::process::Command::new(std::env::current_exe().expect("the test binary"))
        .args([
            &format!("{test}::a_zone_named_by_a_pipe_is_never_waited_on"),
            "--exact",
            "--test-threads=1",
        ])
        .env(ZONE_BODY, "1")
        .env("TZ", &pipe)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("a copy of the test binary");
    let started = Instant::now();
    let ended = loop {
        if let Some(status) = copy.try_wait().expect("the copy's status") {
            break Some(status);
        }
        if started.elapsed() > Duration::from_secs(20) {
            let _ = copy.kill();
            let _ = copy.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let _ = std::fs::remove_file(&pipe);
    let ended = ended.expect("reading the zone waited on the pipe `TZ` names");
    assert!(ended.success(), "{ended}");
}

/// A `TZ` naming something endless, a device here, is never read, and one
/// naming a zone or a POSIX rule still is.
#[cfg(unix)]
#[test]
fn only_a_zone_file_of_a_zone_file_s_size_is_read() {
    use std::ffi::OsStr;

    let nowhere = Path::new("/nowhere/at/all");
    assert!(!readable_zone(Some(OsStr::new("/dev/zero")), nowhere));
    assert!(!readable_zone(Some(OsStr::new(":/dev/zero")), nowhere));
    assert!(!readable_zone(Some(OsStr::new("/")), nowhere));
    assert!(readable_zone(None, nowhere));
    assert!(readable_zone(Some(OsStr::new("")), nowhere));
    assert!(readable_zone(Some(OsStr::new("Europe/Paris")), nowhere));
    assert!(readable_zone(
        Some(OsStr::new("EST5EDT,M3.2.0,M11.1.0")),
        nowhere
    ));
    assert!(readable_zone(Some(OsStr::new("/nowhere/at/all")), nowhere));
}

/// With `TZ` unset the zone is read from `/etc/localtime`, held to the same
/// bound: a pipe, a device or an oversized file there, reached directly or
/// through the usual symlink, is never read, and a zone file there still is.
#[cfg(unix)]
#[test]
fn a_localtime_that_is_not_a_zone_file_s_size_is_never_read() {
    let dir = std::env::temp_dir().join(format!("crucible-localtime-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let pipe = dir.join("pipe");
    let made = std::process::Command::new("mkfifo")
        .arg(&pipe)
        .status()
        .expect("mkfifo");
    assert!(made.success(), "mkfifo: {made}");
    let oversized = dir.join("oversized");
    std::fs::File::create(&oversized)
        .and_then(|file| file.set_len(ZONE_FILE + 1))
        .expect("an oversized file");
    let zone = dir.join("zone");
    std::fs::write(&zone, b"TZif").expect("a zone-sized file");
    let linked_pipe = dir.join("linked-pipe");
    std::os::unix::fs::symlink(&pipe, &linked_pipe).expect("a symlink to the pipe");
    let linked_zone = dir.join("linked-zone");
    std::os::unix::fs::symlink(&zone, &linked_zone).expect("a symlink to the zone");

    let read = |localtime: &Path| readable_zone(None, localtime);
    let read_anyway = [&*pipe, &linked_pipe, &oversized, Path::new("/dev/zero")]
        .into_iter()
        .filter(|localtime| read(localtime))
        .map(|localtime| localtime.display().to_string())
        .collect::<Vec<_>>();
    let accepted = [&*zone, &linked_zone, &dir.join("missing")]
        .into_iter()
        .all(read);
    // A `TZ` that names a zone decides alone, whatever `/etc/localtime` is.
    let named = readable_zone(Some(std::ffi::OsStr::new("Europe/Paris")), &pipe);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        read_anyway.is_empty(),
        "read as /etc/localtime: {read_anyway:?}"
    );
    assert!(accepted);
    assert!(named);
}

#[test]
fn usage_follows_the_colour_rule() {
    // A bar's used part is the one accent on its row, whatever the provider
    // reported, while it is being asked, and however narrow the window.
    for (name, usage, asking) in [
        ("weekly", weekly(), None),
        ("every window", every_window(), None),
        ("keyed", keyed(), None),
        ("stopped", stopped(), None),
        ("a model's group", with_a_group(), None),
        ("counted", counted_only(), None),
        ("asking", weekly(), Some("openai")),
    ] {
        for (columns, glyphs) in [(80, Glyphs::Unicode), (40, Glyphs::Ascii)] {
            let rows = panel(
                "openai · ChatGPT sign-in",
                &usage,
                asking,
                columns,
                glyphs,
                &clock(),
            );

            crate::cli::colour_rule::holds(&format!("usage {name} at {columns}"), &rows, |_| false);
        }
    }
}

#[test]
fn usage_draws_a_model_group_under_the_plan_wide_windows_as_the_mockup_does() {
    let rows = body("", &with_a_group(), None, 80, Glyphs::Unicode, &clock());

    assert_eq!(
        plan_limits(&rows),
        [
            "Plan limits".to_owned(),
            format!("  Weekly window    {}  31% used", bar(15, 49)),
            "                   resets 26 Oct 09:00".to_owned(),
            String::new(),
            "  GPT-5.3-Codex-Spark".to_owned(),
            format!("  5-hour window    {}  12% used", bar(6, 49)),
            "                   resets 22 Oct 15:40".to_owned(),
            String::new(),
            format!("  Weekly window    {}   4% used", bar(2, 49)),
            "                   resets 26 Oct 09:00".to_owned(),
        ]
    );
    let name = rows
        .iter()
        .find(|row| row.text() == "  GPT-5.3-Codex-Spark")
        .unwrap();
    assert_eq!(tone(name, "GPT-5.3-Codex-Spark"), Some(Slot::Strong));
}

#[test]
fn usage_at_40_columns_stands_a_model_group_under_the_plan_wide_windows() {
    let rows = body("", &with_a_group(), None, 40, Glyphs::Unicode, &clock());

    assert_eq!(
        plan_limits(&rows),
        [
            "Plan limits".to_owned(),
            "  Weekly window".to_owned(),
            format!("  {}  31% used", bar(8, 26)),
            "  resets 26 Oct 09:00".to_owned(),
            String::new(),
            "  GPT-5.3-Codex-Spark".to_owned(),
            "  5-hour window".to_owned(),
            format!("  {}  12% used", bar(3, 26)),
            "  resets 22 Oct 15:40".to_owned(),
            String::new(),
            "  Weekly window".to_owned(),
            format!("  {}   4% used", bar(1, 26)),
            "  resets 26 Oct 09:00".to_owned(),
        ]
    );
}

#[test]
fn usage_counts_widen_the_figure_column_and_unlimited_has_no_bar() {
    let rows = body("", &counted_only(), None, 80, Glyphs::Unicode, &clock());

    // The groups follow the heading directly, and a model the plan does not
    // include is left out, name and all.
    assert_eq!(
        plan_limits(&rows),
        [
            "Plan limits".to_owned(),
            "  MiniMax-M2.7".to_owned(),
            format!("  5-hour window    {}  412 of 1,500 used", bar(11, 40)),
            "                   resets 22 Oct 15:40".to_owned(),
            String::new(),
            "  speech-2.8-hd".to_owned(),
            format!("  5-hour window    {}    0 of 9,000 used", bar(0, 40)),
            "                   resets 22 Oct 15:40".to_owned(),
            String::new(),
            "  Weekly window    unlimited".to_owned(),
        ]
    );
    let unlimited = rows.last().unwrap();
    assert_eq!(tone(unlimited, "unlimited"), Some(Slot::Quiet));

    let narrow = body("", &counted_only(), None, 40, Glyphs::Unicode, &clock());
    assert_eq!(
        plan_limits(&narrow),
        [
            "Plan limits".to_owned(),
            "  MiniMax-M2.7".to_owned(),
            "  5-hour window".to_owned(),
            format!("  {}  412 of 1,500 used", bar(5, 17)),
            "  resets 22 Oct 15:40".to_owned(),
            String::new(),
            "  speech-2.8-hd".to_owned(),
            "  5-hour window".to_owned(),
            format!("  {}    0 of 9,000 used", bar(0, 17)),
            "  resets 22 Oct 15:40".to_owned(),
            String::new(),
            "  Weekly window".to_owned(),
            "  unlimited".to_owned(),
        ]
    );
}

#[test]
fn usage_while_asking_says_so_last_in_the_block() {
    for (columns, known) in [
        (
            80,
            vec![
                format!("  Weekly window    {}  31% used", bar(15, 49)),
                "                   resets 26 Oct 09:00".to_owned(),
            ],
        ),
        (
            40,
            vec![
                "  Weekly window".to_owned(),
                format!("  {}  31% used", bar(8, 26)),
                "  resets 26 Oct 09:00".to_owned(),
            ],
        ),
    ] {
        let rows = body(
            "",
            &weekly(),
            Some("openai"),
            columns,
            Glyphs::Unicode,
            &clock(),
        );
        let mut expected = vec!["Plan limits".to_owned()];
        expected.extend(known);
        expected.push("  asking openai…".to_owned());
        assert_eq!(plan_limits(&rows), expected, "at {columns}");
        assert_eq!(
            tone(rows.last().unwrap(), "asking openai…"),
            Some(Slot::Quiet)
        );

        // With nothing known yet, it stands alone, in place of saying none
        // were reported.
        let alone = body(
            "",
            &keyed(),
            Some("openai"),
            columns,
            Glyphs::Unicode,
            &clock(),
        );
        assert_eq!(plan_limits(&alone), ["Plan limits", "  asking openai…"]);
    }
}

#[test]
fn usage_with_more_limits_than_crossed_says_so_below_the_block_and_over_asking() {
    for columns in [80, 40, 12] {
        let rows = body("", &with_more(), None, columns, Glyphs::Unicode, &clock());
        let drawn = plan_limits(&rows);
        let mut whole = plan_limits(&body(
            "",
            &with_a_group(),
            None,
            columns,
            Glyphs::Unicode,
            &clock(),
        ));
        let more = crucible_tui::clip("  more limits not reported", columns).to_owned();
        whole.push(more.clone());
        assert_eq!(drawn, whole, "at {columns}");
        assert_eq!(
            tone(rows.last().unwrap(), "more"),
            Some(Slot::Quiet),
            "at {columns}"
        );

        let asking = plan_limits(&body(
            "",
            &with_more(),
            Some("openai"),
            columns,
            Glyphs::Unicode,
            &clock(),
        ));
        let ask = crucible_tui::clip("  asking openai…", columns).to_owned();
        assert!(asking.ends_with(&[more, ask]), "at {columns}: {asking:?}");
    }
}

#[test]
fn usage_with_neither_a_limit_nor_a_question_out_says_none_were_reported() {
    for columns in [80, 40] {
        let rows = body("", &keyed(), None, columns, Glyphs::Unicode, &clock());
        assert_eq!(
            plan_limits(&rows),
            ["Plan limits", "  limits not reported"],
            "at {columns}"
        );
    }
}

#[test]
fn usage_cuts_a_long_group_name_to_fit_and_names_every_window_length() {
    let name = "a-model-whose-plan-name-runs-on-far-past-the-panel";
    let mut usage = weekly();
    usage.limits.groups.push(group(
        name,
        vec![
            limit(Window::Daily, 50, TODAY),
            limit(Window::Lasting { minutes: 180 }, 1, TODAY),
            limit(Window::Yearly, 2, NOVEMBER),
        ],
    ));
    let rows = body("", &usage, None, 40, Glyphs::Unicode, &clock());
    let art = plan_limits(&rows);

    let cut: String = name.chars().take(37).collect();
    assert!(art.contains(&format!("  {cut}…")), "{art:#?}");
    for label in ["  Daily window", "  3-hour window", "  Yearly window"] {
        assert!(art.contains(&label.to_owned()), "{label}: {art:#?}");
    }
}

#[test]
fn usage_printed_waits_for_the_plan_and_draws_what_it_answered() {
    let (mut conversation, asked) = asking_plan();
    let terms = plain();
    let mut renderer = Renderer::new(Recording::new(80, 40));

    run(&mut renderer, &mut conversation, &terms, false).unwrap();

    let written = renderer.terminal().written().to_string();
    assert_eq!(asked.load(Ordering::Relaxed), 1);
    assert!(written.contains("gpt-5.3-codex-spark"), "{written}");
    assert!(written.contains("31% used"), "{written}");
    // A printed block cannot be drawn again, so it never promises an answer.
    assert!(!written.contains("asking"), "{written}");
}

#[test]
fn usage_opened_again_within_a_minute_asks_nothing_and_shows_the_last_answer() {
    let (mut conversation, asked) = asking_plan();
    let terms = plain();
    for _ in 0..2 {
        let mut renderer = Renderer::new(Recording::new(80, 40));
        run(&mut renderer, &mut conversation, &terms, false).unwrap();
        let written = renderer.terminal().written().to_string();
        assert!(written.contains("gpt-5.3-codex-spark"), "{written}");
    }
    assert_eq!(asked.load(Ordering::Relaxed), 1);
}

/// The panel as a key-reading wait left it, where there was no room to stand
/// it and a key was pressed with the plan's answer still out.
fn after_a_key(usage: api::Usage, conversation: &mut Conversation, terms: &Terms) -> Shown {
    let mut shown = Shown::of(terms, conversation.serving(), usage);
    shown.out = terms.ask_limits(conversation);
    assert!(shown.out.is_some(), "the plan was not asked");
    awaited(&mut shown, terms, conversation, |_| Ok(true)).unwrap();
    shown
}

/// The block printed at 80 columns, after checking that at neither width
/// it promises an answer: a printed block is never drawn again.
fn printed(shown: &Shown) -> Vec<String> {
    for columns in [80, 40] {
        let at = plan_limits(&shown.body(columns, Glyphs::Unicode));
        assert!(
            !at.iter().any(|row| row.contains("asking")),
            "at {columns}: {at:#?}"
        );
    }
    plan_limits(&shown.body(80, Glyphs::Unicode))
}

#[test]
fn usage_cramped_a_key_ends_the_wait_and_prints_what_is_known() {
    let mut conversation = stalled_plan();
    let terms = plain();

    let shown = after_a_key(weekly(), &mut conversation, &terms);

    let art = printed(&shown);
    assert!(art.iter().any(|row| row.contains("31% used")), "{art:#?}");
}

#[test]
fn usage_cramped_a_key_with_nothing_known_prints_limits_not_reported() {
    let mut conversation = stalled_plan();
    let terms = plain();

    let shown = after_a_key(keyed(), &mut conversation, &terms);

    assert_eq!(printed(&shown), ["Plan limits", "  limits not reported"]);
}

#[test]
fn usage_cramped_a_wait_a_key_ended_still_holds_the_minute() {
    let mut conversation = stalled_plan();
    let terms = plain();
    drop(after_a_key(keyed(), &mut conversation, &terms));

    // The question was out, so it counts: the next opening asks nothing.
    assert!(terms.ask_limits(&mut conversation).is_none());
}

#[test]
fn usage_closed_with_the_question_out_answers_it_and_the_next_opening_asks_nothing() {
    let mut conversation = stalled_plan();
    let (client, journal) = crate::cli::client::Client::noting();
    let mut terms = plain();
    terms.client = client;
    let mut shown = Shown::of(&terms, conversation.serving(), keyed());
    shown.out = terms.ask_limits(&mut conversation);
    assert!(shown.out.is_some(), "the plan was not asked");

    closed(shown.out.take(), &terms, &mut conversation);

    // The request is answered, with what was known, as the panel closes.
    assert!(
        matches!(
            journal.noted().as_slice(),
            [crate::cli::client::tests::Noted::Answered {
                asked: api::Command::AskLimits,
                outcome: api::Outcome::Usage(_),
                ..
            }]
        ),
        "{:#?}",
        journal.noted()
    );
    // The question was out, so it counts toward the minute: the panel opened
    // again at once asks nothing, and is answered with what is known.
    assert!(
        terms.ask_limits(&mut conversation).is_none(),
        "the plan was asked again within the minute"
    );
    assert!(
        matches!(
            journal.noted().as_slice(),
            [
                crate::cli::client::tests::Noted::Answered {
                    asked: api::Command::AskLimits,
                    ..
                },
                crate::cli::client::tests::Noted::Answered {
                    asked: api::Command::AskLimits,
                    outcome: api::Outcome::Usage(_),
                    ..
                }
            ]
        ),
        "{:#?}",
        journal.noted()
    );
}

#[test]
fn usage_parts_each_plan_window_from_the_next_with_a_blank_row() {
    // A window is two rows beside its label and three under it; a blank row
    // between one window's reset and the next window's name keeps either
    // from reading as part of the other, in a group as across groups.
    for (columns, usage, blanks) in [
        (80, every_window(), vec![3, 6]),
        (40, every_window(), vec![4, 8]),
        (80, with_a_group(), vec![3, 7]),
        (40, with_a_group(), vec![4, 9]),
    ] {
        let rows = plan_limits(&body("", &usage, None, columns, Glyphs::Unicode, &clock()));
        let blank: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.is_empty())
            .map(|(at, _)| at)
            .collect();
        assert_eq!(blank, blanks, "at {columns}: {rows:#?}");
    }
}

#[test]
fn usage_shows_every_reset_with_its_date_a_reset_later_today_among_them() {
    // The same day, the same week, and beyond it all say the day and month.
    assert_eq!(
        clock().resets(TODAY).as_deref(),
        Some("resets 22 Oct 15:40")
    );
    assert_eq!(
        clock().resets(MONDAY).as_deref(),
        Some("resets 26 Oct 09:00")
    );
    assert_eq!(
        clock().resets(NOVEMBER).as_deref(),
        Some("resets 1 Nov 09:00")
    );
    let earlier_today = TODAY - 6 * 3_600 - 40 * 60;
    assert_eq!(
        clock().resets(earlier_today).as_deref(),
        Some("reset 22 Oct 09:00, since passed")
    );
    for columns in [40, 80] {
        let rows = plan_limits(&body(
            "",
            &every_window(),
            None,
            columns,
            Glyphs::Unicode,
            &clock(),
        ));
        assert!(
            rows.iter()
                .any(|row| row.trim_start() == "resets 22 Oct 15:40"),
            "at {columns}: {rows:#?}"
        );
    }
}

/// The session with two plan windows, the five-hour and the weekly.
fn two_windows() -> api::Usage {
    api::Usage {
        limits: plan(vec![
            limit(Window::FiveHour, 12, TODAY),
            limit(Window::Weekly, 31, MONDAY),
        ]),
        ..weekly()
    }
}

#[test]
fn usage_taller_than_its_room_stands_in_it_and_scrolls_with_the_arrows() {
    let terms = plain();
    let mut shown = Shown::of(&terms, None, two_windows());
    shown.clock = clock();
    // At 40 columns the body is 22 rows, and with the rule, the blank rows
    // and the footer the panel would be 26: a 24-row window holds 19 of them
    // and a row saying how many more are below.
    let stood = art(&shown.laid(40, 24, Glyphs::Unicode));
    assert_eq!(stood.len(), 24, "{stood:#?}");
    assert_eq!(
        stood.first().map(String::as_str),
        Some("─".repeat(40).as_str())
    );
    assert_eq!(
        stood.get(2).map(String::as_str),
        Some("Usage"),
        "{stood:#?}"
    );
    assert_eq!(
        stood.get(21).map(String::as_str),
        Some("  ↓ 3 more"),
        "{stood:#?}"
    );
    assert_eq!(
        stood.last().map(String::as_str),
        Some("esc to close · ↑↓ to see more")
    );

    assert_eq!(shown.pressed(&Pressed::Up), Moved::Still);
    assert_eq!(shown.pressed(&Pressed::Down), Moved::Redraw);
    let scrolled = art(&shown.laid(40, 24, Glyphs::Unicode));
    assert_eq!(scrolled.len(), 24, "{scrolled:#?}");
    assert!(!scrolled.iter().any(|row| row == "Usage"), "{scrolled:#?}");
    assert_eq!(
        scrolled.get(21).map(String::as_str),
        Some("  ↓ 2 more"),
        "{scrolled:#?}"
    );

    // At the foot the last window's reset stands over the footer, and ↓ is
    // spent.
    assert_eq!(shown.pressed(&Pressed::Down), Moved::Redraw);
    assert_eq!(shown.pressed(&Pressed::Down), Moved::Redraw);
    assert_eq!(shown.pressed(&Pressed::Down), Moved::Still);
    let foot = art(&shown.laid(40, 24, Glyphs::Unicode));
    assert!(
        !foot.iter().any(|row| row.trim_start().starts_with('↓')),
        "{foot:#?}"
    );
    assert_eq!(
        foot.iter().rev().nth(2).map(|row| row.trim_start()),
        Some("resets 26 Oct 09:00"),
        "{foot:#?}"
    );
    assert_eq!(shown.pressed(&Pressed::Up), Moved::Redraw);
    assert_eq!(shown.pressed(&Pressed::Escape), Moved::Left);

    // Where the whole panel fits, it stands whole and says only how to close.
    let whole = art(&shown.laid(40, 26, Glyphs::Unicode));
    assert_eq!(whole.len(), 26, "{whole:#?}");
    assert_eq!(
        whole.get(2).map(String::as_str),
        Some("Usage"),
        "{whole:#?}"
    );
    assert_eq!(whole.last().map(String::as_str), Some("esc to close"));

    // In ASCII the arrows and the dot are drawn as the font has them.
    let ascii = art(&shown.laid(40, 24, Glyphs::Ascii));
    assert_eq!(
        ascii.last().map(String::as_str),
        Some("esc to close - ^v to see more")
    );

    // Where even the rule, the blank rows, the footer and a row of the body
    // with the row under it have no room, nothing stands, and the caller
    // prints.
    for room in [0, 4, 5] {
        assert!(
            shown.laid(40, room, Glyphs::Unicode).is_empty(),
            "at {room}"
        );
    }
}
