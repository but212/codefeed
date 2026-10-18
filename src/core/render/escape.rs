//! Single-pass escaping utilities for text and XML output formats.

use std::io::Write;

#[inline(always)]
pub(crate) fn escape_header(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    match bytes[i] {
        b'\\' => Some((1, "\\\\")),
        b'\r' => Some((1, "\\r")),
        b'\n' => Some((1, "\\n")),
        0x00..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F | 0x7F => Some((1, "\u{FFFD}")),
        _ => None,
    }
}

#[inline(always)]
pub(crate) fn escape_xml_body(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    match bytes[i] {
        0..=8 | 11 | 12 | 14..=31 => Some((1, "\u{FFFD}")),
        b'"' => Some((1, "&quot;")),
        b'&' => Some((1, "&amp;")),
        b'\'' => Some((1, "&apos;")),
        b'<' => Some((1, "&lt;")),
        b'>' => Some((1, "&gt;")),
        0xEF if i + 2 < bytes.len()
            && bytes[i + 1] == 0xBF
            && (bytes[i + 2] == 0xBE || bytes[i + 2] == 0xBF) =>
        {
            Some((3, "\u{FFFD}"))
        }
        _ => None,
    }
}

#[inline(always)]
pub(crate) fn escape_xml_single_line(bytes: &[u8], i: usize) -> Option<(usize, &'static str)> {
    match bytes[i] {
        b'\t' => Some((1, "&#9;")),
        b'\n' => Some((1, "&#10;")),
        b'\r' => Some((1, "&#13;")),
        _ => escape_xml_body(bytes, i),
    }
}

pub(crate) fn write_escaped<W, F>(writer: &mut W, value: &str, special_at: F) -> std::io::Result<()>
where
    W: Write + ?Sized,
    F: Fn(&[u8], usize) -> Option<(usize, &'static str)>,
{
    let bytes = value.as_bytes();
    let mut last = 0;
    let mut i = 0;
    while i < bytes.len() {
        if let Some((consumed, esc)) = special_at(bytes, i) {
            writer.write_all(&bytes[last..i])?;
            writer.write_all(esc.as_bytes())?;
            i += consumed;
            last = i;
        } else {
            i += 1;
        }
    }
    writer.write_all(&bytes[last..])
}
