//! What the next request carries, divided by what holds the window.

use crucible_tools::{ToolDescriptor, ToolProvenance, ToolSourceKind};

use super::*;

/// A built-in tool and one a server supplied.
fn served() -> Tools {
    let mut offered = tools([Fixed::new("read")]);
    let search = Fixed::new("search");
    offered
        .add(
            ToolDescriptor::new(
                "search",
                search.schema(),
                ToolProvenance::new(ToolSourceKind::Mcp, "mcp:docs", "docs server").unwrap(),
            )
            .unwrap(),
            Arc::new(search),
        )
        .unwrap();
    offered
}

/// A measured turn that answered and was charged for it.
fn answered() -> Script {
    Script::new(vec![vec![
        Delta::Carried(Carried::new(40_000)),
        Delta::Text("done".into()),
        Delta::Spent(Spend::new(10_000)),
        Delta::Stopped(StopReason::Yielded),
    ]])
}

/// The five categories a request carries, added up.
fn carried_by(breakdown: &Breakdown) -> u64 {
    [
        Category::SystemPrompt,
        Category::ProjectInstructions,
        Category::ToolSchemas,
        Category::McpToolSchemas,
        Category::Messages,
    ]
    .into_iter()
    .map(|category| breakdown.tokens(category))
    .sum()
}

#[test]
fn context_breakdown_between_turns_sums_to_what_the_runner_carries() {
    let mut scripted = Scripted::new(answered(), served(), Verdict::Allow);
    scripted.runner.state.window = Some(200_000);
    scripted.turn("go").expect("a measured turn");

    let breakdown = scripted.runner.breakdown();

    assert_eq!(carried_by(&breakdown), scripted.runner.carrying());
    assert_eq!(breakdown.left(), scripted.runner.left());
    assert_eq!(breakdown.window(), scripted.runner.context_window());
    assert!(breakdown.tokens(Category::ToolSchemas) > 0, "{breakdown:?}");
    assert!(
        breakdown.tokens(Category::McpToolSchemas) > 0,
        "a server's schemas were not told apart: {breakdown:?}"
    );
    assert_eq!(
        breakdown.tokens(Category::Free),
        200_000 - breakdown.tokens(Category::Reserve) - scripted.runner.carrying()
    );
}

#[test]
fn context_breakdown_is_posted_with_every_reading_a_turn_makes() {
    let mut scripted = Scripted::new(answered(), served(), Verdict::Allow);
    scripted.runner.state.window = Some(200_000);
    scripted.turn("go").expect("a measured turn");

    let posted: Vec<(Option<u8>, Breakdown)> = scripted
        .seen
        .try_iter()
        .filter_map(|event| match event {
            Event::Carried { left, breakdown } => Some((left, breakdown)),
            _ => None,
        })
        .collect();

    assert!(!posted.is_empty(), "the turn posted no reading");
    for (left, breakdown) in posted {
        assert_eq!(breakdown.left(), left, "{breakdown:?}");
        assert_eq!(breakdown.window(), Some(200_000), "{breakdown:?}");
        assert!(carried_by(&breakdown) > 0, "{breakdown:?}");
    }
}

#[test]
fn context_breakdown_without_a_window_draws_no_free_room() {
    let mut scripted = Scripted::new(answered(), served(), Verdict::Allow);
    scripted.turn("go").expect("a measured turn");

    let breakdown = scripted.runner.breakdown();

    assert_eq!(breakdown.window(), None);
    assert_eq!(breakdown.left(), None);
    assert_eq!(breakdown.tokens(Category::Free), 0);
    assert_eq!(carried_by(&breakdown), scripted.runner.carrying());
}
