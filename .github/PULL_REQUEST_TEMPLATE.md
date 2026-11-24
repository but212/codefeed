## Summary

<!-- What changed and why? -->

## Validation

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --all-targets --locked -- -D warnings`
- [ ] `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked`
- [ ] `cargo test --locked`
- [ ] Documentation was updated if needed (including `docs/<NAME>.ko.md` translation parity for `README.md`, `CONTRIBUTING.md`, or `CODE_OF_CONDUCT.md`).
- [ ] `CHANGELOG.md` was updated for user-visible changes.

## API and compatibility

- [ ] This change does not alter the public API.
- [ ] If the public API changed, migration notes and compatibility impact are documented.
