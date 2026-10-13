//! In-memory directory tree representation and deterministic ASCII tree rendering.

use std::borrow::Cow;
use std::io::Write;

use super::text::escape_header;
use super::xml::xml_escape;

const TREE_BRANCH: &str = "├── ";
const TREE_LAST: &str = "└── ";
const TREE_VERT: &str = "│   ";
const TREE_SPACE: &str = "    ";

#[derive(Debug, Clone, Default)]
pub(crate) struct BudgetTrieNode {
    pub(crate) name: String,
    pub(crate) display: Option<String>,
    pub(crate) is_dir: bool,
    pub(crate) children: Vec<BudgetTrieNode>,
}

impl BudgetTrieNode {
    pub(crate) fn new(part: &str, is_dir: bool, escape_xml: bool) -> Self {
        let display = match tree_display(part, escape_xml) {
            Cow::Borrowed(_) => None,
            Cow::Owned(s) => Some(s),
        };
        Self {
            name: part.to_string(),
            display,
            is_dir,
            children: Vec::new(),
        }
    }

    #[inline]
    pub(crate) fn display(&self) -> &str {
        self.display.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Debug, Clone, Default)]
pub struct BudgetTrie {
    pub(crate) escape_xml: bool,
    pub(crate) children: Vec<BudgetTrieNode>,
}

fn tree_display<'a>(name: &'a str, escape_xml: bool) -> Cow<'a, str> {
    let escaped = escape_header(name);
    if escape_xml {
        match escaped {
            Cow::Borrowed(b) => xml_escape(b),
            Cow::Owned(ref o) => Cow::Owned(xml_escape(o).into_owned()),
        }
    } else {
        escaped
    }
}

fn find_child(children: &[BudgetTrieNode], part: &str, is_dir: bool) -> Result<usize, usize> {
    let Some(last) = children.last() else {
        return Err(0);
    };
    let ord = last
        .name
        .as_str()
        .cmp(part)
        .then_with(|| last.is_dir.cmp(&is_dir));
    match ord {
        std::cmp::Ordering::Equal => Ok(children.len() - 1),
        std::cmp::Ordering::Less => Err(children.len()),
        std::cmp::Ordering::Greater => children.binary_search_by(|c| {
            c.name
                .as_str()
                .cmp(part)
                .then_with(|| c.is_dir.cmp(&is_dir))
        }),
    }
}

impl BudgetTrie {
    pub fn new(escape_xml: bool) -> Self {
        Self {
            escape_xml,
            children: Vec::new(),
        }
    }

    pub fn insert(&mut self, path: &str) {
        let mut children = &mut self.children;
        let mut parts = path.split('/').filter(|s| !s.is_empty()).peekable();

        while let Some(part) = parts.next() {
            let is_dir = parts.peek().is_some();

            let idx = match find_child(children, part, is_dir) {
                Ok(idx) => idx,
                Err(insert_idx) => {
                    let node = BudgetTrieNode::new(part, is_dir, self.escape_xml);
                    children.insert(insert_idx, node);
                    insert_idx
                }
            };

            if is_dir {
                children = &mut children[idx].children;
            } else {
                break;
            }
        }
    }

    pub fn render_to_writer<W: Write + ?Sized>(&self, writer: &mut W) -> std::io::Result<()> {
        let mut prefix = String::new();
        Self::render_nodes(writer, &self.children, &mut prefix)
    }

    fn render_nodes<W: Write + ?Sized>(
        writer: &mut W,
        nodes: &[BudgetTrieNode],
        prefix: &mut String,
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
            writer.write_all(node.display().as_bytes())?;
            if node.is_dir {
                writer.write_all(b"/")?;
            }
            writer.write_all(b"\n")?;
            if !node.children.is_empty() {
                let prev_len = prefix.len();
                prefix.push_str(if is_last { TREE_SPACE } else { TREE_VERT });
                Self::render_nodes(writer, &node.children, prefix)?;
                prefix.truncate(prev_len);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn renders_ascii_tree_correctly() -> Result<(), Box<dyn std::error::Error>> {
        let mut trie = BudgetTrie::default();
        for path in ["Cargo.toml", "src/core/a.rs", "src/lib.rs"] {
            trie.insert(path);
        }
        let mut buf = Vec::new();
        trie.render_to_writer(&mut buf)?;
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
    fn escapes_tree_node_newlines() -> std::io::Result<()> {
        let mut trie = BudgetTrie::new(false);
        trie.insert("dir\nname/file\rname.txt");
        let mut out = Vec::new();
        trie.render_to_writer(&mut out)?;
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("dir\\nname/"));
        assert!(s.contains("file\\rname.txt"));
        assert!(!s.contains("dir\n"));
        Ok(())
    }

    #[test]
    fn escapes_tree_node_xml_entities() -> std::io::Result<()> {
        let mut trie = BudgetTrie::new(true);
        trie.insert("a & b/c < d.txt");
        let mut out = Vec::new();
        trie.render_to_writer(&mut out)?;
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("a &amp; b/"));
        assert!(s.contains("c &lt; d.txt"));
        Ok(())
    }

    #[test]
    fn test_trie_node_display_and_storage_optimization() {
        use std::borrow::Cow;

        assert!(matches!(tree_display("src", false), Cow::Borrowed("src")));
        assert!(matches!(tree_display("src", true), Cow::Borrowed("src")));

        assert!(matches!(
            tree_display("dir\nname", false),
            Cow::Owned(ref s) if s == "dir\\nname"
        ));
        assert!(matches!(
            tree_display("a & b", true),
            Cow::Owned(ref s) if s == "a &amp; b"
        ));

        let mut trie = BudgetTrie::new(false);
        trie.insert("clean/dir/file.rs");
        let clean_dir = &trie.children[0];
        assert_eq!(clean_dir.name, "clean");
        assert!(clean_dir.display.is_none());
        assert_eq!(clean_dir.display(), "clean");

        let mut trie_esc = BudgetTrie::new(true);
        trie_esc.insert("a & b/c.rs");
        let esc_dir = &trie_esc.children[0];
        assert_eq!(esc_dir.name, "a & b");
        assert_eq!(esc_dir.display.as_deref(), Some("a &amp; b"));
        assert_eq!(esc_dir.display(), "a &amp; b");
    }
}
