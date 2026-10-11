//! Token estimation writer.

use std::io::Write;

pub(crate) struct TokenEstimatingWriter<'a, W: Write + ?Sized> {
    inner: &'a mut W,
    tokens: usize,
    in_word: bool,
    word_len: usize,
}

impl<'a, W: Write + ?Sized> TokenEstimatingWriter<'a, W> {
    pub(crate) fn new(inner: &'a mut W) -> Self {
        Self {
            inner,
            tokens: 0,
            in_word: false,
            word_len: 0,
        }
    }

    pub(crate) fn finish(mut self) -> usize {
        if self.tokens == 0 && !self.in_word {
            return 0;
        }
        if self.in_word {
            self.tokens += 1;
        }
        self.tokens.max(1)
    }
}

impl<W: Write + ?Sized> Write for TokenEstimatingWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write_all(buf)?;

        let text = std::str::from_utf8(buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        for ch in text.chars() {
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
}
