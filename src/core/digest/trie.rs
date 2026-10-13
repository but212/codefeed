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
    pub(crate) name: Box<str>,
    pub(crate) display: Option<Box<str>>,
    pub(crate) is_dir: bool,
    pub(crate) children: Vec<u32>,
}

impl BudgetTrieNode {
    pub(crate) fn new(part: &str, is_dir: bool, escape_xml: bool) -> Self {
        let display = match tree_display(part, escape_xml) {
            Cow::Borrowed(_) => None,
            Cow::Owned(s) => Some(s.into_boxed_str()),
        };
        Self {
            name: part.into(),
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
    pub(crate) nodes: Vec<BudgetTrieNode>,
    pub(crate) root_children: Vec<u32>,
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

fn find_child_idx(
    nodes: &[BudgetTrieNode],
    children: &[u32],
    part: &str,
    is_dir: bool,
) -> Result<usize, usize> {
    let Some(&last_idx) = children.last() else {
        return Err(0);
    };
    let last = &nodes[last_idx as usize];
    let ord = (*last.name).cmp(part).then_with(|| last.is_dir.cmp(&is_dir));
    match ord {
        std::cmp::Ordering::Equal => Ok(children.len() - 1),
        std::cmp::Ordering::Less => Err(children.len()),
        std::cmp::Ordering::Greater => children.binary_search_by(|&c_idx| {
            let c = &nodes[c_idx as usize];
            (*c.name).cmp(part).then_with(|| c.is_dir.cmp(&is_dir))
        }),
    }
}

impl BudgetTrie {
    pub fn new(escape_xml: bool) -> Self {
        Self {
            escape_xml,
            nodes: Vec::new(),
            root_children: Vec::new(),
        }
    }

    pub fn insert(&mut self, path: &str) {
        let mut parts = path.split('/').filter(|s| !s.is_empty()).peekable();
        let mut current_node_idx: Option<u32> = None;

        while let Some(part) = parts.next() {
            let is_dir = parts.peek().is_some();
            let matched_node_idx = self.get_or_insert_child(current_node_idx, part, is_dir);

            if is_dir {
                current_node_idx = Some(matched_node_idx);
            } else {
                break;
            }
        }
    }

    fn get_or_insert_child(
        &mut self,
        parent_idx: Option<u32>,
        part: &str,
        is_dir: bool,
    ) -> u32 {
        let new_node_idx = u32::try_from(self.nodes.len()).expect("trie node count exceeds u32::MAX");
        match parent_idx {
            None => match find_child_idx(&self.nodes, &self.root_children, part, is_dir) {
                Ok(slot) => self.root_children[slot],
                Err(insert_pos) => {
                    self.nodes
                        .push(BudgetTrieNode::new(part, is_dir, self.escape_xml));
                    self.root_children.insert(insert_pos, new_node_idx);
                    new_node_idx
                }
            },
            Some(p_idx) => {
                match find_child_idx(&self.nodes, &self.nodes[p_idx as usize].children, part, is_dir) {
                    Ok(slot) => self.nodes[p_idx as usize].children[slot],
                    Err(insert_pos) => {
                        self.nodes
                            .push(BudgetTrieNode::new(part, is_dir, self.escape_xml));
                        self.nodes[p_idx as usize].children.insert(insert_pos, new_node_idx);
                        new_node_idx
                    }
                }
            }
        }
    }

    pub fn render_to_writer<W: Write + ?Sized>(&self, writer: &mut W) -> std::io::Result<()> {
        let mut prefix = String::new();
        self.render_node_indices(writer, &self.root_children, &mut prefix)
    }

    fn render_node_indices<W: Write + ?Sized>(
        &self,
        writer: &mut W,
        child_indices: &[u32],
        prefix: &mut String,
    ) -> std::io::Result<()> {
        let total = child_indices.len();
        for (idx, &node_idx) in child_indices.iter().enumerate() {
            let is_last = idx + 1 == total;
            let node = &self.nodes[node_idx as usize];

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
                self.render_node_indices(writer, &node.children, prefix)?;
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
        let clean_dir_idx = trie.root_children[0];
        let clean_dir = &trie.nodes[clean_dir_idx as usize];
        assert_eq!(&*clean_dir.name, "clean");
        assert!(clean_dir.display.is_none());
        assert_eq!(clean_dir.display(), "clean");

        let mut trie_esc = BudgetTrie::new(true);
        trie_esc.insert("a & b/c.rs");
        let esc_dir_idx = trie_esc.root_children[0];
        let esc_dir = &trie_esc.nodes[esc_dir_idx as usize];
        assert_eq!(&*esc_dir.name, "a & b");
        assert_eq!(esc_dir.display.as_deref(), Some("a &amp; b"));
        assert_eq!(esc_dir.display(), "a &amp; b");
    }

    #[test]
    fn test_flat_arena_links() {
        let mut trie = BudgetTrie::new(false);
        trie.insert("src/lib.rs");
        assert_eq!(trie.nodes.len(), 2);
        assert_eq!(trie.root_children, vec![0]);
        assert_eq!(&*trie.nodes[0].name, "src");
        assert!(trie.nodes[0].is_dir);
        assert_eq!(trie.nodes[0].children, vec![1]);

        assert_eq!(&*trie.nodes[1].name, "lib.rs");
        assert!(!trie.nodes[1].is_dir);
        assert!(trie.nodes[1].children.is_empty());
    }
}
