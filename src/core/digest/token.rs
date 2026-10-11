//! Token estimation writer.

use std::io::Write;

pub(crate) struct TokenEstimatingWriter<'a, W: Write + ?Sized> {
    inner: &'a mut W,
    tokens: usize,
    in_word: bool,
    word_len: usize,
    pending: [u8; 4],
    pending_len: usize,
}

impl<'a, W: Write + ?Sized> TokenEstimatingWriter<'a, W> {
    pub(crate) fn new(inner: &'a mut W) -> Self {
        Self {
            inner,
            tokens: 0,
            in_word: false,
            word_len: 0,
            pending: [0; 4],
            pending_len: 0,
        }
    }

    pub(crate) fn finish(mut self) -> usize {
        if self.pending_len > 0 {
            self.tokens += 2;
        }
        if self.tokens == 0 && !self.in_word {
            return 0;
        }
        if self.in_word {
            self.tokens += 1;
        }
        self.tokens.max(1)
    }

    fn process_char(&mut self, ch: char) {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            self.in_word = true;
            self.word_len += 1;
            if self.word_len >= 4 {
                self.tokens += 1;
                self.word_len = 0;
            }
        } else {
            if self.in_word {
                self.tokens += 1;
                self.in_word = false;
                self.word_len = 0;
            }
            if !ch.is_ascii_whitespace() {
                if ch.is_ascii() {
                    self.tokens += 1;
                } else {
                    self.tokens += 2;
                }
            } else if ch == '\n' {
                self.tokens += 1;
            }
        }
    }
}

impl<W: Write + ?Sized> Write for TokenEstimatingWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write_all(buf)?;

        let mut input = buf;
        if self.pending_len > 0 {
            while !input.is_empty() && self.pending_len < 4 {
                self.pending[self.pending_len] = input[0];
                self.pending_len += 1;
                input = &input[1..];
                match std::str::from_utf8(&self.pending[..self.pending_len]) {
                    Ok(s) => {
                        let ch = s.chars().next().expect("valid utf8 has at least one char");
                        self.pending_len = 0;
                        self.process_char(ch);
                        break;
                    }
                    Err(e) => {
                        if e.error_len().is_some() {
                            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e));
                        }
                    }
                }
            }
            if self.pending_len > 0 && input.is_empty() {
                return Ok(buf.len());
            }
        }

        if input.is_empty() {
            return Ok(buf.len());
        }

        match std::str::from_utf8(input) {
            Ok(text) => {
                for ch in text.chars() {
                    self.process_char(ch);
                }
            }
            Err(e) => {
                let valid_len = e.valid_up_to();
                if valid_len > 0 {
                    let valid_str = match std::str::from_utf8(&input[..valid_len]) {
                        Ok(s) => s,
                        Err(_) => unreachable!(),
                    };
                    for ch in valid_str.chars() {
                        self.process_char(ch);
                    }
                }
                if e.error_len().is_none() {
                    let remaining = &input[valid_len..];
                    self.pending[..remaining.len()].copy_from_slice(remaining);
                    self.pending_len = remaining.len();
                } else {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e));
                }
            }
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn estimate_tokens(text: &str) -> usize {
        let mut sink = std::io::sink();
        let mut estimator = TokenEstimatingWriter::new(&mut sink);
        let _ = std::io::Write::write_all(&mut estimator, text.as_bytes());
        estimator.finish()
    }

    #[test]
    fn estimates_token_counts() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("hello"), 2);
        assert!(estimate_tokens("let mut x = 123;") >= 4);
        assert!(estimate_tokens("fn main() {\n    println!(\"Hello, world!\");\n}\n") >= 8);
        assert!(estimate_tokens("한글 텍스트 테스트") >= 4);
    }

    #[test]
    fn handles_multibyte_chunk_boundaries() {
        let text = "한글 텍스트 테스트 🦀 Rust 🚀";
        let expected = estimate_tokens(text);

        // Byte-by-byte
        let mut sink = std::io::sink();
        let mut estimator = TokenEstimatingWriter::new(&mut sink);
        for &byte in text.as_bytes() {
            estimator.write_all(&[byte]).unwrap();
        }
        let actual = estimator.finish();
        assert_eq!(actual, expected);

        // 2-byte chunks
        let mut sink2 = std::io::sink();
        let mut estimator2 = TokenEstimatingWriter::new(&mut sink2);
        for chunk in text.as_bytes().chunks(2) {
            estimator2.write_all(chunk).unwrap();
        }
        let actual2 = estimator2.finish();
        assert_eq!(actual2, expected);
    }
}
