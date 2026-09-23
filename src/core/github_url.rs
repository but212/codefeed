//! Pure GitHub URL construction and percent-encoding.

const GITHUB_API_BASE_URL: &str = "https://api.github.com";
const GITHUB_RAW_BASE_URL: &str = "https://raw.githubusercontent.com";

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
