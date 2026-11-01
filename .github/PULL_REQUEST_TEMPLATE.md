<!--
Title: Conventional Commits `type(scope): summary`, e.g. `fix(core,shell): sanitize echoed input`.
Squash merges: PR title becomes the commit subject. Keep it accurate.
Delete inapplicable sections; keep Verification, Security and safety, and Documentation.
-->

## Summary

<!-- What changes and why. Lead with the reason; the diff shows the what. -->

## Related issue

<!-- `Closes #123` closes the issue on merge; `Refs #123` only links it. Write `None` if no issue. -->

## Type of change

- [ ] `feat`: user-visible capability
- [ ] `fix`: bug fix
- [ ] `perf`: performance change
- [ ] `refactor`: no observable behavior change
- [ ] `test`: tests only
- [ ] `docs`: documentation only
- [ ] `style`: formatting or lint fixes only
- [ ] `chore`: build, dependency, or release housekeeping
- [ ] Breaking change (migration note under `### Breaking` in `CHANGELOG.md`)

## Contracts touched

<!-- Tick each changed contract; make the matching update in the same PR. -->

- [ ] CLI flags, option values, or exit codes (`src/core/cli.rs`, `src/shell/cli.rs`, `tests/cli.rs`, `README.md`)
- [ ] Output format or digest header (`src/core/render/`, `README.md`, `context.md` sec. 3.5)
- [ ] Limits or exclusion lists (`src/core/policy/` is the only authority; mirror in `docs/security_and_filtering.md` sec. 8)
- [ ] Filesystem, network, or sanitization behavior (`docs/security_and_filtering.md`)
- [ ] Domain invariant or module boundary (`context.md`)
- [ ] None of the above

## Verification

<!-- Run in this order (AGENTS.md sec. 5). Paste exact commands and results below. Tick only checks run. -->

- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo fmt --check`
- [ ] `cargo doc --no-deps`
- [ ] `cargo test` (or narrowest covering target, with reason)
- [ ] New or changed behavior has deterministic tests for success, boundary, and failure paths
- [ ] Each fixed bug has a regression test that fails without the fix

Commands and results:

```text
<!-- exact commands and results; list unrun checks as Unverified with blocker -->
```

## Security and safety

<!-- Tick the last box if none apply. -->

- [ ] No secrets, tokens, credentials, or personal data in code, tests, fixtures, or logs
- [ ] Untrusted input (CLI, URL, header, archive, file) parsed into a typed value at the boundary
- [ ] Filesystem and network changes follow `docs/security_and_filtering.md` (no symlink following; credentials only to GitHub API origin)
- [ ] Not security-relevant

## Documentation

- [ ] Permanent docs updated for changed behavior, contract, or invariant (routing table: `AGENTS.md` sec. 6)
- [ ] `CHANGELOG.md` entry under `## [Unreleased]` for any behavior change
- [ ] Not needed (internal change, no observable effect)

## Reviewer notes

<!-- Reading order, risky spots, rejected alternatives with reasons, follow-up intentionally excluded. -->
