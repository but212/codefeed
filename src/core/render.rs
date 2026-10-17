//! Digest rendering and streaming token estimation.
//!
//! Formats collected repository contents into LLM-ready text or XML digests,
//! escaping headers and XML entities on the fly and estimating token counts
//! without extra allocations.

use std::borrow::Cow;
use std::io::Write;

use crate::core::error::IngestError;
use crate::core::model::{CollectionResult, DigestFormat, FileRecord};
use crate::core::policy::MAX_OUTPUT_SIZE;
use crate::core::tree::BudgetTrie;

// ---------------------------------------------------------------------------
// Text format specification and escaping
// ---------------------------------------------------------------------------

const REPOSITORY_PREFIX: &str = "Repository: ";
const REVISION_PREFIX: &str = "Revision: ";
const FILES_PREFIX: &str = "Files: ";
const SKIPPED_PREFIX: &str = "\nSkipped: ";
const DIRECTORY_HEADER: &str = "Directory structure:\n";
const FILES_HEADER: &str = "\nFiles:\n\n";
const FILE_BLOCK_PREFIX: &str = "==================================================\nFILE: ";
const FILE_BLOCK_SUFFIX: &str = "\n==================================================\n\n";
const NEWLINE: &str = "\n";
const METADATA_SUFFIX: &str = "\n\n";

const HEADER_ESCAPE_MASK: [u64; 4] = {
    let mut mask = [0u64; 4];
    mask[b'\\' as usize / 64] |= 1 << (b'\\' as usize % 64);
    mask[b'\r' as usize / 64] |= 1 << (b'\r' as usize % 64);
    mask[b'\n' as usize / 64] |= 1 << (b'\n' as usize % 64);
    mask
};

#[inline(always)]
const fn needs_header_escape(b: u8) -> bool {
    (HEADER_ESCAPE_MASK[(b >> 6) as usize] & (1u64 << (b & 63))) != 0
}

#[inline(always)]
const fn header_escape_str(b: u8) -> &'static str {
    match b {
        b'\\' => "\\\\",
        b'\r' => "\\r",
        b'\n' => "\\n",
        _ => "",
    }
}

pub fn escape_header(value: &str) -> Cow<'_, str> {
    let bytes = value.as_bytes();
    let Some(first) = bytes.iter().position(|&b| needs_header_escape(b)) else {
        return Cow::Borrowed(value);
    };

    let mut out = String::with_capacity(value.len() + 8);
    out.push_str(&value[..first]);
    let mut last = first;
    for (i, &b) in bytes[first..].iter().enumerate() {
        let abs_i = first + i;
        if needs_header_escape(b) {
            out.push_str(&value[last..abs_i]);
            out.push_str(header_escape_str(b));
            last = abs_i + 1;
        }
    }
    out.push_str(&value[last..]);
    Cow::Owned(out)
}

/// Left-inverse of `escape_header` satisfying `unescape_header(escape_header(s)) == s`.
pub fn unescape_header(value: &str) -> Cow<'_, str> {
    let bytes = value.as_bytes();
    let Some(first) = bytes.iter().position(|&b| b == b'\\') else {
        return Cow::Borrowed(value);
    };

    let mut out = String::with_capacity(value.len());
    out.push_str(&value[..first]);

    let mut i = first;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'\\' => {
                    out.push('\\');
                    i += 2;
                }
                b'r' => {
                    out.push('\r');
                    i += 2;
                }
                b'n' => {
                    out.push('\n');
                    i += 2;
                }
                _ => {
                    out.push('\\');
                    i += 1;
                }
            }
        } else {
            let start = i;
            while i < bytes.len() && bytes[i] != b'\\' {
                i += 1;
            }
            out.push_str(&value[start..i]);
        }
    }

    Cow::Owned(out)
}

fn render_text<'a, W, I>(
    writer: &mut W,
    label: &str,
    revision: Option<&str>,
    ordered_files: I,
    skipped: usize,
    trie: &BudgetTrie,
) -> Result<(), IngestError>
where
    W: Write + ?Sized,
    I: IntoIterator<Item = FileRecord<'a>>,
    I::IntoIter: ExactSizeIterator,
{
    writer.write_all(REPOSITORY_PREFIX.as_bytes())?;
    writer.write_all(escape_header(label).as_bytes())?;
    writer.write_all(NEWLINE.as_bytes())?;
    if let Some(revision) = revision {
        writer.write_all(REVISION_PREFIX.as_bytes())?;
        writer.write_all(escape_header(revision).as_bytes())?;
        writer.write_all(NEWLINE.as_bytes())?;
    }
    let iter = ordered_files.into_iter();
    writer.write_all(FILES_PREFIX.as_bytes())?;
    write!(writer, "{}", iter.len())?;
    writer.write_all(SKIPPED_PREFIX.as_bytes())?;
    write!(writer, "{}", skipped)?;
    writer.write_all(METADATA_SUFFIX.as_bytes())?;
    writer.write_all(DIRECTORY_HEADER.as_bytes())?;
    trie.render_to_writer(writer, escape_header)?;
    writer.write_all(FILES_HEADER.as_bytes())?;
    for file in iter {
        const TAIL: [&[u8]; 2] = [b"\n", b"\n\n"];
        let needs_extra_nl = (!file.content.ends_with('\n')) as usize;
        writer.write_all(FILE_BLOCK_PREFIX.as_bytes())?;
        writer.write_all(escape_header(file.path).as_bytes())?;
        writer.write_all(FILE_BLOCK_SUFFIX.as_bytes())?;
        writer.write_all(file.content.as_bytes())?;
        writer.write_all(TAIL[needs_extra_nl])?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// XML format specification and escaping
// ---------------------------------------------------------------------------

const XML_REPO_START: &str = "<repository name=\"";
const XML_REVISION_ATTR: &str = "\" revision=\"";
const XML_TAG_CLOSE: &str = "\">\n";
const XML_DIR_START: &str = "<directory_structure>\n";
const XML_DIR_END: &str = "</directory_structure>\n";
const XML_FILES_START: &str = "<files>\n";
const XML_FILES_END: &str = "</files>\n";
const XML_REPO_END: &str = "</repository>\n";
const XML_FILE_START: &str = "<file path=\"";
const FILE_TAILS: [&[u8]; 2] = [b"</file>\n", b"\n</file>\n"];
const SPECIAL_BITSET: [u64; 4] = {
    let mut mask = [0u64; 4];
    let mut i = 0;
    while i <= 8 {
        mask[0] |= 1 << i;
        i += 1;
    }
    mask[0] |= (1 << 0x0B) | (1 << 0x0C);
    let mut j = 0x0E;
    while j <= 0x1F {
        mask[0] |= 1 << j;
        j += 1;
    }
    mask[b'&' as usize / 64] |= 1 << (b'&' as usize % 64);
    mask[b'<' as usize / 64] |= 1 << (b'<' as usize % 64);
    mask[b'>' as usize / 64] |= 1 << (b'>' as usize % 64);
    mask[b'"' as usize / 64] |= 1 << (b'"' as usize % 64);
    mask[b'\'' as usize / 64] |= 1 << (b'\'' as usize % 64);
    mask[0xEF / 64] |= 1 << (0xEF % 64);
    mask
};

#[inline(always)]
const fn is_special_byte(b: u8) -> bool {
    let word = SPECIAL_BITSET[(b >> 6) as usize];
    (word & (1u64 << (b & 63))) != 0
}

const XML_ESCAPES: [&str; 6] = ["\u{FFFD}", "&quot;", "&amp;", "&apos;", "&lt;", "&gt;"];
const ASCII_ESC_IDX: [u8; 64] = {
    let mut t = [0u8; 64];
    t[b'"' as usize] = 1;
    t[b'&' as usize] = 2;
    t[b'\'' as usize] = 3;
    t[b'<' as usize] = 4;
    t[b'>' as usize] = 5;
    t
};

#[inline(always)]
fn xml_special(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    let b = bytes[i];
    if b < 64 {
        return Some((1, XML_ESCAPES[ASCII_ESC_IDX[b as usize] as usize]));
    }
    if b == 0xEF && i + 2 < bytes.len() {
        let tail = ((bytes[i + 1] as u32) << 8) | (bytes[i + 2] as u32);
        return ((tail & !1) == 0xBFBE).then_some((3, "\u{FFFD}"));
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XmlChunk<'a> {
    Raw(&'a str),
    Escaped(&'static str),
}

impl<'a> XmlChunk<'a> {
    #[inline]
    pub const fn as_str(&self) -> &'a str {
        match *self {
            XmlChunk::Raw(s) => s,
            XmlChunk::Escaped(s) => s,
        }
    }

    #[inline]
    pub const fn as_bytes(&self) -> &'a [u8] {
        self.as_str().as_bytes()
    }
}

pub struct XmlChunks<'a> {
    value: &'a str,
    bytes: &'a [u8],
    cursor: usize,
    pending: Option<&'static str>,
}

impl<'a> XmlChunks<'a> {
    #[inline]
    pub const fn from_offset(value: &'a str, offset: usize) -> Self {
        Self {
            value,
            bytes: value.as_bytes(),
            cursor: offset,
            pending: None,
        }
    }
}

impl<'a> Iterator for XmlChunks<'a> {
    type Item = XmlChunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(esc) = self.pending.take() {
            return Some(XmlChunk::Escaped(esc));
        }

        if self.cursor >= self.bytes.len() {
            return None;
        }

        let start = self.cursor;
        while self.cursor < self.bytes.len() {
            if is_special_byte(self.bytes[self.cursor])
                && let Some((consumed, esc)) = xml_special(self.bytes, self.cursor)
            {
                if self.cursor > start {
                    let raw = &self.value[start..self.cursor];
                    self.cursor += consumed;
                    self.pending = Some(esc);
                    return Some(XmlChunk::Raw(raw));
                }
                self.cursor += consumed;
                return Some(XmlChunk::Escaped(esc));
            }
            self.cursor += 1;
        }

        if self.cursor > start {
            Some(XmlChunk::Raw(&self.value[start..]))
        } else {
            None
        }
    }
}

#[inline(always)]
pub(crate) fn first_special_index(bytes: &[u8]) -> Option<usize> {
    bytes
        .iter()
        .enumerate()
        .position(|(i, &b)| is_special_byte(b) && (b != 0xEF || xml_special(bytes, i).is_some()))
}

pub fn xml_escape(value: &str) -> Cow<'_, str> {
    let bytes = value.as_bytes();
    let Some(first) = first_special_index(bytes) else {
        return Cow::Borrowed(value);
    };

    let mut result = String::with_capacity(value.len() + 16);
    result.push_str(&value[..first]);
    for chunk in XmlChunks::from_offset(value, first) {
        result.push_str(chunk.as_str());
    }
    Cow::Owned(result)
}

pub fn write_escaped<W: Write + ?Sized>(writer: &mut W, value: &str) -> std::io::Result<()> {
    let bytes = value.as_bytes();
    let Some(first) = first_special_index(bytes) else {
        return writer.write_all(bytes);
    };

    if first > 0 {
        writer.write_all(&bytes[..first])?;
    }
    for chunk in XmlChunks::from_offset(value, first) {
        writer.write_all(chunk.as_bytes())?;
    }
    Ok(())
}

fn render_xml<'a, W, I>(
    writer: &mut W,
    label: &str,
    revision: Option<&str>,
    ordered_files: I,
    trie: &BudgetTrie,
) -> Result<(), IngestError>
where
    W: Write + ?Sized,
    I: IntoIterator<Item = FileRecord<'a>>,
{
    writer.write_all(XML_REPO_START.as_bytes())?;
    writer.write_all(xml_escape(label).as_bytes())?;
    if let Some(revision) = revision {
        writer.write_all(XML_REVISION_ATTR.as_bytes())?;
        writer.write_all(xml_escape(revision).as_bytes())?;
    }
    writer.write_all(XML_TAG_CLOSE.as_bytes())?;
    writer.write_all(XML_DIR_START.as_bytes())?;
    trie.render_to_writer(writer, |name| {
        let escaped = escape_header(name);
        match escaped {
            Cow::Borrowed(b) => xml_escape(b),
            Cow::Owned(o) => match xml_escape(&o) {
                Cow::Borrowed(_) => Cow::Owned(o),
                Cow::Owned(xml_str) => Cow::Owned(xml_str),
            },
        }
    })?;
    writer.write_all(XML_DIR_END.as_bytes())?;
    writer.write_all(XML_FILES_START.as_bytes())?;
    for file in ordered_files {
        writer.write_all(XML_FILE_START.as_bytes())?;
        writer.write_all(xml_escape(file.path).as_bytes())?;
        writer.write_all(XML_TAG_CLOSE.as_bytes())?;
        write_escaped(writer, file.content)?;
        let needs_nl = (!file.content.ends_with('\n')) as usize;
        writer.write_all(FILE_TAILS[needs_nl])?;
    }
    writer.write_all(XML_FILES_END.as_bytes())?;
    writer.write_all(XML_REPO_END.as_bytes())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Heuristic token estimation
// ---------------------------------------------------------------------------

const METRIC_WORD: u8 = 1 << 0; // [0-9a-zA-Z_]

const CHAR_METRIC: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut b = 0usize;
    while b < 256 {
        let is_num = b >= b'0' as usize && b <= b'9' as usize;
        let is_upper = b >= b'A' as usize && b <= b'Z' as usize;
        let is_lower = b >= b'a' as usize && b <= b'z' as usize;
        let is_under = b == b'_' as usize;
        let is_word = is_num || is_upper || is_lower || is_under;

        if is_word {
            t[b] = METRIC_WORD;
        } else {
            let is_ws =
                b == b' ' as usize || b == b'\t' as usize || b == b'\r' as usize || b == 0x0C;
            let is_nl = b == b'\n' as usize;
            let lambda = if (b as i8) < -64 {
                0
            } else if (b as i8) >= 0 {
                1
            } else {
                2
            };
            let non_ws_weight = (!is_ws && !is_nl) as u8 * lambda;
            let w_b = non_ws_weight + (is_nl as u8);
            t[b] = w_b << 1;
        }
        b += 1;
    }
    t
};

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
        let metric = CHAR_METRIC[b as usize] as usize;
        let omega = metric & (METRIC_WORD as usize);
        let w_b = metric >> 1;

        let prev_in_word = self.in_word as usize;
        self.in_word = omega != 0;

        let next_word_len = (self.word_len + 1) * omega;
        let delta_word = next_word_len / 4;
        self.word_len = next_word_len % 4;

        self.tokens += delta_word + (1 - omega) * prev_in_word + w_b;
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

    pub fn finish(&self) -> usize {
        let tokens = self.tokens + self.in_word as usize;
        let non_empty = (tokens > 0) | self.saw_input;
        tokens.max(non_empty as usize)
    }
}

pub fn estimate_tokens(text: &str) -> usize {
    let mut estimator = TokenEstimator::new();
    estimator.process_str(text);
    estimator.finish()
}

// ---------------------------------------------------------------------------
// Top-level render orchestration & bounded writer
// ---------------------------------------------------------------------------

impl DigestFormat {
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
        W: Write + ?Sized,
        I: IntoIterator<Item = FileRecord<'a>>,
        I::IntoIter: ExactSizeIterator,
    {
        match self {
            Self::Text => render_text(writer, label, revision, files, skipped, trie),
            Self::Xml => render_xml(writer, label, revision, files, trie),
        }
    }
}

const ESTIMATING_WRITER_BUF_SIZE: usize = 8 * 1024;

struct BoundedEstimatingWriter<'a, W: Write + ?Sized> {
    inner: &'a mut W,
    written: u64,
    estimator: TokenEstimator,
    overflowed: bool,
    buf: [u8; ESTIMATING_WRITER_BUF_SIZE],
    buf_len: usize,
}

impl<'a, W: Write + ?Sized> BoundedEstimatingWriter<'a, W> {
    fn new(inner: &'a mut W) -> Self {
        Self {
            inner,
            written: 0,
            estimator: TokenEstimator::new(),
            overflowed: false,
            buf: [0u8; ESTIMATING_WRITER_BUF_SIZE],
            buf_len: 0,
        }
    }

    fn flush_buf(&mut self) -> std::io::Result<()> {
        if self.buf_len > 0 {
            self.inner.write_all(&self.buf[..self.buf_len])?;
            self.estimator.process_bytes(&self.buf[..self.buf_len]);
            self.buf_len = 0;
        }
        Ok(())
    }

    fn finish(mut self) -> std::io::Result<usize> {
        self.flush_buf()?;
        Ok(self.estimator.finish())
    }
}

impl<W: Write + ?Sized> Write for BoundedEstimatingWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let new_written = self.written.saturating_add(buf.len() as u64);
        if new_written > MAX_OUTPUT_SIZE {
            self.overflowed = true;
            return Err(std::io::Error::other("output size limit exceeded"));
        }
        self.written = new_written;

        let space = self.buf.len() - self.buf_len;
        if buf.len() <= space {
            self.buf[self.buf_len..self.buf_len + buf.len()].copy_from_slice(buf);
            self.buf_len += buf.len();
            return Ok(buf.len());
        }

        self.flush_buf()?;

        if buf.len() >= self.buf.len() {
            self.inner.write_all(buf)?;
            self.estimator.process_bytes(buf);
        } else {
            self.buf[..buf.len()].copy_from_slice(buf);
            self.buf_len = buf.len();
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.flush_buf()?;
        self.inner.flush()
    }
}

pub fn render_to_writer<W: Write + ?Sized>(
    writer: &mut W,
    collection: &CollectionResult,
    format: DigestFormat,
) -> Result<usize, IngestError> {
    let mut estimating_writer = BoundedEstimatingWriter::new(writer);
    let render_res = format.render(
        &mut estimating_writer,
        &collection.label,
        collection.revision.as_deref(),
        collection.files.iter(),
        collection.skipped,
        &collection.trie,
    );

    if estimating_writer.overflowed {
        return Err(IngestError::OutputTooLarge {
            largest_files: crate::core::decide::largest_files(collection.files.iter()),
        });
    }

    render_res?;
    estimating_writer.finish().map_err(Into::into)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::core::error::IngestError;
    use crate::core::model::{FileRecord, FileStorage};
    use crate::core::policy::MAX_OUTPUT_SIZE;
    use crate::core::tree::BudgetTrie;

    fn test_trie<'a>(files: impl IntoIterator<Item = FileRecord<'a>>) -> BudgetTrie {
        let mut trie = BudgetTrie::new();
        for file in files {
            trie.insert(file.path).unwrap();
        }
        trie
    }

    fn render_digest(
        label: &str,
        revision: Option<&str>,
        files: &[FileRecord<'_>],
        skipped: usize,
    ) -> Result<String, IngestError> {
        let mut storage: FileStorage = files.iter().copied().collect();
        storage.sort_by_path();
        let trie = test_trie(storage.iter());
        let collection = CollectionResult {
            label: label.to_string(),
            revision: revision.map(str::to_string),
            files: storage,
            skipped,
            trie,
        };
        let mut buffer = Vec::new();
        render_to_writer(&mut buffer, &collection, DigestFormat::Text)?;
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
        let mut storage: FileStorage = files.into_iter().collect();
        storage.sort_by_path();
        let collection = CollectionResult {
            label: "my-repo".into(),
            revision: Some("v1.0".into()),
            files: storage,
            skipped: 2,
            trie: test_trie(files),
        };
        let mut streamed = Vec::new();
        render_to_writer(&mut streamed, &collection, DigestFormat::Text)?;
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
        let trie = test_trie([files[0], files[2]]);
        let mut buf = Vec::new();
        let filtered: Vec<_> = files
            .into_iter()
            .filter(|f| f.path != "excluded.txt")
            .collect();
        DigestFormat::Text.render(&mut buf, "repo", None, filtered, 1, &trie)?;
        let rendered = std::str::from_utf8(&buf)?;
        assert!(rendered.contains("Files: 2"));
        assert!(rendered.contains("FILE: a.txt"));
        assert!(rendered.contains("FILE: b.txt"));
        assert!(!rendered.contains("excluded.txt"));
        Ok(())
    }

    #[test]
    fn render_to_writer_enforces_output_size_cap() {
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
            trie: BudgetTrie::new(),
        };
        let mut sink = Vec::new();
        match render_to_writer(&mut sink, &collection, DigestFormat::Text) {
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
        let mut storage: FileStorage = files.into_iter().collect();
        storage.sort_by_path();
        let collection = CollectionResult {
            label: "my-repo".into(),
            revision: Some("v1.0".into()),
            files: storage,
            skipped: 1,
            trie: test_trie(files),
        };
        let mut out = Vec::new();
        render_to_writer(&mut out, &collection, DigestFormat::Xml)?;
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
            trie: test_trie(files),
        };
        let mut xml_out = Vec::new();
        render_to_writer(&mut xml_out, &collection, DigestFormat::Xml)?;
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
    fn bounded_estimating_writer_finish_does_not_flush_inner_stream() {
        struct BrokenPipeOnFlushWriter {
            buf: Vec<u8>,
        }

        impl Write for BrokenPipeOnFlushWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.buf.extend_from_slice(buf);
                Ok(buf.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "closed pipe",
                ))
            }
        }

        let mut inner = BrokenPipeOnFlushWriter { buf: Vec::new() };
        let mut writer = BoundedEstimatingWriter::new(&mut inner);
        writer.write_all(b"hello world\n").unwrap();
        let tokens = writer
            .finish()
            .expect("finish must succeed even if inner flush fails");
        assert!(tokens > 0);
        assert_eq!(inner.buf, b"hello world\n");
    }

    #[test]
    fn escape_header_borrows_clean_input() {
        assert!(matches!(escape_header("clean_path"), Cow::Borrowed(_)));
        assert!(matches!(
            escape_header("path\nwith\nnewline"),
            Cow::Owned(_)
        ));
    }

    #[test]
    fn escape_header_replaces_all_target_characters_in_single_string() {
        let input = "a\\b\rc\nd\\\\e\r\n";
        let expected = "a\\\\b\\rc\\nd\\\\\\\\e\\r\\n";
        assert_eq!(escape_header(input).as_ref(), expected);
    }

    #[test]
    fn unescape_header_roundtrip_is_left_inverse() {
        let samples = [
            "",
            "clean_path/src/main.rs",
            "path\\with\\backslashes",
            "path\nwith\nnewlines",
            "path\r\nwith\r\ncrlf",
            "complex\\path\nwith\rall\\kinds\r\nof\\\\escapes",
            "lone\\trailing\\",
        ];
        for sample in samples {
            let escaped = escape_header(sample);
            let unescaped = unescape_header(&escaped);
            assert_eq!(
                unescaped.as_ref(),
                sample,
                "roundtrip failed for sample {sample:?}"
            );
        }
    }

    #[test]
    fn xml_escape_and_streaming_consistency() {
        let cases: &[(&str, &str)] = &[
            ("", ""),
            ("plain text", "plain text"),
            ("plain text\n", "plain text\n"),
            ("a & b < c > d", "a &amp; b &lt; c &gt; d"),
            ("let x = a && b;", "let x = a &amp;&amp; b;"),
            (
                "<tag attr=\"val\">'hello'</tag>",
                "&lt;tag attr=&quot;val&quot;&gt;&apos;hello&apos;&lt;/tag&gt;",
            ),
            ("hello\x0Cworld", "hello\u{FFFD}world"),
            ("valid\t\n\rwhitespace", "valid\t\n\rwhitespace"),
            (
                "code\u{0000}\u{0001}\u{0008}\u{000B}\u{001F}",
                "code\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}\u{FFFD}",
            ),
            (
                "한글과 이모지 🦀 & 특수문자\n",
                "한글과 이모지 🦀 &amp; 특수문자\n",
            ),
            ("끝이 개행이 아닌 &amp", "끝이 개행이 아닌 &amp;amp"),
        ];

        for &(text, expected_escaped) in cases {
            let escaped = xml_escape(text);
            assert_eq!(
                escaped.as_ref(),
                expected_escaped,
                "escape mismatch for {text:?}"
            );

            let mut streamed = Vec::new();
            write_escaped(&mut streamed, text).unwrap();
            assert_eq!(
                String::from_utf8(streamed).unwrap(),
                escaped.as_ref(),
                "streamed content mismatch for {text:?}"
            );
        }
    }

    #[test]
    fn xml_chunks_iterator_yields_expected_sequence() {
        let input = "hello <world> & 'test'";
        let chunks: Vec<_> = XmlChunks::from_offset(input, 0).collect();
        assert_eq!(
            chunks,
            vec![
                XmlChunk::Raw("hello "),
                XmlChunk::Escaped("&lt;"),
                XmlChunk::Raw("world"),
                XmlChunk::Escaped("&gt;"),
                XmlChunk::Raw(" "),
                XmlChunk::Escaped("&amp;"),
                XmlChunk::Raw(" "),
                XmlChunk::Escaped("&apos;"),
                XmlChunk::Raw("test"),
                XmlChunk::Escaped("&apos;"),
            ]
        );
    }

    #[test]
    fn test_xml_chunks_handles_non_escaping_ef_lead_bytes() {
        let input = "prefix \u{FFFD} & <\u{FEFF}> middle \u{FF01} suffix";
        let chunks: Vec<_> = XmlChunks::from_offset(input, 0).collect();
        assert_eq!(
            chunks,
            vec![
                XmlChunk::Raw("prefix \u{FFFD} "),
                XmlChunk::Escaped("&amp;"),
                XmlChunk::Raw(" "),
                XmlChunk::Escaped("&lt;"),
                XmlChunk::Raw("\u{FEFF}"),
                XmlChunk::Escaped("&gt;"),
                XmlChunk::Raw(" middle \u{FF01} suffix"),
            ]
        );
        let escaped = xml_escape(input);
        assert_eq!(
            escaped.as_ref(),
            "prefix \u{FFFD} &amp; &lt;\u{FEFF}&gt; middle \u{FF01} suffix"
        );
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
    fn test_finish_is_idempotent() {
        let mut estimator = TokenEstimator::new();
        estimator.process_str("hello world\nfn main() { let x = 42; }");
        let first = estimator.finish();
        let second = estimator.finish();
        let third = estimator.finish();
        assert_eq!(first, second);
        assert_eq!(second, third);
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
