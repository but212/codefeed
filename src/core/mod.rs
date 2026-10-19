//! Functional core: pure domain logic with no I/O, clock, network, or entropy.
//!
//! Core modules never depend on `shell`; the imperative shell depends on the
//! core in one direction.
//!
//! # Referential Transparency & Mutation Rules
//!
//! All functions in `core` are referentially transparent and immutable.
//! Any operation with side effects must live in `src/shell/`.
//!
//! - **Move-Only State Transitions:** State transitions occur strictly via
//!   consuming ownership moves (`self` -> `Self` / linear typestates like
//!   [`selector::PendingEntry`]). `&mut self` is forbidden.
//! - **No Interior Mutability or Unsafe:** `Cell`, `RefCell`, `Atomic*`, `Mutex`,
//!   `RwLock`, `static mut`, and `unsafe` are strictly forbidden.
//! - **Closed List of Mutation Exceptions:**
//!   1. `std::fmt::Display::fmt(&self, f: &mut std::fmt::Formatter<'_>)`: Required
//!      by standard library trait contract.
//!   2. Function-local stack variables and owned value parameters (`let mut` / `mut self`):
//!      Fully isolated scratch computation with zero caller-observable side effects.
//!      Any `&mut` reference parameter (including `&mut self`) is strictly forbidden.

pub(crate) mod archive;
pub(crate) mod cli;
pub(crate) mod error;
pub(crate) mod github;
pub(crate) mod model;
pub(crate) mod policy;
pub(crate) mod render;
pub(crate) mod selector;
pub(crate) mod storage;
