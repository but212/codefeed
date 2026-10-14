//! In-memory directory tree representation and deterministic ASCII tree rendering.

use std::borrow::Cow;
use std::io::Write;

use super::text::escape_header;
use super::xml::xml_escape;

const TREE_BRANCH: &str = "├── ";
const TREE_LAST: &str = "└── ";
const TREE_VERT: &str = "│   ";
const TREE_SPACE: &str = "    ";

pub(crate) const NO_NODE: u32 = u32::MAX;

/// 16-byte fixed-size trie node in a Left-Child Right-Sibling (LCRS) flat arena.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BudgetTrieNode {
    pub(crate) name_offset: u32,
    pub(crate) name_len: u16,
    pub(crate) is_dir: bool,
    pub(crate) _pad: u8,
    pub(crate) first_child: u32,  // NO_NODE means None
    pub(crate) next_sibling: u32, // NO_NODE means None
}

impl BudgetTrieNode {
    pub(crate) fn new(name_offset: u32, name_len: u16, is_dir: bool) -> Self {
        Self {
            name_offset,
            name_len,
            is_dir,
            _pad: 0,
            first_child: NO_NODE,
            next_sibling: NO_NODE,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BudgetTrie {
    pub(crate) nodes: Vec<BudgetTrieNode>,
    pub(crate) label_arena: String,
    pub(crate) last_children: Vec<u32>,
}

impl Default for BudgetTrie {
    fn default() -> Self {
        Self {
            nodes: vec![BudgetTrieNode::new(0, 0, true)],
            label_arena: String::new(),
            last_children: vec![NO_NODE],
        }
    }
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

impl BudgetTrie {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub(crate) fn node_name(&self, idx: u32) -> &str {
        let node = &self.nodes[idx as usize];
        let start = node.name_offset as usize;
        let end = start + node.name_len as usize;
        &self.label_arena[start..end]
    }

    pub fn insert(&mut self, path: &str) {
        let mut parts = path.split('/').filter(|s| !s.is_empty()).peekable();
        let mut current_node_idx: u32 = 0;

        while let Some(part) = parts.next() {
            let is_dir = parts.peek().is_some();
            let matched_node_idx = self.get_or_insert_child(current_node_idx, part, is_dir);

            if is_dir {
                current_node_idx = matched_node_idx;
            } else {
                break;
            }
        }
    }

    fn get_or_insert_child(&mut self, parent_idx: u32, part: &str, is_dir: bool) -> u32 {
        let p = parent_idx as usize;
        let last_child_idx = self.last_children[p];

        if last_child_idx != NO_NODE {
            let last_node = self.nodes[last_child_idx as usize];
            let last_name = &self.label_arena[last_node.name_offset as usize
                ..(last_node.name_offset + last_node.name_len as u32) as usize];
            let ord = last_name
                .cmp(part)
                .then_with(|| last_node.is_dir.cmp(&is_dir));
            if ord == std::cmp::Ordering::Equal {
                return last_child_idx;
            }
            if ord == std::cmp::Ordering::Less {
                // Sequential append (presorted fast path): append after last_child
                let new_idx =
                    u32::try_from(self.nodes.len()).expect("trie node count exceeds u32::MAX");
                let name_offset =
                    u32::try_from(self.label_arena.len()).expect("trie arena exceeds u32::MAX");
                let name_len = u16::try_from(part.len()).expect("part length exceeds u16::MAX");
                self.label_arena.push_str(part);
                self.nodes
                    .push(BudgetTrieNode::new(name_offset, name_len, is_dir));
                self.last_children.push(NO_NODE);

                self.nodes[last_child_idx as usize].next_sibling = new_idx;
                self.last_children[p] = new_idx;
                return new_idx;
            }
        } else {
            // First child for parent
            let new_idx =
                u32::try_from(self.nodes.len()).expect("trie node count exceeds u32::MAX");
            let name_offset =
                u32::try_from(self.label_arena.len()).expect("trie arena exceeds u32::MAX");
            let name_len = u16::try_from(part.len()).expect("part length exceeds u16::MAX");
            self.label_arena.push_str(part);
            self.nodes
                .push(BudgetTrieNode::new(name_offset, name_len, is_dir));
            self.last_children.push(NO_NODE);

            self.nodes[p].first_child = new_idx;
            self.last_children[p] = new_idx;
            return new_idx;
        }

        // Non-sequential / arbitrary insertion: search sibling chain
        let mut curr = self.nodes[p].first_child;
        let mut prev = NO_NODE;

        while curr != NO_NODE {
            let curr_node = self.nodes[curr as usize];
            let curr_name = &self.label_arena[curr_node.name_offset as usize
                ..(curr_node.name_offset + curr_node.name_len as u32) as usize];
            let ord = curr_name
                .cmp(part)
                .then_with(|| curr_node.is_dir.cmp(&is_dir));
            match ord {
                std::cmp::Ordering::Equal => return curr,
                std::cmp::Ordering::Greater => break,
                std::cmp::Ordering::Less => {
                    prev = curr;
                    curr = curr_node.next_sibling;
                }
            }
        }

        let new_idx = u32::try_from(self.nodes.len()).expect("trie node count exceeds u32::MAX");
        let name_offset =
            u32::try_from(self.label_arena.len()).expect("trie arena exceeds u32::MAX");
        let name_len = u16::try_from(part.len()).expect("part length exceeds u16::MAX");
        self.label_arena.push_str(part);
        let mut new_node = BudgetTrieNode::new(name_offset, name_len, is_dir);
        new_node.next_sibling = curr;
        self.nodes.push(new_node);
        self.last_children.push(NO_NODE);

        if prev == NO_NODE {
            self.nodes[p].first_child = new_idx;
        } else {
            self.nodes[prev as usize].next_sibling = new_idx;
        }
        if curr == NO_NODE {
            self.last_children[p] = new_idx;
        }

        new_idx
    }

    pub fn render_to_writer<W: Write + ?Sized>(
        &self,
        writer: &mut W,
        escape_xml: bool,
    ) -> std::io::Result<()> {
        let mut prefix = String::new();
        let first_child = self.nodes.first().map(|n| n.first_child).unwrap_or(NO_NODE);
        if first_child != NO_NODE {
            self.render_sibling_chain(writer, first_child, &mut prefix, escape_xml)?;
        }
        Ok(())
    }

    fn render_sibling_chain<W: Write + ?Sized>(
        &self,
        writer: &mut W,
        first_child: u32,
        prefix: &mut String,
        escape_xml: bool,
    ) -> std::io::Result<()> {
        const BRANCH_STRS: [&[u8]; 2] = [TREE_BRANCH.as_bytes(), TREE_LAST.as_bytes()];
        const PREFIX_STRS: [&str; 2] = [TREE_VERT, TREE_SPACE];
        const SUFFIXES: [&[u8]; 2] = [b"\n", b"/\n"];

        let mut curr = first_child;
        while curr != NO_NODE {
            let node = &self.nodes[curr as usize];
            let is_last = node.next_sibling == NO_NODE;

            writer.write_all(prefix.as_bytes())?;
            writer.write_all(BRANCH_STRS[is_last as usize])?;
            let name = self.node_name(curr);
            let display = tree_display(name, escape_xml);
            writer.write_all(display.as_bytes())?;
            writer.write_all(SUFFIXES[node.is_dir as usize])?;

            if node.first_child != NO_NODE {
                let prev_len = prefix.len();
                prefix.push_str(PREFIX_STRS[is_last as usize]);
                self.render_sibling_chain(writer, node.first_child, prefix, escape_xml)?;
                prefix.truncate(prev_len);
            }

            curr = node.next_sibling;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn renders_ascii_tree_correctly() -> Result<(), Box<dyn std::error::Error>> {
        let mut trie = BudgetTrie::new();
        for path in ["Cargo.toml", "src/core/a.rs", "src/lib.rs"] {
            trie.insert(path);
        }
        let mut buf = Vec::new();
        trie.render_to_writer(&mut buf, false)?;
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
        let mut trie = BudgetTrie::new();
        trie.insert("dir\nname/file\rname.txt");
        let mut out = Vec::new();
        trie.render_to_writer(&mut out, false)?;
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("dir\\nname/"));
        assert!(s.contains("file\\rname.txt"));
        assert!(!s.contains("dir\n"));
        Ok(())
    }

    #[test]
    fn escapes_tree_node_xml_entities() -> std::io::Result<()> {
        let mut trie = BudgetTrie::new();
        trie.insert("a & b/c < d.txt");
        let mut out = Vec::new();
        trie.render_to_writer(&mut out, true)?;
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("a &amp; b/"));
        assert!(s.contains("c &lt; d.txt"));
        Ok(())
    }

    #[test]
    fn test_trie_node_display_and_storage_optimization() {
        use std::borrow::Cow;

        assert_eq!(std::mem::size_of::<BudgetTrieNode>(), 16);

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

        let mut trie = BudgetTrie::new();
        trie.insert("clean/dir/file.rs");
        let clean_dir_idx = trie.nodes[0].first_child;
        assert_eq!(trie.node_name(clean_dir_idx), "clean");

        let mut trie_esc = BudgetTrie::new();
        trie_esc.insert("a & b/c.rs");
        let esc_dir_idx = trie_esc.nodes[0].first_child;
        assert_eq!(trie_esc.node_name(esc_dir_idx), "a & b");
    }

    #[test]
    fn test_flat_arena_links() {
        assert_eq!(std::mem::size_of::<BudgetTrieNode>(), 16);
        let mut trie = BudgetTrie::new();
        trie.insert("src/lib.rs");
        assert_eq!(trie.nodes.len(), 3);
        assert_eq!(trie.nodes[0].first_child, 1);
        assert_eq!(trie.node_name(1), "src");
        assert!(trie.nodes[1].is_dir);
        assert_eq!(trie.nodes[1].first_child, 2);

        assert_eq!(trie.node_name(2), "lib.rs");
        assert!(!trie.nodes[2].is_dir);
        assert_eq!(trie.nodes[2].first_child, NO_NODE);
        assert_eq!(trie.nodes[2].next_sibling, NO_NODE);
    }
}
