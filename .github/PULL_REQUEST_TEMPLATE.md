<!--
Title: use Conventional Commits, `type(scope): summary`, e.g. `fix(core,shell): sanitize echoed input`.
Merges are squash merges and the PR title becomes the commit subject, so keep the title accurate.
Delete sections that do not apply, but keep Verification and Checklist.
-->

## Summary

<!-- What changes and why. Lead with the reason; the diff already shows the what. -->

## Related issue

<!-- `Closes #123` closes the issue on merge; `Refs #123` only links it. Write `None` if there is no issue. -->

## Type of change

- [ ] `feat`: new user-visible capability
- [ ] `fix`: bug fix
- [ ] `perf`: performance change
- [ ] `refactor`: no observable behavior change
- [ ] `test`: tests only
- [ ] `docs`: documentation only
- [ ] `style`: formatting or lint fixes only
- [ ] `chore`: build, dependency, or release housekeeping
- [ ] Breaking change (add a migration note under `### Breaking` in `CHANGELOG.md`)

## Contracts touched

<!-- Tick every contract this PR changes, then make the matching update in the same PR. -->

- [ ] CLI flags, option values, or exit codes (`src/core/cli.rs`, `src/shell/cli.rs`, `tests/cli.rs`, `README.md`)
- [ ] Output format or digest header (`src/core/render/`, `README.md`, `context.md` sec. 3.5)
- [ ] Limits or exclusion lists (`src/core/policy/` is the only authority; mirror in `docs/security_and_filtering.md` sec. 8)
- [ ] Filesystem, network, or sanitization behavior (`docs/security_and_filtering.md`)
- [ ] Domain invariant or module boundary (`context.md`)
- [ ] None of the above

## Verification

<!-- Run in this order (AGENTS.md sec. 5). Paste the exact commands and their results below. Do not tick a box for a check you did not run. -->

- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo fmt --check`
- [ ] `cargo doc --no-deps`
- [ ] `cargo test` (or the narrowest target that covers the change, with the reason)
- [ ] New or changed behavior has deterministic tests for success, boundary, and failure paths
- [ ] Each fixed bug has a regression test that fails without the fix

Commands and results:

```text
<!-- exact commands and results; list any check that could not run as Unverified, with the blocker -->
```

## Security and safety

<!-- If this PR touches none of these areas, tick the last box. -->

- [ ] No secrets, tokens, credentials, or personal data in code, tests, fixtures, or logs
- [ ] Untrusted input (CLI, URL, header, archive, file) is parsed into a typed value at the boundary
- [ ] Filesystem and network changes follow `docs/security_and_filtering.md` (no symlink following, credentials only to the GitHub API origin)
- [ ] Not security-relevant

## Documentation

- [ ] Permanent docs updated for any changed behavior, contract, or invariant (routing table: `AGENTS.md` sec. 6)
- [ ] `CHANGELOG.md` entry under `## [Unreleased]` for any behavior change
- [ ] Not needed (internal change with no observable effect)

## Reviewer notes

<!-- Where to start reading, risky spots, alternatives rejected and why, and follow-up work intentionally left out of this PR. -->
