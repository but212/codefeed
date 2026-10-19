use super::budget::{BoundedBuffer, OutputLimitExceeded};
use super::escape::{escape_header, write_escaped};
use super::tree::render_tree;
use crate::core::model::CollectionResult;

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

pub(super) fn render_text(
    mut buf: BoundedBuffer,
    collection: &CollectionResult,
) -> Result<BoundedBuffer, OutputLimitExceeded> {
    buf = buf.push_bytes(REPOSITORY_PREFIX.as_bytes())?;
    buf = write_escaped(buf, &collection.label, escape_header)?;
    buf = buf.push_bytes(NEWLINE.as_bytes())?;
    if let Some(ref revision) = collection.revision {
        buf = buf.push_bytes(REVISION_PREFIX.as_bytes())?;
        buf = write_escaped(buf, revision, escape_header)?;
        buf = buf.push_bytes(NEWLINE.as_bytes())?;
    }
    buf = buf.push_bytes(FILES_PREFIX.as_bytes())?;
    buf = buf.push_usize(collection.files.len())?;
    buf = buf.push_bytes(SKIPPED_PREFIX.as_bytes())?;
    buf = buf.push_usize(collection.skipped)?;
    buf = buf.push_bytes(METADATA_SUFFIX.as_bytes())?;
    buf = buf.push_bytes(DIRECTORY_HEADER.as_bytes())?;
    buf = render_tree(buf, &collection.files, |b, name| {
        write_escaped(b, name, escape_header)
    })?;
    buf = buf.push_bytes(FILES_HEADER.as_bytes())?;
    for file in collection.files.iter() {
        const TAIL: [&[u8]; 2] = [b"\n", b"\n\n"];
        let needs_extra_nl = (!file.content.ends_with('\n')) as usize;
        buf = buf.push_bytes(FILE_BLOCK_PREFIX.as_bytes())?;
        buf = write_escaped(buf, file.path, escape_header)?;
        buf = buf.push_bytes(FILE_BLOCK_SUFFIX.as_bytes())?;
        buf = buf.push_bytes(file.content.as_bytes())?;
        buf = buf.push_bytes(TAIL[needs_extra_nl])?;
    }

    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::storage::{FileRecord, FileStorage, SortedFiles};

    fn render_text_digest(
        label: &str,
        revision: Option<&str>,
        files: &[FileRecord<'_>],
        skipped: usize,
    ) -> Result<String, OutputLimitExceeded> {
        let storage: FileStorage = files.iter().copied().collect();
        let collection = CollectionResult {
            label: label.to_string(),
            revision: revision.map(str::to_string),
            files: SortedFiles::new(storage),
            skipped,
        };
        let buf = render_text(BoundedBuffer::default(), &collection)?;
        Ok(String::from_utf8(buf.finish().bytes).unwrap())
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
        let digest = render_text_digest("repo", Some("abc"), &files, 1)?;
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
        let rendered = render_text_digest("repo", Some(revision), &[], 0)?;

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
        let storage: FileStorage = files.into_iter().collect();
        let collection = CollectionResult {
            label: "my-repo".into(),
            revision: Some("v1.0".into()),
            files: SortedFiles::new(storage),
            skipped: 2,
        };
        let buf = render_text(BoundedBuffer::default(), &collection)?;
        let digest = buf.finish();
        let streamed_str = std::str::from_utf8(&digest.bytes)?;

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
        let storage = files
            .into_iter()
            .filter(|f| f.path != "excluded.txt")
            .fold(FileStorage::new(), |s, f| s.push(f.path, f.content));
        let collection = CollectionResult {
            label: "repo".into(),
            revision: None,
            files: SortedFiles::new(storage),
            skipped: 1,
        };
        let buf = render_text(BoundedBuffer::default(), &collection)?;
        let digest = buf.finish();
        let rendered = std::str::from_utf8(&digest.bytes)?;
        assert!(rendered.contains("Files: 2"));
        assert!(rendered.contains("FILE: a.txt"));
        assert!(rendered.contains("FILE: b.txt"));
        assert!(!rendered.contains("excluded.txt"));
        Ok(())
    }

    #[test]
    fn escapes_control_characters_in_filename_headers_and_tree()
    -> Result<(), Box<dyn std::error::Error>> {
        let filename = "x\x1b]0;title\x07.md";
        let files = [FileRecord {
            path: filename,
            content: "content\n",
        }];
        let text_rendered = render_text_digest("repo", None, &files, 0)?;
        assert!(!text_rendered.contains("\x1b"));
        assert!(!text_rendered.contains("\x07"));
        assert!(text_rendered.contains("x\u{FFFD}]0;title\u{FFFD}.md"));
        Ok(())
    }
}
