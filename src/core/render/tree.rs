use memchr::memchr;

use super::budget::{BoundedBuffer, OutputLimitExceeded};
use crate::core::storage::SortedFiles;

const TREE_BRANCH: &str = "├── ";
const TREE_LAST: &str = "└── ";
const TREE_VERT: &str = "│   ";
const TREE_SPACE: &str = "    ";

/// Renders an ASCII tree representation of the provided pre-sorted [`SortedFiles`].
pub(super) fn render_tree<F>(
    buf: BoundedBuffer,
    files: &SortedFiles,
    format_name: F,
) -> Result<BoundedBuffer, OutputLimitExceeded>
where
    F: Fn(BoundedBuffer, &str) -> Result<BoundedBuffer, OutputLimitExceeded> + Copy,
{
    let paths: Vec<&str> = files.iter().map(|f| f.path).collect();
    render_level(buf, &paths, 0, "", format_name)
}

fn render_level<F>(
    mut buf: BoundedBuffer,
    paths: &[&str],
    offset: usize,
    prefix: &str,
    format_name: F,
) -> Result<BoundedBuffer, OutputLimitExceeded>
where
    F: Fn(BoundedBuffer, &str) -> Result<BoundedBuffer, OutputLimitExceeded> + Copy,
{
    let mut i = 0;
    while i < paths.len() {
        let (name, is_dir) = component_at_offset(paths[i], offset);
        let start = i;
        i += 1;
        if is_dir {
            let dir_prefix = &paths[start][offset..=(offset + name.len())];
            while paths
                .get(i)
                .is_some_and(|path| path[offset..].starts_with(dir_prefix))
            {
                i += 1;
            }
        }
        let end = i;
        let is_last = end == paths.len();

        buf = buf.push_str(prefix)?;
        buf = buf.push_str(if is_last { TREE_LAST } else { TREE_BRANCH })?;
        buf = format_name(buf, name)?;

        if is_dir {
            buf = buf.push_bytes(b"/\n")?;
            let child_prefix = format!("{prefix}{}", if is_last { TREE_SPACE } else { TREE_VERT });
            buf = render_level(
                buf,
                &paths[start..end],
                offset + name.len() + 1,
                &child_prefix,
                format_name,
            )?;
        } else {
            buf = buf.push_bytes(b"\n")?;
        }
    }

    Ok(buf)
}

/// Returns the path component starting at `offset` and whether a `/` follows it.
///
/// # Panics
///
/// Panics if `offset` is not a char boundary within `path` or leaves no component. Every path in a
/// [`SortedFiles`] shares its parent prefix up to `offset`, ends in a non-empty segment
/// (`NormalizedPath`), and is never the directory prefix of another (I-01), so this cannot happen.
#[must_use]
pub(super) fn component_at_offset(path: &str, offset: usize) -> (&str, bool) {
    let remainder = &path[offset..];
    assert!(
        !remainder.is_empty(),
        "invariant: NormalizedPath extends past every tree offset"
    );
    memchr(b'/', remainder.as_bytes()).map_or((remainder, false), |idx| (&remainder[..idx], true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{path::NormalizedPath, storage::FileStorage};

    #[test]
    fn renders_ascii_tree_correctly() {
        let storage = FileStorage::new()
            .push(&crate::core::path::NormalizedPath::from("Cargo.toml"), "")
            .unwrap()
            .push(
                &crate::core::path::NormalizedPath::from("src/core/a.rs"),
                "",
            )
            .unwrap()
            .push(&crate::core::path::NormalizedPath::from("src/lib.rs"), "")
            .unwrap();
        let files = SortedFiles::new(storage).unwrap();

        let buf = render_tree(BoundedBuffer::default(), &files, BoundedBuffer::push_str).unwrap();
        let digest = buf.finish();
        let tree_str = std::str::from_utf8(&digest.bytes).unwrap();
        let expected = "\
├── Cargo.toml
└── src/
    ├── core/
    │   └── a.rs
    └── lib.rs
";
        assert_eq!(tree_str, expected);
    }

    #[test]
    fn formats_tree_nodes_custom_formatter() {
        let storage = FileStorage::new()
            .push(
                &crate::core::path::NormalizedPath::from("dir_name/file_name.txt"),
                "",
            )
            .unwrap();
        let files = SortedFiles::new(storage).unwrap();

        let buf = render_tree(BoundedBuffer::default(), &files, |b, name| {
            b.push_str(&name.to_uppercase())
        })
        .unwrap();
        let digest = buf.finish();
        let s = String::from_utf8_lossy(&digest.bytes);
        assert!(s.contains("DIR_NAME/"));
        assert!(s.contains("FILE_NAME.TXT"));
    }

    #[test]
    fn renders_nested_siblings_in_alphabetical_order() {
        let storage = FileStorage::new()
            .push(&NormalizedPath::from("a/x.txt"), "")
            .unwrap()
            .push(&NormalizedPath::from("a/y.txt"), "")
            .unwrap()
            .push(&NormalizedPath::from("b/z.txt"), "")
            .unwrap();
        let files = SortedFiles::new(storage).unwrap();

        let buf = render_tree(BoundedBuffer::default(), &files, BoundedBuffer::push_str).unwrap();
        let digest = buf.finish();
        let tree_str = std::str::from_utf8(&digest.bytes).unwrap();
        let expected = "\
├── a/
│   ├── x.txt
│   └── y.txt
└── b/
    └── z.txt
";
        assert_eq!(tree_str, expected);
    }

    #[test]
    #[should_panic(expected = "invariant: NormalizedPath extends past every tree offset")]
    fn render_level_panics_on_path_that_ends_at_a_directory_offset() {
        // `a/` is not a NormalizedPath. Its child level would start past the end of the path, which
        // must fail loudly instead of ending the level and dropping the rest of the tree.
        let _ = render_level(
            BoundedBuffer::default(),
            &["a/"],
            0,
            "",
            BoundedBuffer::push_str,
        );
    }

    #[test]
    fn component_at_offset_slices_correctly_and_handles_boundaries() {
        let path = "alpha/beta/gamma.rs";
        assert_eq!(component_at_offset(path, 0), ("alpha", true));
        assert_eq!(component_at_offset(path, 6), ("beta", true));
        assert_eq!(component_at_offset(path, 11), ("gamma.rs", false));

        let single = "root_file.txt";
        assert_eq!(component_at_offset(single, 0), ("root_file.txt", false));
    }

    #[test]
    #[should_panic(expected = "NormalizedPath extends past every tree offset")]
    fn component_at_offset_panics_when_offset_ends_the_path() {
        let _ = component_at_offset("root_file.txt", 13);
    }

    #[test]
    #[should_panic(expected = "out of bounds")]
    fn component_at_offset_panics_when_offset_passes_the_path() {
        let _ = component_at_offset("root_file.txt", 100);
    }

    #[test]
    fn renders_deep_multi_level_tree_correctly() {
        let storage = FileStorage::new()
            .push(&NormalizedPath::from("a/b/c/d/e.txt"), "")
            .unwrap()
            .push(&NormalizedPath::from("a/b/c/f.txt"), "")
            .unwrap()
            .push(&NormalizedPath::from("a/b/g.txt"), "")
            .unwrap()
            .push(&NormalizedPath::from("root.txt"), "")
            .unwrap();
        let files = SortedFiles::new(storage).unwrap();

        let buf = render_tree(BoundedBuffer::default(), &files, BoundedBuffer::push_str).unwrap();
        let digest = buf.finish();
        let tree_str = std::str::from_utf8(&digest.bytes).unwrap();
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
    }
}
