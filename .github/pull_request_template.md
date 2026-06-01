## What does this PR do?

<!-- One paragraph describing the change and why it was made.
     Link to the relevant section of README.md or a GitHub issue if applicable. -->


## How to test it manually

<!-- Step-by-step instructions for a reviewer to verify the feature works.
     Be specific: which URL to visit, what to click, what to expect. -->

1.
2.
3.

## Definition of Done checklist

Every item must be ticked before this PR is marked ready for review.
If an item does not apply, tick it and add `N/A — <reason>`.

- [ ] `cargo build` produces **zero warnings**
- [ ] `cargo clippy --all-targets -- -D warnings` exits 0
- [ ] `cargo fmt --all -- --check` exits 0 (no unformatted files)
- [ ] `cargo test` passes in full
- [ ] No `unwrap()`, `expect()`, `println!`, `dbg!`, `TODO`, or `FIXME` in the diff
- [ ] `README.md` updated if any environment variable was added
