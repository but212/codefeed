use super::budget::{BoundedBuffer, OutputLimitExceeded};
use super::escape::{escape_xml_body, escape_xml_single_line, write_escaped};
use super::tree::render_tree;
use crate::core::model::CollectionResult;

const XML_REPO_START: &str = "<repository name=\"";
const XML_REVISION_ATTR: &str = "\" revision=\"";
const XML_TAG_CLOSE: &str = "\">\n";
const XML_DIR_START: &str = "<directory_structure>\n";
const XML_DIR_END: &str = "</directory_structure>\n";
const XML_FILES_START: &str = "<files>\n";
const XML_FILES_END: &str = "</files>\n";
const XML_REPO_END: &str = "</repository>\n";
const XML_FILE_START: &str = "<file path=\"";
const FILE_TAILS: [&[u8]; 2] = [b"</file>\n", b"\n</file>\n"];

pub(super) fn render_xml(
    mut buf: BoundedBuffer,
    collection: &CollectionResult,
) -> Result<BoundedBuffer, OutputLimitExceeded> {
    buf = buf.push_bytes(XML_REPO_START.as_bytes())?;
    buf = write_escaped(buf, &collection.label, escape_xml_single_line)?;
    if let Some(ref revision) = collection.revision {
        buf = buf.push_bytes(XML_REVISION_ATTR.as_bytes())?;
        buf = write_escaped(buf, revision, escape_xml_single_line)?;
    }
    buf = buf.push_bytes(XML_TAG_CLOSE.as_bytes())?;
    buf = buf.push_bytes(XML_DIR_START.as_bytes())?;
    buf = render_tree(buf, &collection.files, |b, name| {
        write_escaped(b, name, escape_xml_single_line)
    })?;
    buf = buf.push_bytes(XML_DIR_END.as_bytes())?;
    buf = buf.push_bytes(XML_FILES_START.as_bytes())?;
    for file in collection.files.iter() {
        buf = buf.push_bytes(XML_FILE_START.as_bytes())?;
        buf = write_escaped(buf, file.path, escape_xml_single_line)?;
        buf = buf.push_bytes(XML_TAG_CLOSE.as_bytes())?;
        buf = write_escaped(buf, file.content, escape_xml_body)?;
        let needs_nl = (!file.content.ends_with('\n')) as usize;
        buf = buf.push_bytes(FILE_TAILS[needs_nl])?;
    }
    buf = buf.push_bytes(XML_FILES_END.as_bytes())?;
    buf = buf.push_bytes(XML_REPO_END.as_bytes())?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::storage::{FileRecord, FileStorage, SortedFiles};

    #[test]
    fn renders_xml_in_deterministic_order() -> Result<(), Box<dyn std::error::Error>> {
        let files = [
            FileRecord {
                path: "z.txt",
                content: "z < 10",
            },
            FileRecord {
                path: "a.txt",
                content: "a & b",
            },
        ];
        let storage: FileStorage = files.into_iter().collect();
        let collection = CollectionResult {
            label: "my-repo".into(),
            revision: Some("v1.0".into()),
            files: SortedFiles::new(storage),
            skipped: 1,
        };
        let buf = render_xml(BoundedBuffer::default(), &collection)?;
        let digest = buf.finish();
        let xml_str = std::str::from_utf8(&digest.bytes)?;
        let expected = "\
<repository name=\"my-repo\" revision=\"v1.0\">
<directory_structure>
├── a.txt
└── z.txt
</directory_structure>
<files>
<file path=\"a.txt\">
a &amp; b
</file>
<file path=\"z.txt\">
z &lt; 10
</file>
</files>
</repository>
";
        assert_eq!(xml_str, expected);
        Ok(())
    }

    #[test]
    fn xml_attr_escapes_whitespace_and_preserves_body() -> Result<(), Box<dyn std::error::Error>> {
        let files = [FileRecord {
            path: "dir\nname\r\t\\back/file.rs",
            content: "fn main() {\n\tprintln!(\"hello\");\r\n}\n",
        }];
        let storage: FileStorage = files.into_iter().collect();
        let collection = CollectionResult {
            label: "my\trepo\n".into(),
            revision: Some("rev\r\n1".into()),
            files: SortedFiles::new(storage),
            skipped: 0,
        };
        let buf = render_xml(BoundedBuffer::default(), &collection)?;
        let digest = buf.finish();
        let xml_str = std::str::from_utf8(&digest.bytes)?;

        assert!(
            xml_str.contains("<repository name=\"my&#9;repo&#10;\" revision=\"rev&#13;&#10;1\">")
        );
        assert!(xml_str.contains("<file path=\"dir&#10;name&#13;&#9;\\back/file.rs\">"));
        assert!(xml_str.contains("dir&#10;name&#13;&#9;\\back/"));
        assert!(xml_str.contains("fn main() {\n\tprintln!(&quot;hello&quot;);\r\n}\n"));
        Ok(())
    }
}
