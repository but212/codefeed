use memchr::memchr;

use super::budget::{BoundedBuffer, OutputLimitExceeded};
use crate::core::storage::{FileSpan, SortedFiles};

const TREE_BRANCH: &str = "├── ";
const TREE_LAST: &str = "└── ";
const TREE_VERT: &str = "│   ";
const TREE_SPACE: &str = "    ";

/// Renders an ASCII tree representation of the provided pre-sorted [`SortedFiles`].
pub(crate) fn render_tree<F>(
    buf: BoundedBuffer,
    files: &SortedFiles,
    format_name: F,
) -> Result<BoundedBuffer, OutputLimitExceeded>
where
    F: Fn(BoundedBuffer, &str) -> Result<BoundedBuffer, OutputLimitExceeded> + Copy,
{
    render_level(
        buf,
        files.spans(),
        files.path_arena(),
        0,
        String::new(),
        format_name,
    )
    .map(|(buf, _)| buf)
}

fn render_level<F>(
    mut buf: BoundedBuffer,
    spans: &[FileSpan],
    path_arena: &str,
    offset: usize,
    mut prefix: String,
    format_name: F,
) -> Result<(BoundedBuffer, String), OutputLimitExceeded>
where
    F: Fn(BoundedBuffer, &str) -> Result<BoundedBuffer, OutputLimitExceeded> + Copy,
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

        buf = buf.push_str(&prefix)?;
        buf = buf.push_str(if is_last { TREE_LAST } else { TREE_BRANCH })?;
        buf = format_name(buf, name)?;

        if is_dir {
            buf = buf.push_bytes(b"/\n")?;
            let prefix_len = prefix.len();
            prefix.push_str(if is_last { TREE_SPACE } else { TREE_VERT });
            (buf, prefix) = render_level(
                buf,
                &spans[start..end],
                path_arena,
                offset + name.len() + 1,
                prefix,
                format_name,
            )?;
            prefix.truncate(prefix_len);
        } else {
            buf = buf.push_bytes(b"\n")?;
        }
    }

    Ok((buf, prefix))
}

pub(super) fn component_at_offset(path: &str, offset: usize) -> Option<(&str, bool)> {
    let remainder = path.get(offset..)?;
    if remainder.is_empty() {
        return None;
    }
    match memchr(b'/', remainder.as_bytes()) {
        Some(idx) => Some((&remainder[..idx], true)),
        None => Some((remainder, false)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::storage::FileStorage;

    #[test]
    fn renders_ascii_tree_correctly() -> Result<(), Box<dyn std::error::Error>> {
        let storage = FileStorage::new()
            .push(&crate::core::policy::NormalizedPath::from("Cargo.toml"), "")?
            .push(
                &crate::core::policy::NormalizedPath::from("src/core/a.rs"),
                "",
            )?
            .push(&crate::core::policy::NormalizedPath::from("src/lib.rs"), "")?;
        let files = SortedFiles::new(storage);

        let buf = render_tree(BoundedBuffer::default(), &files, |b, s| b.push_str(s))?;
        let digest = buf.finish();
        let tree_str = std::str::from_utf8(&digest.bytes)?;
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
    fn formats_tree_nodes_custom_formatter() -> Result<(), Box<dyn std::error::Error>> {
        let storage = FileStorage::new().push(
            &crate::core::policy::NormalizedPath::from("dir_name/file_name.txt"),
            "",
        )?;
        let files = SortedFiles::new(storage);

        let buf = render_tree(BoundedBuffer::default(), &files, |b, name| {
            b.push_str(&name.to_uppercase())
        })?;
        let digest = buf.finish();
        let s = String::from_utf8_lossy(&digest.bytes);
        assert!(s.contains("DIR_NAME/"));
        assert!(s.contains("FILE_NAME.TXT"));
        Ok(())
    }

    #[test]
    fn renders_nested_siblings_in_alphabetical_order() -> Result<(), Box<dyn std::error::Error>> {
        let storage = FileStorage::new()
            .push(&crate::core::policy::NormalizedPath::from("a/x.txt"), "")?
            .push(&crate::core::policy::NormalizedPath::from("a/y.txt"), "")?
            .push(&crate::core::policy::NormalizedPath::from("b/z.txt"), "")?;
        let files = SortedFiles::new(storage);

        let buf = render_tree(BoundedBuffer::default(), &files, |b, s| b.push_str(s))?;
        let digest = buf.finish();
        let tree_str = std::str::from_utf8(&digest.bytes)?;
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
        let storage = FileStorage::new()
            .push(
                &crate::core::policy::NormalizedPath::from("a/b/c/d/e.txt"),
                "",
            )?
            .push(
                &crate::core::policy::NormalizedPath::from("a/b/c/f.txt"),
                "",
            )?
            .push(&crate::core::policy::NormalizedPath::from("a/b/g.txt"), "")?
            .push(&crate::core::policy::NormalizedPath::from("root.txt"), "")?;
        let files = SortedFiles::new(storage);

        let buf = render_tree(BoundedBuffer::default(), &files, |b, s| b.push_str(s))?;
        let digest = buf.finish();
        let tree_str = std::str::from_utf8(&digest.bytes)?;
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
