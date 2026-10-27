//! Qwen's byte-level BPE tokenizer, for the units the aligner is given:
//! letters, digits and apostrophes only (see `forced::units`), so its
//! pre-tokenizer reduces to a few rules instead of a regex.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

pub struct Bpe {
    ids: HashMap<String, i64>,
    /// Merge rank of each pair, keyed `"left right"` (pieces never hold a
    /// plain space; byte-level text writes it as `Ġ`).
    ranks: HashMap<String, u32>,
    /// The character each byte is written as.
    bytes: [char; 256],
}

impl Bpe {
    /// From the tokenizer's `vocab.json` and `merges.txt`.
    pub fn load(vocab: &Path, merges: &Path) -> Result<Self> {
        let ids: HashMap<String, i64> = serde_json::from_slice(
            &std::fs::read(vocab).with_context(|| format!("could not read {}", vocab.display()))?,
        )
        .with_context(|| format!("{} is not a vocabulary", vocab.display()))?;
        let merges = std::fs::read_to_string(merges)
            .with_context(|| format!("could not read {}", merges.display()))?;
        let ranks = merges
            .lines()
            .filter(|l| !l.starts_with("#version") && !l.is_empty())
            .enumerate()
            .map(|(rank, l)| (l.to_string(), rank as u32))
            .collect();
        Ok(Self {
            ids,
            ranks,
            bytes: byte_chars(),
        })
    }

    /// Token ids of `unit`, as the tokenizer gives them for it standing
    /// between two special tokens.
    pub fn encode(&self, unit: &str) -> Vec<i64> {
        let text: String = unit.nfc().collect();
        pieces(&text)
            .into_iter()
            .flat_map(|piece| self.merge(piece))
            .collect()
    }

    fn merge(&self, piece: &str) -> Vec<i64> {
        let mut parts: Vec<String> = piece
            .bytes()
            .map(|b| self.bytes[b as usize].to_string())
            .collect();
        // Always the lowest-ranked pair next, everywhere it occurs.
        while let Some((_, i)) = (0..parts.len().saturating_sub(1))
            .filter_map(|i| {
                let pair = format!("{} {}", parts[i], parts[i + 1]);
                self.ranks.get(&pair).map(|&rank| (rank, i))
            })
            .min()
        {
            let (a, b) = (parts[i].clone(), parts[i + 1].clone());
            let mut merged = Vec::with_capacity(parts.len());
            let mut k = 0;
            while k < parts.len() {
                if k + 1 < parts.len() && parts[k] == a && parts[k + 1] == b {
                    merged.push(format!("{a}{b}"));
                    k += 2;
                } else {
                    merged.push(std::mem::take(&mut parts[k]));
                    k += 1;
                }
            }
            parts = merged;
        }
        parts
            .iter()
            .filter_map(|p| self.ids.get(p).copied())
            .collect()
    }
}

/// Qwen2's pre-tokenizer, `(?i:'s|'t|'re|'ve|'m|'ll|'d)|[^\r\n\p{L}\p{N}]?\p{L}+|\p{N}| ?[^\s\p{L}\p{N}]+…`,
/// on text without spaces.
fn pieces(text: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let letter = |i: usize| {
        chars
            .get(i)
            .is_some_and(|c| c.1.is_alphabetic() && !c.1.is_numeric())
    };
    let other = |i: usize| {
        chars
            .get(i)
            .is_some_and(|c| !c.1.is_alphabetic() && !c.1.is_numeric())
    };
    let letters_from = |mut j: usize| {
        while letter(j) {
            j += 1;
        }
        j
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i].1;
        let next = if c.is_numeric() {
            i + 1
        } else if letter(i) {
            letters_from(i)
        } else {
            let ahead: String = chars[i + 1..]
                .iter()
                .take(2)
                .map(|c| c.1.to_ascii_lowercase())
                .collect();
            let contraction = match () {
                _ if c != '\'' => 0,
                _ if ["re", "ve", "ll"].iter().any(|s| ahead.starts_with(s)) => 3,
                _ if ahead.starts_with(['s', 't', 'm', 'd']) => 2,
                _ => 0,
            };
            if contraction > 0 {
                i + contraction
            } else if letter(i + 1) {
                letters_from(i + 1)
            } else {
                let mut j = i + 1;
                while other(j) {
                    j += 1;
                }
                j
            }
        };
        let end = chars.get(next).map_or(text.len(), |c| c.0);
        out.push(&text[chars[i].0..end]);
        i = next;
    }
    out
}

/// GPT-2's printable stand-ins for the 256 byte values.
fn byte_chars() -> [char; 256] {
    let mut table = ['\0'; 256];
    let mut extra = 0;
    for b in 0..256u32 {
        let printable =
            (0x21..=0x7e).contains(&b) || (0xa1..=0xac).contains(&b) || (0xae..=0xff).contains(&b);
        let code = if printable {
            b
        } else {
            extra += 1;
            255 + extra
        };
        table[b as usize] = char::from_u32(code).expect("valid code point");
    }
    table
}
