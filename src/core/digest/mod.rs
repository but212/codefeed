//! Digest budgeting, tree structuring, format rendering, and token estimation.

mod format;
mod text;
mod token;
mod trie;
mod xml;

pub(crate) use format::spec_for;
pub(crate) use token::TokenEstimatingWriter;
pub(crate) use trie::BudgetTrie;

use crate::core::error::IngestError;
use crate::core::model::{DigestFormat, FileRecord};
use crate::core::policy::MAX_OUTPUT_SIZE;

#[derive(Debug, Clone)]
pub(crate) struct DigestBudget {
    pub(crate) format: DigestFormat,
    pub(crate) label_bytes: u64,
    pub(crate) revision_bytes: Option<u64>,
    pub(crate) file_count: usize,
    pub(crate) skipped: usize,
    pub(crate) path_bytes: u64,
    pub(crate) content_bytes: u64,
    pub(crate) trailing_bytes: u64,
    pub(crate) trie: BudgetTrie,
}

impl DigestBudget {
    pub(crate) fn new(
        label: &str,
        revision: Option<&str>,
        format: DigestFormat,
    ) -> Result<Self, IngestError> {
        let spec = spec_for(format);
        let label_bytes = byte_count(&spec.escape_label_or_revision(label))?;
        let revision_bytes = revision
            .map(|value| byte_count(&spec.escape_label_or_revision(value)))
            .transpose()?;

        Ok(Self {
            format,
            label_bytes,
            revision_bytes,
            file_count: 0,
            skipped: 0,
            path_bytes: 0,
            content_bytes: 0,
            trailing_bytes: 0,
            trie: BudgetTrie::new(spec.escape_tree_xml()),
        })
    }

    pub(crate) fn add(&mut self, file: &FileRecord) -> Result<(), IngestError> {
        let (path_b, content_b, trailing_b) = spec_for(self.format).calculate_file_bytes(file)?;

        let new_file_count = self
            .file_count
            .checked_add(1)
            .ok_or_else(|| IngestError::InvalidMetadata("file count overflow".into()))?;

        let new_path_bytes =
            self.path_bytes
                .checked_add(path_b)
                .ok_or_else(|| IngestError::OutputTooLarge {
                    largest_files: Vec::new(),
                })?;
        let new_content_bytes = self.content_bytes.checked_add(content_b).ok_or_else(|| {
            IngestError::OutputTooLarge {
                largest_files: Vec::new(),
            }
        })?;
        let new_trailing_bytes = self.trailing_bytes.checked_add(trailing_b).ok_or_else(|| {
            IngestError::OutputTooLarge {
                largest_files: Vec::new(),
            }
        })?;

        let undo = self.trie.insert_with_undo(&file.path);

        self.file_count = new_file_count;
        self.path_bytes = new_path_bytes;
        self.content_bytes = new_content_bytes;
        self.trailing_bytes = new_trailing_bytes;

        if let Err(err) = self.ensure_fits() {
            self.trie.rollback(undo);
            self.file_count -= 1;
            self.path_bytes -= path_b;
            self.content_bytes -= content_b;
            self.trailing_bytes -= trailing_b;
            return Err(err);
        }

        Ok(())
    }

    pub(crate) fn skip(&mut self) -> Result<(), IngestError> {
        self.skipped = self
            .skipped
            .checked_add(1)
            .ok_or_else(|| IngestError::InvalidMetadata("skipped file count overflow".into()))?;
        Ok(())
    }

    pub(crate) fn skipped(&self) -> usize {
        self.skipped
    }

    fn ensure_fits(&self) -> Result<(), IngestError> {
        let base_size = self
            .rendered_base_size()
            .ok_or_else(|| IngestError::OutputTooLarge {
                largest_files: Vec::new(),
            })?;
        let conservative_tree_upper = self
            .trie
            .display_bytes
            .saturating_add((self.trie.node_count as u64).saturating_mul(200));
        if base_size.saturating_add(conservative_tree_upper) <= MAX_OUTPUT_SIZE {
            return Ok(());
        }

        if self
            .rendered_size()
            .is_none_or(|size| size > MAX_OUTPUT_SIZE)
        {
            return Err(IngestError::OutputTooLarge {
                largest_files: Vec::new(),
            });
        }
        Ok(())
    }

    fn rendered_base_size(&self) -> Option<u64> {
        spec_for(self.format).rendered_base_size(self)
    }

    fn rendered_size(&self) -> Option<u64> {
        let base = self.rendered_base_size()?;
        base.checked_add(self.trie.calculate_size())
    }
}

pub(crate) fn largest_files<'a>(
    files: impl IntoIterator<Item = &'a FileRecord>,
) -> Vec<(String, usize)> {
    let mut entries: Vec<(String, usize)> = files
        .into_iter()
        .map(|f| (f.path.clone(), f.content.len()))
        .collect();
    entries.sort_by_key(|b| std::cmp::Reverse(b.1));
    entries.truncate(5);
    entries
}

pub(crate) fn byte_count(value: &str) -> Result<u64, IngestError> {
    u64::try_from(value.len()).map_err(|_| IngestError::OutputTooLarge {
        largest_files: Vec::new(),
    })
}

pub(crate) fn render_digest_to_writer<W: std::io::Write + ?Sized>(
    writer: &mut W,
    label: &str,
    revision: Option<&str>,
    files: &[FileRecord],
    skipped: usize,
    format: DigestFormat,
) -> Result<usize, IngestError> {
    let mut budget = DigestBudget::new(label, revision, format)?;
    budget.skipped = skipped;
    let overflow = (|| -> Result<(), IngestError> {
        for file in files {
            budget.add(file)?;
        }
        budget.ensure_fits()?;
        Ok(())
    })();
    if let Err(IngestError::OutputTooLarge { .. }) = overflow {
        return Err(IngestError::OutputTooLarge {
            largest_files: largest_files(files),
        });
    }
    overflow?;

    let mut estimator = TokenEstimatingWriter::new(writer);
    spec_for(format).render(
        &mut estimator,
        label,
        revision,
        files,
        skipped,
        &budget.trie,
    )?;
    Ok(estimator.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::IngestError;
    use crate::core::model::FileRecord;
    use crate::core::policy::MAX_OUTPUT_SIZE;

    fn render_digest(
        label: &str,
        revision: Option<&str>,
        files: &[FileRecord],
        skipped: usize,
    ) -> Result<String, IngestError> {
        let mut buffer = Vec::new();
        render_digest_to_writer(
            &mut buffer,
            label,
            revision,
            files,
            skipped,
            DigestFormat::Text,
        )?;
        String::from_utf8(buffer).map_err(|error| IngestError::Io(std::io::Error::other(error)))
    }

    #[test]
    fn renders_files_in_deterministic_order() -> Result<(), Box<dyn std::error::Error>> {
        let files = vec![
            FileRecord {
                path: "z.txt".into(),
                content: "z".into(),
            },
            FileRecord {
                path: "a.txt".into(),
                content: "a".into(),
            },
        ];
        let digest = render_digest("repo", Some("abc"), &files, 1)?;
        let first = digest
            .find("FILE: a.txt")
            .ok_or_else(|| std::io::Error::other("a.txt is missing"))?;
        let second = digest
            .find("FILE: z.txt")
            .ok_or_else(|| std::io::Error::other("z.txt is missing"))?;
        assert!(first < second);
        assert!(digest.contains("Revision: abc"));
        assert!(digest.contains("Skipped: 1"));
        Ok(())
    }

    #[test]
    fn escapes_revision_header() -> Result<(), Box<dyn std::error::Error>> {
        let revision = "feature\\x\r\nFiles: 9";
        let rendered = render_digest("repo", Some(revision), &[], 0)?;

        assert_eq!(
            rendered.lines().nth(1),
            Some("Revision: feature\\\\x\\r\\nFiles: 9")
        );
        assert!(!rendered.contains(revision));
        Ok(())
    }

    #[test]
    fn streaming_matches_string_render() -> Result<(), Box<dyn std::error::Error>> {
        let files = vec![
            FileRecord {
                path: "b.txt".into(),
                content: "hello\n".into(),
            },
            FileRecord {
                path: "a.txt".into(),
                content: "world".into(),
            },
        ];
        let mut streamed = Vec::new();
        render_digest_to_writer(
            &mut streamed,
            "my-repo",
            Some("v1.0"),
            &files,
            2,
            DigestFormat::Text,
        )?;
        let streamed_str = std::str::from_utf8(&streamed)?;

        let expected = "\
Repository: my-repo\n\
Revision: v1.0\n\
Files: 2\n\
Skipped: 2\n\
\n\
Directory structure:\n\
├── a.txt\n\
└── b.txt\n\
\n\
Files:\n\
\n\
==================================================\n\
FILE: a.txt\n\
==================================================\n\
\n\
world\n\
\n\
==================================================\n\
FILE: b.txt\n\
==================================================\n\
\n\
hello\n\
\n";
        assert_eq!(streamed_str, expected);
        Ok(())
    }

    #[test]
    fn digest_budget_matches_rendered_size() -> Result<(), Box<dyn std::error::Error>> {
        let files = vec![
            FileRecord {
                path: "z.txt".into(),
                content: "z\n".into(),
            },
            FileRecord {
                path: "a.txt".into(),
                content: "a".into(),
            },
        ];
        for revision in ["abc", "feature\\x\r\nFiles: 9"] {
            let mut budget = DigestBudget::new("repo", Some(revision), DigestFormat::Text)?;
            for file in &files {
                budget.add(file)?;
            }
            budget.skip()?;
            let rendered = render_digest("repo", Some(revision), &files, budget.skipped())?;

            assert_eq!(
                budget.rendered_size(),
                Some(rendered.len() as u64),
                "revision: {revision:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn rejects_digest_before_storing_over_budget_content() {
        let candidate = FileRecord {
            path: "large.txt".into(),
            content: "x".repeat(MAX_OUTPUT_SIZE as usize),
        };
        match render_digest("repo", None, &[candidate], 0) {
            Err(IngestError::OutputTooLarge { largest_files }) => {
                assert_eq!(largest_files.len(), 1);
                assert_eq!(largest_files[0].0, "large.txt");
                assert_eq!(largest_files[0].1, MAX_OUTPUT_SIZE as usize);
            }
            other => panic!("expected OutputTooLarge with largest_files, got {other:?}"),
        }
    }

    #[test]
    fn streaming_rejects_overbudget() {
        let candidate = FileRecord {
            path: "large.txt".into(),
            content: "x".repeat(MAX_OUTPUT_SIZE as usize),
        };
        let mut sink = Vec::new();
        match render_digest_to_writer(&mut sink, "repo", None, &[candidate], 0, DigestFormat::Text)
        {
            Err(IngestError::OutputTooLarge { largest_files }) => {
                assert_eq!(largest_files.len(), 1);
                assert_eq!(largest_files[0].0, "large.txt");
                assert_eq!(largest_files[0].1, MAX_OUTPUT_SIZE as usize);
            }
            other => panic!("expected OutputTooLarge with largest_files, got {other:?}"),
        }
        assert!(
            sink.is_empty(),
            "sink should not have partial content written on overflow"
        );
    }

    #[test]
    fn renders_xml_in_deterministic_order() -> Result<(), Box<dyn std::error::Error>> {
        let files = vec![
            FileRecord {
                path: "z.txt".into(),
                content: "z < 10".into(),
            },
            FileRecord {
                path: "a.txt".into(),
                content: "a & b".into(),
            },
        ];
        let mut out = Vec::new();
        render_digest_to_writer(
            &mut out,
            "my-repo",
            Some("v1.0"),
            &files,
            1,
            DigestFormat::Xml,
        )?;
        let xml_str = std::str::from_utf8(&out)?;
        let expected = "\
<repository name=\"my-repo\" revision=\"v1.0\">
<directory_structure>
├── a.txt
└── z.txt
</directory_structure>
<files>
<file path=\"a.txt\">
a &amp; b
</file>
<file path=\"z.txt\">
z &lt; 10
</file>
</files>
</repository>
";
        assert_eq!(xml_str, expected);
        Ok(())
    }

    #[test]
    fn xml_budget_matches_rendered_size() -> Result<(), Box<dyn std::error::Error>> {
        let files = vec![
            FileRecord {
                path: "z.txt".into(),
                content: "z & 1\n".into(),
            },
            FileRecord {
                path: "a.txt".into(),
                content: "a < 2".into(),
            },
        ];
        for revision in [None, Some("v1.0")] {
            let mut budget = DigestBudget::new("repo", revision, DigestFormat::Xml)?;
            for file in &files {
                budget.add(file)?;
            }
            let mut out = Vec::new();
            render_digest_to_writer(&mut out, "repo", revision, &files, 0, DigestFormat::Xml)?;
            assert_eq!(budget.rendered_size(), Some(out.len() as u64));
        }
        Ok(())
    }

    #[test]
    fn escapes_tree_node_newlines() -> Result<(), Box<dyn std::error::Error>> {
        let files = vec![FileRecord {
            path: "dir\nname/file\rname.txt".into(),
            content: "content".into(),
        }];
        let rendered = render_digest("repo", None, &files, 0)?;
        assert!(rendered.contains("dir\\nname/"));
        assert!(rendered.contains("file\\rname.txt"));
        assert!(!rendered.contains("dir\n"));
        Ok(())
    }

    #[test]
    fn escapes_tree_node_xml_entities() -> Result<(), Box<dyn std::error::Error>> {
        let files = vec![FileRecord {
            path: "a & b/c < d.txt".into(),
            content: "content".into(),
        }];
        let mut out = Vec::new();
        render_digest_to_writer(&mut out, "repo", None, &files, 0, DigestFormat::Xml)?;
        let xml_str = std::str::from_utf8(&out)?;
        assert!(xml_str.contains("a &amp; b/"));
        assert!(xml_str.contains("c &lt; d.txt"));
        Ok(())
    }

    #[test]
    fn budget_rollback_restores_exact_state() {
        let mut budget = DigestBudget::new("repo", None, DigestFormat::Text).unwrap();
        budget
            .add(&FileRecord {
                path: "src/main.rs".into(),
                content: "fn main() {}".into(),
            })
            .unwrap();
        let valid_size = budget.rendered_size().unwrap();

        let oversized = FileRecord {
            path: "large.rs".into(),
            content: "a".repeat(crate::core::policy::MAX_OUTPUT_SIZE as usize),
        };
        assert!(budget.add(&oversized).is_err());
        assert_eq!(budget.rendered_size(), Some(valid_size));
        assert_eq!(budget.file_count, 1);
    }
}
