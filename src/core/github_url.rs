//! Pure GitHub URL construction, percent-encoding, and origin classification
//! used to scope credentials and API-only headers.

const GITHUB_API_BASE_URL: &str = "https://api.github.com";
const GITHUB_RAW_BASE_URL: &str = "https://raw.githubusercontent.com";

/// Origin of a request URL: which headers and credentials the shell may attach.
///
/// `Other` covers every non-GitHub origin and host-confusion lookalikes
/// (`https://api.github.com.evil.example/...`, `https://api.github.com@evil.example/...`);
/// those requests must never carry `Authorization`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequestTarget {
    Api,
    Raw,
    Other,
}

pub(crate) fn request_target(url: &str) -> RequestTarget {
    if has_origin(url, GITHUB_API_BASE_URL) {
        RequestTarget::Api
    } else if has_origin(url, GITHUB_RAW_BASE_URL) {
        RequestTarget::Raw
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

pub(crate) fn api_url(segments: &[&str]) -> String {
    let mut url = String::from(GITHUB_API_BASE_URL);
    for segment in segments {
        push_encoded_segment(&mut url, segment);
    }
    url
}

pub(crate) fn raw_url(owner: &str, repo: &str, revision: &str, path: &str) -> String {
    let mut url = String::from(GITHUB_RAW_BASE_URL);
    for segment in [owner, repo, revision] {
        push_encoded_segment(&mut url, segment);
    }
    for part in path.split('/') {
        push_encoded_segment(&mut url, part);
    }
    url
}

fn push_encoded_segment(url: &mut String, segment: &str) {
    url.push('/');
    url.push_str(&encode_path_segment(segment));
}

pub(crate) fn encode_path_segment(segment: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    encoded
}
