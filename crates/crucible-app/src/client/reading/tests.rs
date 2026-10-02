//! What a session's figures read as once they cross to a client.

use crucible_client_api as api;
use crucible_runner::SessionCost;
use std::time::{Duration, SystemTime};

use crucible_types::{
    CostAmount, GroupName, MAX_LIMIT_GROUPS, ModelGroup, ModelKey, PlanWindows, PricingCurrency,
    PricingUnit, Scope, Window, WindowReading,
};

use super::{cost, limits};

const USD: PricingCurrency = PricingCurrency::new("USD");

/// `femtocurrency` of a dollar.
fn dollars(femtocurrency: u128) -> CostAmount {
    CostAmount::new(USD, PricingUnit::MillionTokens, femtocurrency)
}

#[test]
fn usage_a_cost_crosses_in_millionths_of_its_currency() {
    let usd = api::Name::new("USD").unwrap();
    assert_eq!(
        cost(SessionCost::Priced(dollars(1_840_000_000_000_000))),
        api::Cost::Priced {
            currency: usd.clone(),
            micros: 1_840_000,
        }
    );
    assert_eq!(
        cost(SessionCost::AtLeast(dollars(400_000_000_000_000))),
        api::Cost::AtLeast {
            currency: usd,
            micros: 400_000,
        }
    );
    assert_eq!(cost(SessionCost::Unspent), api::Cost::Unspent);
    assert_eq!(cost(SessionCost::NotPriced), api::Cost::NotPriced);
}

#[test]
fn usage_a_cost_too_large_to_cross_is_not_priced_rather_than_capped() {
    // One micro past what the contract can carry: capping it would cross a
    // figure nobody spent as though it were the sum.
    let past = (u128::from(u64::MAX) + 1) * 1_000_000_000;
    assert_eq!(
        cost(SessionCost::Priced(dollars(past))),
        api::Cost::NotPriced
    );
    assert_eq!(
        cost(SessionCost::AtLeast(dollars(past))),
        api::Cost::NotPriced
    );

    // The largest that fits still crosses whole.
    let most = u128::from(u64::MAX) * 1_000_000_000;
    assert_eq!(
        cost(SessionCost::Priced(dollars(most))),
        api::Cost::Priced {
            currency: api::Name::new("USD").unwrap(),
            micros: u64::MAX,
        }
    );
}

#[test]
fn limit_every_window_reading_and_group_crosses_under_its_own_name() {
    // The two `Window` enums are coupled through the match in `window`: a
    // window added to one and not the other fails to compile there, and one
    // crossed under another's name fails here.
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let spark = kept_for("gpt-5.3-codex-spark");
    let reported = PlanWindows::new(SystemTime::UNIX_EPOCH)
        .within(
            spark.clone(),
            Window::Lasting(180),
            WindowReading::new(7, None),
        )
        .with(Window::Monthly, WindowReading::unlimited())
        .with(Window::FiveHour, WindowReading::new(23, Some(at)))
        .with(
            Window::Daily,
            WindowReading::counted(412, 1_500, None).unwrap(),
        )
        .with(Window::Weekly, WindowReading::new(100, None))
        .within(spark, Window::Yearly, WindowReading::new(1, None));

    let crossed = limits(&reported, Some("openai"));

    let limit = |window, reading, resets_at| api::Limit {
        window,
        reading,
        resets_at,
    };
    let percent = |used| api::Reading::Percent(api::Percent::new(used).unwrap());
    assert_eq!(
        crossed,
        api::Limits {
            groups: vec![
                api::LimitGroup {
                    model: None,
                    limits: vec![
                        limit(api::Window::FiveHour, percent(23), Some(1_700_000_000)),
                        limit(
                            api::Window::Daily,
                            api::Reading::Counted {
                                used: 412,
                                total: 1_500,
                            },
                            None,
                        ),
                        limit(api::Window::Weekly, percent(100), None),
                        limit(api::Window::Monthly, api::Reading::Unlimited, None),
                    ],
                },
                api::LimitGroup {
                    model: Some(api::Name::new("gpt-5.3-codex-spark").unwrap()),
                    limits: vec![
                        limit(api::Window::Lasting { minutes: 180 }, percent(7), None),
                        limit(api::Window::Yearly, percent(1), None),
                    ],
                },
            ],
            more: false,
        }
    );
    assert!(limits(&PlanWindows::new(SystemTime::UNIX_EPOCH), None).is_empty());
}

/// The group a provider module keeps for the model whose slug is `slug`,
/// named as the vendor named it.
fn kept_for(slug: &str) -> Scope {
    Scope::Model(ModelGroup::new(
        GroupName::new(slug).unwrap(),
        ModelKey::exact(slug),
    ))
}

/// What the one model's group in a reading of `scope` crosses as, read in
/// the words of `serving`.
fn drawn(scope: Scope, serving: Option<&str>) -> Option<String> {
    let reported = PlanWindows::new(SystemTime::UNIX_EPOCH).within(
        scope,
        Window::FiveHour,
        WindowReading::new(7, None),
    );
    let crossed = limits(&reported, serving);
    let model = crossed.groups.first()?.model.as_ref()?;
    Some(model.as_str().to_owned())
}

#[test]
fn limit_a_model_group_is_drawn_by_the_catalogs_name_for_its_model_else_as_the_vendor_named_it() {
    assert_eq!(
        drawn(kept_for("gpt-6-astra"), Some("openai")).as_deref(),
        Some("GPT-6 Astra")
    );
    assert_eq!(
        drawn(kept_for("gpt-5.3-codex-spark"), Some("openai")).as_deref(),
        Some("gpt-5.3-codex-spark")
    );
    // Another provider's catalog does not name it, and neither does none.
    assert_eq!(
        drawn(kept_for("gpt-6-astra"), Some("anthropic")).as_deref(),
        Some("gpt-6-astra")
    );
    assert_eq!(
        drawn(kept_for("gpt-6-astra"), None).as_deref(),
        Some("gpt-6-astra")
    );
    // A group kept for no model by name is drawn as the vendor named it,
    // even where that name is a model the catalog knows.
    let unkeyed = Scope::Model(ModelGroup::new(
        GroupName::new("gpt-6-astra").unwrap(),
        None,
    ));
    assert_eq!(
        drawn(unkeyed, Some("openai")).as_deref(),
        Some("gpt-6-astra")
    );
}

/// A reading of the plan-wide limit and `models` more, one for each model.
fn reading_of(models: usize) -> PlanWindows {
    (0..models).fold(
        PlanWindows::new(SystemTime::UNIX_EPOCH).with(Window::Weekly, WindowReading::new(5, None)),
        |reading, model| {
            reading.within(
                kept_for(&format!("model-{model}")),
                Window::Weekly,
                WindowReading::new(3, None),
            )
        },
    )
}

#[test]
fn limit_a_reading_cut_at_its_ceiling_crosses_saying_there_are_more() {
    let full = limits(&reading_of(MAX_LIMIT_GROUPS - 1), Some("openai"));
    assert_eq!(full.groups.len(), MAX_LIMIT_GROUPS);
    assert!(!full.more);

    let over = limits(&reading_of(MAX_LIMIT_GROUPS), Some("openai"));
    assert_eq!(over.groups.len(), MAX_LIMIT_GROUPS);
    assert!(over.more);

    let said = limits(
        &PlanWindows::new(SystemTime::UNIX_EPOCH)
            .with(Window::Weekly, WindowReading::new(5, None))
            .cut(),
        None,
    );
    assert!(said.more);
}
