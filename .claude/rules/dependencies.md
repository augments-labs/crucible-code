---
paths:
  - "Cargo.toml"
  - "crates/**/Cargo.toml"
  - "deny.toml"
---

# Dependencies are `=`-pinned and justified

## A task that needs one gets one

The ladder in the `add-a-dependency` skill decides *whether* a crate is the
right rung. It does not decide whether the feature ships. Where the ladder comes
out at the crate — the work has a protocol, a parser, platform branching, or a
timeout `std` has no spelling for — the crate is added and the feature is built.

Scoping the feature down to avoid the dependency is the failure this paragraph
exists to stop. It looks like discipline and it is not: it delivers less than
was asked for, quietly, and records the reason nowhere. Add the crate, walk the
skill, and say in the pull request what it does that `std` does not.

What still holds is everything the skill and the section below ask of the crate
once chosen: the pin, the comment, the licence, the absence of a panicking or
printing API, and the weight against the budgets.



A new one needs a comment in `Cargo.toml` saying why it is needed;
`scripts/check.sh` fails without both.

Pinning is also what hides an advisory published afterwards, so `deny.toml` is
scanned on a clock instead — that check cannot live in a script whose whole
promise is the same answer for the same tree.
