//! In-memory deterministic ASCII tree rendering.
//!
//! Renders formatted ASCII directory trees directly from FileStorage
//! with format-aware escaping and zero allocations.

use std::io::Write;

use crate::core::storage::{FileSpan, FileStorage};

const TREE_BRANCH: &str = "├── ";
const TREE_LAST: &str = "└── ";
const TREE_VERT: &str = "│   ";
const TREE_SPACE: &str = "    ";

/// Renders an ASCII tree representation of the provided pre-sorted [`FileStorage`] to the writer.
///
/// Uses `format_name` to stream individual path components per format specification.
///
/// # Errors
///
/// Returns an [`std::io::Error`] if writing to `writer` fails.
pub(crate) fn render_tree<W: Write + ?Sized, F>(
    writer: &mut W,
    storage: &FileStorage,
    mut format_name: F,
) -> std::io::Result<()>
where
    F: FnMut(&mut W, &str) -> std::io::Result<()>,
{
    let mut prefix = String::new();
    render_level(
        writer,
        storage.spans(),
        storage.path_arena(),
        0,
        &mut prefix,
        &mut format_name,
    )
}

fn render_level<W: Write + ?Sized, F>(
    writer: &mut W,
    spans: &[FileSpan],
    path_arena: &str,
    depth: usize,
    prefix: &mut String,
    format_name: &mut F,
) -> std::io::Result<()>
where
    F: FnMut(&mut W, &str) -> std::io::Result<()>,
{
    if spans.is_empty() {
        return Ok(());
    }

    let mut i = 0;
    while i < spans.len() {
        let (name, is_dir) = component_at(spans[i].path(path_arena), depth);
        if name.is_empty() {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < spans.len() {
            let (next_name, next_is_dir) = component_at(spans[i].path(path_arena), depth);
            if next_name == name && next_is_dir == is_dir {
                i += 1;
            } else {
                break;
            }
        }
        let end = i;
        let is_last = end == spans.len();

        writer.write_all(prefix.as_bytes())?;
        writer.write_all(if is_last { TREE_LAST } else { TREE_BRANCH }.as_bytes())?;
        format_name(writer, name)?;

        if is_dir {
            writer.write_all(b"/\n")?;
            let prev_len = prefix.len();
            prefix.push_str(if is_last { TREE_SPACE } else { TREE_VERT });
            render_level(
                writer,
                &spans[start..end],
                path_arena,
                depth + 1,
                prefix,
                format_name,
            )?;
            prefix.truncate(prev_len);
        } else {
            writer.write_all(b"\n")?;
        }
    }

    Ok(())
}

fn component_at(path: &str, depth: usize) -> (&str, bool) {
    let mut parts = path.split('/').filter(|s| !s.is_empty());
    for _ in 0..depth {
        if parts.next().is_none() {
            return ("", false);
        }
    }
    match parts.next() {
        Some(name) => {
            let is_dir = parts.next().is_some();
            (name, is_dir)
        }
        None => ("", false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_ascii_tree_correctly() -> Result<(), Box<dyn std::error::Error>> {
        let mut storage = FileStorage::new();
        storage.push("Cargo.toml", "");
        storage.push("src/core/a.rs", "");
        storage.push("src/lib.rs", "");
        storage.sort_by_path();

        let mut buf = Vec::new();
        render_tree(&mut buf, &storage, |w, s| w.write_all(s.as_bytes()))?;
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
    fn formats_tree_nodes_custom_formatter() -> std::io::Result<()> {
        let mut storage = FileStorage::new();
        storage.push("dir_name/file_name.txt", "");

        let mut out = Vec::new();
        render_tree(&mut out, &storage, |w, name| {
            w.write_all(name.to_uppercase().as_bytes())
        })?;
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("DIR_NAME/"));
        assert!(s.contains("FILE_NAME.TXT"));
        Ok(())
    }

    #[test]
    fn renders_nested_siblings_in_alphabetical_order() -> Result<(), Box<dyn std::error::Error>> {
        let mut storage = FileStorage::new();
        storage.push("a/x.txt", "");
        storage.push("a/y.txt", "");
        storage.push("b/z.txt", "");
        storage.sort_by_path();

        let mut buf = Vec::new();
        render_tree(&mut buf, &storage, |w, s| w.write_all(s.as_bytes()))?;
        let tree_str = std::str::from_utf8(&buf)?;
        let expected = "\
├── a/
│   ├── x.txt
│   └── y.txt
└── b/
    └── z.txt
";
        assert_eq!(tree_str, expected);
        Ok(())
    }
}
