use std::io::Write;

use crate::core::error::IngestError;
use crate::core::model::{DigestFormat, FileRecord};
use crate::core::policy::MAX_OUTPUT_SIZE;

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

const XML_REPO_START: &str = "<repository name=\"";
const XML_REVISION_ATTR: &str = "\" revision=\"";
const XML_TAG_CLOSE: &str = "\">\n";
const XML_DIR_START: &str = "<directory_structure>\n";
const XML_DIR_END: &str = "</directory_structure>\n";
const XML_FILES_START: &str = "<files>\n";
const XML_FILES_END: &str = "</files>\n";
const XML_REPO_END: &str = "</repository>\n";
const XML_FILE_START: &str = "<file path=\"";
const XML_FILE_END: &str = "</file>\n";

const TREE_BRANCH: &str = "├── ";
const TREE_LAST: &str = "└── ";
const TREE_VERT: &str = "│   ";
const TREE_SPACE: &str = "    ";

#[derive(Debug, Clone, Default)]
struct BudgetTrieNode {
    name: String,
    is_dir: bool,
    children: Vec<BudgetTrieNode>,
}

#[derive(Debug, Clone, Default)]
struct BudgetTrie {
    children: Vec<BudgetTrieNode>,
}

#[derive(Debug, Default)]
struct TrieUndo {
    actions: Vec<Vec<usize>>,
}

impl BudgetTrie {
    fn insert_with_undo(&mut self, path: &str) -> TrieUndo {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut undo = TrieUndo::default();
        if parts.is_empty() {
            return undo;
        }
        let mut current_indices = Vec::new();
        Self::insert_into_with_undo(&mut self.children, &parts, &mut current_indices, &mut undo);
        undo
    }

    fn insert_into_with_undo(
        children: &mut Vec<BudgetTrieNode>,
        parts: &[&str],
        current_indices: &mut Vec<usize>,
        undo: &mut TrieUndo,
    ) {
        if parts.is_empty() {
            return;
        }
        let part = parts[0];
        let is_dir = parts.len() > 1;

        let (idx, created) = match children.binary_search_by(|c| {
            c.name
                .as_str()
                .cmp(part)
                .then_with(|| c.is_dir.cmp(&is_dir))
        }) {
            Ok(idx) => (idx, false),
            Err(idx) => {
                children.insert(
                    idx,
                    BudgetTrieNode {
                        name: part.to_string(),
                        is_dir,
                        children: Vec::new(),
                    },
                );
                (idx, true)
            }
        };

        current_indices.push(idx);

        if created {
            undo.actions.push(current_indices.clone());
        }

        if is_dir {
            Self::insert_into_with_undo(
                &mut children[idx].children,
                &parts[1..],
                current_indices,
                undo,
            );
        }

        current_indices.pop();
    }

    fn rollback(&mut self, undo: TrieUndo) {
        // Rollback in reverse order of creation
        for path_indices in undo.actions.into_iter().rev() {
            Self::remove_node_at(&mut self.children, &path_indices);
        }
    }

    fn remove_node_at(mut nodes: &mut Vec<BudgetTrieNode>, path_indices: &[usize]) {
        if path_indices.is_empty() {
            return;
        }
        for &idx in &path_indices[..path_indices.len() - 1] {
            if idx >= nodes.len() {
                return;
            }
            nodes = &mut nodes[idx].children;
        }
        let last_idx = path_indices[path_indices.len() - 1];
        if last_idx < nodes.len() {
            nodes.remove(last_idx);
        }
    }

    fn calculate_size(&self, escape_xml: bool) -> u64 {
        Self::calculate_nodes_size(&self.children, 0, escape_xml)
    }

    fn calculate_nodes_size(nodes: &[BudgetTrieNode], prefix_len: u64, escape_xml: bool) -> u64 {
        let mut total = 0_u64;
        let count = nodes.len();
        for (idx, node) in nodes.iter().enumerate() {
            let is_last = idx + 1 == count;
            total += prefix_len + 10;
            let name_len = if escape_xml {
                xml_escape(&node.name).len() as u64
            } else {
                node.name.len() as u64
            };
            total += name_len;
            if node.is_dir {
                total += 2;
            } else {
                total += 1;
            }
            if !node.children.is_empty() {
                let child_prefix_len = if is_last {
                    prefix_len + 4
                } else {
                    prefix_len + 6
                };
                total += Self::calculate_nodes_size(&node.children, child_prefix_len, escape_xml);
            }
        }
        total
    }

    fn render_to_writer<W: std::io::Write + ?Sized>(
        &self,
        writer: &mut W,
        escape_xml: bool,
    ) -> std::io::Result<()> {
        Self::render_nodes(writer, &self.children, "", escape_xml)
    }

    fn render_nodes<W: std::io::Write + ?Sized>(
        writer: &mut W,
        nodes: &[BudgetTrieNode],
        prefix: &str,
        escape_xml: bool,
    ) -> std::io::Result<()> {
        let total = nodes.len();
        for (idx, node) in nodes.iter().enumerate() {
            let is_last = idx + 1 == total;
            writer.write_all(prefix.as_bytes())?;
            if is_last {
                writer.write_all(TREE_LAST.as_bytes())?;
            } else {
                writer.write_all(TREE_BRANCH.as_bytes())?;
            }
            if escape_xml {
                writer.write_all(xml_escape(&node.name).as_bytes())?;
            } else {
                writer.write_all(node.name.as_bytes())?;
            }
            if node.is_dir {
                writer.write_all(b"/")?;
            }
            writer.write_all(b"\n")?;
            if !node.children.is_empty() {
                let child_prefix = if is_last {
                    format!("{prefix}{TREE_SPACE}")
                } else {
                    format!("{prefix}{TREE_VERT}")
                };
                Self::render_nodes(writer, &node.children, &child_prefix, escape_xml)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DigestBudget {
    format: DigestFormat,
    label_bytes: u64,
    revision_bytes: Option<u64>,
    file_count: usize,
    skipped: usize,
    path_bytes: u64,
    content_bytes: u64,
    trailing_bytes: u64,
    trie: BudgetTrie,
}

impl DigestBudget {
    pub(crate) fn new(
        label: &str,
        revision: Option<&str>,
        format: DigestFormat,
    ) -> Result<Self, IngestError> {
        let (label_bytes, revision_bytes) = match format {
            DigestFormat::Text => (
                byte_count(&escape_header(label))?,
                revision
                    .map(|value| byte_count(&escape_header(value)))
                    .transpose()?,
            ),
            DigestFormat::Xml => (
                byte_count(&xml_escape(label))?,
                revision
                    .map(|value| byte_count(&xml_escape(value)))
                    .transpose()?,
            ),
        };
        Ok(Self {
            format,
            label_bytes,
            revision_bytes,
            file_count: 0,
            skipped: 0,
            path_bytes: 0,
            content_bytes: 0,
            trailing_bytes: 0,
            trie: BudgetTrie::default(),
        })
    }

    pub(crate) fn add(&mut self, file: &FileRecord) -> Result<(), IngestError> {
        let (path_b, content_b, trailing_b) = self.calculate_file_bytes(file)?;

        let new_file_count = self
            .file_count
            .checked_add(1)
            .ok_or_else(|| IngestError::InvalidMetadata("file count overflow".into()))?;

        let new_path_bytes = self
            .path_bytes
            .checked_add(path_b)
            .ok_or_else(|| IngestError::OutputTooLarge {
                largest_files: Vec::new(),
            })?;
        let new_content_bytes = self
            .content_bytes
            .checked_add(content_b)
            .ok_or_else(|| IngestError::OutputTooLarge {
                largest_files: Vec::new(),
            })?;
        let new_trailing_bytes = self
            .trailing_bytes
            .checked_add(trailing_b)
            .ok_or_else(|| IngestError::OutputTooLarge {
                largest_files: Vec::new(),
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

    fn calculate_file_bytes(&self, file: &FileRecord) -> Result<(u64, u64, u64), IngestError> {
        let (path_bytes, content_bytes, trailing_bytes) = match self.format {
            DigestFormat::Text => {
                let path_b = byte_count(&escape_header(&file.path))?;
                let content_b = byte_count(&file.content)?;
                let trailing_b = (usize::from(!file.content.ends_with('\n')) + 1) as u64;
                (path_b, content_b, trailing_b)
            }
            DigestFormat::Xml => {
                let path_b = byte_count(&xml_escape(&file.path))?;
                let content_escaped = xml_escape(&file.content);
                let content_b = byte_count(&content_escaped)?;
                let trailing_b = usize::from(!content_escaped.ends_with('\n')) as u64;
                (path_b, content_b, trailing_b)
            }
        };
        Ok((path_bytes, content_bytes, trailing_bytes))
    }

    fn ensure_fits(&self) -> Result<(), IngestError> {
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

    fn rendered_size(&self) -> Option<u64> {
        let file_count = u64::try_from(self.file_count).ok()?;
        let tree_bytes = self.trie.calculate_size(self.format == DigestFormat::Xml);
        match self.format {
            DigestFormat::Text => {
                let mut size = 0_u64;
                size = size.checked_add(REPOSITORY_PREFIX.len() as u64)?;
                size = size.checked_add(self.label_bytes)?;
                size = size.checked_add(NEWLINE.len() as u64)?;
                if let Some(revision_bytes) = self.revision_bytes {
                    size = size.checked_add(REVISION_PREFIX.len() as u64)?;
                    size = size.checked_add(revision_bytes)?;
                    size = size.checked_add(NEWLINE.len() as u64)?;
                }
                size = size.checked_add(FILES_PREFIX.len() as u64)?;
                size = size.checked_add(self.file_count.to_string().len() as u64)?;
                size = size.checked_add(SKIPPED_PREFIX.len() as u64)?;
                size = size.checked_add(self.skipped.to_string().len() as u64)?;
                size = size.checked_add(METADATA_SUFFIX.len() as u64)?;
                size = size.checked_add(DIRECTORY_HEADER.len() as u64)?;
                size = size.checked_add(tree_bytes)?;
                size = size.checked_add(FILES_HEADER.len() as u64)?;
                let file_block_bytes = FILE_BLOCK_PREFIX.len() + FILE_BLOCK_SUFFIX.len();
                size = size.checked_add(file_count.checked_mul(file_block_bytes as u64)?)?;
                size = size.checked_add(self.path_bytes)?;
                size = size.checked_add(self.content_bytes)?;
                size.checked_add(self.trailing_bytes)
            }
            DigestFormat::Xml => {
                let mut size = 0_u64;
                size = size.checked_add(XML_REPO_START.len() as u64)?;
                size = size.checked_add(self.label_bytes)?;
                if let Some(revision_bytes) = self.revision_bytes {
                    size = size.checked_add(XML_REVISION_ATTR.len() as u64)?;
                    size = size.checked_add(revision_bytes)?;
                }
                size = size.checked_add(XML_TAG_CLOSE.len() as u64)?;
                size = size.checked_add(XML_DIR_START.len() as u64)?;
                size = size.checked_add(tree_bytes)?;
                size = size.checked_add(XML_DIR_END.len() as u64)?;
                size = size.checked_add(XML_FILES_START.len() as u64)?;
                let per_file_tag_bytes =
                    XML_FILE_START.len() + XML_TAG_CLOSE.len() + XML_FILE_END.len();
                size = size.checked_add(file_count.checked_mul(per_file_tag_bytes as u64)?)?;
                size = size.checked_add(self.path_bytes)?;
                size = size.checked_add(self.content_bytes)?;
                size = size.checked_add(self.trailing_bytes)?;
                size = size.checked_add(XML_FILES_END.len() as u64)?;
                size.checked_add(XML_REPO_END.len() as u64)
            }
        }
    }
}

fn byte_count(value: &str) -> Result<u64, IngestError> {
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
    // Defense-in-depth: Selector accounts for budget incrementally as files are selected;
    // render_digest_to_writer validates final rendered byte bounds before committing the formatted digest.
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
        let mut entries: Vec<(String, usize)> = files
            .iter()
            .map(|f| (f.path.clone(), f.content.len()))
            .collect();
        entries.sort_by_key(|b| std::cmp::Reverse(b.1));
        entries.truncate(5);
        return Err(IngestError::OutputTooLarge {
            largest_files: entries,
        });
    }
    overflow?;

    let mut estimator = TokenEstimatingWriter::new(writer);
    let writer = &mut estimator;

    match format {
        DigestFormat::Text => {
            writer.write_all(REPOSITORY_PREFIX.as_bytes())?;
            writer.write_all(escape_header(label).as_bytes())?;
            writer.write_all(NEWLINE.as_bytes())?;
            if let Some(revision) = revision {
                writer.write_all(REVISION_PREFIX.as_bytes())?;
                writer.write_all(escape_header(revision).as_bytes())?;
                writer.write_all(NEWLINE.as_bytes())?;
            }
            writer.write_all(FILES_PREFIX.as_bytes())?;
            writer.write_all(files.len().to_string().as_bytes())?;
            writer.write_all(SKIPPED_PREFIX.as_bytes())?;
            writer.write_all(skipped.to_string().as_bytes())?;
            writer.write_all(METADATA_SUFFIX.as_bytes())?;

            let mut ordered_files: Vec<_> = files
                .iter()
                .map(|file| (escape_header(&file.path), file))
                .collect();
            ordered_files.sort_by(|left, right| left.1.path.cmp(&right.1.path));

            writer.write_all(DIRECTORY_HEADER.as_bytes())?;
            budget.trie.render_to_writer(writer, false)?;
            writer.write_all(FILES_HEADER.as_bytes())?;
            for (escaped_path, file) in ordered_files {
                writer.write_all(FILE_BLOCK_PREFIX.as_bytes())?;
                writer.write_all(escaped_path.as_bytes())?;
                writer.write_all(FILE_BLOCK_SUFFIX.as_bytes())?;
                writer.write_all(file.content.as_bytes())?;
                if !file.content.ends_with('\n') {
                    writer.write_all(NEWLINE.as_bytes())?;
                }
                writer.write_all(NEWLINE.as_bytes())?;
            }
        }
        DigestFormat::Xml => {
            writer.write_all(XML_REPO_START.as_bytes())?;
            writer.write_all(xml_escape(label).as_bytes())?;
            if let Some(revision) = revision {
                writer.write_all(XML_REVISION_ATTR.as_bytes())?;
                writer.write_all(xml_escape(revision).as_bytes())?;
            }
            writer.write_all(XML_TAG_CLOSE.as_bytes())?;
            writer.write_all(XML_DIR_START.as_bytes())?;
            budget.trie.render_to_writer(writer, true)?;
            writer.write_all(XML_DIR_END.as_bytes())?;
            writer.write_all(XML_FILES_START.as_bytes())?;
            let mut ordered_files: Vec<_> = files.iter().collect();
            ordered_files.sort_by(|left, right| left.path.cmp(&right.path));
            for file in ordered_files {
                writer.write_all(XML_FILE_START.as_bytes())?;
                writer.write_all(xml_escape(&file.path).as_bytes())?;
                writer.write_all(XML_TAG_CLOSE.as_bytes())?;
                let escaped_content = xml_escape(&file.content);
                writer.write_all(escaped_content.as_bytes())?;
                if !escaped_content.ends_with('\n') {
                    writer.write_all(NEWLINE.as_bytes())?;
                }
                writer.write_all(XML_FILE_END.as_bytes())?;
            }
            writer.write_all(XML_FILES_END.as_bytes())?;
            writer.write_all(XML_REPO_END.as_bytes())?;
        }
    }
    Ok(estimator.finish())
}

struct TokenEstimatingWriter<'a, W: std::io::Write + ?Sized> {
    inner: &'a mut W,
    tokens: usize,
    in_word: bool,
    word_len: usize,
    pending: Vec<u8>,
}

impl<'a, W: std::io::Write + ?Sized> TokenEstimatingWriter<'a, W> {
    fn new(inner: &'a mut W) -> Self {
        Self {
            inner,
            tokens: 0,
            in_word: false,
            word_len: 0,
            pending: Vec::new(),
        }
    }

    fn finish(mut self) -> usize {
        if self.tokens == 0 && !self.in_word {
            return 0;
        }
        if self.in_word {
            self.tokens += 1;
        }
        self.tokens.max(1)
    }
}

impl<W: std::io::Write + ?Sized> std::io::Write for TokenEstimatingWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write_all(buf)?;

        let slice = if self.pending.is_empty() {
            buf
        } else {
            self.pending.extend_from_slice(buf);
            &self.pending[..]
        };

        let (valid_str, remaining) = match std::str::from_utf8(slice) {
            Ok(s) => (s, &[][..]),
            Err(e) => {
                let valid_up_to = e.valid_up_to();
                let valid_str = std::str::from_utf8(&slice[..valid_up_to]).unwrap_or("");
                (valid_str, &slice[valid_up_to..])
            }
        };

        for ch in valid_str.chars() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                self.in_word = true;
                self.word_len += 1;
                if self.word_len >= 4 {
                    self.tokens += 1;
                    self.word_len = 0;
                }
            } else {
                if self.in_word {
                    self.tokens += 1;
                    self.in_word = false;
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

        if !self.pending.is_empty() {
            let rem_vec = remaining.to_vec();
            self.pending = rem_vec;
        } else if !remaining.is_empty() {
            self.pending = remaining.to_vec();
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn escape_header(value: &str) -> std::borrow::Cow<'_, str> {
    if !value.contains('\\') && !value.contains('\r') && !value.contains('\n') {
        return std::borrow::Cow::Borrowed(value);
    }
    std::borrow::Cow::Owned(
        value
            .replace('\\', "\\\\")
            .replace('\r', "\\r")
            .replace('\n', "\\n"),
    )
}

fn xml_escape(value: &str) -> std::borrow::Cow<'_, str> {
    if !value.contains('&')
        && !value.contains('<')
        && !value.contains('>')
        && !value.contains('"')
        && !value.contains('\'')
    {
        return std::borrow::Cow::Borrowed(value);
    }
    std::borrow::Cow::Owned(
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;"),
    )
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

    fn estimate_tokens(text: &str) -> usize {
        let mut sink = std::io::sink();
        let mut estimator = TokenEstimatingWriter::new(&mut sink);
        let _ = std::io::Write::write_all(&mut estimator, text.as_bytes());
        estimator.finish()
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
    fn escape_header_borrows_clean_input() {
        use std::borrow::Cow;
        assert!(matches!(
            escape_header("clean/path/file.rs"),
            Cow::Borrowed(_)
        ));
        assert!(matches!(
            escape_header("path\\with\\backslash"),
            Cow::Owned(_)
        ));
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
    fn escapes_xml_entities() {
        assert_eq!(xml_escape("plain text"), "plain text");
        assert_eq!(xml_escape("a & b < c > d"), "a &amp; b &lt; c &gt; d");
        assert_eq!(xml_escape("let x = a && b;"), "let x = a &amp;&amp; b;");
        assert_eq!(
            xml_escape("<tag attr=\"val\">'hello'</tag>"),
            "&lt;tag attr=&quot;val&quot;&gt;&apos;hello&apos;&lt;/tag&gt;"
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
    fn estimates_token_counts() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("hello"), 2);
        assert!(estimate_tokens("let mut x = 123;") >= 4);
        assert!(estimate_tokens("fn main() {\n    println!(\"Hello, world!\");\n}\n") >= 8);
        assert!(estimate_tokens("한글 텍스트 테스트") >= 4);
    }

    #[test]
    fn renders_ascii_tree_correctly() -> Result<(), Box<dyn std::error::Error>> {
        let mut trie = BudgetTrie::default();
        for path in ["Cargo.toml", "src/core/a.rs", "src/lib.rs"] {
            trie.insert_with_undo(path);
        }
        let mut buf = Vec::new();
        trie.render_to_writer(&mut buf, false)?;
        let tree_str = std::str::from_utf8(&buf)?;
        let expected = "\
├── Cargo.toml
└── src/
    ├── core/
    │   └── a.rs
    └── lib.rs
";
        assert_eq!(tree_str, expected);
        Ok(())
    }

    #[test]
    fn trie_budget_matches_rendered_size() -> Result<(), Box<dyn std::error::Error>> {
        let test_cases = vec![
            vec![
                "Cargo.toml",
                "src/lib.rs",
                "src/core/digest.rs",
                "src/core/model.rs",
            ],
            vec!["a.txt", "b/c.txt", "b/d/e.txt", "f/g/h/i.txt"],
            vec![
                "file & with <special> \"chars'.txt",
                "nested & dir/sub <dir>/test.rs",
            ],
        ];

        for paths in test_cases {
            for escape_xml in [false, true] {
                let mut trie = BudgetTrie::default();
                for p in &paths {
                    trie.insert_with_undo(p);
                }

                let trie_size = trie.calculate_size(escape_xml);
                let mut rendered = Vec::new();
                trie.render_to_writer(&mut rendered, escape_xml)?;
                assert_eq!(
                    trie_size,
                    rendered.len() as u64,
                    "Trie size must match rendered tree bytes exactly (escape_xml: {escape_xml})"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn trie_rollback_restores_exact_state() {
        let mut trie = BudgetTrie::default();
        trie.insert_with_undo("src/core/digest.rs");
        let initial_size = trie.calculate_size(false);

        let undo = trie.insert_with_undo("src/core/sub/nested/file.rs");
        let modified_size = trie.calculate_size(false);
        assert!(modified_size > initial_size);

        trie.rollback(undo);
        assert_eq!(trie.calculate_size(false), initial_size);

        // Also test budget rollback
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
