mod text;
mod trie;
mod xml;

pub use trie::BudgetTrie;
pub use xml::{write_xml_escaped, xml_escape};

use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, DigestFormat, FileRecord};
use crate::core::policy::MAX_OUTPUT_SIZE;

const FLAG_WORD: u8 = 1 << 0; // [0-9a-zA-Z_]
const FLAG_WS: u8 = 1 << 1; // whitespace
const FLAG_NL: u8 = 1 << 2; // \n

const CHAR_FLAGS: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut b = 0usize;
    while b < 256 {
        let is_num = b >= b'0' as usize && b <= b'9' as usize;
        let is_upper = b >= b'A' as usize && b <= b'Z' as usize;
        let is_lower = b >= b'a' as usize && b <= b'z' as usize;
        let is_under = b == b'_' as usize;
        if is_num || is_upper || is_lower || is_under {
            t[b] |= FLAG_WORD;
        }
        if b == b' ' as usize || b == b'\t' as usize || b == b'\r' as usize || b == 0x0C {
            t[b] |= FLAG_WS;
        }
        if b == b'\n' as usize {
            t[b] |= FLAG_WS | FLAG_NL;
        }
        b += 1;
    }
    t
};

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
        I: IntoIterator<Item = FileRecord<'a>>,
    {
        let mut ordered_files: Vec<FileRecord<'a>> = files.into_iter().collect();
        ordered_files.sort_unstable_by_key(|f| f.path);

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

    pub(crate) fn add(&mut self, file: &FileRecord<'_>) -> Result<(), IngestError> {
        let file_bytes = (file.path.len() + file.content.len()) as u64;
        let new_total = self.content_bytes.saturating_add(file_bytes);
        if new_total > MAX_OUTPUT_SIZE {
            return Err(IngestError::OutputTooLarge {
                largest_files: Vec::new(),
            });
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

    #[inline(always)]
    pub fn process_byte(&mut self, b: u8) {
        self.saw_input = true;
        let flags = CHAR_FLAGS[b as usize];

        let is_word = (flags & FLAG_WORD) as usize; // 0 or 1
        let is_ws = ((flags & FLAG_WS) >> 1) as usize; // 0 or 1
        let is_nl = ((flags & FLAG_NL) >> 2) as usize; // 0 or 1

        let prev_in_word = self.in_word as usize;
        self.in_word = is_word != 0;

        let next_word_len = (self.word_len + 1) * is_word;
        let full_word = (next_word_len >= 4) as usize;
        self.word_len = next_word_len * (1 - full_word);

        let is_cont = ((b & 0xC0) == 0x80) as usize;
        let is_lead = 1 - is_cont;
        let is_non_ascii_lead = ((b >= 0x80) as usize) & is_lead;

        let non_ws_weight = (1 - is_ws) * (is_lead + is_non_ascii_lead);
        let non_word_tokens = (1 - is_word) * (prev_in_word + non_ws_weight + is_nl);

        self.tokens += full_word + non_word_tokens;
    }

    #[inline]
    pub fn process_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.process_byte(b);
        }
    }

    pub fn process_str(&mut self, text: &str) {
        self.process_bytes(text.as_bytes());
    }

    pub fn finish(mut self) -> usize {
        self.tokens += self.in_word as usize;
        let non_empty = (self.tokens > 0) | self.saw_input;
        self.tokens.max(non_empty as usize)
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
    overflowed: bool,
}

impl<'a, W: std::io::Write + ?Sized> BoundedEstimatingWriter<'a, W> {
    fn new(inner: &'a mut W) -> Self {
        Self {
            inner,
            written: 0,
            estimator: TokenEstimator::new(),
            overflowed: false,
        }
    }

    fn finish(self) -> usize {
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
        self.estimator.process_bytes(buf);

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
        collection.files.iter(),
        collection.skipped,
        &collection.trie,
    );

    if estimating_writer.overflowed {
        return Err(IngestError::OutputTooLarge {
            largest_files: largest_files(collection.files.iter()),
        });
    }

    render_res?;
    Ok(estimating_writer.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::IngestError;
    use crate::core::model::{FileRecord, FileStorage};
    use crate::core::policy::MAX_OUTPUT_SIZE;

    fn test_trie<'a>(
        files: impl IntoIterator<Item = FileRecord<'a>>,
        escape_xml: bool,
    ) -> BudgetTrie {
        let mut trie = BudgetTrie::new(escape_xml);
        for file in files {
            trie.insert(file.path);
        }
        trie
    }

    fn render_digest(
        label: &str,
        revision: Option<&str>,
        files: &[FileRecord<'_>],
        skipped: usize,
    ) -> Result<String, IngestError> {
        let storage: FileStorage = files.iter().copied().collect();
        let trie = test_trie(files.iter().copied(), false);
        let collection = CollectionResult {
            label: label.to_string(),
            revision: revision.map(str::to_string),
            files: storage,
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
                path: "z.txt",
                content: "z",
            },
            FileRecord {
                path: "a.txt",
                content: "a",
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
        let files = [
            FileRecord {
                path: "b.txt",
                content: "hello\n",
            },
            FileRecord {
                path: "a.txt",
                content: "world",
            },
        ];
        let collection = CollectionResult {
            label: "my-repo".into(),
            revision: Some("v1.0".into()),
            files: files.into_iter().collect(),
            skipped: 2,
            trie: test_trie(files, false),
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
                path: "b.txt",
                content: "content_b\n",
            },
            FileRecord {
                path: "excluded.txt",
                content: "skip_me\n",
            },
            FileRecord {
                path: "a.txt",
                content: "content_a\n",
            },
        ];
        let trie = test_trie([files[0], files[2]], false);
        let mut buf = Vec::new();
        let filtered_iter = files.into_iter().filter(|f| f.path != "excluded.txt");
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
        let content = "x".repeat(MAX_OUTPUT_SIZE as usize + 1);
        let candidate = FileRecord {
            path: "large.txt",
            content: &content,
        };
        let mut budget = DigestBudget::new(DigestFormat::Text);
        match budget.add(&candidate) {
            Err(IngestError::OutputTooLarge { .. }) => {}
            other => panic!("expected OutputTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn render_digest_to_writer_enforces_output_size_cap() {
        let content = "x".repeat(MAX_OUTPUT_SIZE as usize);
        let candidate = FileRecord {
            path: "large.txt",
            content: &content,
        };
        let mut storage = FileStorage::new();
        storage.push(candidate.path, candidate.content);
        let collection = CollectionResult {
            label: "repo".into(),
            revision: None,
            files: storage,
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
        let files = [
            FileRecord {
                path: "z.txt",
                content: "z < 10",
            },
            FileRecord {
                path: "a.txt",
                content: "a & b",
            },
        ];
        let collection = CollectionResult {
            label: "my-repo".into(),
            revision: Some("v1.0".into()),
            files: files.into_iter().collect(),
            skipped: 1,
            trie: test_trie(files, true),
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
        let files = [FileRecord {
            path: "ansi.txt",
            content: raw_ansi,
        }];

        let text_rendered = render_digest("repo", None, &files, 0)?;
        assert!(
            text_rendered.contains(raw_ansi),
            "Text format must preserve raw ANSI/control characters for LLM fidelity"
        );

        let collection = CollectionResult {
            label: "repo".into(),
            revision: None,
            files: files.into_iter().collect(),
            skipped: 0,
            trie: test_trie(files, true),
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

    #[test]
    fn test_estimate_tokens_counts() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("hello"), 2);
        assert!(estimate_tokens("let mut x = 123;") >= 4);
        assert!(estimate_tokens("fn main() {\n    println!(\"Hello, world!\");\n}\n") >= 8);
        assert!(estimate_tokens("한글 텍스트 테스트") >= 4);
        assert!(estimate_tokens("word1 word2...word3\nword4") >= 5);
    }

    #[test]
    fn test_token_estimator_matches_char_oracle() {
        fn char_oracle(text: &str) -> usize {
            let mut tokens = 0;
            let mut in_word = false;
            let mut word_len = 0;
            let mut saw_input = false;

            for ch in text.chars() {
                saw_input = true;
                if ch.is_ascii_alphanumeric() || ch == '_' {
                    in_word = true;
                    word_len += 1;
                    let full_word = (word_len >= 4) as usize;
                    tokens += full_word;
                    word_len *= 1 - full_word;
                } else {
                    let was_in_word = std::mem::take(&mut in_word) as usize;
                    tokens += was_in_word;
                    word_len = 0;

                    let is_ws = ch.is_ascii_whitespace();
                    let is_nl = (ch == '\n') as usize;
                    let non_ws_weight = (!is_ws) as usize * (1 + (!ch.is_ascii()) as usize);
                    tokens += non_ws_weight + is_nl;
                }
            }

            tokens += in_word as usize;
            let non_empty = (tokens > 0) | saw_input;
            tokens.max(non_empty as usize)
        }

        let samples = [
            "",
            "hello",
            "let mut x = 123;",
            "fn main() {\n    println!(\"Hello, world!\");\n}\n",
            "한",
            "🚀",
            "한글 텍스트 테스트",
            "word1 word2...word3\nword4",
            "Special: 🚀 emoji and \t\r\n symbols! 👍",
            "Cyrillic: Привет мир",
            "Accents: café, naïve, façade",
        ];

        for sample in samples {
            let oracle = char_oracle(sample);
            let estimated = estimate_tokens(sample);
            assert_eq!(
                estimated, oracle,
                "token estimation mismatch for sample {sample:?}"
            );
        }
    }
}
