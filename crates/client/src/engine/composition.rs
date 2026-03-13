use crate::{
    engine::user_action::UserAction,
    extension::VKeyExt as _,
    tsf::factory::{TextServiceFactory, TextServiceFactory_Impl},
};

use super::{
    client_action::{ClientAction, SetSelectionType, SetTextType},
    full_width::{convert_kana_symbol, to_fullwidth, to_halfwidth},
    input_mode::InputMode,
    ipc_service::{Candidates, IPCService},
    state::IMEState,
    text_util::{to_half_katakana, to_katakana},
    user_action::{Function, Navigation},
};
use shared::{
    zenzai_cpu_backend_supported, AppConfig, NumpadInputMode, RomajiRule, SpaceInputMode,
};
use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    UI::{
        Input::KeyboardAndMouse::{
            VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_MENU, VK_RCONTROL, VK_RMENU, VK_SHIFT,
        },
        TextServices::{ITfComposition, ITfCompositionSink_Impl, ITfContext},
    },
};

use anyhow::{Context, Result};

#[derive(Default, Clone, PartialEq, Debug)]
pub enum CompositionState {
    #[default]
    None,
    Composing,
    Previewing,
    Selecting,
}

#[derive(Default, Clone, Debug)]
pub struct Composition {
    pub preview: String, // text to be previewed
    pub suffix: String,  // text to be appended after preview
    pub raw_input: String,
    pub raw_hiragana: String,
    pub fixed_prefix: String,

    pub corresponding_count: i32, // corresponding count of the preview

    pub selection_index: i32,
    pub candidates: Candidates,
    pub clause_snapshots: Vec<ClauseSnapshot>,
    pub future_clause_snapshots: Vec<FutureClauseSnapshot>,

    pub state: CompositionState,
    pub temporary_latin: bool,
    pub temporary_latin_shift_pending: bool,
    pub tip_composition: Option<ITfComposition>,
}

#[derive(Clone, Debug)]
pub struct ClauseSnapshot {
    preview: String,
    suffix: String,
    raw_input: String,
    raw_hiragana: String,
    fixed_prefix: String,
    corresponding_count: i32,
    selection_index: i32,
    candidates: Candidates,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FutureClauseSnapshot {
    clause_preview: String,
    suffix: String,
    raw_input: String,
    raw_hiragana: String,
    is_conservative: bool,
    corresponding_count: i32,
    selection_index: i32,
    selected_text: String,
    selected_sub_text: String,
    candidates: Candidates,
}

#[derive(Debug, Clone)]
struct CandidateSelection {
    index: i32,
    text: String,
    sub_text: String,
    hiragana: String,
    corresponding_count: i32,
}

impl ITfCompositionSink_Impl for TextServiceFactory_Impl {
    #[macros::anyhow]
    fn OnCompositionTerminated(
        &self,
        _ecwrite: u32,
        _pcomposition: Option<&ITfComposition>,
    ) -> Result<()> {
        // if user clicked outside the composition, the composition will be terminated
        tracing::debug!("OnCompositionTerminated");

        let actions = vec![ClientAction::EndComposition];
        self.handle_action(&actions, CompositionState::None)?;

        Ok(())
    }
}

impl TextServiceFactory {
    const MOVE_CURSOR_CLEAR_CLAUSE_SNAPSHOTS: i32 = 125;
    const MOVE_CURSOR_PUSH_CLAUSE_SNAPSHOT: i32 = 126;
    const MOVE_CURSOR_POP_CLAUSE_SNAPSHOT: i32 = 127;

    #[inline]
    fn is_ctrl_pressed() -> bool {
        VK_CONTROL.is_pressed() || VK_LCONTROL.is_pressed() || VK_RCONTROL.is_pressed()
    }

    #[inline]
    fn is_shift_pressed() -> bool {
        VK_SHIFT.is_pressed()
    }

    #[inline]
    fn is_shift_key(wparam: WPARAM) -> bool {
        matches!(wparam.0, 0x10 | 0xA0 | 0xA1)
    }

    #[inline]
    fn is_shift_alphabet_shortcut(wparam: WPARAM, is_shift_pressed: bool) -> bool {
        is_shift_pressed && (0x41..=0x5A).contains(&wparam.0)
    }

    #[inline]
    fn direct_text_for_action(action: &UserAction) -> Option<String> {
        match action {
            UserAction::Input(input_char) => {
                Some(Self::normalize_direct_symbol_char(*input_char).to_string())
            }
            UserAction::Space => Some(" ".to_string()),
            UserAction::NumpadSymbol(symbol) => Some(symbol.to_string()),
            UserAction::Number { value, .. } => {
                let digit = char::from_digit(*value as u32, 10).unwrap_or('0');
                Some(digit.to_string())
            }
            _ => None,
        }
    }

    #[inline]
    fn normalize_direct_symbol_char(c: char) -> char {
        let halfwidth_ascii = Self::to_halfwidth_ascii_char(c);
        if halfwidth_ascii.is_ascii_punctuation() {
            return halfwidth_ascii;
        }

        if c.is_ascii_punctuation() {
            return c;
        }

        let converted = to_halfwidth(&c.to_string());
        let mut converted_chars = converted.chars();
        match (converted_chars.next(), converted_chars.next()) {
            (Some(converted_char), None) if converted_char.is_ascii_punctuation() => converted_char,
            _ => c,
        }
    }

    #[inline]
    fn is_alt_backquote(wparam: WPARAM, lparam: LPARAM) -> bool {
        const VK_OEM_3: usize = 0xC0;
        const SCAN_CODE_BACKQUOTE: usize = 0x29;
        const ALT_CONTEXT_BIT: usize = 0x2000_0000;
        let is_alt_pressed = VK_MENU.is_pressed()
            || VK_LMENU.is_pressed()
            || VK_RMENU.is_pressed()
            || ((lparam.0 as usize) & ALT_CONTEXT_BIT) != 0;
        let scan_code = ((lparam.0 as usize) >> 16) & 0xFF;
        let is_backquote_key = wparam.0 == VK_OEM_3 || scan_code == SCAN_CODE_BACKQUOTE;

        is_alt_pressed && is_backquote_key
    }

    #[inline]
    fn to_fullwidth_ascii_char(c: char) -> char {
        if c == ' ' {
            return '　';
        }

        if c.is_ascii_punctuation() || c.is_ascii_digit() {
            return char::from_u32(c as u32 + 0xFEE0).unwrap_or(c);
        }

        c
    }

    #[inline]
    fn to_halfwidth_ascii_char(c: char) -> char {
        if c == '　' {
            return ' ';
        }

        if ('！'..='～').contains(&c) {
            return char::from_u32(c as u32 - 0xFEE0).unwrap_or(c);
        }

        c
    }

    #[inline]
    fn numpad_text_for_mode(
        c: char,
        mode: NumpadInputMode,
        allow_direct_passthrough: bool,
    ) -> Option<String> {
        match mode {
            NumpadInputMode::DirectInput if allow_direct_passthrough => None,
            NumpadInputMode::DirectInput | NumpadInputMode::AlwaysHalf => {
                Some(Self::to_halfwidth_ascii_char(c).to_string())
            }
            NumpadInputMode::FollowInputMode => Some(Self::to_fullwidth_ascii_char(c).to_string()),
        }
    }

    #[inline]
    fn normalize_symbol_variant(c: char) -> Option<char> {
        match c {
            'ˆ' | '＾' => Some('^'),
            '〜' | '～' => Some('~'),
            '＼' | '￥' | '¥' => Some('\\'),
            '，' => Some(','),
            '．' => Some('.'),
            '”' => Some('"'),
            '’' => Some('\''),
            _ => None,
        }
    }

    #[inline]
    fn single_symbol_candidates(input: &str) -> Option<Vec<char>> {
        let mut chars = input.chars();
        let ch = chars.next()?;
        if chars.next().is_some() {
            return None;
        }

        let mut candidates = Vec::with_capacity(4);
        let mut push_unique = |candidate: char| {
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        };

        if ch.is_ascii_punctuation() || ch.is_ascii_digit() {
            push_unique(ch);
        }

        let halfwidth = Self::to_halfwidth_ascii_char(ch);
        if halfwidth.is_ascii_punctuation() || halfwidth.is_ascii_digit() {
            push_unique(ch);
            push_unique(halfwidth);
        }

        if let Some(mapped) = Self::normalize_symbol_variant(ch) {
            push_unique(ch);
            push_unique(mapped);
        }

        let converted = to_halfwidth(&ch.to_string());
        let mut converted_chars = converted.chars();
        if let (Some(converted_char), None) = (converted_chars.next(), converted_chars.next()) {
            if converted_char.is_ascii_punctuation() || converted_char.is_ascii_digit() {
                push_unique(ch);
                push_unique(converted_char);
            }
        }

        if candidates.is_empty() {
            None
        } else {
            Some(candidates)
        }
    }

    fn has_romaji_table_context(
        raw_input_before: &str,
        symbol: char,
        romaji_rows: &[RomajiRule],
    ) -> bool {
        if romaji_rows.is_empty() {
            return false;
        }

        let mut combined = String::with_capacity(raw_input_before.len() + symbol.len_utf8());
        combined.push_str(raw_input_before);
        combined.push(symbol);

        let combined_chars: Vec<char> = combined.chars().collect();
        let max_row_len = romaji_rows
            .iter()
            .map(|row| row.input.chars().count())
            .max()
            .unwrap_or(0);

        if max_row_len == 0 {
            return false;
        }

        let start_index = combined_chars.len().saturating_sub(max_row_len);
        for suffix_start in start_index..combined_chars.len() {
            let suffix: String = combined_chars[suffix_start..].iter().collect();
            if suffix.is_empty() {
                continue;
            }

            if romaji_rows
                .iter()
                .filter_map(|row| {
                    let trimmed = row.input.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed)
                    }
                })
                .any(|input| input.starts_with(&suffix))
            {
                return true;
            }
        }

        false
    }

    #[inline]
    fn should_apply_symbol_fallback(
        raw_input_before: &str,
        input: &str,
        romaji_rows: &[RomajiRule],
    ) -> bool {
        let Some(symbols) = Self::single_symbol_candidates(input) else {
            return false;
        };

        !symbols
            .iter()
            .any(|symbol| Self::has_romaji_table_context(raw_input_before, *symbol, romaji_rows))
    }

    #[inline]
    fn has_multi_character_romaji_context(
        raw_input_before: &str,
        symbol: char,
        romaji_rows: &[RomajiRule],
    ) -> bool {
        if romaji_rows.is_empty() {
            return false;
        }

        let mut combined = String::with_capacity(raw_input_before.len() + symbol.len_utf8());
        combined.push_str(raw_input_before);
        combined.push(symbol);

        let combined_chars: Vec<char> = combined.chars().collect();
        let max_row_len = romaji_rows
            .iter()
            .map(|row| row.input.chars().count())
            .max()
            .unwrap_or(0);

        if max_row_len <= 1 {
            return false;
        }

        let start_index = combined_chars.len().saturating_sub(max_row_len);
        for suffix_start in start_index..combined_chars.len() {
            let suffix: String = combined_chars[suffix_start..].iter().collect();
            if suffix.is_empty() {
                continue;
            }

            if romaji_rows
                .iter()
                .filter_map(|row| {
                    let trimmed = row.input.trim();
                    if trimmed.is_empty() || trimmed.chars().count() <= 1 {
                        None
                    } else {
                        Some(trimmed)
                    }
                })
                .any(|input| input.starts_with(&suffix))
            {
                return true;
            }
        }

        false
    }

    #[inline]
    fn effective_zenzai_runtime_enabled(app_config: &AppConfig) -> bool {
        if !app_config.zenzai.enable {
            return false;
        }

        let backend = app_config.zenzai.backend.trim().to_ascii_lowercase();
        if backend.is_empty() || backend == "cpu" {
            return zenzai_cpu_backend_supported();
        }

        true
    }

    #[inline]
    fn single_symbol_romaji_output(input: &str, romaji_rows: &[RomajiRule]) -> Option<String> {
        let symbols = Self::single_symbol_candidates(input)?;

        romaji_rows.iter().find_map(|row| {
            if !row.next_input.trim().is_empty() || row.output.is_empty() {
                return None;
            }

            let trimmed = row.input.trim();
            let mut chars = trimmed.chars();
            let symbol = chars.next()?;
            if chars.next().is_some() {
                return None;
            }

            symbols.contains(&symbol).then(|| row.output.clone())
        })
    }

    #[inline]
    fn resolve_symbol_input_text(
        raw_input_before: &str,
        input: &str,
        app_config: &AppConfig,
    ) -> Option<String> {
        let symbols = Self::single_symbol_candidates(input)?;
        let is_zenzai_enabled = Self::effective_zenzai_runtime_enabled(app_config);
        if is_zenzai_enabled {
            if symbols.iter().any(|symbol| {
                Self::has_multi_character_romaji_context(
                    raw_input_before,
                    *symbol,
                    &app_config.romaji_table.rows,
                )
            }) {
                return None;
            }

            if let Some(mapped) =
                Self::single_symbol_romaji_output(input, &app_config.romaji_table.rows)
            {
                return Some(mapped);
            }

            return Some(convert_kana_symbol(
                input,
                &app_config.general,
                &app_config.character_width,
                &app_config.romaji_table.rows,
            ));
        }

        if Self::should_apply_symbol_fallback(
            raw_input_before,
            input,
            &app_config.romaji_table.rows,
        ) {
            return Some(convert_kana_symbol(
                input,
                &app_config.general,
                &app_config.character_width,
                &app_config.romaji_table.rows,
            ));
        }

        None
    }

    #[inline]
    fn clear_temporary_latin_shift_pending_if_needed(
        &self,
        should_clear_shift_pending: bool,
    ) -> Result<()> {
        if !should_clear_shift_pending {
            return Ok(());
        }

        let text_service = self.borrow()?;
        let mut composition = text_service.borrow_mut_composition()?;
        composition.temporary_latin_shift_pending = false;
        Ok(())
    }

    #[inline]
    fn select_candidate(candidates: &Candidates, desired_index: i32) -> Option<CandidateSelection> {
        if candidates.texts.is_empty() {
            return None;
        }

        let max_index = candidates.texts.len().saturating_sub(1);
        let index = desired_index.max(0) as usize;
        let index = index.min(max_index);

        Some(CandidateSelection {
            index: index as i32,
            text: candidates.texts.get(index).cloned().unwrap_or_default(),
            sub_text: candidates.sub_texts.get(index).cloned().unwrap_or_default(),
            hiragana: candidates.hiragana.clone(),
            corresponding_count: candidates
                .corresponding_count
                .get(index)
                .copied()
                .unwrap_or(0),
        })
    }

    #[inline]
    fn merge_preview_with_prefix(fixed_prefix: &str, clause_preview: &str) -> String {
        if fixed_prefix.is_empty() {
            clause_preview.to_string()
        } else {
            format!("{fixed_prefix}{clause_preview}")
        }
    }

    #[inline]
    fn current_clause_preview(preview: &str, fixed_prefix: &str) -> String {
        preview
            .strip_prefix(fixed_prefix)
            .unwrap_or(preview)
            .to_string()
    }

    #[inline]
    fn build_clause_snapshot(
        preview: &str,
        suffix: &str,
        raw_input: &str,
        raw_hiragana: &str,
        fixed_prefix: &str,
        corresponding_count: i32,
        selection_index: i32,
        candidates: &Candidates,
    ) -> ClauseSnapshot {
        ClauseSnapshot {
            preview: preview.to_string(),
            suffix: suffix.to_string(),
            raw_input: raw_input.to_string(),
            raw_hiragana: raw_hiragana.to_string(),
            fixed_prefix: fixed_prefix.to_string(),
            corresponding_count,
            selection_index,
            candidates: candidates.clone(),
        }
    }

    #[inline]
    fn build_future_clause_snapshot(
        preview: &str,
        suffix: &str,
        raw_input: &str,
        raw_hiragana: &str,
        fixed_prefix: &str,
        corresponding_count: i32,
        selection_index: i32,
        candidates: &Candidates,
    ) -> FutureClauseSnapshot {
        let selected =
            Self::select_candidate(candidates, selection_index).unwrap_or(CandidateSelection {
                index: selection_index.max(0),
                text: Self::current_clause_preview(preview, fixed_prefix),
                sub_text: suffix.to_string(),
                hiragana: raw_hiragana.to_string(),
                corresponding_count,
            });

        FutureClauseSnapshot {
            clause_preview: Self::current_clause_preview(preview, fixed_prefix),
            suffix: suffix.to_string(),
            raw_input: raw_input.to_string(),
            raw_hiragana: raw_hiragana.to_string(),
            is_conservative: false,
            corresponding_count,
            selection_index: selected.index,
            selected_text: selected.text.clone(),
            selected_sub_text: selected.sub_text.clone(),
            candidates: candidates.clone(),
        }
    }

    #[inline]
    fn build_conservative_future_clause_snapshot(
        clause_preview: &str,
        suffix: &str,
        raw_input: &str,
        raw_hiragana: &str,
        corresponding_count: i32,
    ) -> FutureClauseSnapshot {
        let candidates = Candidates {
            texts: vec![clause_preview.to_string()],
            sub_texts: vec![suffix.to_string()],
            hiragana: raw_hiragana.to_string(),
            corresponding_count: vec![corresponding_count],
        };
        FutureClauseSnapshot {
            clause_preview: clause_preview.to_string(),
            suffix: suffix.to_string(),
            raw_input: raw_input.to_string(),
            raw_hiragana: raw_hiragana.to_string(),
            is_conservative: true,
            corresponding_count,
            selection_index: 0,
            selected_text: clause_preview.to_string(),
            selected_sub_text: suffix.to_string(),
            candidates,
        }
    }

    #[inline]
    fn future_clause_display(snapshot: &FutureClauseSnapshot) -> String {
        format!("{}{}", snapshot.clause_preview, snapshot.suffix)
    }

    #[cfg(test)]
    #[inline]
    fn clause_texts_for_log(
        preview: &str,
        fixed_prefix: &str,
        clause_snapshots: &[ClauseSnapshot],
        future_clause_snapshots: &[FutureClauseSnapshot],
    ) -> String {
        let mut clauses = clause_snapshots
            .iter()
            .map(|snapshot| Self::current_clause_preview(&snapshot.preview, &snapshot.fixed_prefix))
            .collect::<Vec<_>>();

        if !preview.is_empty() {
            clauses.push(Self::current_clause_preview(preview, fixed_prefix));
        }

        clauses.extend(
            future_clause_snapshots
                .iter()
                .rev()
                .map(|snapshot| snapshot.clause_preview.clone()),
        );

        clauses.join(" / ")
    }

    #[cfg(test)]
    #[inline]
    fn clause_raw_preview(raw_hiragana: &str, corresponding_count: i32) -> String {
        raw_hiragana
            .chars()
            .take(corresponding_count.max(0) as usize)
            .collect()
    }

    #[cfg(test)]
    #[inline]
    fn clause_raw_texts_for_log(
        raw_hiragana: &str,
        corresponding_count: i32,
        clause_snapshots: &[ClauseSnapshot],
        future_clause_snapshots: &[FutureClauseSnapshot],
    ) -> String {
        let mut clauses = clause_snapshots
            .iter()
            .map(|snapshot| {
                Self::clause_raw_preview(&snapshot.raw_hiragana, snapshot.corresponding_count)
            })
            .collect::<Vec<_>>();

        if !raw_hiragana.is_empty() {
            clauses.push(Self::clause_raw_preview(raw_hiragana, corresponding_count));
        }

        clauses.extend(future_clause_snapshots.iter().rev().map(|snapshot| {
            Self::clause_raw_preview(&snapshot.raw_hiragana, snapshot.corresponding_count)
        }));

        clauses.join(" / ")
    }

    fn clear_clause_snapshots(
        clause_snapshots: &mut Vec<ClauseSnapshot>,
        ipc_service: &mut IPCService,
    ) -> Result<()> {
        if clause_snapshots.is_empty() {
            return Ok(());
        }

        clause_snapshots.clear();
        let _ = ipc_service.move_cursor(Self::MOVE_CURSOR_CLEAR_CLAUSE_SNAPSHOTS)?;
        Ok(())
    }

    #[inline]
    fn clear_future_clause_snapshots(future_clause_snapshots: &mut Vec<FutureClauseSnapshot>) {
        future_clause_snapshots.clear();
    }

    #[inline]
    fn clear_clause_caches(
        clause_snapshots: &mut Vec<ClauseSnapshot>,
        future_clause_snapshots: &mut Vec<FutureClauseSnapshot>,
        ipc_service: &mut IPCService,
    ) -> Result<()> {
        Self::clear_future_clause_snapshots(future_clause_snapshots);
        Self::clear_clause_snapshots(clause_snapshots, ipc_service)
    }

    #[inline]
    fn current_raw_suffix(raw_hiragana: &str, corresponding_count: i32) -> String {
        raw_hiragana
            .chars()
            .skip(corresponding_count.max(0) as usize)
            .collect()
    }

    #[inline]
    fn replace_future_suffix_in_sub_text(
        sub_text: &str,
        future_snapshot: &FutureClauseSnapshot,
    ) -> Option<String> {
        let future_raw = future_snapshot.raw_hiragana.as_str();
        let future_display = Self::future_clause_display(future_snapshot);

        sub_text
            .strip_suffix(future_raw)
            .map(|prefix| format!("{prefix}{future_display}"))
            .or_else(|| (sub_text == future_raw).then(|| future_display.clone()))
    }

    #[inline]
    fn sync_current_clause_future_suffix(
        candidates: &mut Candidates,
        selection_index: i32,
        corresponding_count: i32,
        future_clause_snapshots: &[FutureClauseSnapshot],
    ) -> String {
        let Some(future_snapshot) = future_clause_snapshots.last() else {
            return candidates
                .sub_texts
                .get(selection_index.max(0) as usize)
                .cloned()
                .unwrap_or_default();
        };

        for sub_text in candidates.sub_texts.iter_mut() {
            if let Some(updated) =
                Self::replace_future_suffix_in_sub_text(sub_text, future_snapshot)
            {
                *sub_text = updated;
            }
        }

        candidates
            .sub_texts
            .get(selection_index.max(0) as usize)
            .cloned()
            .unwrap_or_else(|| Self::current_raw_suffix(&candidates.hiragana, corresponding_count))
    }

    #[inline]
    fn push_current_future_clause_snapshot(
        future_clause_snapshots: &mut Vec<FutureClauseSnapshot>,
        preview: &str,
        suffix: &str,
        raw_input: &str,
        raw_hiragana: &str,
        fixed_prefix: &str,
        corresponding_count: i32,
        selection_index: i32,
        candidates: &Candidates,
    ) {
        let snapshot = Self::build_future_clause_snapshot(
            preview,
            suffix,
            raw_input,
            raw_hiragana,
            fixed_prefix,
            corresponding_count,
            selection_index,
            candidates,
        );
        future_clause_snapshots.push(snapshot);
    }

    #[inline]
    fn future_snapshot_matches_raw_suffix(
        future_snapshot: &FutureClauseSnapshot,
        raw_suffix: &str,
    ) -> bool {
        raw_suffix == future_snapshot.raw_hiragana
            || raw_suffix.ends_with(&future_snapshot.raw_hiragana)
    }

    #[inline]
    fn maybe_push_split_future_clause_snapshot(
        future_clause_snapshots: &mut Vec<FutureClauseSnapshot>,
        raw_input: &str,
        raw_hiragana: &str,
        corresponding_count: i32,
        raw_suffix_hint: &str,
    ) {
        if future_clause_snapshots.is_empty() {
            return;
        }

        let raw_suffix = if !raw_suffix_hint.is_empty()
            && future_clause_snapshots
                .iter()
                .rev()
                .any(|snapshot| Self::future_snapshot_matches_raw_suffix(snapshot, raw_suffix_hint))
        {
            raw_suffix_hint.to_string()
        } else {
            Self::current_raw_suffix(raw_hiragana, corresponding_count)
        };
        if raw_suffix.is_empty() {
            return;
        }

        while let Some(snapshot) = future_clause_snapshots.last() {
            if Self::future_snapshot_matches_raw_suffix(snapshot, &raw_suffix) {
                break;
            }
            future_clause_snapshots.pop();
        }

        let existing_future = future_clause_snapshots.last().cloned();
        let raw_input_suffix: String = raw_input
            .chars()
            .skip(corresponding_count.max(0) as usize)
            .collect();
        let Some(snapshot) = existing_future.as_ref() else {
            return;
        };

        if snapshot.is_conservative {
            let trailing_raw_hiragana = future_clause_snapshots
                .iter()
                .rev()
                .nth(1)
                .map(|snapshot| snapshot.raw_hiragana.clone())
                .unwrap_or_default();
            let trailing_raw_input = future_clause_snapshots
                .iter()
                .rev()
                .nth(1)
                .map(|snapshot| snapshot.raw_input.clone())
                .unwrap_or_default();
            let split_preview = if trailing_raw_hiragana.is_empty() {
                raw_suffix.clone()
            } else {
                raw_suffix
                    .strip_suffix(&trailing_raw_hiragana)
                    .unwrap_or(&raw_suffix)
                    .to_string()
            };
            let split_corresponding_count = if trailing_raw_input.is_empty() {
                raw_input_suffix.chars().count() as i32
            } else {
                raw_input_suffix
                    .strip_suffix(&trailing_raw_input)
                    .unwrap_or(&raw_input_suffix)
                    .chars()
                    .count() as i32
            };
            let replaced_snapshot = Self::build_conservative_future_clause_snapshot(
                &split_preview,
                &snapshot.suffix,
                &raw_input_suffix,
                &raw_suffix,
                split_corresponding_count,
            );
            future_clause_snapshots.pop();
            future_clause_snapshots.push(replaced_snapshot);
            return;
        }

        let Some(split_preview) = raw_suffix.strip_suffix(&snapshot.raw_hiragana) else {
            return;
        };
        if split_preview.is_empty() {
            return;
        }

        let split_corresponding_count = raw_input_suffix
            .strip_suffix(&snapshot.raw_input)
            .map(|prefix| prefix.chars().count() as i32)
            .unwrap_or(split_preview.chars().count() as i32);
        let split_snapshot = Self::build_conservative_future_clause_snapshot(
            split_preview,
            &Self::future_clause_display(snapshot),
            &raw_input_suffix,
            &raw_suffix,
            split_corresponding_count,
        );
        future_clause_snapshots.push(split_snapshot);
    }

    #[inline]
    fn restore_future_clause_snapshot(
        preview: &mut String,
        suffix: &mut String,
        raw_input: &mut String,
        raw_hiragana: &mut String,
        corresponding_count: &mut i32,
        selection_index: &mut i32,
        candidates: &mut Candidates,
        fixed_prefix: &str,
        snapshot: &FutureClauseSnapshot,
    ) {
        *preview = Self::merge_preview_with_prefix(fixed_prefix, &snapshot.clause_preview);
        *suffix = snapshot.suffix.clone();
        *raw_input = snapshot.raw_input.clone();
        *raw_hiragana = snapshot.raw_hiragana.clone();
        *corresponding_count = snapshot.corresponding_count;
        *selection_index = Self::resolve_selection_index(
            &snapshot.candidates,
            &snapshot.selected_text,
            &snapshot.selected_sub_text,
            snapshot.corresponding_count,
            snapshot.selection_index,
        );
        *candidates = snapshot.candidates.clone();
    }

    #[inline]
    fn resolve_selection_index(
        candidates: &Candidates,
        selected_text: &str,
        selected_sub_text: &str,
        corresponding_count: i32,
        fallback_index: i32,
    ) -> i32 {
        if let Some(index) = candidates
            .texts
            .iter()
            .zip(candidates.sub_texts.iter())
            .zip(candidates.corresponding_count.iter())
            .position(|((text, sub_text), candidate_corresponding_count)| {
                text == selected_text
                    && sub_text == selected_sub_text
                    && *candidate_corresponding_count == corresponding_count
            })
        {
            return index as i32;
        }

        let max_index = candidates.texts.len().saturating_sub(1) as i32;
        fallback_index.clamp(0, max_index)
    }

    #[inline]
    fn future_snapshot_matches_server(
        future_clause_snapshots: &[FutureClauseSnapshot],
        server_candidates: &Candidates,
    ) -> bool {
        future_clause_snapshots
            .last()
            .map(|snapshot| server_candidates.hiragana == snapshot.raw_hiragana)
            .unwrap_or(false)
    }

    #[inline]
    fn sync_clause_snapshot_suffixes(
        clause_snapshots: &mut [ClauseSnapshot],
        preview: &str,
        suffix: &str,
    ) {
        if clause_snapshots.is_empty() {
            return;
        }

        let mut full_text = String::with_capacity(preview.len() + suffix.len());
        full_text.push_str(preview);
        full_text.push_str(suffix);

        for snapshot in clause_snapshots.iter_mut() {
            if let Some(updated_suffix) = full_text.strip_prefix(&snapshot.preview) {
                let previous_suffix = snapshot.suffix.clone();
                let updated_suffix = updated_suffix.to_string();
                snapshot.suffix = updated_suffix.clone();

                for sub_text in snapshot.candidates.sub_texts.iter_mut() {
                    if let Some(prefix_part) = sub_text.strip_suffix(&previous_suffix) {
                        *sub_text = format!("{prefix_part}{updated_suffix}");
                    }
                }

                if let Some((sub_text, _)) = snapshot
                    .candidates
                    .sub_texts
                    .iter_mut()
                    .zip(snapshot.candidates.corresponding_count.iter())
                    .find(|(_, candidate_corresponding_count)| {
                        **candidate_corresponding_count == snapshot.corresponding_count
                    })
                {
                    *sub_text = updated_suffix;
                }
            }
        }
    }

    #[inline]
    fn commit_current_clause_actions(
        composition: &Composition,
    ) -> (CompositionState, Vec<ClientAction>) {
        if composition.suffix.is_empty() {
            (CompositionState::None, vec![ClientAction::EndComposition])
        } else {
            (
                CompositionState::Composing,
                vec![ClientAction::ShrinkText("".to_string())],
            )
        }
    }

    #[inline]
    fn commit_first_clause_actions(
        composition: &Composition,
    ) -> (CompositionState, Vec<ClientAction>) {
        let mut actions = Vec::with_capacity(composition.clause_snapshots.len() + 1);

        for _ in 0..composition.clause_snapshots.len() {
            actions.push(ClientAction::MoveClause(-1));
        }

        let first_suffix_is_empty = composition
            .clause_snapshots
            .first()
            .map(|snapshot| snapshot.suffix.is_empty())
            .unwrap_or(composition.suffix.is_empty());

        if first_suffix_is_empty {
            actions.push(ClientAction::EndComposition);
            (CompositionState::None, actions)
        } else {
            actions.push(ClientAction::ShrinkText("".to_string()));
            (CompositionState::Composing, actions)
        }
    }

    #[tracing::instrument]
    pub fn process_key(
        &self,
        context: Option<&ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<Option<(Vec<ClientAction>, CompositionState)>> {
        if context.is_none() {
            return Ok(None);
        };

        let is_ctrl_pressed = Self::is_ctrl_pressed();
        let is_shift_pressed = Self::is_shift_pressed();
        let is_ctrl_space = is_ctrl_pressed && wparam.0 == 0x20;
        let is_ctrl_enter = is_ctrl_pressed && wparam.0 == 0x0D;
        let is_ctrl_down = is_ctrl_pressed && wparam.0 == 0x28;
        let is_shift_left = is_shift_pressed && wparam.0 == 0x25;
        let is_shift_right = is_shift_pressed && wparam.0 == 0x27;
        let is_shift_key = Self::is_shift_key(wparam);
        let is_alt_backquote = Self::is_alt_backquote(wparam, lparam);
        let app_config = AppConfig::read();

        // check shortcut keys
        if is_ctrl_pressed && !is_ctrl_space && !is_alt_backquote && !is_ctrl_enter && !is_ctrl_down
        {
            self.clear_temporary_latin_shift_pending_if_needed(!Self::is_shift_key(wparam))?;
            return Ok(None);
        }

        if is_ctrl_space || is_alt_backquote {
            let shortcuts = &app_config.shortcuts;

            if is_ctrl_space && !shortcuts.ctrl_space_toggle {
                self.clear_temporary_latin_shift_pending_if_needed(!Self::is_shift_key(wparam))?;
                return Ok(None);
            }

            if is_alt_backquote && !shortcuts.alt_backquote_toggle {
                self.clear_temporary_latin_shift_pending_if_needed(!Self::is_shift_key(wparam))?;
                return Ok(None);
            }
        }

        #[allow(clippy::let_and_return)]
        let (composition, mode) = {
            let text_service = self.borrow()?;
            let composition = text_service.borrow_composition()?.clone();
            let mode = IMEState::get()?.input_mode.clone();
            (composition, mode)
        };
        let start_temporary_latin = !composition.temporary_latin
            && mode == InputMode::Kana
            && Self::is_shift_alphabet_shortcut(wparam, is_shift_pressed);

        if composition.temporary_latin && is_shift_key && !is_shift_left && !is_shift_right {
            return Ok(Some((
                vec![ClientAction::SetTemporaryLatinShiftPending(true)],
                composition.state.clone(),
            )));
        }

        let should_clear_shift_pending = composition.temporary_latin_shift_pending && !is_shift_key;

        let action = if is_alt_backquote {
            UserAction::ToggleInputMode
        } else if is_ctrl_enter {
            UserAction::CommitFirstClause
        } else if is_ctrl_down {
            UserAction::CommitAndNextClause
        } else if is_shift_left {
            UserAction::AdjustClauseBoundary(-1)
        } else if is_shift_right {
            UserAction::AdjustClauseBoundary(1)
        } else {
            UserAction::try_from(wparam.0)?
        };

        let (transition, mut actions) = match composition.state {
            CompositionState::None => match action {
                _ if (composition.temporary_latin || start_temporary_latin)
                    && Self::direct_text_for_action(&action).is_some() =>
                {
                    let Some(text) = Self::direct_text_for_action(&action) else {
                        self.clear_temporary_latin_shift_pending_if_needed(
                            should_clear_shift_pending,
                        )?;
                        return Ok(None);
                    };

                    let mut actions = vec![ClientAction::StartComposition];
                    if start_temporary_latin {
                        actions.push(ClientAction::SetTemporaryLatin(true));
                    }
                    actions.push(ClientAction::AppendTextDirect(text));
                    (CompositionState::Composing, actions)
                }
                UserAction::NumpadSymbol(symbol) if mode == InputMode::Kana => {
                    let Some(text) =
                        Self::numpad_text_for_mode(symbol, app_config.general.numpad_input, true)
                    else {
                        self.clear_temporary_latin_shift_pending_if_needed(
                            should_clear_shift_pending,
                        )?;
                        return Ok(None);
                    };

                    (
                        CompositionState::Composing,
                        vec![
                            ClientAction::StartComposition,
                            ClientAction::AppendTextRaw(text),
                        ],
                    )
                }
                UserAction::Input(char) if mode == InputMode::Kana => (
                    CompositionState::Composing,
                    vec![
                        ClientAction::StartComposition,
                        ClientAction::AppendText(char.to_string()),
                    ],
                ),
                UserAction::Number {
                    value,
                    is_numpad: true,
                } if mode == InputMode::Kana => {
                    let digit = char::from_digit(value as u32, 10).unwrap_or('0');
                    let Some(text) =
                        Self::numpad_text_for_mode(digit, app_config.general.numpad_input, true)
                    else {
                        self.clear_temporary_latin_shift_pending_if_needed(
                            should_clear_shift_pending,
                        )?;
                        return Ok(None);
                    };

                    (
                        CompositionState::Composing,
                        vec![
                            ClientAction::StartComposition,
                            ClientAction::AppendTextRaw(text),
                        ],
                    )
                }
                UserAction::Number {
                    value,
                    is_numpad: false,
                } if mode == InputMode::Kana => (
                    CompositionState::Composing,
                    vec![
                        ClientAction::StartComposition,
                        ClientAction::AppendText(value.to_string()),
                    ],
                ),
                UserAction::Space if mode == InputMode::Kana => {
                    let mut use_halfwidth =
                        matches!(app_config.general.space_input, SpaceInputMode::AlwaysHalf);
                    if is_shift_pressed {
                        use_halfwidth = !use_halfwidth;
                    }
                    let space = if use_halfwidth { " " } else { "　" };
                    (
                        CompositionState::None,
                        vec![
                            ClientAction::StartComposition,
                            ClientAction::AppendText(space.to_string()),
                            ClientAction::EndComposition,
                        ],
                    )
                }
                UserAction::ToggleInputMode => (
                    CompositionState::None,
                    vec![match mode {
                        InputMode::Kana => ClientAction::SetIMEMode(InputMode::Latin),
                        InputMode::Latin => ClientAction::SetIMEMode(InputMode::Kana),
                    }],
                ),
                UserAction::InputModeOn => (
                    CompositionState::None,
                    vec![ClientAction::SetIMEMode(InputMode::Kana)],
                ),
                UserAction::InputModeOff => (
                    CompositionState::None,
                    vec![ClientAction::SetIMEMode(InputMode::Latin)],
                ),
                _ => {
                    self.clear_temporary_latin_shift_pending_if_needed(should_clear_shift_pending)?;
                    return Ok(None);
                }
            },
            CompositionState::Composing => match action {
                _ if (composition.temporary_latin || start_temporary_latin)
                    && Self::direct_text_for_action(&action).is_some() =>
                {
                    let Some(text) = Self::direct_text_for_action(&action) else {
                        self.clear_temporary_latin_shift_pending_if_needed(
                            should_clear_shift_pending,
                        )?;
                        return Ok(None);
                    };

                    let mut actions = vec![];
                    if start_temporary_latin {
                        actions.push(ClientAction::SetTemporaryLatin(true));
                    }
                    actions.push(ClientAction::AppendTextDirect(text));
                    (CompositionState::Composing, actions)
                }
                UserAction::NumpadSymbol(symbol) if mode == InputMode::Kana => {
                    let text =
                        Self::numpad_text_for_mode(symbol, app_config.general.numpad_input, false)
                            .unwrap_or_else(|| symbol.to_string());
                    (
                        CompositionState::Composing,
                        vec![ClientAction::AppendTextRaw(text)],
                    )
                }
                UserAction::Input(char) => (
                    CompositionState::Composing,
                    vec![ClientAction::AppendText(char.to_string())],
                ),
                UserAction::Number {
                    value,
                    is_numpad: true,
                } if mode == InputMode::Kana => {
                    let digit = char::from_digit(value as u32, 10).unwrap_or('0');
                    let text =
                        Self::numpad_text_for_mode(digit, app_config.general.numpad_input, false)
                            .unwrap_or_else(|| digit.to_string());
                    (
                        CompositionState::Composing,
                        vec![ClientAction::AppendTextRaw(text)],
                    )
                }
                UserAction::Number { value, .. } => (
                    CompositionState::Composing,
                    vec![ClientAction::AppendText(value.to_string())],
                ),
                UserAction::Backspace => {
                    // preview length can differ from raw input (e.g. "れい" -> "例"),
                    // so decide end-of-composition by remaining raw input length.
                    if composition.raw_input.chars().count() <= 1 {
                        (
                            CompositionState::None,
                            vec![ClientAction::RemoveText, ClientAction::EndComposition],
                        )
                    } else {
                        (CompositionState::Composing, vec![ClientAction::RemoveText])
                    }
                }
                UserAction::Enter | UserAction::CommitAndNextClause => {
                    Self::commit_current_clause_actions(&composition)
                }
                UserAction::CommitFirstClause => Self::commit_first_clause_actions(&composition),
                UserAction::AdjustClauseBoundary(direction) => (
                    CompositionState::Composing,
                    vec![ClientAction::AdjustBoundary(direction)],
                ),
                UserAction::Escape => (
                    CompositionState::None,
                    vec![ClientAction::RemoveText, ClientAction::EndComposition],
                ),
                UserAction::Navigation(ref direction) => match direction {
                    Navigation::Right => (
                        CompositionState::Composing,
                        vec![ClientAction::MoveClause(1)],
                    ),
                    Navigation::Left => (
                        CompositionState::Composing,
                        vec![ClientAction::MoveClause(-1)],
                    ),
                    Navigation::Up => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetSelection(SetSelectionType::Up)],
                    ),
                    Navigation::Down => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetSelection(SetSelectionType::Down)],
                    ),
                },
                UserAction::ToggleInputMode => (
                    CompositionState::None,
                    vec![
                        ClientAction::EndComposition,
                        ClientAction::SetIMEMode(InputMode::Latin),
                    ],
                ),
                UserAction::InputModeOn => (
                    CompositionState::None,
                    vec![
                        ClientAction::EndComposition,
                        ClientAction::SetIMEMode(InputMode::Kana),
                    ],
                ),
                UserAction::InputModeOff => (
                    CompositionState::None,
                    vec![
                        ClientAction::EndComposition,
                        ClientAction::SetIMEMode(InputMode::Latin),
                    ],
                ),
                UserAction::Space | UserAction::Tab => (
                    CompositionState::Previewing,
                    vec![ClientAction::SetSelection(SetSelectionType::Down)],
                ),
                UserAction::Function(ref key) => match key {
                    Function::Six => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::Hiragana)],
                    ),
                    Function::Seven => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::Katakana)],
                    ),
                    Function::Eight => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::HalfKatakana)],
                    ),
                    Function::Nine => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::FullLatin)],
                    ),
                    Function::Ten => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::HalfLatin)],
                    ),
                },
                _ => {
                    self.clear_temporary_latin_shift_pending_if_needed(should_clear_shift_pending)?;
                    return Ok(None);
                }
            },
            CompositionState::Previewing => match action {
                _ if (composition.temporary_latin || start_temporary_latin)
                    && Self::direct_text_for_action(&action).is_some() =>
                {
                    let Some(text) = Self::direct_text_for_action(&action) else {
                        self.clear_temporary_latin_shift_pending_if_needed(
                            should_clear_shift_pending,
                        )?;
                        return Ok(None);
                    };

                    let mut actions = vec![];
                    if start_temporary_latin {
                        actions.push(ClientAction::SetTemporaryLatin(true));
                    }
                    actions.push(ClientAction::ShrinkTextDirect(text));
                    (CompositionState::Composing, actions)
                }
                UserAction::NumpadSymbol(symbol) if mode == InputMode::Kana => {
                    let text =
                        Self::numpad_text_for_mode(symbol, app_config.general.numpad_input, false)
                            .unwrap_or_else(|| symbol.to_string());
                    (
                        CompositionState::Composing,
                        vec![ClientAction::ShrinkTextRaw(text)],
                    )
                }
                UserAction::Input(char) => (
                    CompositionState::Composing,
                    vec![ClientAction::ShrinkText(char.to_string())],
                ),
                UserAction::Number {
                    value,
                    is_numpad: true,
                } if mode == InputMode::Kana => {
                    let digit = char::from_digit(value as u32, 10).unwrap_or('0');
                    let text =
                        Self::numpad_text_for_mode(digit, app_config.general.numpad_input, false)
                            .unwrap_or_else(|| digit.to_string());
                    (
                        CompositionState::Composing,
                        vec![ClientAction::ShrinkTextRaw(text)],
                    )
                }
                UserAction::Number { value, .. } => (
                    CompositionState::Composing,
                    vec![ClientAction::ShrinkText(value.to_string())],
                ),
                UserAction::Backspace => {
                    // preview length can differ from raw input (e.g. "れい" -> "例"),
                    // so decide end-of-composition by remaining raw input length.
                    if composition.raw_input.chars().count() <= 1 {
                        (
                            CompositionState::None,
                            vec![ClientAction::RemoveText, ClientAction::EndComposition],
                        )
                    } else {
                        (CompositionState::Composing, vec![ClientAction::RemoveText])
                    }
                }
                UserAction::Enter | UserAction::CommitAndNextClause => {
                    Self::commit_current_clause_actions(&composition)
                }
                UserAction::CommitFirstClause => Self::commit_first_clause_actions(&composition),
                UserAction::AdjustClauseBoundary(direction) => (
                    CompositionState::Previewing,
                    vec![ClientAction::AdjustBoundary(direction)],
                ),
                UserAction::Escape => (
                    CompositionState::None,
                    vec![ClientAction::RemoveText, ClientAction::EndComposition],
                ),
                UserAction::Navigation(ref direction) => match direction {
                    Navigation::Right => (
                        CompositionState::Composing,
                        vec![ClientAction::MoveClause(1)],
                    ),
                    Navigation::Left => (
                        CompositionState::Composing,
                        vec![ClientAction::MoveClause(-1)],
                    ),
                    Navigation::Up => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetSelection(SetSelectionType::Up)],
                    ),
                    Navigation::Down => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetSelection(SetSelectionType::Down)],
                    ),
                },
                UserAction::ToggleInputMode => (
                    CompositionState::None,
                    vec![
                        ClientAction::EndComposition,
                        ClientAction::SetIMEMode(InputMode::Latin),
                    ],
                ),
                UserAction::InputModeOn => (
                    CompositionState::None,
                    vec![
                        ClientAction::EndComposition,
                        ClientAction::SetIMEMode(InputMode::Kana),
                    ],
                ),
                UserAction::InputModeOff => (
                    CompositionState::None,
                    vec![
                        ClientAction::EndComposition,
                        ClientAction::SetIMEMode(InputMode::Latin),
                    ],
                ),
                UserAction::Space | UserAction::Tab => (
                    CompositionState::Previewing,
                    vec![ClientAction::SetSelection(SetSelectionType::Down)],
                ),
                UserAction::Function(ref key) => match key {
                    Function::Six => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::Hiragana)],
                    ),
                    Function::Seven => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::Katakana)],
                    ),
                    Function::Eight => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::HalfKatakana)],
                    ),
                    Function::Nine => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::FullLatin)],
                    ),
                    Function::Ten => (
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::HalfLatin)],
                    ),
                },
                _ => {
                    self.clear_temporary_latin_shift_pending_if_needed(should_clear_shift_pending)?;
                    return Ok(None);
                }
            },
            _ => {
                self.clear_temporary_latin_shift_pending_if_needed(should_clear_shift_pending)?;
                return Ok(None);
            }
        };

        if composition.temporary_latin {
            let should_reset_on_confirm = matches!(
                action,
                UserAction::Enter | UserAction::CommitAndNextClause | UserAction::CommitFirstClause
            );
            let should_reset_on_end = transition == CompositionState::None
                || actions.iter().any(|current_action| {
                    matches!(
                        current_action,
                        ClientAction::EndComposition | ClientAction::SetIMEMode(_)
                    )
                });

            if (should_reset_on_confirm || should_reset_on_end)
                && !actions.iter().any(|current_action| {
                    matches!(current_action, ClientAction::SetTemporaryLatin(false))
                })
            {
                actions.insert(0, ClientAction::SetTemporaryLatin(false));
            }
        }

        if should_clear_shift_pending
            && !actions.iter().any(|current_action| {
                matches!(
                    current_action,
                    ClientAction::SetTemporaryLatinShiftPending(_)
                )
            })
        {
            actions.insert(0, ClientAction::SetTemporaryLatinShiftPending(false));
        }

        Ok(Some((actions, transition)))
    }

    #[tracing::instrument]
    pub fn process_key_up(
        &self,
        context: Option<&ITfContext>,
        wparam: WPARAM,
        _lparam: LPARAM,
    ) -> Result<Option<(Vec<ClientAction>, CompositionState)>> {
        if context.is_none() || !Self::is_shift_key(wparam) {
            return Ok(None);
        }

        let composition = {
            let text_service = self.borrow()?;
            let composition = text_service.borrow_composition()?.clone();
            composition
        };

        if !composition.temporary_latin_shift_pending {
            return Ok(None);
        }

        let mut actions = vec![ClientAction::SetTemporaryLatinShiftPending(false)];
        if composition.temporary_latin {
            actions.insert(0, ClientAction::SetTemporaryLatin(false));
        }

        Ok(Some((actions, composition.state.clone())))
    }

    #[tracing::instrument]
    pub fn handle_key(
        &self,
        context: Option<&ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<bool> {
        let result: Result<bool> = (|| {
            if let Some(context) = context {
                self.borrow_mut()?.context = Some(context.clone());
            } else {
                return Ok(false);
            };

            if let Some((actions, transition)) = self.process_key(context, wparam, lparam)? {
                self.handle_action(&actions, transition)?;
                Ok(true)
            } else {
                Ok(false)
            }
        })();

        match result {
            Ok(handled) => Ok(handled),
            Err(error) => {
                tracing::error!("handle_key failed: {error:?}");
                self.recover_after_key_error();
                Ok(false)
            }
        }
    }

    #[tracing::instrument]
    pub fn handle_key_up(
        &self,
        context: Option<&ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<bool> {
        let result: Result<bool> = (|| {
            if let Some(context) = context {
                self.borrow_mut()?.context = Some(context.clone());
            } else {
                return Ok(false);
            };

            if let Some((actions, transition)) = self.process_key_up(context, wparam, lparam)? {
                self.handle_action(&actions, transition)?;
                return Ok(true);
            }

            Ok(false)
        })();

        match result {
            Ok(handled) => Ok(handled),
            Err(error) => {
                tracing::error!("handle_key_up failed: {error:?}");
                self.recover_after_key_error();
                Ok(false)
            }
        }
    }

    fn recover_after_key_error(&self) {
        let _ = self.abort_composition();

        if let Ok(text_service) = self.borrow() {
            if let Ok(mut composition) = text_service.borrow_mut_composition() {
                *composition = Composition::default();
            }
        }

        if let Ok(mut ime_state) = IMEState::get() {
            if let Some(mut ipc_service) = ime_state.ipc_service.clone() {
                let _ = ipc_service.hide_window();
                let _ = ipc_service.set_candidates(vec![]);
                let _ = ipc_service.clear_text();
                ime_state.ipc_service = Some(ipc_service);
            }
        }
    }

    #[tracing::instrument]
    pub fn handle_action(
        &self,
        actions: &[ClientAction],
        transition: CompositionState,
    ) -> Result<()> {
        #[allow(clippy::let_and_return)]
        let (composition, mode) = {
            let text_service = self.borrow()?;
            let composition = text_service.borrow_composition()?.clone();
            let mode = IMEState::get()?.input_mode.clone();
            (composition, mode)
        };
        let app_config = AppConfig::read();

        let mut preview = composition.preview.clone();
        let mut suffix = composition.suffix.clone();
        let mut raw_input = composition.raw_input.clone();
        let mut raw_hiragana = composition.raw_hiragana.clone();
        let mut fixed_prefix = composition.fixed_prefix.clone();
        let mut corresponding_count = composition.corresponding_count.clone();
        let mut candidates = composition.candidates.clone();
        let mut clause_snapshots = composition.clause_snapshots.clone();
        let mut future_clause_snapshots = composition.future_clause_snapshots.clone();
        let mut selection_index = composition.selection_index;
        let mut temporary_latin = composition.temporary_latin;
        let mut temporary_latin_shift_pending = composition.temporary_latin_shift_pending;
        let mut ipc_service = IMEState::get()?
            .ipc_service
            .clone()
            .context("ipc_service is None")?;
        let mut transition = transition;

        self.update_context(&preview)?;

        for action in actions {
            match action {
                ClientAction::StartComposition => {
                    self.start_composition()?;
                    self.update_pos()?;
                    ipc_service.show_window()?;
                }
                ClientAction::EndComposition => {
                    self.end_composition()?;
                    selection_index = 0;
                    corresponding_count = 0;
                    temporary_latin = false;
                    temporary_latin_shift_pending = false;
                    preview.clear();
                    suffix.clear();
                    raw_input.clear();
                    raw_hiragana.clear();
                    fixed_prefix.clear();
                    clause_snapshots.clear();
                    future_clause_snapshots.clear();
                    ipc_service.hide_window()?;
                    ipc_service.set_candidates(vec![])?;
                    ipc_service.clear_text()?;
                }
                ClientAction::AppendText(text) => {
                    Self::clear_clause_caches(
                        &mut clause_snapshots,
                        &mut future_clause_snapshots,
                        &mut ipc_service,
                    )?;
                    let resolved_symbol_text = match mode {
                        InputMode::Kana => {
                            Self::resolve_symbol_input_text(&raw_input, text, &app_config)
                        }
                        InputMode::Latin => None,
                    };
                    let text = match mode {
                        InputMode::Kana => resolved_symbol_text.unwrap_or_else(|| text.to_string()),
                        InputMode::Latin => text.to_string(),
                    };

                    candidates = ipc_service.append_text_with_context(text.clone(), &candidates)?;
                    raw_input.push_str(&text);
                    if let Some(selected) = Self::select_candidate(&candidates, selection_index) {
                        selection_index = selected.index;
                        corresponding_count = selected.corresponding_count;
                        preview = Self::merge_preview_with_prefix(&fixed_prefix, &selected.text);
                        suffix = selected.sub_text.clone();
                        raw_hiragana = selected.hiragana;

                        self.set_text(&preview, &suffix)?;
                        ipc_service.set_candidates(candidates.texts.clone())?;
                        ipc_service.set_selection(selection_index)?;
                    }
                }
                ClientAction::AppendTextRaw(text) => {
                    Self::clear_clause_caches(
                        &mut clause_snapshots,
                        &mut future_clause_snapshots,
                        &mut ipc_service,
                    )?;
                    candidates = ipc_service.append_text_with_context(text.clone(), &candidates)?;
                    raw_input.push_str(text);
                    if let Some(selected) = Self::select_candidate(&candidates, selection_index) {
                        selection_index = selected.index;
                        corresponding_count = selected.corresponding_count;
                        preview = Self::merge_preview_with_prefix(&fixed_prefix, &selected.text);
                        suffix = selected.sub_text.clone();
                        raw_hiragana = selected.hiragana;

                        self.set_text(&preview, &suffix)?;
                        ipc_service.set_candidates(candidates.texts.clone())?;
                        ipc_service.set_selection(selection_index)?;
                    }
                }
                ClientAction::AppendTextDirect(text) => {
                    Self::clear_clause_caches(
                        &mut clause_snapshots,
                        &mut future_clause_snapshots,
                        &mut ipc_service,
                    )?;
                    candidates =
                        ipc_service.append_text_direct_with_context(text.clone(), &candidates)?;
                    raw_input.push_str(text);
                    if let Some(selected) = Self::select_candidate(&candidates, selection_index) {
                        selection_index = selected.index;
                        corresponding_count = selected.corresponding_count;
                        preview = Self::merge_preview_with_prefix(&fixed_prefix, &selected.text);
                        suffix = selected.sub_text.clone();
                        raw_hiragana = selected.hiragana;

                        self.set_text(&preview, &suffix)?;
                        ipc_service.set_candidates(candidates.texts.clone())?;
                        ipc_service.set_selection(selection_index)?;
                    }
                }
                ClientAction::RemoveText => {
                    Self::clear_clause_caches(
                        &mut clause_snapshots,
                        &mut future_clause_snapshots,
                        &mut ipc_service,
                    )?;
                    raw_input.pop();
                    candidates = ipc_service.remove_text()?;
                    if let Some(selected) = Self::select_candidate(&candidates, selection_index) {
                        selection_index = selected.index;
                        corresponding_count = selected.corresponding_count;

                        preview = Self::merge_preview_with_prefix(&fixed_prefix, &selected.text);
                        suffix = selected.sub_text.clone();
                        raw_hiragana = selected.hiragana;

                        self.set_text(&preview, &suffix)?;
                        ipc_service.set_candidates(candidates.texts.clone())?;
                        ipc_service.set_selection(selection_index)?;
                    } else {
                        // Server side text is fully removed. Close TSF composition too
                        // so preedit text does not linger in an inconsistent state.
                        let committed_prefix = fixed_prefix.clone();

                        transition = CompositionState::None;
                        selection_index = 0;
                        corresponding_count = 0;
                        temporary_latin = false;
                        temporary_latin_shift_pending = false;
                        suffix.clear();
                        raw_input.clear();
                        raw_hiragana.clear();
                        clause_snapshots.clear();
                        future_clause_snapshots.clear();

                        if committed_prefix.is_empty() {
                            self.set_text("", "")?;
                        } else {
                            self.set_text(&committed_prefix, "")?;
                        }
                        self.end_composition()?;
                        ipc_service.hide_window()?;
                        ipc_service.set_candidates(vec![])?;
                        ipc_service.clear_text()?;

                        preview.clear();
                        fixed_prefix.clear();
                    }
                }
                ClientAction::MoveCursor(offset) => {
                    candidates = ipc_service.move_cursor(*offset)?;
                    if let Some(selected) = Self::select_candidate(&candidates, selection_index) {
                        selection_index = selected.index;
                        corresponding_count = selected.corresponding_count;
                        preview = Self::merge_preview_with_prefix(&fixed_prefix, &selected.text);
                        suffix = selected.sub_text.clone();
                        raw_hiragana = selected.hiragana;

                        self.set_text(&preview, &suffix)?;
                        ipc_service.set_candidates(candidates.texts.clone())?;
                        ipc_service.set_selection(selection_index)?;
                        self.update_pos()?;
                    }
                }
                ClientAction::MoveClause(direction) => {
                    if *direction > 0 {
                        if suffix.is_empty() {
                            continue;
                        }

                        let snapshot = Self::build_clause_snapshot(
                            &preview,
                            &suffix,
                            &raw_input,
                            &raw_hiragana,
                            &fixed_prefix,
                            corresponding_count,
                            selection_index,
                            &candidates,
                        );
                        let current_clause_preview =
                            Self::current_clause_preview(&preview, &fixed_prefix);
                        let current_corresponding_count = corresponding_count;

                        ipc_service.move_cursor(Self::MOVE_CURSOR_PUSH_CLAUSE_SNAPSHOT)?;
                        clause_snapshots.push(snapshot);

                        candidates = ipc_service.shrink_text(current_corresponding_count)?;
                        selection_index = 0;
                        raw_input = raw_input
                            .chars()
                            .skip(current_corresponding_count.max(0) as usize)
                            .collect();
                        fixed_prefix.push_str(&current_clause_preview);

                        if Self::future_snapshot_matches_server(
                            &future_clause_snapshots,
                            &candidates,
                        ) {
                            if let Some(restored_future) = future_clause_snapshots.pop() {
                                Self::restore_future_clause_snapshot(
                                    &mut preview,
                                    &mut suffix,
                                    &mut raw_input,
                                    &mut raw_hiragana,
                                    &mut corresponding_count,
                                    &mut selection_index,
                                    &mut candidates,
                                    &fixed_prefix,
                                    &restored_future,
                                );
                                suffix = Self::sync_current_clause_future_suffix(
                                    &mut candidates,
                                    selection_index,
                                    corresponding_count,
                                    &future_clause_snapshots,
                                );
                                self.set_text(&preview, &suffix)?;
                                ipc_service.set_candidates(candidates.texts.clone())?;
                                ipc_service.set_selection(selection_index)?;
                                self.update_pos()?;
                            }
                        } else if let Some(selected) =
                            Self::select_candidate(&candidates, selection_index)
                        {
                            if !future_clause_snapshots.is_empty() {
                                future_clause_snapshots.clear();
                            }
                            selection_index = selected.index;
                            corresponding_count = selected.corresponding_count;
                            preview =
                                Self::merge_preview_with_prefix(&fixed_prefix, &selected.text);
                            suffix = selected.sub_text.clone();
                            raw_hiragana = selected.hiragana;

                            self.set_text(&preview, &suffix)?;
                            ipc_service.set_candidates(candidates.texts.clone())?;
                            ipc_service.set_selection(selection_index)?;
                            self.update_pos()?;
                        } else {
                            ipc_service.move_cursor(Self::MOVE_CURSOR_POP_CLAUSE_SNAPSHOT)?;
                            if let Some(restored) = clause_snapshots.pop() {
                                preview = restored.preview;
                                suffix = restored.suffix;
                                raw_input = restored.raw_input;
                                raw_hiragana = restored.raw_hiragana;
                                fixed_prefix = restored.fixed_prefix;
                                corresponding_count = restored.corresponding_count;
                                selection_index = restored.selection_index;
                                candidates = restored.candidates;

                                self.set_text(&preview, &suffix)?;
                                ipc_service.set_candidates(candidates.texts.clone())?;
                                ipc_service.set_selection(selection_index)?;
                                self.update_pos()?;
                            }
                        }
                    } else if *direction < 0 {
                        if let Some(restored) = clause_snapshots.pop() {
                            Self::push_current_future_clause_snapshot(
                                &mut future_clause_snapshots,
                                &preview,
                                &suffix,
                                &raw_input,
                                &raw_hiragana,
                                &fixed_prefix,
                                corresponding_count,
                                selection_index,
                                &candidates,
                            );
                            ipc_service.move_cursor(Self::MOVE_CURSOR_POP_CLAUSE_SNAPSHOT)?;

                            preview = restored.preview;
                            suffix = restored.suffix;
                            raw_input = restored.raw_input;
                            raw_hiragana = restored.raw_hiragana;
                            fixed_prefix = restored.fixed_prefix;
                            corresponding_count = restored.corresponding_count;
                            selection_index = restored.selection_index;
                            candidates = restored.candidates;

                            self.set_text(&preview, &suffix)?;
                            ipc_service.set_candidates(candidates.texts.clone())?;
                            ipc_service.set_selection(selection_index)?;
                            self.update_pos()?;
                        }
                    }
                }
                ClientAction::AdjustBoundary(direction) => {
                    if *direction == 0 {
                        continue;
                    }

                    ipc_service.move_cursor(*direction)?;
                    let boundary_candidates = ipc_service.move_cursor(0)?;
                    if boundary_candidates.texts.is_empty() {
                        // Keep at least one hiragana on the left side for clause adjustment.
                        // If cursor moved to head, rollback so Shift+Left works as no-op.
                        if *direction < 0 {
                            let _ = ipc_service.move_cursor(1)?;
                        }
                        continue;
                    }

                    candidates = boundary_candidates;
                    if let Some(selected) = Self::select_candidate(&candidates, 0) {
                        selection_index = selected.index;
                        corresponding_count = selected.corresponding_count;
                        preview = Self::merge_preview_with_prefix(&fixed_prefix, &selected.text);
                        raw_hiragana = selected.hiragana;
                        Self::maybe_push_split_future_clause_snapshot(
                            &mut future_clause_snapshots,
                            &raw_input,
                            &raw_hiragana,
                            corresponding_count,
                            &selected.sub_text,
                        );
                        suffix = Self::sync_current_clause_future_suffix(
                            &mut candidates,
                            selection_index,
                            corresponding_count,
                            &future_clause_snapshots,
                        );
                        Self::sync_clause_snapshot_suffixes(
                            &mut clause_snapshots,
                            &preview,
                            &suffix,
                        );

                        self.set_text(&preview, &suffix)?;
                        ipc_service.set_candidates(candidates.texts.clone())?;
                        ipc_service.set_selection(selection_index)?;
                        self.update_pos()?;
                    }
                }
                ClientAction::SetIMEMode(mode) => {
                    self.start_composition()?;
                    self.update_pos()?;
                    self.end_composition()?;

                    let mut ime_state = IMEState::get()?;
                    ime_state.input_mode = mode.clone();

                    // update the language bar
                    self.update_lang_bar()?;

                    let mode = match mode {
                        InputMode::Latin => "A",
                        InputMode::Kana => "あ",
                    };

                    ipc_service.set_input_mode(mode)?;

                    selection_index = 0;
                    corresponding_count = 0;
                    temporary_latin = false;
                    temporary_latin_shift_pending = false;
                    preview.clear();
                    suffix.clear();
                    raw_input.clear();
                    raw_hiragana.clear();
                    fixed_prefix.clear();
                    clause_snapshots.clear();
                    future_clause_snapshots.clear();
                    ipc_service.clear_text()?;
                }
                ClientAction::SetSelection(selection) => {
                    let desired_index = match selection {
                        SetSelectionType::Up => selection_index - 1,
                        SetSelectionType::Down => selection_index + 1,
                        SetSelectionType::Number(number) => *number,
                    };

                    if let Some(selected) = Self::select_candidate(&candidates, desired_index) {
                        selection_index = selected.index;
                        corresponding_count = selected.corresponding_count;
                        preview = Self::merge_preview_with_prefix(&fixed_prefix, &selected.text);
                        raw_hiragana = selected.hiragana;
                        suffix = Self::sync_current_clause_future_suffix(
                            &mut candidates,
                            selection_index,
                            corresponding_count,
                            &future_clause_snapshots,
                        );
                        Self::sync_clause_snapshot_suffixes(
                            &mut clause_snapshots,
                            &preview,
                            &suffix,
                        );

                        ipc_service.set_selection(selection_index)?;
                        self.set_text(&preview, &suffix)?;
                    }
                }
                ClientAction::ShrinkText(text) => {
                    fixed_prefix.clear();
                    Self::clear_clause_caches(
                        &mut clause_snapshots,
                        &mut future_clause_snapshots,
                        &mut ipc_service,
                    )?;
                    let shrunk_raw_input: String = raw_input
                        .chars()
                        .skip(corresponding_count.max(0) as usize)
                        .collect();
                    let resolved_symbol_text = match mode {
                        InputMode::Kana => {
                            Self::resolve_symbol_input_text(&shrunk_raw_input, text, &app_config)
                        }
                        InputMode::Latin => None,
                    };
                    let mut updated_raw_input = shrunk_raw_input.clone();
                    updated_raw_input.push_str(text);

                    let shrunk_candidates = ipc_service.shrink_text(corresponding_count)?;
                    let text = match mode {
                        InputMode::Kana => resolved_symbol_text.unwrap_or_else(|| text.to_string()),
                        InputMode::Latin => text.to_string(),
                    };
                    candidates = ipc_service.append_text_with_context(text, &shrunk_candidates)?;
                    raw_input = updated_raw_input;
                    selection_index = 0;

                    if let Some(selected) = Self::select_candidate(&candidates, selection_index) {
                        self.shift_start(&preview, &selected.text)?;

                        selection_index = selected.index;
                        corresponding_count = selected.corresponding_count;
                        preview = selected.text.clone();
                        suffix = selected.sub_text.clone();
                        raw_hiragana = selected.hiragana;

                        ipc_service.set_candidates(candidates.texts.clone())?;
                        ipc_service.set_selection(selection_index)?;
                        self.update_pos()?;
                    }

                    transition = CompositionState::Composing;
                }
                ClientAction::ShrinkTextRaw(text) => {
                    fixed_prefix.clear();
                    Self::clear_clause_caches(
                        &mut clause_snapshots,
                        &mut future_clause_snapshots,
                        &mut ipc_service,
                    )?;
                    let mut updated_raw_input: String = raw_input
                        .chars()
                        .skip(corresponding_count.max(0) as usize)
                        .collect();
                    updated_raw_input.push_str(text);

                    let shrunk_candidates = ipc_service.shrink_text(corresponding_count)?;
                    candidates =
                        ipc_service.append_text_with_context(text.clone(), &shrunk_candidates)?;
                    raw_input = updated_raw_input;
                    selection_index = 0;

                    if let Some(selected) = Self::select_candidate(&candidates, selection_index) {
                        self.shift_start(&preview, &selected.text)?;

                        selection_index = selected.index;
                        corresponding_count = selected.corresponding_count;
                        preview = selected.text.clone();
                        suffix = selected.sub_text.clone();
                        raw_hiragana = selected.hiragana;

                        ipc_service.set_candidates(candidates.texts.clone())?;
                        ipc_service.set_selection(selection_index)?;
                        self.update_pos()?;
                    }

                    transition = CompositionState::Composing;
                }
                ClientAction::ShrinkTextDirect(text) => {
                    fixed_prefix.clear();
                    Self::clear_clause_caches(
                        &mut clause_snapshots,
                        &mut future_clause_snapshots,
                        &mut ipc_service,
                    )?;
                    let mut updated_raw_input: String = raw_input
                        .chars()
                        .skip(corresponding_count.max(0) as usize)
                        .collect();
                    updated_raw_input.push_str(text);

                    let shrunk_candidates = ipc_service.shrink_text(corresponding_count)?;
                    candidates = ipc_service
                        .append_text_direct_with_context(text.clone(), &shrunk_candidates)?;
                    raw_input = updated_raw_input;
                    selection_index = 0;

                    if let Some(selected) = Self::select_candidate(&candidates, selection_index) {
                        self.shift_start(&preview, &selected.text)?;

                        selection_index = selected.index;
                        corresponding_count = selected.corresponding_count;
                        preview = selected.text.clone();
                        suffix = selected.sub_text.clone();
                        raw_hiragana = selected.hiragana;

                        ipc_service.set_candidates(candidates.texts.clone())?;
                        ipc_service.set_selection(selection_index)?;
                        self.update_pos()?;
                    }

                    transition = CompositionState::Composing;
                }
                ClientAction::SetTemporaryLatin(is_temporary_latin) => {
                    temporary_latin = *is_temporary_latin;
                    if !temporary_latin {
                        temporary_latin_shift_pending = false;
                    }
                }
                ClientAction::SetTemporaryLatinShiftPending(is_shift_pending) => {
                    temporary_latin_shift_pending = *is_shift_pending;
                }
                ClientAction::SetTextWithType(set_type) => {
                    let text = match set_type {
                        SetTextType::Hiragana => raw_hiragana.clone(),
                        SetTextType::Katakana => to_katakana(&raw_hiragana),
                        SetTextType::HalfKatakana => to_half_katakana(&raw_hiragana),
                        SetTextType::FullLatin => to_fullwidth(&raw_input, true),
                        SetTextType::HalfLatin => to_halfwidth(&raw_input),
                    };

                    self.set_text(&text, "")?;
                }
            }
        }

        let text_service = self.borrow()?;
        let mut composition = text_service.borrow_mut_composition()?;

        composition.preview = preview.clone();
        composition.state = transition;
        composition.selection_index = selection_index;
        composition.raw_input = raw_input.clone();
        composition.raw_hiragana = raw_hiragana.clone();
        composition.fixed_prefix = fixed_prefix.clone();
        composition.candidates = candidates;
        composition.clause_snapshots = clause_snapshots;
        composition.future_clause_snapshots = future_clause_snapshots;
        composition.suffix = suffix.clone();
        composition.corresponding_count = corresponding_count;
        composition.temporary_latin = temporary_latin;
        composition.temporary_latin_shift_pending = temporary_latin_shift_pending;

        drop(composition);
        drop(text_service);

        if let Ok(mut ime_state) = IMEState::get() {
            ime_state.ipc_service = Some(ipc_service);
        } else {
            tracing::warn!("Failed to persist updated IPC service into IMEState");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Candidates, TextServiceFactory};
    use shared::{get_default_romaji_rows, AppConfig, RomajiRule, WidthMode};

    fn row(input: &str, output: &str, next_input: &str) -> RomajiRule {
        RomajiRule {
            input: input.to_string(),
            output: output.to_string(),
            next_input: next_input.to_string(),
        }
    }

    fn candidates(
        texts: &[&str],
        sub_texts: &[&str],
        hiragana: &str,
        corresponding_count: &[i32],
    ) -> Candidates {
        Candidates {
            texts: texts.iter().map(|value| (*value).to_string()).collect(),
            sub_texts: sub_texts.iter().map(|value| (*value).to_string()).collect(),
            hiragana: hiragana.to_string(),
            corresponding_count: corresponding_count.to_vec(),
        }
    }

    fn actual_future_snapshot(
        clause_preview: &str,
        suffix: &str,
        raw_input: &str,
        raw_hiragana: &str,
        corresponding_count: i32,
    ) -> super::FutureClauseSnapshot {
        TextServiceFactory::build_future_clause_snapshot(
            clause_preview,
            suffix,
            raw_input,
            raw_hiragana,
            "",
            corresponding_count,
            0,
            &candidates(
                &[clause_preview],
                &[suffix],
                raw_hiragana,
                &[corresponding_count],
            ),
        )
    }

    #[test]
    fn future_clause_snapshot_uses_relative_clause_preview() {
        let snapshot = TextServiceFactory::build_future_clause_snapshot(
            "いいかげん",
            "とういつしろ",
            "kagentouitusiro",
            "かげんとういつしろ",
            "いい",
            4,
            0,
            &candidates(&["かげん"], &["とういつしろ"], "かげんとういつしろ", &[4]),
        );

        assert_eq!(snapshot.clause_preview, "かげん");
        assert_eq!(snapshot.selected_text, "かげん");
    }

    #[test]
    fn move_clause_left_pushes_current_clause_into_future_cache() {
        let mut future = vec![
            TextServiceFactory::build_conservative_future_clause_snapshot(
                "しろ", "", "siro", "しろ", 2,
            ),
        ];

        TextServiceFactory::push_current_future_clause_snapshot(
            &mut future,
            "いいかげん",
            "とういつしろ",
            "kagentouitusiro",
            "かげんとういつしろ",
            "いい",
            4,
            0,
            &candidates(&["かげん"], &["とういつしろ"], "かげんとういつしろ", &[4]),
        );

        assert_eq!(future.len(), 2);
        assert_eq!(
            future
                .last()
                .map(|snapshot| snapshot.clause_preview.as_str()),
            Some("かげん")
        );
        assert_eq!(
            TextServiceFactory::clause_texts_for_log("", "", &[], &future),
            "かげん / しろ"
        );
    }

    #[test]
    fn move_clause_right_restores_future_clause_without_dropping_following_clauses() {
        let trailing = TextServiceFactory::build_conservative_future_clause_snapshot(
            "しろ", "", "siro", "しろ", 2,
        );
        let restored = TextServiceFactory::build_future_clause_snapshot(
            "いいかげんとういつ",
            "しろ",
            "touitusiro",
            "とういつしろ",
            "いいかげん",
            4,
            0,
            &candidates(&["とういつ"], &["しろ"], "とういつしろ", &[4]),
        );
        let mut future = vec![trailing, restored];

        let restored = future.pop().expect("restored future snapshot");
        let mut preview = String::new();
        let mut suffix = String::new();
        let mut raw_input = String::new();
        let mut raw_hiragana = String::new();
        let mut corresponding_count = 0;
        let mut selection_index = 0;
        let mut restored_candidates = Candidates::default();
        TextServiceFactory::restore_future_clause_snapshot(
            &mut preview,
            &mut suffix,
            &mut raw_input,
            &mut raw_hiragana,
            &mut corresponding_count,
            &mut selection_index,
            &mut restored_candidates,
            "いいかげん",
            &restored,
        );
        suffix = TextServiceFactory::sync_current_clause_future_suffix(
            &mut restored_candidates,
            selection_index,
            corresponding_count,
            &future,
        );

        assert_eq!(preview, "いいかげんとういつ");
        assert_eq!(suffix, "しろ");
        assert_eq!(
            TextServiceFactory::clause_texts_for_log(&preview, "いいかげん", &[], &future),
            "とういつ / しろ"
        );
    }

    #[test]
    fn adjust_boundary_split_keeps_future_clause_sequence() {
        let mut future = vec![
            actual_future_snapshot("しろ", "", "siro", "しろ", 2),
            actual_future_snapshot("とういつ", "しろ", "touitusiro", "とういつしろ", 4),
        ];

        TextServiceFactory::maybe_push_split_future_clause_snapshot(
            &mut future,
            "kagentouitusiro",
            "かげんとういつしろ",
            4,
            "んとういつしろ",
        );

        assert_eq!(
            future
                .last()
                .map(|snapshot| snapshot.clause_preview.as_str()),
            Some("ん")
        );
        assert_eq!(
            future.last().map(|snapshot| snapshot.raw_input.as_str()),
            Some("ntouitusiro")
        );
        assert_eq!(
            TextServiceFactory::clause_raw_texts_for_log("", 0, &[], &future),
            "ん / とういつ / しろ"
        );
    }

    #[test]
    fn adjust_boundary_without_existing_future_cache_does_not_capture_split_clause() {
        let mut future = Vec::new();

        TextServiceFactory::maybe_push_split_future_clause_snapshot(
            &mut future,
            "iikagentouitusiro",
            "いいかげんとういつしろ",
            1,
            "いかげんとういつしろ",
        );

        assert!(future.is_empty());
    }

    #[test]
    fn adjust_boundary_replaces_existing_conservative_future_clause() {
        let mut future = vec![
            actual_future_snapshot("しろ", "", "siro", "しろ", 2),
            TextServiceFactory::build_conservative_future_clause_snapshot(
                "つ",
                "しろ",
                "tusiro",
                "つしろ",
                2,
            ),
        ];

        TextServiceFactory::maybe_push_split_future_clause_snapshot(
            &mut future,
            "touitusiro",
            "とういつしろ",
            3,
            "いつしろ",
        );

        assert_eq!(
            future
                .last()
                .map(|snapshot| snapshot.clause_preview.as_str()),
            Some("いつ")
        );
        assert_eq!(
            future.last().map(|snapshot| snapshot.suffix.as_str()),
            Some("しろ")
        );
        assert_eq!(
            future.last().map(|snapshot| snapshot.raw_input.as_str()),
            Some("itusiro")
        );
        assert_eq!(
            future.last().map(|snapshot| snapshot.raw_hiragana.as_str()),
            Some("いつしろ")
        );
    }

    #[test]
    fn adjust_boundary_trim_drops_stale_split_snapshot_before_rejoin() {
        let mut future = vec![
            actual_future_snapshot("しろ", "", "siro", "しろ", 2),
            actual_future_snapshot(
                "かげん",
                "とういつしろ",
                "kagentouitusiro",
                "かげんとういつしろ",
                5,
            ),
        ];

        TextServiceFactory::maybe_push_split_future_clause_snapshot(
            &mut future,
            "iikagentouitusiro",
            "いかげんとういつしろ",
            1,
            "いかげんとういつしろ",
        );
        TextServiceFactory::maybe_push_split_future_clause_snapshot(
            &mut future,
            "iikagentouitusiro",
            "かげんとういつしろ",
            2,
            "かげんとういつしろ",
        );

        assert_eq!(future.len(), 2);
        assert_eq!(
            future
                .last()
                .map(|snapshot| snapshot.clause_preview.as_str()),
            Some("かげん")
        );
    }

    #[test]
    fn restore_selection_prefers_exact_match_then_fallback() {
        let restored_candidates = candidates(
            &["候補A", "候補B", "候補C"],
            &["残り", "残り", "別"],
            "こうほ",
            &[2, 2, 1],
        );

        assert_eq!(
            TextServiceFactory::resolve_selection_index(
                &restored_candidates,
                "候補B",
                "残り",
                2,
                0,
            ),
            1
        );
        assert_eq!(
            TextServiceFactory::resolve_selection_index(
                &restored_candidates,
                "候補X",
                "残り",
                2,
                2,
            ),
            2
        );
    }

    #[test]
    fn adjust_boundary_prefers_hint_over_corresponding_count_suffix() {
        let mut future = vec![
            actual_future_snapshot("しろ", "", "siro", "しろ", 2),
            actual_future_snapshot("とういつ", "しろ", "touitusiro", "とういつしろ", 4),
        ];

        TextServiceFactory::maybe_push_split_future_clause_snapshot(
            &mut future,
            "iikagentouitusiro",
            "いいかげんとういつしろ",
            6,
            "んとういつしろ",
        );

        assert_eq!(
            future
                .last()
                .map(|snapshot| snapshot.clause_preview.as_str()),
            Some("ん")
        );
        assert_eq!(
            future.last().map(|snapshot| snapshot.raw_hiragana.as_str()),
            Some("んとういつしろ")
        );
    }

    #[test]
    fn symbol_fallback_is_disabled_in_romaji_context() {
        let rows = vec![row("z/", "・", "")];
        let should_apply = TextServiceFactory::should_apply_symbol_fallback("z", "/", &rows);
        assert!(!should_apply);
    }

    #[test]
    fn symbol_fallback_is_enabled_for_standalone_symbol() {
        let rows = vec![row("z/", "・", "")];
        let should_apply = TextServiceFactory::should_apply_symbol_fallback("abc", "/", &rows);
        assert!(should_apply);
    }

    #[test]
    fn symbol_fallback_is_disabled_for_non_symbol_input() {
        let rows = vec![row("ka", "か", "")];
        let should_apply = TextServiceFactory::should_apply_symbol_fallback("k", "a", &rows);
        assert!(!should_apply);
    }

    #[test]
    fn symbol_fallback_is_enabled_for_non_ascii_symbol_variant() {
        let rows = vec![row("ka", "か", "")];
        let should_apply = TextServiceFactory::should_apply_symbol_fallback("", "￥", &rows);
        assert!(should_apply);
    }

    #[test]
    fn symbol_fallback_is_disabled_for_non_ascii_symbol_in_romaji_context() {
        let rows = vec![row("n\\", "んー", "")];
        let should_apply = TextServiceFactory::should_apply_symbol_fallback("n", "￥", &rows);
        assert!(!should_apply);
    }

    #[test]
    fn single_symbol_romaji_output_matches_exact_symbol_rule() {
        let rows = vec![row("-", "ー", "")];
        let output = TextServiceFactory::single_symbol_romaji_output("-", &rows);
        assert_eq!(output, Some("ー".to_string()));
    }

    #[test]
    fn single_symbol_romaji_output_ignores_multi_character_rule() {
        let rows = vec![row("z/", "・", "")];
        let output = TextServiceFactory::single_symbol_romaji_output("/", &rows);
        assert_eq!(output, None);
    }

    #[test]
    fn zenzai_symbol_input_prefers_explicit_single_symbol_rule() {
        let mut app_config = AppConfig::default();
        app_config.zenzai.enable = true;
        app_config.zenzai.backend = "vulkan".to_string();
        app_config.character_width.groups.math_symbol = WidthMode::Half;
        app_config.romaji_table.rows = vec![row("-", "ー", "")];

        let output = TextServiceFactory::resolve_symbol_input_text("", "-", &app_config);
        assert_eq!(output, Some("ー".to_string()));
    }

    #[test]
    fn zenzai_symbol_input_falls_back_to_width_setting_without_symbol_rule() {
        let mut app_config = AppConfig::default();
        app_config.zenzai.enable = true;
        app_config.zenzai.backend = "vulkan".to_string();
        app_config.character_width.groups.math_symbol = WidthMode::Half;

        let output = TextServiceFactory::resolve_symbol_input_text("", "-", &app_config);
        assert_eq!(output, Some("-".to_string()));
    }

    #[test]
    fn zenzai_symbol_input_keeps_default_multi_character_dash_sequence() {
        let mut app_config = AppConfig::default();
        app_config.zenzai.enable = true;
        app_config.zenzai.backend = "vulkan".to_string();
        app_config.romaji_table.rows = get_default_romaji_rows();

        let output = TextServiceFactory::resolve_symbol_input_text("z", "-", &app_config);
        assert_eq!(output, None);
    }

    #[test]
    fn zenzai_symbol_input_keeps_default_multi_character_symbol_sequence() {
        let mut app_config = AppConfig::default();
        app_config.zenzai.enable = true;
        app_config.zenzai.backend = "vulkan".to_string();
        app_config.romaji_table.rows = get_default_romaji_rows();

        let output = TextServiceFactory::resolve_symbol_input_text("z", "/", &app_config);
        assert_eq!(output, None);
    }

    #[test]
    fn zenzai_symbol_input_keeps_default_n_apostrophe_sequence() {
        let mut app_config = AppConfig::default();
        app_config.zenzai.enable = true;
        app_config.zenzai.backend = "vulkan".to_string();
        app_config.romaji_table.rows = get_default_romaji_rows();

        let output = TextServiceFactory::resolve_symbol_input_text("n", "'", &app_config);
        assert_eq!(output, None);
    }

    #[test]
    fn zenzai_symbol_input_still_applies_standalone_symbol_rule_without_multi_character_context() {
        let mut app_config = AppConfig::default();
        app_config.zenzai.enable = true;
        app_config.zenzai.backend = "vulkan".to_string();
        app_config.romaji_table.rows = get_default_romaji_rows();

        let output = TextServiceFactory::resolve_symbol_input_text("a", "-", &app_config);
        assert_eq!(output, Some("ー".to_string()));
    }

    #[test]
    fn top_row_digit_input_uses_number_width_setting_when_zenzai_is_disabled() {
        let mut app_config = AppConfig::default();
        app_config.character_width.groups.number = WidthMode::Full;

        let output = TextServiceFactory::resolve_symbol_input_text("", "1", &app_config);
        assert_eq!(output, Some("１".to_string()));
    }

    #[test]
    fn top_row_digit_input_uses_number_width_setting_with_existing_raw_input() {
        let mut app_config = AppConfig::default();
        app_config.character_width.groups.number = WidthMode::Full;

        let output = TextServiceFactory::resolve_symbol_input_text("a", "1", &app_config);
        assert_eq!(output, Some("１".to_string()));
    }

    #[test]
    fn top_row_digit_input_uses_number_width_setting_when_zenzai_is_enabled() {
        let mut app_config = AppConfig::default();
        app_config.zenzai.enable = true;
        app_config.zenzai.backend = "vulkan".to_string();
        app_config.character_width.groups.number = WidthMode::Full;

        let output = TextServiceFactory::resolve_symbol_input_text("", "1", &app_config);
        assert_eq!(output, Some("１".to_string()));
    }

    #[test]
    fn top_row_digit_input_preserves_single_digit_romaji_rule_when_zenzai_is_disabled() {
        let mut app_config = AppConfig::default();
        app_config.character_width.groups.number = WidthMode::Full;
        app_config.romaji_table.rows = vec![row("1", "一", "")];

        let output = TextServiceFactory::resolve_symbol_input_text("", "1", &app_config);
        assert_eq!(output, None);
    }

    #[test]
    fn top_row_digit_input_preserves_single_digit_romaji_rule_when_zenzai_is_enabled() {
        let mut app_config = AppConfig::default();
        app_config.zenzai.enable = true;
        app_config.zenzai.backend = "vulkan".to_string();
        app_config.character_width.groups.number = WidthMode::Full;
        app_config.romaji_table.rows = vec![row("1", "一", "")];

        let output = TextServiceFactory::resolve_symbol_input_text("", "1", &app_config);
        assert_eq!(output, Some("一".to_string()));
    }

    #[test]
    fn top_row_digit_input_preserves_multi_character_romaji_context_when_zenzai_is_enabled() {
        let mut app_config = AppConfig::default();
        app_config.zenzai.enable = true;
        app_config.zenzai.backend = "vulkan".to_string();
        app_config.character_width.groups.number = WidthMode::Full;
        app_config.romaji_table.rows = vec![row("z1", "座布団", "")];

        let output = TextServiceFactory::resolve_symbol_input_text("z", "1", &app_config);
        assert_eq!(output, None);
    }
}
