use std::collections::{HashMap, HashSet};

use shared::RomajiRule;

#[derive(Clone, Debug, Default)]
pub(super) struct RomajiLookup {
    pub(super) max_input_len: usize,
    pub(super) max_multi_char_input_len: usize,
    prefix_set: HashSet<String>,
    multi_char_prefix_set: HashSet<String>,
    single_symbol_outputs: HashMap<char, String>,
    single_symbol_output_order: HashMap<char, usize>,
}

impl RomajiLookup {
    pub(super) fn from_rows(rows: &[RomajiRule]) -> Self {
        let mut lookup = Self::default();

        for (row_index, row) in rows.iter().enumerate() {
            let input = row.input.trim();
            if input.is_empty() {
                continue;
            }

            let input_len = input.chars().count();
            lookup.max_input_len = lookup.max_input_len.max(input_len);
            Self::insert_prefixes(input, &mut lookup.prefix_set);

            if input_len > 1 {
                lookup.max_multi_char_input_len = lookup.max_multi_char_input_len.max(input_len);
                Self::insert_prefixes(input, &mut lookup.multi_char_prefix_set);
            } else if row.next_input.trim().is_empty() && !row.output.is_empty() {
                if let Some(symbol) = Self::single_char(input) {
                    lookup
                        .single_symbol_outputs
                        .entry(symbol)
                        .or_insert_with(|| row.output.clone());
                    lookup
                        .single_symbol_output_order
                        .entry(symbol)
                        .or_insert(row_index);
                }
            }
        }

        lookup
    }

    fn insert_prefixes(input: &str, prefixes: &mut HashSet<String>) {
        let mut end = 0;
        for ch in input.chars() {
            end += ch.len_utf8();
            prefixes.insert(input[..end].to_string());
        }
    }

    fn single_char(input: &str) -> Option<char> {
        let mut chars = input.chars();
        let ch = chars.next()?;
        chars.next().is_none().then_some(ch)
    }

    pub(super) fn has_romaji_table_context(&self, raw_input_before: &str, symbol: char) -> bool {
        self.has_context(
            raw_input_before,
            symbol,
            self.max_input_len,
            &self.prefix_set,
        )
    }

    pub(super) fn has_multi_character_romaji_context(
        &self,
        raw_input_before: &str,
        symbol: char,
    ) -> bool {
        self.has_context(
            raw_input_before,
            symbol,
            self.max_multi_char_input_len,
            &self.multi_char_prefix_set,
        )
    }

    fn has_context(
        &self,
        raw_input_before: &str,
        symbol: char,
        max_input_len: usize,
        prefixes: &HashSet<String>,
    ) -> bool {
        if max_input_len == 0 || prefixes.is_empty() {
            return false;
        }

        let mut tail: Vec<char> = raw_input_before
            .chars()
            .rev()
            .take(max_input_len.saturating_sub(1))
            .collect();
        tail.reverse();

        let mut combined = String::with_capacity(raw_input_before.len() + symbol.len_utf8());
        for ch in tail {
            combined.push(ch);
        }
        combined.push(symbol);

        combined
            .char_indices()
            .any(|(suffix_start, _)| prefixes.contains(&combined[suffix_start..]))
    }

    pub(super) fn single_symbol_output(&self, symbols: &[char]) -> Option<String> {
        symbols
            .iter()
            .filter_map(|symbol| {
                let order = self.single_symbol_output_order.get(symbol)?;
                let output = self.single_symbol_outputs.get(symbol)?;
                Some((*order, output))
            })
            .min_by_key(|(order, _)| *order)
            .map(|(_, output)| output.clone())
    }
}
