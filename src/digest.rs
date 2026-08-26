use crate::{IngestError, MAX_OUTPUT_SIZE};

#[derive(Debug)]
pub(crate) struct FileRecord {
    pub(crate) path: String,
    pub(crate) content: String,
}

pub(crate) fn ensure_digest_fits(
    label: &str,
    revision: Option<&str>,
    files: &[FileRecord],
    skipped: usize,
    candidate: Option<&FileRecord>,
) -> Result<(), IngestError> {
    if rendered_size(label, revision, files, skipped, candidate)
        .is_none_or(|size| size > MAX_OUTPUT_SIZE)
    {
        return Err(IngestError::OutputTooLarge);
    }
    Ok(())
}

fn rendered_size(
    label: &str,
    revision: Option<&str>,
    files: &[FileRecord],
    skipped: usize,
    candidate: Option<&FileRecord>,
) -> Option<u64> {
    let mut size = 0_u64;
    let mut add = |value: usize| {
        let Some(next) = size.checked_add(value as u64) else {
            return false;
        };
        size = next;
        true
    };
    if !add("Repository: ".len() + escape_header(label).len() + 1) {
        return None;
    }
    if let Some(revision) = revision
        && !add("Revision: ".len() + revision.len() + 1)
    {
        return None;
    }
    if !add("Files: ".len()
        + (files.len() + usize::from(candidate.is_some()))
            .to_string()
            .len()
        + "\nSkipped: ".len()
        + skipped.to_string().len()
        + 2)
    {
        return None;
    }
    if !add("Directory structure:\n".len()) {
        return None;
    }
    for file in files.iter().chain(candidate) {
        if !add(2 + escape_header(&file.path).len() + 1) {
            return None;
        }
    }
    if !add("\nFiles:\n\n".len()) {
        return None;
    }
    for file in files.iter().chain(candidate) {
        if !add("==================================================\nFILE: ".len())
            || !add(escape_header(&file.path).len())
            || !add("\n==================================================\n\n".len())
            || !add(file.content.len())
            || !add(usize::from(!file.content.ends_with('\n')) + 1)
        {
            return None;
        }
    }
    Some(size)
}

pub(crate) fn render_digest(
    label: &str,
    revision: Option<&str>,
    files: &[FileRecord],
    skipped: usize,
) -> Result<String, IngestError> {
    ensure_digest_fits(label, revision, files, skipped, None)?;
    let mut digest = String::new();
    digest.push_str(&format!("Repository: {}\n", escape_header(label)));
    if let Some(revision) = revision {
        digest.push_str(&format!("Revision: {revision}\n"));
    }
    digest.push_str(&format!("Files: {}\nSkipped: {skipped}\n\n", files.len()));
    let mut ordered_files: Vec<_> = files.iter().collect();
    ordered_files.sort_by(|left, right| left.path.cmp(&right.path));

    digest.push_str("Directory structure:\n");
    for file in &ordered_files {
        digest.push_str("- ");
        digest.push_str(&escape_header(&file.path));
        digest.push('\n');
    }
    digest.push_str("\nFiles:\n\n");
    for file in ordered_files {
        digest.push_str("==================================================\nFILE: ");
        digest.push_str(&escape_header(&file.path));
        digest.push_str("\n==================================================\n\n");
        digest.push_str(&file.content);
        if !file.content.ends_with('\n') {
            digest.push('\n');
        }
        digest.push('\n');
    }
    Ok(digest)
}

fn escape_header(value: &str) -> String {
    value.replace('\r', "\\r").replace('\n', "\\n")
}
