//! Which providers `/logout` has to offer, given what the store holds.

use super::*;

#[test]
fn what_is_offered_is_what_the_store_holds_and_this_build_serves() {
    // `gemini` is a store written by hand, or by a version serving more than
    // this one. There is no provider here to log out of and no name to draw it
    // under, so it is not on the list this build can show.
    let held: Vec<&str> = held(&["openai", "gemini"])
        .iter()
        .map(|one| one.name)
        .collect();

    assert_eq!(held, ["openai"]);
}

#[test]
fn the_list_reads_in_the_order_this_build_names_them_rather_than_the_store_s() {
    // A file is written in whatever order somebody logged in, and `/login`'s
    // panel is in `PROVIDERS` order. Two lists of the same providers that read
    // differently down is the same list twice, mistrusted once.
    let every: Vec<&str> = PROVIDERS.iter().map(|one| one.name).collect();
    let backwards: Vec<&str> = every.iter().copied().rev().collect();

    let held: Vec<&str> = held(&backwards).iter().map(|one| one.name).collect();

    assert_eq!(held, every);
}
