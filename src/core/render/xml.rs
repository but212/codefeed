//! XML format serializer.

use super::escape::{escape_xml_body, escape_xml_single_line, write_escaped};
use super::tree::render_tree;
use crate::core::model::CollectionResult;
use std::io::Write;

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

pub(super) fn render_xml<W: Write + ?Sized>(
    writer: &mut W,
    collection: &CollectionResult,
) -> std::io::Result<()> {
    writer.write_all(XML_REPO_START.as_bytes())?;
    write_escaped(writer, &collection.label, escape_xml_single_line)?;
    if let Some(ref revision) = collection.revision {
        writer.write_all(XML_REVISION_ATTR.as_bytes())?;
        write_escaped(writer, revision, escape_xml_single_line)?;
    }
    writer.write_all(XML_TAG_CLOSE.as_bytes())?;
    writer.write_all(XML_DIR_START.as_bytes())?;
    render_tree(writer, &collection.files, |w, name| {
        write_escaped(w, name, escape_xml_single_line)
    })?;
    writer.write_all(XML_DIR_END.as_bytes())?;
    writer.write_all(XML_FILES_START.as_bytes())?;
    for file in collection.files.iter() {
        writer.write_all(XML_FILE_START.as_bytes())?;
        write_escaped(writer, file.path, escape_xml_single_line)?;
        writer.write_all(XML_TAG_CLOSE.as_bytes())?;
        write_escaped(writer, file.content, escape_xml_body)?;
        let needs_nl = (!file.content.ends_with('\n')) as usize;
        writer.write_all(FILE_TAILS[needs_nl])?;
    }
    writer.write_all(XML_FILES_END.as_bytes())?;
    writer.write_all(XML_REPO_END.as_bytes())?;
    Ok(())
}
