//! In-memory directory tree representation and deterministic ASCII tree rendering.
//!
//! Provides a compact 16-byte fixed-size Left-Child Right-Sibling (LCRS) arena
//! representation of file hierarchy (`BudgetTrie`) and zero-allocation deterministic
//! ASCII tree rendering with format-aware escaping.

use std::borrow::Cow;
use std::io::Write;

use crate::core::policy::{MAX_COMPONENT_LEN, find_overlong_component};

const TREE_BRANCH: &str = "├── ";
const TREE_LAST: &str = "└── ";
const TREE_VERT: &str = "│   ";
const TREE_SPACE: &str = "    ";
const FLAG_DIR: u16 = 1 << 0;

const _: () = assert!(std::mem::size_of::<BudgetTrieNode>() == 16);

pub(crate) const NO_NODE: u32 = u32::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsertError {
    ComponentTooLong { len: usize },
}

impl std::fmt::Display for InsertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ComponentTooLong { len } => write!(
                f,
                "path component is {len} bytes; maximum is {MAX_COMPONENT_LEN}"
            ),
        }
    }
}

impl std::error::Error for InsertError {}

/// 16-byte fixed-size trie node in a Left-Child Right-Sibling (LCRS) flat arena.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BudgetTrieNode {
    pub(crate) name_offset: u32,
    pub(crate) name_len: u16,
    pub(crate) flags: u16,
    pub(crate) first_child: u32,  // NO_NODE means None
    pub(crate) next_sibling: u32, // NO_NODE means None
}

impl BudgetTrieNode {
    pub(crate) const fn new(name_offset: u32, name_len: u16, is_dir: bool) -> Self {
        Self {
            name_offset,
            name_len,
            flags: if is_dir { FLAG_DIR } else { 0 },
            first_child: NO_NODE,
            next_sibling: NO_NODE,
        }
    }

    pub(crate) const fn is_dir(&self) -> bool {
        (self.flags & FLAG_DIR) != 0
    }

    #[inline(always)]
    pub(crate) fn name_bytes<'a>(&self, arena: &'a [u8]) -> &'a [u8] {
        let start = self.name_offset as usize;
        let end = start + self.name_len as usize;
        &arena[start..end]
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

impl BudgetTrie {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub(crate) fn node_name(&self, index: u32) -> &str {
        let node = &self.nodes[index as usize];
        let start = node.name_offset as usize;
        let end = start + node.name_len as usize;
        &self.label_arena[start..end]
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn node_name_bytes(&self, index: u32) -> &[u8] {
        let node = &self.nodes[index as usize];
        node.name_bytes(self.label_arena.as_bytes())
    }

    pub fn insert(&mut self, path: &str) -> Result<(), InsertError> {
        if let Some(len) = find_overlong_component(path) {
            return Err(InsertError::ComponentTooLong { len });
        }

        let mut parts = path.split('/').filter(|s| !s.is_empty()).peekable();
        let mut current_node_index: u32 = 0;

        while let Some(part) = parts.next() {
            let is_dir = parts.peek().is_some();
            current_node_index = self.get_or_insert_child(current_node_index, part, is_dir);
        }
        Ok(())
    }

    fn push_node(&mut self, part: &str, is_dir: bool, next_sibling: u32) -> u32 {
        let new_index = u32::try_from(self.nodes.len()).expect("trie node count exceeds u32::MAX");
        let name_offset =
            u32::try_from(self.label_arena.len()).expect("trie arena exceeds u32::MAX");
        let name_len = part.len() as u16;
        self.label_arena.push_str(part);
        let mut node = BudgetTrieNode::new(name_offset, name_len, is_dir);
        node.next_sibling = next_sibling;
        self.nodes.push(node);
        self.last_children.push(NO_NODE);
        new_index
    }

    fn get_or_insert_child(&mut self, parent_index: u32, part: &str, is_dir: bool) -> u32 {
        let parent_node = parent_index as usize;
        let last_child_index = self.last_children[parent_node];
        let part_bytes = part.as_bytes();

        if last_child_index != NO_NODE {
            let last_node = self.nodes[last_child_index as usize];
            let last_name = last_node.name_bytes(self.label_arena.as_bytes());
            let ordering = last_name
                .cmp(part_bytes)
                .then_with(|| last_node.is_dir().cmp(&is_dir));
            if ordering == std::cmp::Ordering::Equal {
                return last_child_index;
            }
            if ordering == std::cmp::Ordering::Less {
                // Sequential append (presorted fast path): append after last_child
                let new_index = self.push_node(part, is_dir, NO_NODE);
                self.nodes[last_child_index as usize].next_sibling = new_index;
                self.last_children[parent_node] = new_index;
                return new_index;
            }
        } else {
            // First child for parent
            let new_index = self.push_node(part, is_dir, NO_NODE);
            self.nodes[parent_node].first_child = new_index;
            self.last_children[parent_node] = new_index;
            return new_index;
        }

        // Non-sequential / arbitrary insertion: search sibling chain
        let mut curr = self.nodes[parent_node].first_child;
        let mut prev = NO_NODE;

        let arena_bytes = self.label_arena.as_bytes();
        while curr != NO_NODE {
            let curr_node = self.nodes[curr as usize];
            let curr_name = curr_node.name_bytes(arena_bytes);
            let ord = curr_name
                .cmp(part_bytes)
                .then_with(|| curr_node.is_dir().cmp(&is_dir));
            match ord {
                std::cmp::Ordering::Equal => return curr,
                std::cmp::Ordering::Greater => break,
                std::cmp::Ordering::Less => {
                    prev = curr;
                    curr = curr_node.next_sibling;
                }
            }
        }

        let new_index = self.push_node(part, is_dir, curr);
        if prev == NO_NODE {
            self.nodes[parent_node].first_child = new_index;
        } else {
            self.nodes[prev as usize].next_sibling = new_index;
        }
        if curr == NO_NODE {
            self.last_children[parent_node] = new_index;
        }

        new_index
    }

    pub fn render_to_writer<W: Write + ?Sized, F>(
        &self,
        writer: &mut W,
        format_name: F,
    ) -> std::io::Result<()>
    where
        F: Fn(&str) -> Cow<'_, str>,
    {
        let mut prefix = String::new();
        let first_child = self.nodes.first().map(|n| n.first_child).unwrap_or(NO_NODE);
        if first_child != NO_NODE {
            self.render_sibling_chain(writer, first_child, &mut prefix, &format_name)?;
        }
        Ok(())
    }

    fn render_sibling_chain<W: Write + ?Sized, F>(
        &self,
        writer: &mut W,
        first_child: u32,
        prefix: &mut String,
        format_name: &F,
    ) -> std::io::Result<()>
    where
        F: Fn(&str) -> Cow<'_, str>,
    {
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
            let display = format_name(name);
            writer.write_all(display.as_bytes())?;
            writer.write_all(SUFFIXES[node.is_dir() as usize])?;

            if node.first_child != NO_NODE {
                let prev_len = prefix.len();
                prefix.push_str(PREFIX_STRS[is_last as usize]);
                self.render_sibling_chain(writer, node.first_child, prefix, format_name)?;
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
            trie.insert(path).unwrap();
        }
        let mut buf = Vec::new();
        trie.render_to_writer(&mut buf, |s| Cow::Borrowed(s))?;
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
        let mut trie = BudgetTrie::new();
        trie.insert("dir_name/file_name.txt").unwrap();
        let mut out = Vec::new();
        trie.render_to_writer(&mut out, |name| Cow::Owned(name.to_uppercase()))?;
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("DIR_NAME/"));
        assert!(s.contains("FILE_NAME.TXT"));
        Ok(())
    }

    #[test]
    fn test_trie_node_storage_optimization() {
        assert_eq!(std::mem::size_of::<BudgetTrieNode>(), 16);

        let mut trie = BudgetTrie::new();
        trie.insert("clean/dir/file.rs").unwrap();
        let clean_dir_index = trie.nodes[0].first_child;
        assert_eq!(trie.node_name(clean_dir_index), "clean");

        let mut trie_esc = BudgetTrie::new();
        trie_esc.insert("a & b/c.rs").unwrap();
        let esc_dir_index = trie_esc.nodes[0].first_child;
        assert_eq!(trie_esc.node_name(esc_dir_index), "a & b");
    }

    #[test]
    fn test_flat_arena_links() {
        assert_eq!(std::mem::size_of::<BudgetTrieNode>(), 16);
        let mut trie = BudgetTrie::new();
        trie.insert("src/lib.rs").unwrap();
        assert_eq!(trie.nodes.len(), 3);
        assert_eq!(trie.nodes[0].first_child, 1);
        assert_eq!(trie.node_name(1), "src");
        assert!(trie.nodes[1].is_dir());
        assert_eq!(trie.nodes[1].first_child, 2);

        assert_eq!(trie.node_name(2), "lib.rs");
        assert!(!trie.nodes[2].is_dir());
        assert_eq!(trie.nodes[2].first_child, NO_NODE);
        assert_eq!(trie.nodes[2].next_sibling, NO_NODE);
    }

    #[test]
    fn test_insert_rejects_overlong_component() {
        let mut trie = BudgetTrie::new();
        let too_long = u16::MAX as usize + 1;
        let path = format!("{}/file.rs", "a".repeat(too_long));

        let err = trie.insert(&path).unwrap_err();
        assert_eq!(err, InsertError::ComponentTooLong { len: too_long });
        assert_eq!(trie.nodes.len(), 1);
    }

    #[test]
    fn test_insert_accepts_max_length_component() {
        let mut trie = BudgetTrie::new();
        let max = "a".repeat(MAX_COMPONENT_LEN);
        trie.insert(&format!("{max}/file.rs")).unwrap();
        let child = trie.nodes[0].first_child;
        assert_eq!(trie.node_name(child).len(), MAX_COMPONENT_LEN);
    }

    #[test]
    fn test_trie_node_name_bytes_parity() {
        let mut trie = BudgetTrie::new();
        trie.insert("foo/bar.rs").unwrap();
        let child = trie.nodes[0].first_child;
        assert_eq!(
            trie.node_name_bytes(child),
            trie.node_name(child).as_bytes()
        );
    }
}
