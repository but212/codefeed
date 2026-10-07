use crate::core::error::IngestError;
use crate::core::limits::MAX_OUTPUT_SIZE;
use crate::core::model::FileRecord;

const REPOSITORY_PREFIX: &str = "Repository: ";
const REVISION_PREFIX: &str = "Revision: ";
const FILES_PREFIX: &str = "Files: ";
const SKIPPED_PREFIX: &str = "\nSkipped: ";
const DIRECTORY_HEADER: &str = "Directory structure:\n";
const DIRECTORY_ENTRY_PREFIX: &str = "- ";
const FILES_HEADER: &str = "\nFiles:\n\n";
const FILE_BLOCK_PREFIX: &str = "==================================================\nFILE: ";
const FILE_BLOCK_SUFFIX: &str = "\n==================================================\n\n";
const NEWLINE: &str = "\n";
const METADATA_SUFFIX: &str = "\n\n";

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
            revision_bytes: revision
                .map(|value| byte_count(&escape_header(value)))
                .transpose()?,
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
        size = size.checked_add(REPOSITORY_PREFIX.len() as u64)?;
        size = size.checked_add(self.label_bytes)?;
        size = size.checked_add(NEWLINE.len() as u64)?;
        if let Some(revision_bytes) = self.revision_bytes {
            size = size.checked_add(REVISION_PREFIX.len() as u64)?;
            size = size.checked_add(revision_bytes)?;
            size = size.checked_add(NEWLINE.len() as u64)?;
        }
        size = size.checked_add(FILES_PREFIX.len() as u64)?;
        size = size.checked_add(self.file_count.to_string().len() as u64)?;
        size = size.checked_add(SKIPPED_PREFIX.len() as u64)?;
        size = size.checked_add(self.skipped.to_string().len() as u64)?;
        size = size.checked_add(METADATA_SUFFIX.len() as u64)?;
        size = size.checked_add(DIRECTORY_HEADER.len() as u64)?;
        size = size.checked_add(
            file_count.checked_mul((DIRECTORY_ENTRY_PREFIX.len() + NEWLINE.len()) as u64)?,
        )?;
        size = size.checked_add(self.path_bytes)?;
        size = size.checked_add(FILES_HEADER.len() as u64)?;
        let file_block_bytes = FILE_BLOCK_PREFIX.len() + FILE_BLOCK_SUFFIX.len();
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
    digest.push_str(REPOSITORY_PREFIX);
    digest.push_str(&escape_header(label));
    digest.push_str(NEWLINE);
    if let Some(revision) = revision {
        digest.push_str(REVISION_PREFIX);
        digest.push_str(&escape_header(revision));
        digest.push_str(NEWLINE);
    }
    digest.push_str(FILES_PREFIX);
    digest.push_str(&files.len().to_string());
    digest.push_str(SKIPPED_PREFIX);
    digest.push_str(&skipped.to_string());
    digest.push_str(METADATA_SUFFIX);
    let mut ordered_files: Vec<_> = files.iter().collect();
    ordered_files.sort_by(|left, right| left.path.cmp(&right.path));

    digest.push_str(DIRECTORY_HEADER);
    for file in &ordered_files {
        digest.push_str(DIRECTORY_ENTRY_PREFIX);
        digest.push_str(&escape_header(&file.path));
        digest.push_str(NEWLINE);
    }
    digest.push_str(FILES_HEADER);
    for file in ordered_files {
        digest.push_str(FILE_BLOCK_PREFIX);
        digest.push_str(&escape_header(&file.path));
        digest.push_str(FILE_BLOCK_SUFFIX);
        digest.push_str(&file.content);
        if !file.content.ends_with('\n') {
            digest.push_str(NEWLINE);
        }
        digest.push_str(NEWLINE);
    }
    Ok(digest)
}

fn escape_header(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::IngestError;
    use crate::core::limits::MAX_OUTPUT_SIZE;
    use crate::core::model::FileRecord;

    #[test]
    fn renders_files_in_deterministic_order() -> Result<(), Box<dyn std::error::Error>> {
        let files = vec![
            FileRecord {
                path: "z.txt".into(),
                content: "z".into(),
            },
            FileRecord {
                path: "a.txt".into(),
                content: "a".into(),
            },
        ];
        let digest = render_digest("repo", Some("abc"), &files, 1)?;
        let first = digest
            .find("FILE: a.txt")
            .ok_or_else(|| std::io::Error::other("a.txt is missing"))?;
        let second = digest
            .find("FILE: z.txt")
            .ok_or_else(|| std::io::Error::other("z.txt is missing"))?;
        assert!(first < second);
        assert!(digest.contains("Revision: abc"));
        assert!(digest.contains("Skipped: 1"));
        Ok(())
    }

    #[test]
    fn escapes_revision_header() -> Result<(), Box<dyn std::error::Error>> {
        let revision = "feature\\x\r\nFiles: 9";
        let rendered = render_digest("repo", Some(revision), &[], 0)?;

        assert_eq!(
            rendered.lines().nth(1),
            Some("Revision: feature\\\\x\\r\\nFiles: 9")
        );
        assert!(!rendered.contains(revision));
        Ok(())
    }

    #[test]
    fn digest_budget_matches_rendered_size() -> Result<(), Box<dyn std::error::Error>> {
        let files = vec![
            FileRecord {
                path: "z.txt".into(),
                content: "z\n".into(),
            },
            FileRecord {
                path: "a.txt".into(),
                content: "a".into(),
            },
        ];
        for revision in ["abc", "feature\\x\r\nFiles: 9"] {
            let mut budget = DigestBudget::new("repo", Some(revision))?;
            for file in &files {
                budget.add(file)?;
            }
            budget.skip()?;
            let rendered = render_digest("repo", Some(revision), &files, budget.skipped())?;

            assert_eq!(
                budget.rendered_size(),
                Some(rendered.len() as u64),
                "revision: {revision:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn rejects_digest_before_storing_over_budget_content() {
        let candidate = FileRecord {
            path: "large.txt".into(),
            content: "x".repeat(MAX_OUTPUT_SIZE as usize),
        };
        assert!(matches!(
            render_digest("repo", None, &[candidate], 0),
            Err(IngestError::OutputTooLarge)
        ));
    }
}
