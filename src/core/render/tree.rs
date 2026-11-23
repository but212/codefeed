use std::ops::Range;

use super::budget::{BoundedBuffer, OutputLimitExceeded};
use crate::core::storage::SortedFiles;

const TREE_BRANCH: &str = "├── ";
const TREE_LAST: &str = "└── ";
const TREE_VERT: &str = "│   ";
const TREE_SPACE: &str = "    ";

type WriteName = fn(BoundedBuffer, &str) -> Result<BoundedBuffer, OutputLimitExceeded>;

/// Renders the tree of pre-sorted files using the caller's label escaping.
pub(super) fn render_tree(
    buf: BoundedBuffer,
    files: &SortedFiles,
    write_name: WriteName,
) -> Result<BoundedBuffer, OutputLimitExceeded> {
    render_level(buf, files, 0..files.len(), 0, String::new(), write_name).map(|(buf, _)| buf)
}

fn render_level(
    mut buf: BoundedBuffer,
    files: &SortedFiles,
    range: Range<usize>,
    offset: usize,
    mut prefix: String,
    write_name: WriteName,
) -> Result<(BoundedBuffer, String), OutputLimitExceeded> {
    let mut i = range.start;
    while i < range.end {
        let first_path = files.path_at(i);
        let (name, is_dir) = component_at_offset(first_path, offset);
        let start = i;
        i += 1;
        if is_dir {
            let dir_prefix = &first_path[offset..=(offset + name.len())];
            while i < range.end && files.path_at(i)[offset..].starts_with(dir_prefix) {
                i += 1;
            }
        }
        let end = i;
        let is_last = end == range.end;

        buf = buf.push_str(&prefix)?;
        buf = buf.push_str(if is_last { TREE_LAST } else { TREE_BRANCH })?;
        buf = write_name(buf, name)?;

        if is_dir {
            buf = buf.push_bytes(b"/\n")?;
            let saved_len = prefix.len();
            prefix.push_str(if is_last { TREE_SPACE } else { TREE_VERT });
            (buf, prefix) = render_level(
                buf,
                files,
                start..end,
                offset + name.len() + 1,
                prefix,
                write_name,
            )?;
            prefix.truncate(saved_len);
        } else {
            buf = buf.push_bytes(b"\n")?;
        }
    }

    Ok((buf, prefix))
}

/// Returns the path component at `offset` and whether `/` follows it.
///
/// # Panics
///
/// Panics if `offset` is off a char boundary or empty; unreachable for [`SortedFiles`] (`NormalizedPath`, I-01).
#[must_use]
fn component_at_offset(path: &str, offset: usize) -> (&str, bool) {
    let remainder = &path[offset..];
    assert!(
        !remainder.is_empty(),
        "invariant: NormalizedPath extends past every tree offset"
    );
    remainder
        .split_once('/')
        .map_or((remainder, false), |(name, _)| (name, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{path::NormalizedPath, storage::FileStorage};

    #[test]
    fn renders_ascii_tree_correctly() {
        let cases: &[(&[&str], &str)] = &[
            (
                &["Cargo.toml", "src/core/a.rs", "src/lib.rs"],
                "\
├── Cargo.toml
└── src/
    ├── core/
    │   └── a.rs
    └── lib.rs
",
            ),
            (
                &["a/x.txt", "a/y.txt", "b/z.txt"],
                "\
├── a/
│   ├── x.txt
│   └── y.txt
└── b/
    └── z.txt
",
            ),
            (
                &["a/b/c/d/e.txt", "a/b/c/f.txt", "a/b/g.txt", "root.txt"],
                "\
├── a/
│   └── b/
│       ├── c/
│       │   ├── d/
│       │   │   └── e.txt
│       │   └── f.txt
│       └── g.txt
└── root.txt
",
            ),
        ];

        for &(paths, expected) in cases {
            let mut storage = FileStorage::new();
            for path in paths {
                storage = storage.push(&NormalizedPath::from(*path), "").unwrap();
            }
            let files = SortedFiles::new(storage).unwrap();
            let buf =
                render_tree(BoundedBuffer::default(), &files, BoundedBuffer::push_str).unwrap();
            let digest = buf.finish();
            assert_eq!(std::str::from_utf8(&digest.bytes).unwrap(), expected);
        }
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
}
