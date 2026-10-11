//! In-memory directory tree representation and deterministic ASCII tree rendering.

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
    pub(crate) display: String,
    pub(crate) is_dir: bool,
    pub(crate) children: Vec<BudgetTrieNode>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BudgetTrie {
    pub(crate) escape_xml: bool,
    pub(crate) node_count: usize,
    pub(crate) display_bytes: u64,
    pub(crate) children: Vec<BudgetTrieNode>,
}

#[derive(Debug, Default)]
pub(crate) struct TrieUndo {
    pub(crate) actions: Vec<Vec<usize>>,
}

fn tree_display(name: &str, escape_xml: bool) -> String {
    let escaped = escape_header(name);
    if escape_xml {
        xml_escape(&escaped).into_owned()
    } else {
        escaped.into_owned()
    }
}

impl BudgetTrie {
    pub(crate) fn new(escape_xml: bool) -> Self {
        Self {
            escape_xml,
            node_count: 0,
            display_bytes: 0,
            children: Vec::new(),
        }
    }

    pub(crate) fn insert_with_undo(&mut self, path: &str) -> TrieUndo {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut undo = TrieUndo::default();
        if parts.is_empty() {
            return undo;
        }
        let mut current_indices = Vec::new();
        Self::insert_into_with_undo(
            &mut self.children,
            &parts,
            &mut current_indices,
            &mut undo,
            self.escape_xml,
            &mut self.node_count,
            &mut self.display_bytes,
        );
        undo
    }

    fn insert_into_with_undo(
        children: &mut Vec<BudgetTrieNode>,
        parts: &[&str],
        current_indices: &mut Vec<usize>,
        undo: &mut TrieUndo,
        escape_xml: bool,
        node_count: &mut usize,
        display_bytes: &mut u64,
    ) {
        if parts.is_empty() {
            return;
        }
        let part = parts[0];
        let is_dir = parts.len() > 1;

        let (idx, created) = match children.binary_search_by(|c| {
            c.name
                .as_str()
                .cmp(part)
                .then_with(|| c.is_dir.cmp(&is_dir))
        }) {
            Ok(idx) => (idx, false),
            Err(idx) => {
                let display = tree_display(part, escape_xml);
                *display_bytes += display.len() as u64;
                *node_count += 1;
                children.insert(
                    idx,
                    BudgetTrieNode {
                        name: part.to_string(),
                        display,
                        is_dir,
                        children: Vec::new(),
                    },
                );
                (idx, true)
            }
        };

        current_indices.push(idx);

        if created {
            undo.actions.push(current_indices.clone());
        }

        if is_dir {
            Self::insert_into_with_undo(
                &mut children[idx].children,
                &parts[1..],
                current_indices,
                undo,
                escape_xml,
                node_count,
                display_bytes,
            );
        }

        current_indices.pop();
    }

    pub(crate) fn rollback(&mut self, undo: TrieUndo) {
        for path_indices in undo.actions.into_iter().rev() {
            Self::remove_node_at(
                &mut self.children,
                &path_indices,
                &mut self.node_count,
                &mut self.display_bytes,
            );
        }
    }

    fn remove_node_at(
        mut nodes: &mut Vec<BudgetTrieNode>,
        path_indices: &[usize],
        node_count: &mut usize,
        display_bytes: &mut u64,
    ) {
        if path_indices.is_empty() {
            return;
        }
        for &idx in &path_indices[..path_indices.len() - 1] {
            if idx >= nodes.len() {
                return;
            }
            nodes = &mut nodes[idx].children;
        }
        let last_idx = path_indices[path_indices.len() - 1];
        if last_idx < nodes.len() {
            let removed = nodes.remove(last_idx);
            *display_bytes -= removed.display.len() as u64;
            *node_count -= 1;
        }
    }

    pub(crate) fn calculate_size(&self) -> u64 {
        Self::calculate_nodes_size(&self.children, 0)
    }

    fn calculate_nodes_size(nodes: &[BudgetTrieNode], prefix_len: u64) -> u64 {
        let mut total = 0_u64;
        let count = nodes.len();
        for (idx, node) in nodes.iter().enumerate() {
            let is_last = idx + 1 == count;
            total += prefix_len + 10;
            total += node.display.len() as u64;
            if node.is_dir {
                total += 2;
            } else {
                total += 1;
            }
            if !node.children.is_empty() {
                let child_prefix_len = if is_last {
                    prefix_len + 4
                } else {
                    prefix_len + 6
                };
                total += Self::calculate_nodes_size(&node.children, child_prefix_len);
            }
        }
        total
    }

    pub(crate) fn render_to_writer<W: Write + ?Sized>(
        &self,
        writer: &mut W,
    ) -> std::io::Result<()> {
        Self::render_nodes(writer, &self.children, "")
    }

    fn render_nodes<W: Write + ?Sized>(
        writer: &mut W,
        nodes: &[BudgetTrieNode],
        prefix: &str,
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
            writer.write_all(node.display.as_bytes())?;
            if node.is_dir {
                writer.write_all(b"/")?;
            }
            writer.write_all(b"\n")?;
            if !node.children.is_empty() {
                let child_prefix = if is_last {
                    format!("{prefix}{TREE_SPACE}")
                } else {
                    format!("{prefix}{TREE_VERT}")
                };
                Self::render_nodes(writer, &node.children, &child_prefix)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_ascii_tree_correctly() -> Result<(), Box<dyn std::error::Error>> {
        let mut trie = BudgetTrie::default();
        for path in ["Cargo.toml", "src/core/a.rs", "src/lib.rs"] {
            trie.insert_with_undo(path);
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
    fn trie_budget_matches_rendered_size() -> Result<(), Box<dyn std::error::Error>> {
        let test_cases = vec![
            vec![
                "Cargo.toml",
                "src/lib.rs",
                "src/core/digest.rs",
                "src/core/model.rs",
            ],
            vec!["a.txt", "b/c.txt", "b/d/e.txt", "f/g/h/i.txt"],
            vec![
                "file & with <special> \"chars'.txt",
                "nested & dir/sub <dir>/test.rs",
            ],
        ];

        for paths in test_cases {
            for escape_xml in [false, true] {
                let mut trie = BudgetTrie::new(escape_xml);
                for p in &paths {
                    trie.insert_with_undo(p);
                }

                let trie_size = trie.calculate_size();
                let mut rendered = Vec::new();
                trie.render_to_writer(&mut rendered)?;
                assert_eq!(
                    trie_size,
                    rendered.len() as u64,
                    "Trie size must match rendered tree bytes exactly (escape_xml: {escape_xml})"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn trie_rollback_restores_exact_state() {
        let mut trie = BudgetTrie::default();
        trie.insert_with_undo("src/core/digest.rs");
        let initial_size = trie.calculate_size();

        let undo = trie.insert_with_undo("src/core/sub/nested/file.rs");
        let modified_size = trie.calculate_size();
        assert!(modified_size > initial_size);

        trie.rollback(undo);
        assert_eq!(trie.calculate_size(), initial_size);
    }

    #[test]
    fn escapes_tree_node_newlines() -> std::io::Result<()> {
        let mut trie = BudgetTrie::new(false);
        trie.insert_with_undo("dir\nname/file\rname.txt");
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
        trie.insert_with_undo("a & b/c < d.txt");
        let mut out = Vec::new();
        trie.render_to_writer(&mut out)?;
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("a &amp; b/"));
        assert!(s.contains("c &lt; d.txt"));
        Ok(())
    }
}
