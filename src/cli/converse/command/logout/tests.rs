//! Which providers `/logout` has to offer, given what the store holds.

use super::*;

/// The built-in providers, as one generation to read the store against.
fn catalogue() -> Providers {
    crucible_app::providers::providers()
        .expect("the built-in providers register")
        .snapshot()
}

#[test]
fn what_is_offered_is_what_the_store_holds_and_this_build_serves() {
    // `gemini` is a store written by hand, or by a version serving more than
    // this one. There is no provider here to log out of and no name to draw it
    // under, so it is not on the list this build can show.
    let held: Vec<&str> = held(&catalogue(), &["openai", "gemini"])
        .iter()
        .map(|one| one.name)
        .collect();

    assert_eq!(held, ["openai"]);
}

#[test]
fn the_list_reads_in_the_order_this_build_names_them_rather_than_the_store_s() {
    // A file is written in whatever order somebody logged in, and `/login`'s
    // panel is in registry order. Two lists of the same providers that read
    // differently down is the same list twice, mistrusted once.
    let every: Vec<&str> = offered(&catalogue()).map(|one| one.name).collect();
    let backwards: Vec<&str> = every.iter().copied().rev().collect();

    let held: Vec<&str> = held(&catalogue(), &backwards)
        .iter()
        .map(|one| one.name)
        .collect();

    assert_eq!(held, every);
}

#[test]
fn the_agent_connect_claims_every_capability_but_the_one_whose_moves_it_cannot_carry_out() {
    // A client that claims install can be offered a move on an agent in place
    // of the connect, and this panel has no way to carry one out, so the
    // connect it sends withholds install. `/login`'s connect claims it.
    let mut terms = crate::cli::converse::tests::plain();
    let (client, claims) = crate::cli::client::Client::noting_claims();
    terms.client = client;
    let mut conversation = crate::cli::converse::tests::silent();
    let instance = Name::new("space").expect("a name the contract takes");

    terms
        .operate(&mut conversation, asked(Act::Connect, instance), |_| {
            Ok::<_, Fatal>(None)
        })
        .expect("nobody answering fails nothing");

    let taken = claims.taken();
    let [(command, claimed)] = taken.as_slice() else {
        panic!("one request answered, not {}", taken.len());
    };
    assert!(
        matches!(command, Command::Connect { instance: Some(_) }),
        "{command:?}"
    );
    assert!(
        !claimed.has(crucible_client_api::Capability::Install),
        "{claimed:?}"
    );
    assert_eq!(
        *claimed,
        crucible_client_api::Capabilities::every()
            .without(crucible_client_api::Capability::Install),
        "every other capability is still claimed"
    );
}
