//! Format specification trait abstracting format-specific escaping, budgeting, and rendering.

use std::borrow::Cow;
use std::io::Write;

use super::DigestBudget;
use super::text::TextFormat;
use super::trie::BudgetTrie;
use super::xml::XmlFormat;
use crate::core::error::IngestError;
use crate::core::model::{DigestFormat, FileRecord};

pub(crate) trait FormatSpec: Sync {
    fn escape_label_or_revision<'a>(&self, text: &'a str) -> Cow<'a, str>;
    fn escape_tree_xml(&self) -> bool;
    fn calculate_file_bytes(&self, file: &FileRecord) -> Result<(u64, u64, u64), IngestError>;
    fn rendered_base_size(&self, budget: &DigestBudget) -> Option<u64>;
    fn render(
        &self,
        writer: &mut dyn Write,
        label: &str,
        revision: Option<&str>,
        files: &[FileRecord],
        skipped: usize,
        trie: &BudgetTrie,
    ) -> Result<(), IngestError>;
}

pub(crate) fn spec_for(format: DigestFormat) -> &'static dyn FormatSpec {
    match format {
        DigestFormat::Text => &TextFormat,
        DigestFormat::Xml => &XmlFormat,
    }
}
