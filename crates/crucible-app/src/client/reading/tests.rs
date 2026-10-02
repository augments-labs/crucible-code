//! What a session's figures read as once they cross to a client.

use crucible_client_api as api;
use crucible_runner::SessionCost;
use crucible_types::{CostAmount, PricingCurrency, PricingUnit};

use super::cost;

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
