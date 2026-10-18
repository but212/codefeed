//! Format-neutral ASCII hierarchy tree rendering.

use crate::core::storage::{FileSpan, FileStorage};
use std::io::Write;

const TREE_BRANCH: &str = "├── ";
const TREE_LAST: &str = "└── ";
const TREE_VERT: &str = "│   ";
const TREE_SPACE: &str = "    ";

/// Renders an ASCII tree representation of the provided pre-sorted [`FileStorage`] to the writer.
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
