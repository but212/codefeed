mod text;
mod token;
mod trie;
mod xml;

pub use token::TokenEstimatingWriter;
pub use trie::BudgetTrie;
pub use xml::{escaped_metrics, write_xml_escaped, xml_escape};

use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, DigestFormat, FileRecord};
use crate::core::policy::MAX_OUTPUT_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FormatFrameMetrics {
    pub(crate) fixed_frame: u64,
    pub(crate) revision_overhead: u64,
    pub(crate) per_file: u64,
}

impl FormatFrameMetrics {
    pub(crate) const TEXT: Self = text::FRAME_METRICS;
    pub(crate) const XML: Self = xml::FRAME_METRICS;
}

impl DigestFormat {
    pub(crate) fn escape_label_or_revision<'a>(self, text: &'a str) -> std::borrow::Cow<'a, str> {
        match self {
            Self::Text => text::escape_header(text),
            Self::Xml => xml::xml_escape(text),
        }
    }

    pub(crate) const fn escape_tree_xml(self) -> bool {
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

    pub(crate) const fn frame_metrics(self) -> FormatFrameMetrics {
        match self {
            Self::Text => FormatFrameMetrics::TEXT,
            Self::Xml => FormatFrameMetrics::XML,
        }
    }

    pub(crate) fn extra_metadata_bytes(self, budget: &DigestBudget) -> Option<u64> {
        match self {
            Self::Text => text::extra_metadata_bytes(budget.content.file_count, budget.skipped),
            Self::Xml => Some(0),
        }
    }

    pub(crate) fn rendered_base_size(self, budget: &DigestBudget) -> Option<u64> {
        let metrics = self.frame_metrics();
        let count = u64::try_from(budget.content.file_count).ok()?;
        let mut size = metrics.fixed_frame;
        size = size.checked_add(budget.label_bytes)?;
        if let Some(rev) = budget.revision_bytes {
            size = size.checked_add(metrics.revision_overhead)?;
            size = size.checked_add(rev)?;
        }
        size = size.checked_add(self.extra_metadata_bytes(budget)?)?;
        size = size.checked_add(count.checked_mul(metrics.per_file)?)?;
        size = size.checked_add(budget.content.path_bytes)?;
        size = size.checked_add(budget.content.content_bytes)?;
        size.checked_add(budget.content.trailing_bytes)
    }

    pub(crate) fn render<'a, W, I>(
        self,
        writer: &mut W,
        label: &str,
        revision: Option<&str>,
        files: I,
        skipped: usize,
        trie: &BudgetTrie,
    ) -> Result<(), IngestError>
    where
        W: std::io::Write + ?Sized,
        I: IntoIterator<Item = &'a FileRecord>,
    {
        match self {
            Self::Text => text::render(writer, label, revision, files, skipped, trie),
            Self::Xml => xml::render(writer, label, revision, files, trie),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ContentMetrics {
    pub(crate) file_count: usize,
    pub(crate) path_bytes: u64,
    pub(crate) content_bytes: u64,
    pub(crate) trailing_bytes: u64,
}

impl ContentMetrics {
    pub(crate) fn checked_add_file(
        &self,
        path_b: u64,
        content_b: u64,
        trailing_b: u64,
    ) -> Result<Self, IngestError> {
        let file_count = self
            .file_count
            .checked_add(1)
            .ok_or_else(|| IngestError::InvalidMetadata("file count overflow".into()))?;

        let path_bytes =
            self.path_bytes
                .checked_add(path_b)
                .ok_or_else(|| IngestError::OutputTooLarge {
                    largest_files: Vec::new(),
                })?;

        let content_bytes = self.content_bytes.checked_add(content_b).ok_or_else(|| {
            IngestError::OutputTooLarge {
                largest_files: Vec::new(),
            }
        })?;

        let trailing_bytes = self.trailing_bytes.checked_add(trailing_b).ok_or_else(|| {
            IngestError::OutputTooLarge {
                largest_files: Vec::new(),
            }
        })?;

        Ok(Self {
            file_count,
            path_bytes,
            content_bytes,
            trailing_bytes,
        })
    }
}

/// Accounting budget for digest size. Once any operation returns an `Err`,
/// the budget enters a terminal error state and must not be used further.
#[derive(Debug, Clone)]
pub(crate) struct DigestBudget {
    pub(crate) format: DigestFormat,
    pub(crate) label_bytes: u64,
    pub(crate) revision_bytes: Option<u64>,
    pub(crate) content: ContentMetrics,
    pub(crate) skipped: usize,
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
            content: ContentMetrics::default(),
            skipped: 0,
            trie: BudgetTrie::new(format.escape_tree_xml()),
        })
    }

    pub(crate) fn add(&mut self, file: &FileRecord) -> Result<(), IngestError> {
        let (path_b, content_b, trailing_b) = self.format.calculate_file_bytes(file)?;
        let new_content = self
            .content
            .checked_add_file(path_b, content_b, trailing_b)?;

        self.trie.insert(&file.path);
        self.content = new_content;

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
            top.insert(idx, (file.path.as_str(), size));
            top.truncate(5);
        }
    }

    top.into_iter()
        .map(|(path, size)| (path.to_string(), size))
        .collect()
}

pub(crate) fn byte_count(value: &str) -> Result<u64, IngestError> {
    u64::try_from(value.len()).map_err(|_| IngestError::OutputTooLarge {
        largest_files: Vec::new(),
    })
}

pub fn render_digest_to_writer<W: std::io::Write + ?Sized>(
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

    #[test]
    fn format_frame_metrics_and_flags_evaluate_in_const() {
        const {
            assert!(!DigestFormat::Text.escape_tree_xml());
            assert!(DigestFormat::Xml.escape_tree_xml());
            assert!(DigestFormat::Text.frame_metrics().fixed_frame > 0);
            assert!(DigestFormat::Xml.frame_metrics().fixed_frame > 0);
            assert!(
                FormatFrameMetrics::TEXT.fixed_frame
                    == DigestFormat::Text.frame_metrics().fixed_frame
            );
            assert!(
                FormatFrameMetrics::XML.fixed_frame
                    == DigestFormat::Xml.frame_metrics().fixed_frame
            );
        }
    }

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
    fn renders_from_arbitrary_iterator() -> Result<(), Box<dyn std::error::Error>> {
        let files = [
            FileRecord {
                path: "b.txt".into(),
                content: "content_b\n".into(),
            },
            FileRecord {
                path: "excluded.txt".into(),
                content: "skip_me\n".into(),
            },
            FileRecord {
                path: "a.txt".into(),
                content: "content_a\n".into(),
            },
        ];
        let trie = test_trie(&files[0..1], false);
        let mut buf = Vec::new();
        let filtered_iter = files.iter().filter(|f| f.path != "excluded.txt");
        DigestFormat::Text.render(&mut buf, "repo", None, filtered_iter, 1, &trie)?;
        let rendered = std::str::from_utf8(&buf)?;
        assert!(rendered.contains("Files: 2"));
        assert!(rendered.contains("FILE: a.txt"));
        assert!(rendered.contains("FILE: b.txt"));
        assert!(!rendered.contains("excluded.txt"));
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
        budget.content.content_bytes += MAX_OUTPUT_SIZE - current;
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

    #[test]
    fn largest_files_selects_top_five_with_stability() {
        let files = vec![
            FileRecord {
                path: "f1.txt".into(),
                content: "1".into(),
            },
            FileRecord {
                path: "f2.txt".into(),
                content: "22".into(),
            },
            FileRecord {
                path: "f3.txt".into(),
                content: "333".into(),
            },
            FileRecord {
                path: "f4.txt".into(),
                content: "4444".into(),
            },
            FileRecord {
                path: "f5_a.txt".into(),
                content: "55555".into(),
            },
            FileRecord {
                path: "f5_b.txt".into(),
                content: "55555".into(),
            },
            FileRecord {
                path: "f6.txt".into(),
                content: "666666".into(),
            },
        ];
        let result = largest_files(&files);
        assert_eq!(result.len(), 5);
        assert_eq!(result[0], ("f6.txt".into(), 6));
        assert_eq!(result[1], ("f5_a.txt".into(), 5));
        assert_eq!(result[2], ("f5_b.txt".into(), 5));
        assert_eq!(result[3], ("f4.txt".into(), 4));
        assert_eq!(result[4], ("f3.txt".into(), 3));
    }

    #[test]
    fn content_metrics_checked_add_file_accumulates_and_detects_overflow() {
        let metrics = ContentMetrics::default();
        let next = metrics.checked_add_file(10, 100, 1).unwrap();
        assert_eq!(
            next,
            ContentMetrics {
                file_count: 1,
                path_bytes: 10,
                content_bytes: 100,
                trailing_bytes: 1,
            }
        );

        let max_content = ContentMetrics {
            file_count: 1,
            path_bytes: 0,
            content_bytes: u64::MAX,
            trailing_bytes: 0,
        };
        assert!(max_content.checked_add_file(0, 1, 0).is_err());
    }
}
