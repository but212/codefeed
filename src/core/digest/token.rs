//! Heuristic token count estimation over streaming bytes and strings.

const FLAG_WORD: u8 = 1 << 0; // [0-9a-zA-Z_]
const FLAG_WS: u8 = 1 << 1; // whitespace
const FLAG_NL: u8 = 1 << 2; // \n

const CHAR_FLAGS: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut b = 0usize;
    while b < 256 {
        let is_num = b >= b'0' as usize && b <= b'9' as usize;
        let is_upper = b >= b'A' as usize && b <= b'Z' as usize;
        let is_lower = b >= b'a' as usize && b <= b'z' as usize;
        let is_under = b == b'_' as usize;
        if is_num || is_upper || is_lower || is_under {
            t[b] |= FLAG_WORD;
        }
        if b == b' ' as usize || b == b'\t' as usize || b == b'\r' as usize || b == 0x0C {
            t[b] |= FLAG_WS;
        }
        if b == b'\n' as usize {
            t[b] |= FLAG_WS | FLAG_NL;
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
        let flags = CHAR_FLAGS[b as usize];

        let is_word = (flags & FLAG_WORD) as usize; // 0 or 1
        let is_ws = ((flags & FLAG_WS) >> 1) as usize; // 0 or 1
        let is_nl = ((flags & FLAG_NL) >> 2) as usize; // 0 or 1

        let prev_in_word = self.in_word as usize;
        self.in_word = is_word != 0;

        let next_word_len = (self.word_len + 1) * is_word;
        let full_word = (next_word_len >= 4) as usize;
        self.word_len = next_word_len * (1 - full_word);

        let is_cont = ((b & 0xC0) == 0x80) as usize;
        let is_lead = 1 - is_cont;
        let is_non_ascii_lead = ((b >= 0x80) as usize) & is_lead;

        let non_ws_weight = (1 - is_ws) * (is_lead + is_non_ascii_lead);
        let non_word_tokens = (1 - is_word) * (prev_in_word + non_ws_weight + is_nl);

        self.tokens += full_word + non_word_tokens;
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

    pub fn finish(mut self) -> usize {
        self.tokens += self.in_word as usize;
        let non_empty = (self.tokens > 0) | self.saw_input;
        self.tokens.max(non_empty as usize)
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
