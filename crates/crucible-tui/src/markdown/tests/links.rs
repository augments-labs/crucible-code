//! Links, addresses and forge references: what the reader draws for each, the
//! quiet address it adds after words that are not their address, and what it
//! leaves as written.

use super::*;

#[test]
fn a_number_on_a_later_line_is_counted_against_the_same_repository() {
    // The repository and the checkout are the session's, not the line's: a
    // line break forgets what the line was doing and nothing about where the
    // session is.
    let said = counted("one\nlanded in #487 yesterday\n");

    assert_eq!(
        points(&said),
        [(
            "#487",
            "https://github.com/augments-labs/crucible-code/issues/487"
        )]
    );

    let opened = opened("one\nsee [a](src/a.rs:3)\n");
    assert_eq!(points(&opened), [("a", "file:///repo/src/a.rs#3")]);
}
