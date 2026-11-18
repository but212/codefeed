//! Functional core: pure domain logic with no I/O, clock, network, or entropy.
//!
//! Never depends on `shell`; purity rules and mutation exceptions live in
//! `docs/coding_convention.md` § 3.1.

/// Shared configuration for property tests: a fixed seed and no on-disk failure persistence, so runs
/// are reproducible and leave no `proptest-regressions/` files behind.
#[cfg(test)]
pub(crate) fn proptest_config() -> proptest::test_runner::Config {
    use proptest::test_runner::{Config, RngSeed};

    Config {
        cases: 256,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(0x00C0_DEFE_ED00_0001),
        ..Config::default()
    }
}

pub(crate) mod cli;
pub(crate) mod error;
pub(crate) mod github;
pub(crate) mod model;
pub(crate) mod path;
pub(crate) mod policy;
pub(crate) mod render;
pub(crate) mod selector;
pub(crate) mod source;
pub(crate) mod storage;
