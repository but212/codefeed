mod text;
mod token;
mod trie;
mod xml;

pub(crate) use token::TokenEstimatingWriter;
pub(crate) use trie::BudgetTrie;

use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, DigestFormat, FileRecord};
use crate::core::policy::MAX_OUTPUT_SIZE;

impl DigestFormat {
    pub(crate) fn escape_label_or_revision<'a>(self, text: &'a str) -> std::borrow::Cow<'a, str> {
        match self {
            Self::Text => text::escape_header(text),
            Self::Xml => xml::xml_escape(text),
        }
    }

    pub(crate) fn escape_tree_xml(self) -> bool {
        match self {
            Self::Text => false,
            Self::Xml => true,
        }
    }

    pub(crate) fn calculate_file_bytes(
        self,
        file: &FileRecord,
    ) -> Result<(u64, u64, u64), IngestError> {
        match self {
            Self::Text => text::calculate_file_bytes(file),
            Self::Xml => xml::calculate_file_bytes(file),
        }
    }

    pub(crate) fn rendered_base_size(self, budget: &DigestBudget) -> Option<u64> {
        match self {
            Self::Text => text::rendered_base_size(budget),
            Self::Xml => xml::rendered_base_size(budget),
        }
    }

    pub(crate) fn render<W: std::io::Write + ?Sized>(
        self,
        writer: &mut W,
        label: &str,
        revision: Option<&str>,
        files: &[FileRecord],
        skipped: usize,
        trie: &BudgetTrie,
    ) -> Result<(), IngestError> {
        match self {
            Self::Text => text::render(writer, label, revision, files, skipped, trie),
            Self::Xml => xml::render(writer, label, revision, files, trie),
        }
    }
}

/// Accounting budget for digest size. Once any operation returns an `Err`,
/// the budget enters a terminal error state and must not be used further.
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
        let label_bytes = byte_count(&format.escape_label_or_revision(label))?;
        let revision_bytes = revision
            .map(|value| byte_count(&format.escape_label_or_revision(value)))
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
            trie: BudgetTrie::new(format.escape_tree_xml()),
        })
    }

    pub(crate) fn add(&mut self, file: &FileRecord) -> Result<(), IngestError> {
        let (path_b, content_b, trailing_b) = self.format.calculate_file_bytes(file)?;

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

        self.trie.insert(&file.path);

        self.file_count = new_file_count;
        self.path_bytes = new_path_bytes;
        self.content_bytes = new_content_bytes;
        self.trailing_bytes = new_trailing_bytes;

        self.ensure_fits()?;

        Ok(())
    }

    pub(crate) fn skip(&mut self) -> Result<(), IngestError> {
        self.skipped = self
            .skipped
            .checked_add(1)
            .ok_or_else(|| IngestError::InvalidMetadata("skipped file count overflow".into()))?;
        self.ensure_fits()
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
        let conservative_tree_upper = self.trie.upper_bound_bytes;
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
        self.format.rendered_base_size(self)
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
    collection: &CollectionResult,
    format: DigestFormat,
) -> Result<usize, IngestError> {
    let mut estimator = TokenEstimatingWriter::new(writer);
    format.render(
        &mut estimator,
        &collection.label,
        collection.revision.as_deref(),
        &collection.files,
        collection.skipped,
        &collection.trie,
    )?;
    Ok(estimator.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::IngestError;
    use crate::core::model::FileRecord;
    use crate::core::policy::MAX_OUTPUT_SIZE;

    fn test_trie(files: &[FileRecord], escape_xml: bool) -> BudgetTrie {
        let mut trie = BudgetTrie::new(escape_xml);
        for file in files {
            trie.insert(&file.path);
        }
        trie
    }

    fn render_digest(
        label: &str,
        revision: Option<&str>,
        files: &[FileRecord],
        skipped: usize,
    ) -> Result<String, IngestError> {
        let trie = test_trie(files, false);
        let collection = CollectionResult {
            label: label.to_string(),
            revision: revision.map(str::to_string),
            files: files.to_vec(),
            skipped,
            trie,
        };
        let mut buffer = Vec::new();
        render_digest_to_writer(&mut buffer, &collection, DigestFormat::Text)?;
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
        let collection = CollectionResult {
            label: "my-repo".into(),
            revision: Some("v1.0".into()),
            files: files.clone(),
            skipped: 2,
            trie: test_trie(&files, false),
        };
        let mut streamed = Vec::new();
        render_digest_to_writer(&mut streamed, &collection, DigestFormat::Text)?;
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
        let mut budget = DigestBudget::new("repo", None, DigestFormat::Text).unwrap();
        match budget.add(&candidate) {
            Err(IngestError::OutputTooLarge { .. }) => {}
            other => panic!("expected OutputTooLarge, got {other:?}"),
        }
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
        let collection = CollectionResult {
            label: "my-repo".into(),
            revision: Some("v1.0".into()),
            files: files.clone(),
            skipped: 1,
            trie: test_trie(&files, true),
        };
        let mut out = Vec::new();
        render_digest_to_writer(&mut out, &collection, DigestFormat::Xml)?;
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
            let collection = CollectionResult {
                label: "repo".into(),
                revision: revision.map(str::to_string),
                files: files.clone(),
                skipped: 0,
                trie: budget.trie.clone(),
            };
            let mut out = Vec::new();
            render_digest_to_writer(&mut out, &collection, DigestFormat::Xml)?;
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
        let collection = CollectionResult {
            label: "repo".into(),
            revision: None,
            files: files.clone(),
            skipped: 0,
            trie: test_trie(&files, true),
        };
        let mut out = Vec::new();
        render_digest_to_writer(&mut out, &collection, DigestFormat::Xml)?;
        let xml_str = std::str::from_utf8(&out)?;
        assert!(xml_str.contains("a &amp; b/"));
        assert!(xml_str.contains("c &lt; d.txt"));
        Ok(())
    }

    #[test]
    fn skip_enforces_budget_limit_on_digit_growth() {
        let mut budget = DigestBudget::new("repo", None, DigestFormat::Text).unwrap();
        for _ in 0..9 {
            budget.skip().unwrap();
        }
        budget
            .add(&FileRecord {
                path: "a.txt".into(),
                content: "a\n".into(),
            })
            .unwrap();
        let current = budget.rendered_size().unwrap();
        budget.content_bytes += MAX_OUTPUT_SIZE - current;
        assert_eq!(budget.rendered_size(), Some(MAX_OUTPUT_SIZE));

        assert!(budget.skip().is_err());
    }

    #[test]
    fn deep_tree_enforces_output_size_limit() {
        let deep_path = trie::tests::deep_test_path(1800);

        let mut budget = DigestBudget::new("repo", None, DigestFormat::Text).unwrap();
        let large_content_len = (MAX_OUTPUT_SIZE - 2 * 1024 * 1024) as usize;
        let file = FileRecord {
            path: deep_path,
            content: "a".repeat(large_content_len),
        };
        let result = budget.add(&file);
        assert!(
            matches!(result, Err(IngestError::OutputTooLarge { .. })),
            "Expected OutputTooLarge due to actual tree size exceeding budget, got {result:?}"
        );
    }

    #[test]
    fn control_character_policy_preserves_raw_text_and_sanitizes_xml()
    -> Result<(), Box<dyn std::error::Error>> {
        let raw_ansi = "\x1b[31mRed Alert\x1b[0m\tTabbed\r\nNewline";
        let files = vec![FileRecord {
            path: "ansi.txt".into(),
            content: raw_ansi.into(),
        }];

        // 1. Plain text format preserves all UTF-8 bytes verbatim for LLM fidelity
        let text_rendered = render_digest("repo", None, &files, 0)?;
        assert!(
            text_rendered.contains(raw_ansi),
            "Text format must preserve raw ANSI/control characters for LLM fidelity"
        );

        // 2. XML format sanitizes disallowed XML 1.0 control characters (e.g. \x1b -> \u{FFFD})
        let collection = CollectionResult {
            label: "repo".into(),
            revision: None,
            files: files.clone(),
            skipped: 0,
            trie: test_trie(&files, true),
        };
        let mut xml_out = Vec::new();
        render_digest_to_writer(&mut xml_out, &collection, DigestFormat::Xml)?;
        let xml_str = std::str::from_utf8(&xml_out)?;
        assert!(
            !xml_str.contains("\x1b"),
            "XML format must not emit raw ASCII ESC"
        );
        assert!(
            xml_str.contains("\u{FFFD}[31mRed Alert\u{FFFD}[0m\tTabbed\r\nNewline"),
            "XML format must sanitize ESC to replacement char while preserving valid whitespace"
        );

        Ok(())
    }
}
