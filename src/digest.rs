use crate::{IngestError, MAX_OUTPUT_SIZE};

#[derive(Debug)]
pub(crate) struct FileRecord {
    pub(crate) path: String,
    pub(crate) content: String,
}

#[derive(Debug, Clone)]
pub(crate) struct DigestBudget {
    label_bytes: u64,
    revision_bytes: Option<u64>,
    file_count: usize,
    skipped: usize,
    path_bytes: u64,
    content_bytes: u64,
    trailing_bytes: u64,
}

impl DigestBudget {
    pub(crate) fn new(label: &str, revision: Option<&str>) -> Result<Self, IngestError> {
        Ok(Self {
            label_bytes: byte_count(&escape_header(label))?,
            revision_bytes: revision.map(byte_count).transpose()?,
            file_count: 0,
            skipped: 0,
            path_bytes: 0,
            content_bytes: 0,
            trailing_bytes: 0,
        })
    }

    pub(crate) fn add(&mut self, file: &FileRecord) -> Result<(), IngestError> {
        let mut next = self.clone();
        next.file_count = next
            .file_count
            .checked_add(1)
            .ok_or_else(|| IngestError::InvalidMetadata("file count overflow".into()))?;
        next.add_file_bytes(file)?;
        next.ensure_fits()?;
        *self = next;
        Ok(())
    }

    pub(crate) fn skip(&mut self) -> Result<(), IngestError> {
        self.skipped = self
            .skipped
            .checked_add(1)
            .ok_or_else(|| IngestError::InvalidMetadata("skipped file count overflow".into()))?;
        Ok(())
    }

    pub(crate) fn skipped(&self) -> usize {
        self.skipped
    }

    #[cfg(test)]
    pub(crate) fn size(&self) -> Option<u64> {
        self.rendered_size()
    }

    fn add_file_bytes(&mut self, file: &FileRecord) -> Result<(), IngestError> {
        let path_bytes = byte_count(&escape_header(&file.path))?;
        self.path_bytes = self
            .path_bytes
            .checked_add(path_bytes)
            .ok_or(IngestError::OutputTooLarge)?;
        self.content_bytes = self
            .content_bytes
            .checked_add(byte_count(&file.content)?)
            .ok_or(IngestError::OutputTooLarge)?;
        self.trailing_bytes = self
            .trailing_bytes
            .checked_add((usize::from(!file.content.ends_with('\n')) + 1) as u64)
            .ok_or(IngestError::OutputTooLarge)?;
        Ok(())
    }

    fn ensure_fits(&self) -> Result<(), IngestError> {
        if self
            .rendered_size()
            .is_none_or(|size| size > MAX_OUTPUT_SIZE)
        {
            return Err(IngestError::OutputTooLarge);
        }
        Ok(())
    }

    fn rendered_size(&self) -> Option<u64> {
        let file_count = u64::try_from(self.file_count).ok()?;
        let mut size = 0_u64;
        size = size.checked_add("Repository: ".len() as u64)?;
        size = size.checked_add(self.label_bytes)?;
        size = size.checked_add(1)?;
        if let Some(revision_bytes) = self.revision_bytes {
            size = size.checked_add("Revision: ".len() as u64)?;
            size = size.checked_add(revision_bytes)?;
            size = size.checked_add(1)?;
        }
        size = size.checked_add("Files: ".len() as u64)?;
        size = size.checked_add(self.file_count.to_string().len() as u64)?;
        size = size.checked_add("\nSkipped: ".len() as u64)?;
        size = size.checked_add(self.skipped.to_string().len() as u64)?;
        size = size.checked_add(2)?;
        size = size.checked_add("Directory structure:\n".len() as u64)?;
        size = size.checked_add(file_count.checked_mul(3)?)?;
        size = size.checked_add(self.path_bytes)?;
        size = size.checked_add("\nFiles:\n\n".len() as u64)?;
        let file_block_bytes = "==================================================\nFILE: ".len()
            + "\n==================================================\n\n".len();
        size = size.checked_add(file_count.checked_mul(file_block_bytes as u64)?)?;
        size = size.checked_add(self.path_bytes)?;
        size = size.checked_add(self.content_bytes)?;
        size.checked_add(self.trailing_bytes)
    }
}

fn byte_count(value: &str) -> Result<u64, IngestError> {
    u64::try_from(value.len()).map_err(|_| IngestError::OutputTooLarge)
}

pub(crate) fn render_digest(
    label: &str,
    revision: Option<&str>,
    files: &[FileRecord],
    skipped: usize,
) -> Result<String, IngestError> {
    let mut budget = DigestBudget::new(label, revision)?;
    budget.skipped = skipped;
    for file in files {
        budget.add(file)?;
    }
    budget.ensure_fits()?;
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
    value
        .replace('\\', "\\\\")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}
