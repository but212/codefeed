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
    pub(crate) upper_bound_bytes: u64,
    pub(crate) children: Vec<BudgetTrieNode>,
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
            upper_bound_bytes: 0,
            children: Vec::new(),
        }
    }

    pub(crate) fn insert(&mut self, path: &str) {
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            return;
        }
        Self::insert_into(
            &mut self.children,
            &parts,
            self.escape_xml,
            &mut self.node_count,
            &mut self.display_bytes,
            &mut self.upper_bound_bytes,
            0,
        );
    }

    fn insert_into(
        children: &mut Vec<BudgetTrieNode>,
        parts: &[&str],
        escape_xml: bool,
        node_count: &mut usize,
        display_bytes: &mut u64,
        upper_bound_bytes: &mut u64,
        depth: u64,
    ) {
        if parts.is_empty() {
            return;
        }
        let part = parts[0];
        let is_dir = parts.len() > 1;

        let idx = match children.binary_search_by(|c| {
            c.name
                .as_str()
                .cmp(part)
                .then_with(|| c.is_dir.cmp(&is_dir))
        }) {
            Ok(idx) => idx,
            Err(idx) => {
                let display = tree_display(part, escape_xml);
                let display_len = display.len() as u64;
                *display_bytes += display_len;
                *node_count += 1;
                // Maximum prefix cost: depth * 6 (each ancestor contributes up to 6 bytes: '│   ').
                // Branch connector: 10 bytes ('├── ' or '└── ').
                // Node terminator: 2 bytes ('/' + '\n') for dir or 1 byte ('\n') for file.
                // Maximum total formatted line cost for this node: 6 * depth + 12 + display.len().
                let node_max_format = depth.saturating_mul(6).saturating_add(12);
                let node_upper = node_max_format.saturating_add(display_len);
                *upper_bound_bytes = upper_bound_bytes.saturating_add(node_upper);

                children.insert(
                    idx,
                    BudgetTrieNode {
                        name: part.to_string(),
                        display,
                        is_dir,
                        children: Vec::new(),
                    },
                );
                idx
            }
        };

        if is_dir {
            Self::insert_into(
                &mut children[idx].children,
                &parts[1..],
                escape_xml,
                node_count,
                display_bytes,
                upper_bound_bytes,
                depth.saturating_add(1),
            );
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
                    trie.insert(p);
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
    fn trie_upper_bound_bytes_is_always_upper_bound() {
        // 1. Shallow and branching structures
        let cases = vec![
            vec!["a.txt"],
            vec!["a.txt", "b.txt", "c/d.txt", "c/e/f.txt"],
            vec!["x & y/z < w.txt", "x & y/a/b.txt"],
        ];
        for escape_xml in [false, true] {
            for paths in &cases {
                let mut trie = BudgetTrie::new(escape_xml);
                for p in paths {
                    trie.insert(p);
                }
                assert!(
                    trie.calculate_size() <= trie.upper_bound_bytes,
                    "calculate_size ({}) must be <= upper_bound_bytes ({})",
                    trie.calculate_size(),
                    trie.upper_bound_bytes
                );
            }

            // 2. Deep nested tree (1,800 depth)
            let deep_path = deep_test_path(1800);

            let mut deep_trie = BudgetTrie::new(escape_xml);
            deep_trie.insert(&deep_path);
            let calc = deep_trie.calculate_size();
            let upper = deep_trie.upper_bound_bytes;
            assert!(
                calc <= upper,
                "deep tree calculate_size ({calc}) must be <= upper_bound_bytes ({upper}) (escape_xml: {escape_xml})"
            );
        }
    }

    pub(crate) fn deep_test_path(depth: usize) -> String {
        let mut path = String::new();
        for i in 0..depth {
            use std::fmt::Write;
            let _ = write!(path, "d{i}/");
        }
        path.push_str("leaf.txt");
        path
    }
}
