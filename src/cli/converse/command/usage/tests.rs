use super::*;

use crucible_client_api::{Limit, Limits, Model, Name, Percent, Used};
use jiff::tz::Offset;

use super::super::{Command, MidTurn};

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

fn limit(used: u8, resets_at: u64) -> Limit {
    Limit {
        used: Percent::new(used).unwrap(),
        resets_at: Some(resets_at),
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
        limits: Limits {
            weekly: Some(limit(31, MONDAY)),
            ..Limits::default()
        },
    }
}

/// The same session with all three windows reported.
fn every_window() -> api::Usage {
    api::Usage {
        limits: Limits {
            five_hour: Some(limit(12, TODAY)),
            weekly: Some(limit(31, MONDAY)),
            monthly: Some(limit(9, NOVEMBER)),
        },
        ..weekly()
    }
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
            "                   resets Mon 09:00",
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
    let rows = body("", &every_window(), 80, Glyphs::Unicode, &clock());
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
            "                   resets 15:40".to_owned(),
            format!("  Weekly window    {}  31% used", bar(15, 49)),
            "                   resets Mon 09:00".to_owned(),
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
            "  resets Mon 09:00",
            "",
            "esc to close",
        ]
    );
}

#[test]
fn usage_fits_every_width_down_to_one_column() {
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        for usage in [weekly(), every_window(), keyed()] {
            for columns in 1..=120 {
                for row in panel(
                    "openai · ChatGPT sign-in",
                    &usage,
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
fn usage_draws_with_ascii_where_the_font_has_no_blocks() {
    let rows = body(
        "openai - ChatGPT sign-in",
        &weekly(),
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
    assert_eq!(east.reads(MONDAY).as_deref(), Some("Mon 11:00"));
    assert_eq!(east.reads(TODAY).as_deref(), Some("17:40"));

    // A machine whose zone could not be read is shown UTC, and says so.
    let guessed = Clock {
        guessed: true,
        ..clock()
    };
    assert_eq!(guessed.reads(TODAY).as_deref(), Some("15:40 UTC"));
}

#[test]
fn usage_with_no_window_known_says_so_in_place_of_the_bar() {
    let mut usage = keyed();
    usage.context.left = None;
    usage.context.window = None;
    let rows = body("", &usage, 80, Glyphs::Unicode, &clock());

    assert!(art(&rows).contains(&"  window not known".to_owned()));
}

#[test]
fn usage_stands_over_a_running_turn() {
    // It reads and changes nothing, so it stands over the running turn with
    // the figures that turn last reported.
    assert_eq!(Command::Usage.mid_turn(), MidTurn::Live);
}
