//! Heuristic token estimation state machine and lookup table.

const METRIC_WORD: u8 = 1 << 0; // [0-9a-zA-Z_]

const CHAR_METRIC: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut b = 0usize;
    while b < 256 {
        let is_num = b >= b'0' as usize && b <= b'9' as usize;
        let is_upper = b >= b'A' as usize && b <= b'Z' as usize;
        let is_lower = b >= b'a' as usize && b <= b'z' as usize;
        let is_under = b == b'_' as usize;
        let is_word = is_num || is_upper || is_lower || is_under;

        if is_word {
            t[b] = METRIC_WORD;
        } else {
            let is_ws =
                b == b' ' as usize || b == b'\t' as usize || b == b'\r' as usize || b == 0x0C;
            let is_nl = b == b'\n' as usize;
            let lambda = if (b as i8) < -64 {
                0
            } else if (b as i8) >= 0 {
                1
            } else {
                2
            };
            let non_ws_weight = (!is_ws && !is_nl) as u8 * lambda;
            let w_b = non_ws_weight + (is_nl as u8);
            t[b] = w_b << 1;
        }
        b += 1;
    }
    t
};

/// State machine tracking token count heuristics across character streams.
#[derive(Default, Debug, Clone)]
pub struct TokenEstimator {
    tokens: usize,
    in_word: bool,
    word_len: usize,
    saw_input: bool,
}

impl TokenEstimator {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline(always)]
    pub fn process_byte(&mut self, b: u8) {
        self.saw_input = true;
        let metric = CHAR_METRIC[b as usize] as usize;
        let omega = metric & (METRIC_WORD as usize);
        let w_b = metric >> 1;

        let prev_in_word = self.in_word as usize;
        self.in_word = omega != 0;

        let next_word_len = (self.word_len + 1) * omega;
        let delta_word = next_word_len / 4;
        self.word_len = next_word_len % 4;

        self.tokens += delta_word + (1 - omega) * prev_in_word + w_b;
    }

    #[inline]
    pub fn process_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.process_byte(b);
        }
    }

    pub fn process_str(&mut self, text: &str) {
        self.process_bytes(text.as_bytes());
    }

    pub fn finish(&self) -> usize {
        let tokens = self.tokens + self.in_word as usize;
        let non_empty = (tokens > 0) | self.saw_input;
        tokens.max(non_empty as usize)
    }
}

pub fn estimate_tokens(text: &str) -> usize {
    let mut estimator = TokenEstimator::new();
    estimator.process_str(text);
    estimator.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens_counts() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("hello"), 2);
        assert!(estimate_tokens("let mut x = 123;") >= 4);
        assert!(estimate_tokens("fn main() {\n    println!(\"Hello, world!\");\n}\n") >= 8);
        assert!(estimate_tokens("한글 텍스트 테스트") >= 4);
        assert!(estimate_tokens("word1 word2...word3\nword4") >= 5);
    }

    #[test]
    fn test_finish_is_idempotent() {
        let mut estimator = TokenEstimator::new();
        estimator.process_str("hello world\nfn main() { let x = 42; }");
        let first = estimator.finish();
        let second = estimator.finish();
        let third = estimator.finish();
        assert_eq!(first, second);
        assert_eq!(second, third);
    }

    #[test]
    fn test_token_estimator_matches_char_oracle() {
        fn char_oracle(text: &str) -> usize {
            let mut tokens = 0;
            let mut in_word = false;
            let mut word_len = 0;
            let mut saw_input = false;

            for ch in text.chars() {
                saw_input = true;
                if ch.is_ascii_alphanumeric() || ch == '_' {
                    in_word = true;
                    word_len += 1;
                    let full_word = (word_len >= 4) as usize;
                    tokens += full_word;
                    word_len *= 1 - full_word;
                } else {
                    let was_in_word = std::mem::take(&mut in_word) as usize;
                    tokens += was_in_word;
                    word_len = 0;

                    let is_ws = ch.is_ascii_whitespace();
                    let is_nl = (ch == '\n') as usize;
                    let non_ws_weight = (!is_ws) as usize * (1 + (!ch.is_ascii()) as usize);
                    tokens += non_ws_weight + is_nl;
                }
            }

            tokens += in_word as usize;
            let non_empty = (tokens > 0) | saw_input;
            tokens.max(non_empty as usize)
        }

        let samples = [
            "",
            "hello",
            "let mut x = 123;",
            "fn main() {\n    println!(\"Hello, world!\");\n}\n",
            "한",
            "🚀",
            "한글 텍스트 테스트",
            "word1 word2...word3\nword4",
            "Special: 🚀 emoji and \t\r\n symbols! 👍",
            "Cyrillic: Привет мир",
            "Accents: café, naïve, façade",
        ];

        for sample in samples {
            let oracle = char_oracle(sample);
            let estimated = estimate_tokens(sample);
            assert_eq!(
                estimated, oracle,
                "token estimation mismatch for sample {sample:?}"
            );
        }
    }
}
