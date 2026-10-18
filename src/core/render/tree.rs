use crate::core::storage::{FileSpan, SortedFiles};
use std::io::Write;

const TREE_BRANCH: &str = "├── ";
const TREE_LAST: &str = "└── ";
const TREE_VERT: &str = "│   ";
const TREE_SPACE: &str = "    ";

/// Renders an ASCII tree representation of the provided pre-sorted [`SortedFiles`] to the writer.
pub(crate) fn render_tree<W: Write + ?Sized, F>(
    writer: &mut W,
    files: &SortedFiles,
    mut format_name: F,
) -> std::io::Result<()>
where
    F: FnMut(&mut W, &str) -> std::io::Result<()>,
{
    let mut prefix = String::new();
    render_level(
        writer,
        files.spans(),
        files.path_arena(),
        0,
        &mut prefix,
        &mut format_name,
    )
}

fn render_level<W: Write + ?Sized, F>(
    writer: &mut W,
    spans: &[FileSpan],
    path_arena: &str,
    offset: usize,
    prefix: &mut String,
    format_name: &mut F,
) -> std::io::Result<()>
where
    F: FnMut(&mut W, &str) -> std::io::Result<()>,
{
    let mut i = 0;
    while let Some((name, is_dir)) = spans
        .get(i)
        .and_then(|s| component_at_offset(s.path(path_arena), offset))
    {
        let start = i;
        i += 1;
        if is_dir {
            let dir_prefix = &spans[start].path(path_arena)[offset..offset + name.len() + 1];
            while spans
                .get(i)
                .and_then(|s| s.path(path_arena).get(offset..))
                .is_some_and(|rem| rem.starts_with(dir_prefix))
            {
                i += 1;
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
                offset + name.len() + 1,
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

#[inline]
pub(super) fn component_at_offset(path: &str, offset: usize) -> Option<(&str, bool)> {
    let remainder = path.get(offset..)?;
    if remainder.is_empty() {
        return None;
    }
    match remainder.find('/') {
        Some(idx) => Some((&remainder[..idx], true)),
        None => Some((remainder, false)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::storage::{FileStorage, SortedFiles};

    #[test]
    fn renders_ascii_tree_correctly() -> Result<(), Box<dyn std::error::Error>> {
        let mut storage = FileStorage::new();
        storage.push("Cargo.toml", "");
        storage.push("src/core/a.rs", "");
        storage.push("src/lib.rs", "");
        let files = SortedFiles::new(storage);

        let mut buf = Vec::new();
        render_tree(&mut buf, &files, |w, s| w.write_all(s.as_bytes()))?;
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
        let files = SortedFiles::new(storage);

        let mut out = Vec::new();
        render_tree(&mut out, &files, |w, name| {
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
        let files = SortedFiles::new(storage);

        let mut buf = Vec::new();
        render_tree(&mut buf, &files, |w, s| w.write_all(s.as_bytes()))?;
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

    #[test]
    fn component_at_offset_slices_correctly_and_handles_boundaries() {
        let path = "alpha/beta/gamma.rs";
        assert_eq!(component_at_offset(path, 0), Some(("alpha", true)));
        assert_eq!(component_at_offset(path, 6), Some(("beta", true)));
        assert_eq!(component_at_offset(path, 11), Some(("gamma.rs", false)));
        assert_eq!(component_at_offset(path, 19), None);
        assert_eq!(component_at_offset(path, 100), None);

        let single = "root_file.txt";
        assert_eq!(
            component_at_offset(single, 0),
            Some(("root_file.txt", false))
        );
        assert_eq!(component_at_offset(single, 13), None);
    }

    #[test]
    fn renders_deep_multi_level_tree_correctly() -> Result<(), Box<dyn std::error::Error>> {
        let mut storage = FileStorage::new();
        storage.push("a/b/c/d/e.txt", "");
        storage.push("a/b/c/f.txt", "");
        storage.push("a/b/g.txt", "");
        storage.push("root.txt", "");
        let files = SortedFiles::new(storage);

        let mut buf = Vec::new();
        render_tree(&mut buf, &files, |w, s| w.write_all(s.as_bytes()))?;
        let tree_str = std::str::from_utf8(&buf)?;
        let expected = "\
├── a/
│   └── b/
│       ├── c/
│       │   ├── d/
│       │   │   └── e.txt
│       │   └── f.txt
│       └── g.txt
└── root.txt
";
        assert_eq!(tree_str, expected);
        Ok(())
    }
}
