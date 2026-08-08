<!-- Security issues do not belong here. See SECURITY.md. -->

## What this changes

<!-- One paragraph. The diff shows what; say why. -->

## How you verified it

<!--
The question that matters. Name the test, the command, or the session you ran,
and what its output was. "Tests pass" is not an answer — which test failed
before this change?
-->

## Checklist

- [ ] `scripts/check.sh` passes
- [ ] New behaviour has a test that failed before the change; a fix has a test
      that reproduced the bug
- [ ] No performance budget moved, or the trade is explained above
- [ ] New dependencies are `=`-pinned with a comment in `Cargo.toml` saying why
