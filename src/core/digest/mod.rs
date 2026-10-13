mod text;
mod trie;
mod xml;

pub use trie::BudgetTrie;
pub use xml::{escaped_metrics, write_xml_escaped, xml_escape};

use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, DigestFormat, FileRecord};
use crate::core::policy::MAX_OUTPUT_SIZE;

impl DigestFormat {
    pub(crate) const fn escape_tree_xml(self) -> bool {
        match self {
            Self::Text => false,
            Self::Xml => true,
        }
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
        let mut ordered_files: Vec<(&str, &FileRecord)> =
            files.into_iter().map(|f| (f.path.as_str(), f)).collect();
        ordered_files.sort_unstable_by_key(|(path, _)| *path);

        match self {
            Self::Text => text::render(writer, label, revision, &ordered_files, skipped, trie),
            Self::Xml => xml::render(writer, label, revision, &ordered_files, trie),
        }
    }
}

/// Accounting budget for digest size.
#[derive(Debug, Clone)]
pub(crate) struct DigestBudget {
    pub(crate) format: DigestFormat,
    pub(crate) content_bytes: u64,
    pub(crate) skipped: usize,
    pub(crate) trie: BudgetTrie,
}

impl DigestBudget {
    pub(crate) fn new(format: DigestFormat) -> Self {
        Self {
            format,
            content_bytes: 0,
            skipped: 0,
            trie: BudgetTrie::new(format.escape_tree_xml()),
        }
    }

    pub(crate) fn add(&mut self, file: &FileRecord) -> Result<(), IngestError> {
        let file_bytes = (file.path.len() + file.content.len()) as u64;
        let new_total = self.content_bytes.saturating_add(file_bytes);
        if new_total > MAX_OUTPUT_SIZE {
            return Err(IngestError::OutputTooLarge {
                largest_files: Vec::new(),
            });
        }
        self.trie.insert(&file.path);
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

/// State machine tracking token count heuristics across character streams.
#[derive(Default, Debug, Clone)]
pub struct TokenEstimator {
    tokens: usize,
    in_word: bool,
    word_len: usize,
    saw_input: bool,
}

impl TokenEstimator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn process_char(&mut self, ch: char) {
        self.saw_input = true;
        if ch.is_ascii_alphanumeric() || ch == '_' {
            self.in_word = true;
            self.word_len += 1;
            if self.word_len >= 4 {
                self.tokens += 1;
                self.word_len = 0;
            }
        } else {
            if std::mem::take(&mut self.in_word) {
                self.tokens += 1;
                self.word_len = 0;
            }
            if !ch.is_ascii_whitespace() {
                if ch.is_ascii() {
                    self.tokens += 1;
                } else {
                    self.tokens += 2;
                }
            } else if ch == '\n' {
                self.tokens += 1;
            }
        }
    }

    pub fn process_str(&mut self, text: &str) {
        for ch in text.chars() {
            self.process_char(ch);
        }
    }

    pub fn add_fallback_tokens(&mut self, count: usize) {
        self.saw_input = true;
        self.tokens += count;
    }

    pub fn finish(mut self) -> usize {
        if self.in_word {
            self.tokens += 1;
        }
        if self.tokens == 0 && !self.saw_input {
            0
        } else {
            self.tokens.max(1)
        }
    }
}

pub fn estimate_tokens(text: &str) -> usize {
    let mut estimator = TokenEstimator::new();
    estimator.process_str(text);
    estimator.finish()
}

struct BoundedEstimatingWriter<'a, W: std::io::Write + ?Sized> {
    inner: &'a mut W,
    written: u64,
    estimator: TokenEstimator,
    pending: [u8; 4],
    pending_len: usize,
    overflowed: bool,
}

impl<'a, W: std::io::Write + ?Sized> BoundedEstimatingWriter<'a, W> {
    fn new(inner: &'a mut W) -> Self {
        Self {
            inner,
            written: 0,
            estimator: TokenEstimator::new(),
            pending: [0; 4],
            pending_len: 0,
            overflowed: false,
        }
    }

    fn finish(mut self) -> usize {
        if self.pending_len > 0 {
            self.estimator.add_fallback_tokens(2);
        }
        self.estimator.finish()
    }
}

impl<W: std::io::Write + ?Sized> std::io::Write for BoundedEstimatingWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let new_written = self.written.saturating_add(buf.len() as u64);
        if new_written > MAX_OUTPUT_SIZE {
            self.overflowed = true;
            return Err(std::io::Error::other("output size limit exceeded"));
        }

        self.inner.write_all(buf)?;
        self.written = new_written;

        let mut input = buf;
        if self.pending_len > 0 {
            while !input.is_empty() && self.pending_len < 4 {
                self.pending[self.pending_len] = input[0];
                self.pending_len += 1;
                input = &input[1..];
                if let Ok(s) = std::str::from_utf8(&self.pending[..self.pending_len]) {
                    let ch = s.chars().next().expect("valid utf8 has at least one char");
                    self.pending_len = 0;
                    self.estimator.process_char(ch);
                    break;
                }
            }
        }

        if !input.is_empty() {
            match std::str::from_utf8(input) {
                Ok(text) => {
                    self.estimator.process_str(text);
                }
                Err(e) => {
                    let valid_len = e.valid_up_to();
                    if valid_len > 0 {
                        let valid_str = match std::str::from_utf8(&input[..valid_len]) {
                            Ok(s) => s,
                            Err(_) => unreachable!(),
                        };
                        self.estimator.process_str(valid_str);
                    }
                    if e.error_len().is_none() {
                        let remaining = &input[valid_len..];
                        self.pending[..remaining.len()].copy_from_slice(remaining);
                        self.pending_len = remaining.len();
                    }
                }
            }
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

pub fn render_digest_to_writer<W: std::io::Write + ?Sized>(
    writer: &mut W,
    collection: &CollectionResult,
) -> Result<usize, IngestError> {
    let mut estimating_writer = BoundedEstimatingWriter::new(writer);
    let render_res = collection.format.render(
        &mut estimating_writer,
        &collection.label,
        collection.revision.as_deref(),
        &collection.files,
        collection.skipped,
        &collection.trie,
    );

    if estimating_writer.overflowed {
        return Err(IngestError::OutputTooLarge {
            largest_files: largest_files(&collection.files),
        });
    }

    render_res?;
    Ok(estimating_writer.finish())
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
            format: DigestFormat::Text,
        };
        let mut buffer = Vec::new();
        render_digest_to_writer(&mut buffer, &collection)?;
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
            format: DigestFormat::Text,
        };
        let mut streamed = Vec::new();
        render_digest_to_writer(&mut streamed, &collection)?;
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
        let trie = test_trie(&[files[0].clone(), files[2].clone()], false);
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
    fn rejects_digest_before_storing_over_budget_content() {
        let candidate = FileRecord {
            path: "large.txt".into(),
            content: "x".repeat(MAX_OUTPUT_SIZE as usize + 1),
        };
        let mut budget = DigestBudget::new(DigestFormat::Text);
        match budget.add(&candidate) {
            Err(IngestError::OutputTooLarge { .. }) => {}
            other => panic!("expected OutputTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn render_digest_to_writer_enforces_output_size_cap() {
        let candidate = FileRecord {
            path: "large.txt".into(),
            content: "x".repeat(MAX_OUTPUT_SIZE as usize),
        };
        let collection = CollectionResult {
            label: "repo".into(),
            revision: None,
            files: vec![candidate],
            skipped: 0,
            trie: BudgetTrie::new(false),
            format: DigestFormat::Text,
        };
        let mut sink = Vec::new();
        match render_digest_to_writer(&mut sink, &collection) {
            Err(IngestError::OutputTooLarge { largest_files }) => {
                assert_eq!(largest_files.len(), 1);
                assert_eq!(largest_files[0].0, "large.txt");
            }
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
            format: DigestFormat::Xml,
        };
        let mut out = Vec::new();
        render_digest_to_writer(&mut out, &collection)?;
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
    fn control_character_policy_preserves_raw_text_and_sanitizes_xml()
    -> Result<(), Box<dyn std::error::Error>> {
        let raw_ansi = "\x1b[31mRed Alert\x1b[0m\tTabbed\r\nNewline";
        let files = vec![FileRecord {
            path: "ansi.txt".into(),
            content: raw_ansi.into(),
        }];

        let text_rendered = render_digest("repo", None, &files, 0)?;
        assert!(
            text_rendered.contains(raw_ansi),
            "Text format must preserve raw ANSI/control characters for LLM fidelity"
        );

        let collection = CollectionResult {
            label: "repo".into(),
            revision: None,
            files: files.clone(),
            skipped: 0,
            trie: test_trie(&files, true),
            format: DigestFormat::Xml,
        };
        let mut xml_out = Vec::new();
        render_digest_to_writer(&mut xml_out, &collection)?;
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
    fn test_estimate_tokens_counts() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("hello"), 2);
        assert!(estimate_tokens("let mut x = 123;") >= 4);
        assert!(estimate_tokens("fn main() {\n    println!(\"Hello, world!\");\n}\n") >= 8);
        assert!(estimate_tokens("한글 텍스트 테스트") >= 4);
        assert!(estimate_tokens("word1 word2...word3\nword4") >= 5);
    }
}
