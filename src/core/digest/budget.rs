//! Accounting budget and diagnostic ranking for digest size.

use super::trie::BudgetTrie;
use crate::core::model::FileRecord;
use crate::core::policy::MAX_OUTPUT_SIZE;

/// Signal indicating that a candidate file would exceed MAX_OUTPUT_SIZE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BudgetOverflow;

/// Accounting budget for digest size.
#[derive(Debug, Clone, Default)]
pub(crate) struct DigestBudget {
    pub(crate) content_bytes: u64,
    pub(crate) skipped: usize,
    pub(crate) trie: BudgetTrie,
}

impl DigestBudget {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn add(&mut self, file: &FileRecord<'_>) -> Result<(), BudgetOverflow> {
        let file_bytes = (file.path.len() + file.content.len()) as u64;
        let new_total = self.content_bytes.saturating_add(file_bytes);
        if new_total > MAX_OUTPUT_SIZE {
            return Err(BudgetOverflow);
        }
        self.trie.insert(file.path);
        self.content_bytes = new_total;
        Ok(())
    }

    pub(crate) fn skip(&mut self) {
        self.skipped = self.skipped.saturating_add(1);
    }

    pub(crate) fn skipped(&self) -> usize {
        self.skipped
    }
}

pub(crate) fn largest_files<'a>(
    files: impl IntoIterator<Item = FileRecord<'a>>,
) -> Vec<(String, usize)> {
    let mut top: Vec<(&'a str, usize)> = Vec::with_capacity(6);

    for file in files {
        let size = file.content.len();
        if top.len() == 5 && size <= top.last().unwrap().1 {
            continue;
        }

        let idx = top
            .iter()
            .position(|probe| probe.1 < size)
            .unwrap_or(top.len());
        if idx < 5 {
            top.insert(idx, (file.path, size));
            top.truncate(5);
        }
    }

    top.into_iter()
        .map(|(path, size)| (path.to_string(), size))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn largest_files_selects_top_five_with_stability() {
        let files = [
            FileRecord {
                path: "f1.txt",
                content: "1",
            },
            FileRecord {
                path: "f2.txt",
                content: "22",
            },
            FileRecord {
                path: "f3.txt",
                content: "333",
            },
            FileRecord {
                path: "f4.txt",
                content: "4444",
            },
            FileRecord {
                path: "f5_a.txt",
                content: "55555",
            },
            FileRecord {
                path: "f5_b.txt",
                content: "55555",
            },
            FileRecord {
                path: "f6.txt",
                content: "666666",
            },
        ];
        let result = largest_files(files);
        assert_eq!(result.len(), 5);
        assert_eq!(result[0], ("f6.txt".into(), 6));
        assert_eq!(result[1], ("f5_a.txt".into(), 5));
        assert_eq!(result[2], ("f5_b.txt".into(), 5));
        assert_eq!(result[3], ("f4.txt".into(), 4));
        assert_eq!(result[4], ("f3.txt".into(), 3));
    }
}
