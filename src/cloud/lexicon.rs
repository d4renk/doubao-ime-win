use std::collections::HashMap;

use super::NerWord;

const DEFAULT_CAPACITY: usize = 1024;

#[derive(Clone, Debug)]
struct Entry {
    freq: u32,
    updated_at: u64,
}

/// An in-process, recently-updated dictionary populated by NER responses.
#[derive(Clone, Debug)]
pub struct NerLexicon {
    entries: HashMap<String, Entry>,
    capacity: usize,
    clock: u64,
}

impl Default for NerLexicon {
    fn default() -> Self {
        Self::new()
    }
}

impl NerLexicon {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(capacity),
            capacity,
            clock: 0,
        }
    }

    pub fn update(&mut self, words: impl IntoIterator<Item = NerWord>) {
        for word in words {
            if self.capacity == 0 || word.word.trim().is_empty() || word.freq == 0 {
                continue;
            }

            self.clock = self.clock.wrapping_add(1);
            self.entries.insert(
                word.word,
                Entry {
                    freq: word.freq,
                    updated_at: self.clock,
                },
            );

            if self.entries.len() > self.capacity {
                self.evict_oldest();
            }
        }
    }

    pub fn get(&self, word: &str) -> Option<u32> {
        self.entries.get(word).map(|entry| entry.freq)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the newest entries first.
    pub fn snapshot(&self) -> Vec<NerWord> {
        let mut entries: Vec<_> = self.entries.iter().collect();
        entries.sort_unstable_by_key(|(_, entry)| std::cmp::Reverse(entry.updated_at));
        entries
            .into_iter()
            .map(|(word, entry)| NerWord {
                word: word.clone(),
                freq: entry.freq,
            })
            .collect()
    }

    fn evict_oldest(&mut self) {
        if let Some(oldest) = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.updated_at)
            .map(|(word, _)| word.clone())
        {
            self.entries.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(value: &str, freq: u32) -> NerWord {
        NerWord {
            word: value.into(),
            freq,
        }
    }

    #[test]
    fn keeps_recently_updated_entries() {
        let mut lexicon = NerLexicon::with_capacity(2);
        lexicon.update([word("old", 1), word("kept", 2)]);
        lexicon.update([word("old", 3), word("new", 4)]);

        assert_eq!(lexicon.len(), 2);
        assert_eq!(lexicon.get("old"), Some(3));
        assert_eq!(lexicon.get("new"), Some(4));
        assert_eq!(lexicon.get("kept"), None);
        assert_eq!(lexicon.snapshot(), vec![word("new", 4), word("old", 3)]);
    }

    #[test]
    fn rejects_empty_and_zero_frequency_words() {
        let mut lexicon = NerLexicon::new();
        lexicon.update([word("", 1), word("valid", 0), word("valid", 2)]);

        assert_eq!(lexicon.snapshot(), vec![word("valid", 2)]);
    }
}
