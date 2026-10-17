//! In-memory deterministic ASCII tree rendering.
//!
//! Renders formatted ASCII directory trees directly from sorted path slices
//! with format-aware escaping.

use std::borrow::Cow;
use std::io::Write;

const TREE_BRANCH: &str = "├── ";
const TREE_LAST: &str = "└── ";
const TREE_VERT: &str = "│   ";
const TREE_SPACE: &str = "    ";

pub fn render_tree<W: Write + ?Sized, F>(
    writer: &mut W,
    paths: &[&str],
    format_name: F,
) -> std::io::Result<()>
where
    F: Fn(&str) -> Cow<'_, str>,
{
    let mut prefix = String::new();
    render_level(writer, paths, 0, &mut prefix, &format_name)
}

struct Child<'a> {
    name: &'a str,
    is_dir: bool,
    start: usize,
    end: usize,
}

fn render_level<W: Write + ?Sized, F>(
    writer: &mut W,
    paths: &[&str],
    depth: usize,
    prefix: &mut String,
    format_name: &F,
) -> std::io::Result<()>
where
    F: Fn(&str) -> Cow<'_, str>,
{
    if paths.is_empty() {
        return Ok(());
    }

    let mut children: Vec<Child<'_>> = Vec::new();
    let mut i = 0;
    while i < paths.len() {
        let (name, is_dir) = component_at(paths[i], depth);
        if name.is_empty() {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < paths.len() {
            let (next_name, next_is_dir) = component_at(paths[i], depth);
            if next_name == name && next_is_dir == is_dir {
                i += 1;
            } else {
                break;
            }
        }
        children.push(Child {
            name,
            is_dir,
            start,
            end: i,
        });
    }

    children.sort_unstable_by(|a, b| a.name.cmp(b.name).then_with(|| a.is_dir.cmp(&b.is_dir)));

    let total = children.len();
    for (idx, child) in children.iter().enumerate() {
        let is_last = idx + 1 == total;
        writer.write_all(prefix.as_bytes())?;
        writer.write_all(if is_last { TREE_LAST } else { TREE_BRANCH }.as_bytes())?;
        let formatted = format_name(child.name);
        writer.write_all(formatted.as_bytes())?;
        if child.is_dir {
            writer.write_all(b"/\n")?;
            let prev_len = prefix.len();
            prefix.push_str(if is_last { TREE_SPACE } else { TREE_VERT });
            render_level(
                writer,
                &paths[child.start..child.end],
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
        let paths = ["Cargo.toml", "src/core/a.rs", "src/lib.rs"];
        let mut buf = Vec::new();
        render_tree(&mut buf, &paths, |s| Cow::Borrowed(s))?;
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
        let paths = ["dir_name/file_name.txt"];
        let mut out = Vec::new();
        render_tree(&mut out, &paths, |name| Cow::Owned(name.to_uppercase()))?;
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("DIR_NAME/"));
        assert!(s.contains("FILE_NAME.TXT"));
        Ok(())
    }

    #[test]
    fn renders_nested_siblings_in_alphabetical_order() -> Result<(), Box<dyn std::error::Error>> {
        let paths = ["a/x.txt", "a/y.txt", "b/z.txt"];
        let mut buf = Vec::new();
        render_tree(&mut buf, &paths, |s| Cow::Borrowed(s))?;
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
