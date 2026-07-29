<!--
Thanks for contributing to Annex. Keep the subject imperative, lowercase, and free of a
trailing period. Describe what changed and how you verified it.
-->

## What this changes

<!-- A sentence or two. Link the issue it closes, if any. -->

## How it was verified

<!--
What did you run and what did you see? If it touches capture, encode, transport or input, say
so. "cargo test" plus the manual check you did is ideal.
-->

## Checklist

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] Private Apple APIs, if any, stay inside `crates/virtual-display`
- [ ] Apple frameworks are only called on the main thread
- [ ] The security invariants and their tests in `crates/transport/src/auth.rs` still hold
- [ ] Documentation updated if behavior or flags changed
