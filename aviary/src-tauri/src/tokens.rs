//! Token counting.
//!
//! Uses `tiktoken-rs` with the `o200k_base` encoding — the one GPT-4o and
//! Codex use. Anthropic does not publish an offline tokenizer, so counts for
//! Claude are close but not exact; the UI labels them as estimates rather than
//! implying a precision we do not have.

use std::sync::OnceLock;
use tiktoken_rs::CoreBPE;

fn encoder() -> &'static CoreBPE {
    static ENC: OnceLock<CoreBPE> = OnceLock::new();
    ENC.get_or_init(|| {
        tiktoken_rs::o200k_base().expect("o200k_base encoding is bundled with tiktoken-rs")
    })
}

/// Counts tokens in a string.
pub fn count(text: &str) -> usize {
    encoder().encode_with_special_tokens(text).len()
}

/// Counts tokens in a file, returning 0 if it cannot be read.
pub fn count_file(path: &str) -> usize {
    std::fs::read_to_string(path)
        .map(|s| count(&s))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_sane() {
        // A rough sanity anchor: real tokenisation lands well under bytes/2
        // and well above bytes/8 for ordinary English prose.
        let text = "The quick brown fox jumps over the lazy dog. ".repeat(20);
        let n = count(&text);
        eprintln!("bytes={} tokens={}", text.len(), n);
        assert!(n > text.len() / 8);
        assert!(n < text.len() / 2);
    }
}
