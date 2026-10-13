//! Pure GitHub URL construction, percent-encoding, and origin classification
//! used to scope credentials and API-only headers.

const GITHUB_API_BASE_URL: &str = "https://api.github.com";

/// Origin of a request URL: which headers and credentials the shell may attach.
///
/// `Other` covers every non-API origin and host-confusion lookalikes
/// (`https://api.github.com.evil.example/...`, `https://api.github.com@evil.example/...`);
/// those requests must never carry `Authorization`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequestTarget {
    Api,
    Other,
}

pub(crate) fn request_target(url: &str) -> RequestTarget {
    if has_origin(url, GITHUB_API_BASE_URL) {
        RequestTarget::Api
    } else {
        RequestTarget::Other
    }
}

/// Exact-origin match: `url == base` or `url` starts with `base + "/"`.
/// A plain prefix check would also match `https://api.github.com.evil.example`.
fn has_origin(url: &str, base: &str) -> bool {
    url.strip_prefix(base)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

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

fn push_encoded_segment(url: &mut String, segment: &str) {
    url.push('/');
    append_encoded_segment(url, segment);
}

fn append_encoded_segment(url: &mut String, segment: &str) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            url.push(byte as char);
        } else {
            url.push('%');
            url.push(HEX[(byte >> 4) as usize] as char);
            url.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
}

#[cfg(test)]
fn encode_path_segment(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    append_encoded_segment(&mut encoded, segment);
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encodes_url_path_segments() {
        assert_eq!(encode_path_segment("AZaz09-._~"), "AZaz09-._~");
        assert_eq!(encode_path_segment("feature/x"), "feature%2Fx");
        assert_eq!(encode_path_segment("a b"), "a%20b");
        assert_eq!(encode_path_segment("100%"), "100%25");
        assert_eq!(encode_path_segment("a#b?c"), "a%23b%3Fc");
        assert_eq!(encode_path_segment("café"), "caf%C3%A9");
    }

    #[test]
    fn request_target_classifies_origins_correctly() {
        let cases = [
            (
                "https://api.github.com/repos/owner/repo",
                RequestTarget::Api,
            ),
            ("https://api.github.com", RequestTarget::Api),
            (
                "https://raw.githubusercontent.com/owner/repo/main/lib.rs",
                RequestTarget::Other,
            ),
            ("https://raw.githubusercontent.com", RequestTarget::Other),
            (
                "https://api.github.com.evil.example/x",
                RequestTarget::Other,
            ),
            (
                "https://api.github.com@evil.example/x",
                RequestTarget::Other,
            ),
            (
                "https://raw.githubusercontent.com.evil.example/x",
                RequestTarget::Other,
            ),
            (
                "https://raw.githubusercontent.com@evil.example/x",
                RequestTarget::Other,
            ),
            (
                "http://127.0.0.1:8080/repos/owner/repo",
                RequestTarget::Other,
            ),
            ("https://evil.example/", RequestTarget::Other),
        ];
        for (url, expected) in cases {
            assert_eq!(
                request_target(url),
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
