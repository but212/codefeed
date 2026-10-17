//! Pure decision logic: candidate observation, budget accounting, and storage accumulation.

mod budget;
mod item;
mod selector;

pub use budget::CandidateBudget;
pub use item::{CandidateInput, text_str};
pub use selector::Selector;
pub(crate) use selector::largest_files;
