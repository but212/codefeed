//! Single source of truth for ingestion policies, limits, and conventions.

pub const MAX_FILE_SIZE: u64 = 1024 * 1024;
pub const MAX_OUTPUT_SIZE: u64 = 20 * 1024 * 1024;
pub const MAX_FILE_COUNT: usize = 10_000;
pub const MAX_JSON_SIZE: u64 = 8 * 1024 * 1024;
pub const FETCH_WINDOW: usize = 16;

pub const NOISE: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "go.sum",
];

pub const BINARY_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "ico", "webp", "woff", "woff2", "ttf", "pdf", "zip",
];

pub(crate) fn is_noise_path(path: &str) -> bool {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    NOISE.contains(&file_name)
}

pub(crate) fn has_binary_extension(path: &str) -> bool {
    if let Some((_, ext)) = path.rsplit_once('.') {
        BINARY_EXT.iter().any(|&b| b.eq_ignore_ascii_case(ext))
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_noise_filenames() {
        assert!(is_noise_path("Cargo.lock"));
        assert!(is_noise_path("nested/dir/package-lock.json"));
        assert!(!is_noise_path("Cargo.toml"));
    }

    #[test]
    fn classifies_binary_extensions() {
        assert!(has_binary_extension("image.PNG"));
        assert!(has_binary_extension("assets/font.woff2"));
        assert!(!has_binary_extension("src/main.rs"));
        assert!(!has_binary_extension("Makefile"));
    }
}
