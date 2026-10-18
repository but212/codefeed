//! Pure GitHub URL construction, percent-encoding, and origin classification
//! used to scope credentials and API-only headers.

/// Canonical base URL for GitHub REST API v3 endpoints.
const GITHUB_API_BASE_URL: &str = "https://api.github.com";

/// Exact-origin match: returns `true` if `url == GITHUB_API_BASE_URL` or starts with `GITHUB_API_BASE_URL + "/"`.
pub(crate) fn is_github_api_origin(url: &str) -> bool {
    has_origin(url, GITHUB_API_BASE_URL)
}

/// Exact-origin match: `url == base` or `url` starts with `base + "/"`.
/// A plain prefix check would also match `https://api.github.com.evil.example`.
fn has_origin(url: &str, base: &str) -> bool {
    url.strip_prefix(base)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Constructs the tarball download URL for the given repository and optional Git reference.
pub(crate) fn tarball_url(owner: &str, repo: &str, reference: Option<&str>) -> String {
    let mut url = String::from(GITHUB_API_BASE_URL);
    for segment in ["repos", owner, repo, "tarball"] {
        push_encoded_segment(&mut url, segment);
    }
    if let Some(reference) = reference {
        push_encoded_segment(&mut url, reference);
    }
    url
}

#[inline(always)]
const fn is_unreserved(b: u8) -> bool {
    ((b | 32).wrapping_sub(b'a') < 26)
        | (b.wrapping_sub(b'0') < 10)
        | (b == b'-')
        | (b == b'.')
        | (b == b'_')
        | (b == b'~')
}

fn push_encoded_segment(url: &mut String, segment: &str) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    url.push('/');
    for &b in segment.as_bytes() {
        if is_unreserved(b) {
            url.push(b as char);
        } else {
            url.push('%');
            url.push(HEX[(b >> 4) as usize] as char);
            url.push(HEX[(b & 0xF) as usize] as char);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_segment(segment: &str) -> String {
        let mut out = String::new();
        push_encoded_segment(&mut out, segment);
        out.strip_prefix('/').unwrap().to_string()
    }

    #[test]
    fn percent_encodes_url_path_segments() {
        assert_eq!(encode_segment("AZaz09-._~"), "AZaz09-._~");
        assert_eq!(encode_segment("feature/x"), "feature%2Fx");
        assert_eq!(encode_segment("a b"), "a%20b");
        assert_eq!(encode_segment("100%"), "100%25");
        assert_eq!(encode_segment("a#b?c"), "a%23b%3Fc");
        assert_eq!(encode_segment("café"), "caf%C3%A9");
    }

    #[test]
    fn origin_classification_identifies_github_api_correctly() {
        let cases = [
            ("https://api.github.com/repos/owner/repo", true),
            ("https://api.github.com", true),
            (
                "https://raw.githubusercontent.com/owner/repo/main/lib.rs",
                false,
            ),
            ("https://raw.githubusercontent.com", false),
            ("https://api.github.com.evil.example/x", false),
            ("https://api.github.com@evil.example/x", false),
            ("https://raw.githubusercontent.com.evil.example/x", false),
            ("https://raw.githubusercontent.com@evil.example/x", false),
            ("http://127.0.0.1:8080/repos/owner/repo", false),
            ("https://evil.example/", false),
        ];
        for (url, expected) in cases {
            assert_eq!(
                is_github_api_origin(url),
                expected,
                "unexpected auth scope for: {url}"
            );
        }
    }

    #[test]
    fn builds_tarball_urls() {
        assert_eq!(
            tarball_url("rust-lang", "cargo", None),
            "https://api.github.com/repos/rust-lang/cargo/tarball"
        );
        assert_eq!(
            tarball_url("rust-lang", "cargo", Some("main")),
            "https://api.github.com/repos/rust-lang/cargo/tarball/main"
        );
        assert_eq!(
            tarball_url("owner", "repo", Some("feature/test")),
            "https://api.github.com/repos/owner/repo/tarball/feature%2Ftest"
        );
    }
}
