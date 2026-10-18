//! Plain-text format serializer.

use super::escape::{escape_header, write_escaped};
use super::tree::render_tree;
use crate::core::model::CollectionResult;
use std::io::Write;

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

pub(super) fn render_text<W: Write + ?Sized>(
    writer: &mut W,
    collection: &CollectionResult,
) -> std::io::Result<()> {
    writer.write_all(REPOSITORY_PREFIX.as_bytes())?;
    write_escaped(writer, &collection.label, escape_header)?;
    writer.write_all(NEWLINE.as_bytes())?;
    if let Some(ref revision) = collection.revision {
        writer.write_all(REVISION_PREFIX.as_bytes())?;
        write_escaped(writer, revision, escape_header)?;
        writer.write_all(NEWLINE.as_bytes())?;
    }
    writer.write_all(FILES_PREFIX.as_bytes())?;
    write!(writer, "{}", collection.files.len())?;
    writer.write_all(SKIPPED_PREFIX.as_bytes())?;
    write!(writer, "{}", collection.skipped)?;
    writer.write_all(METADATA_SUFFIX.as_bytes())?;
    writer.write_all(DIRECTORY_HEADER.as_bytes())?;
    render_tree(writer, &collection.files, |w, name| {
        write_escaped(w, name, escape_header)
    })?;
    writer.write_all(FILES_HEADER.as_bytes())?;
    for file in collection.files.iter() {
        const TAIL: [&[u8]; 2] = [b"\n", b"\n\n"];
        let needs_extra_nl = (!file.content.ends_with('\n')) as usize;
        writer.write_all(FILE_BLOCK_PREFIX.as_bytes())?;
        write_escaped(writer, file.path, escape_header)?;
        writer.write_all(FILE_BLOCK_SUFFIX.as_bytes())?;
        writer.write_all(file.content.as_bytes())?;
        writer.write_all(TAIL[needs_extra_nl])?;
    }

    Ok(())
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
    ) -> std::io::Result<String> {
        let mut storage: FileStorage = files.iter().copied().collect();
        storage.sort_by_path();
        let collection = CollectionResult {
            label: label.to_string(),
            revision: revision.map(str::to_string),
            files: SortedFiles::new(storage),
            skipped,
        };
        let mut buffer = Vec::new();
        render_text(&mut buffer, &collection)?;
        String::from_utf8(buffer).map_err(std::io::Error::other)
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
        let mut storage: FileStorage = files.into_iter().collect();
        storage.sort_by_path();
        let collection = CollectionResult {
            label: "my-repo".into(),
            revision: Some("v1.0".into()),
            files: SortedFiles::new(storage),
            skipped: 2,
        };
        let mut streamed = Vec::new();
        render_text(&mut streamed, &collection)?;
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
        let mut storage = FileStorage::new();
        for f in files.into_iter().filter(|f| f.path != "excluded.txt") {
            storage.push(f.path, f.content);
        }
        storage.sort_by_path();
        let collection = CollectionResult {
            label: "repo".into(),
            revision: None,
            files: SortedFiles::new(storage),
            skipped: 1,
        };
        let mut buf = Vec::new();
        render_text(&mut buf, &collection)?;
        let rendered = std::str::from_utf8(&buf)?;
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
