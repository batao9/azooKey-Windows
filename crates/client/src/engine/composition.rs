use crate::{
    engine::user_action::UserAction,
    extension::VKeyExt as _,
    trace::diagnostic_log,
    tsf::factory::{TextServiceFactory, TextServiceFactory_Impl},
};

use super::{
    client_action::{ClientAction, SetSelectionType, SetTextType},
    full_width::{convert_kana_symbol, to_fullwidth, to_halfwidth},
    input_mode::InputMode,
    ipc_service::{Candidates, IPCService},
    state::{keyboard_disabled_from_context, IMEState},
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
    pub current_clause_is_split_derived: bool,
    pub current_clause_is_direct_split_remainder: bool,
    pub current_clause_has_split_left_neighbor: bool,
    pub current_clause_split_group_id: Option<u64>,
    pub next_split_group_id: u64,

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
    is_split_derived: bool,
    is_direct_split_remainder: bool,
    has_split_left_neighbor: bool,
    split_group_id: Option<u64>,
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
    is_split_derived: bool,
    is_direct_split_remainder: bool,
    has_split_left_neighbor: bool,
    split_group_id: Option<u64>,
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

trait ClauseActionBackend {
    fn move_cursor(&mut self, offset: i32) -> Result<Candidates>;
    fn shrink_text(&mut self, offset: i32) -> Result<Candidates>;
}

impl ClauseActionBackend for IPCService {
    fn move_cursor(&mut self, offset: i32) -> Result<Candidates> {
        IPCService::move_cursor(self, offset)
    }

    fn shrink_text(&mut self, offset: i32) -> Result<Candidates> {
        IPCService::shrink_text(self, offset)
    }
}

struct ClauseActionStateMut<'a> {
    preview: &'a mut String,
    suffix: &'a mut String,
    raw_input: &'a mut String,
    raw_hiragana: &'a mut String,
    fixed_prefix: &'a mut String,
    corresponding_count: &'a mut i32,
    selection_index: &'a mut i32,
    candidates: &'a mut Candidates,
    clause_snapshots: &'a mut Vec<ClauseSnapshot>,
    future_clause_snapshots: &'a mut Vec<FutureClauseSnapshot>,
    current_clause_is_split_derived: &'a mut bool,
    current_clause_is_direct_split_remainder: &'a mut bool,
    current_clause_has_split_left_neighbor: &'a mut bool,
    current_clause_split_group_id: &'a mut Option<u64>,
    next_split_group_id: &'a mut u64,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct ClauseActionEffect {
    applied: bool,
    update_pos: bool,
}

impl ClauseActionEffect {
    fn skipped() -> Self {
        Self {
            applied: false,
            update_pos: false,
        }
    }

    fn applied(update_pos: bool) -> Self {
        Self {
            applied: true,
            update_pos,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MoveClauseProgressMarker {
    preview: String,
    suffix: String,
    raw_input: String,
    raw_hiragana: String,
    fixed_prefix: String,
    corresponding_count: i32,
    selection_index: i32,
    clause_snapshot_count: usize,
    future_clause_snapshot_count: usize,
    current_clause_is_split_derived: bool,
    current_clause_is_direct_split_remainder: bool,
    current_clause_has_split_left_neighbor: bool,
    current_clause_split_group_id: Option<u64>,
}

impl MoveClauseProgressMarker {
    fn from_state(state: &ClauseActionStateMut<'_>) -> Self {
        Self {
            preview: state.preview.clone(),
            suffix: state.suffix.clone(),
            raw_input: state.raw_input.clone(),
            raw_hiragana: state.raw_hiragana.clone(),
            fixed_prefix: state.fixed_prefix.clone(),
            corresponding_count: *state.corresponding_count,
            selection_index: *state.selection_index,
            clause_snapshot_count: state.clause_snapshots.len(),
            future_clause_snapshot_count: state.future_clause_snapshots.len(),
            current_clause_is_split_derived: *state.current_clause_is_split_derived,
            current_clause_is_direct_split_remainder: *state
                .current_clause_is_direct_split_remainder,
            current_clause_has_split_left_neighbor: *state.current_clause_has_split_left_neighbor,
            current_clause_split_group_id: *state.current_clause_split_group_id,
        }
    }
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
    const MOVE_CLAUSE_TO_LAST: i32 = i32::MAX;

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
    fn should_shrink_before_direct_append(
        composition: &Composition,
        start_temporary_latin: bool,
    ) -> bool {
        start_temporary_latin
            && composition.raw_input.ends_with('n')
            && composition.raw_hiragana.ends_with('ん')
            && Self::current_raw_input_suffix(
                &composition.raw_input,
                composition.corresponding_count,
            )
            .is_empty()
    }

    #[inline]
    fn normalize_direct_symbol_char(c: char) -> char {
        let halfwidth_ascii = Self::to_halfwidth_ascii_char(c);
        if halfwidth_ascii.is_ascii_graphic() || halfwidth_ascii == ' ' {
            return halfwidth_ascii;
        }

        if c.is_ascii_graphic() || c == ' ' {
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
    fn candidate_splits_raw_input(selected: &CandidateSelection, raw_input: &str) -> bool {
        selected.corresponding_count > 0
            && selected.corresponding_count < raw_input.chars().count() as i32
            && !selected.sub_text.is_empty()
    }

    #[inline]
    fn display_suffix_after_selected_clause(
        preview: &str,
        fixed_prefix: &str,
        current_suffix: &str,
        selected: &CandidateSelection,
    ) -> String {
        let current_preview = Self::current_clause_preview(preview, fixed_prefix);
        current_preview
            .strip_prefix(&selected.text)
            .or_else(|| current_suffix.strip_prefix(&selected.text))
            .map(str::to_string)
            .unwrap_or_else(|| selected.sub_text.clone())
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
        is_split_derived: bool,
        has_split_left_neighbor: bool,
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
            is_split_derived,
            is_direct_split_remainder: false,
            has_split_left_neighbor,
            split_group_id: None,
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
            is_split_derived: false,
            is_direct_split_remainder: false,
            has_split_left_neighbor: false,
            split_group_id: None,
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
            is_split_derived: true,
            is_direct_split_remainder: true,
            has_split_left_neighbor: true,
            split_group_id: None,
            selected_text: clause_preview.to_string(),
            selected_sub_text: suffix.to_string(),
            candidates,
        }
    }

    #[inline]
    fn future_clause_display(snapshot: &FutureClauseSnapshot) -> String {
        format!("{}{}", snapshot.clause_preview, snapshot.suffix)
    }

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

    #[inline]
    fn clause_raw_preview(
        raw_hiragana: &str,
        next_raw_hiragana: Option<&str>,
        corresponding_count: i32,
    ) -> String {
        next_raw_hiragana
            .and_then(|next_raw| raw_hiragana.strip_suffix(next_raw))
            .filter(|prefix| !prefix.is_empty())
            .map(|prefix| prefix.to_string())
            .unwrap_or_else(|| {
                raw_hiragana
                    .chars()
                    .take(corresponding_count.max(0) as usize)
                    .collect()
            })
    }

    #[inline]
    fn clause_raw_input_preview(
        raw_input: &str,
        next_raw_input: Option<&str>,
        corresponding_count: i32,
    ) -> String {
        next_raw_input
            .and_then(|next_raw| raw_input.strip_suffix(next_raw))
            .filter(|prefix| !prefix.is_empty())
            .map(|prefix| prefix.to_string())
            .unwrap_or_else(|| {
                raw_input
                    .chars()
                    .take(corresponding_count.max(0) as usize)
                    .collect()
            })
    }

    #[inline]
    fn current_clause_raw_input_preview(
        raw_input: &str,
        corresponding_count: i32,
        future_clause_snapshots: &[FutureClauseSnapshot],
    ) -> String {
        Self::clause_raw_input_preview(
            raw_input,
            future_clause_snapshots
                .last()
                .map(|snapshot| snapshot.raw_input.as_str()),
            corresponding_count,
        )
    }

    #[inline]
    fn current_clause_raw_hiragana_preview(
        raw_hiragana: &str,
        corresponding_count: i32,
        future_clause_snapshots: &[FutureClauseSnapshot],
    ) -> String {
        Self::clause_raw_preview(
            raw_hiragana,
            future_clause_snapshots
                .last()
                .map(|snapshot| snapshot.raw_hiragana.as_str()),
            corresponding_count,
        )
    }

    #[inline]
    fn converted_clause_preview_text(
        set_type: &SetTextType,
        raw_input: &str,
        raw_hiragana: &str,
    ) -> String {
        match set_type {
            SetTextType::Hiragana => raw_hiragana.to_string(),
            SetTextType::Katakana => to_katakana(raw_hiragana),
            SetTextType::HalfKatakana => to_half_katakana(raw_hiragana),
            SetTextType::FullLatin => to_fullwidth(raw_input, true),
            SetTextType::HalfLatin => to_halfwidth(raw_input),
        }
    }

    #[inline]
    fn clause_raw_texts_for_log(
        raw_hiragana: &str,
        corresponding_count: i32,
        clause_snapshots: &[ClauseSnapshot],
        future_clause_snapshots: &[FutureClauseSnapshot],
    ) -> String {
        let mut clauses = Vec::new();

        for (index, snapshot) in clause_snapshots.iter().enumerate() {
            let next_raw_hiragana = clause_snapshots
                .get(index + 1)
                .map(|next| next.raw_hiragana.as_str())
                .or_else(|| (!raw_hiragana.is_empty()).then_some(raw_hiragana));
            clauses.push(Self::clause_raw_preview(
                &snapshot.raw_hiragana,
                next_raw_hiragana,
                snapshot.corresponding_count,
            ));
        }

        if !raw_hiragana.is_empty() {
            clauses.push(Self::clause_raw_preview(
                raw_hiragana,
                future_clause_snapshots
                    .last()
                    .map(|snapshot| snapshot.raw_hiragana.as_str()),
                corresponding_count,
            ));
        }

        let ordered_future = future_clause_snapshots.iter().rev().collect::<Vec<_>>();
        for (index, snapshot) in ordered_future.iter().enumerate() {
            let next_raw_hiragana = ordered_future
                .get(index + 1)
                .map(|next| next.raw_hiragana.as_str());
            clauses.push(Self::clause_raw_preview(
                &snapshot.raw_hiragana,
                next_raw_hiragana,
                snapshot.corresponding_count,
            ));
        }

        clauses.join(" / ")
    }

    #[inline]
    fn clause_input_lengths_for_log(
        corresponding_count: i32,
        clause_snapshots: &[ClauseSnapshot],
        future_clause_snapshots: &[FutureClauseSnapshot],
    ) -> String {
        let mut clause_lengths = clause_snapshots
            .iter()
            .map(|snapshot| snapshot.corresponding_count.to_string())
            .collect::<Vec<_>>();

        if corresponding_count > 0 {
            clause_lengths.push(corresponding_count.to_string());
        }

        clause_lengths.extend(
            future_clause_snapshots
                .iter()
                .rev()
                .map(|snapshot| snapshot.corresponding_count.to_string()),
        );

        clause_lengths.join(" / ")
    }

    #[inline]
    fn sanitize_log_field(value: &str) -> String {
        value
            .replace('\t', " ")
            .replace('\r', " ")
            .replace('\n', " ")
    }

    #[inline]
    fn debug_candidates(candidates: &Candidates, selection_index: i32) -> String {
        candidates
            .texts
            .iter()
            .zip(candidates.sub_texts.iter())
            .zip(candidates.corresponding_count.iter())
            .enumerate()
            .map(|(index, ((text, sub_text), corresponding_count))| {
                let selected = if index as i32 == selection_index {
                    "*"
                } else {
                    ""
                };
                format!(
                    "{}{}|{}|{}",
                    selected,
                    Self::sanitize_log_field(text),
                    Self::sanitize_log_field(sub_text),
                    corresponding_count
                )
            })
            .collect::<Vec<_>>()
            .join(" ; ")
    }

    #[inline]
    fn debug_clause_snapshots(clause_snapshots: &[ClauseSnapshot]) -> String {
        clause_snapshots
            .iter()
            .map(|snapshot| {
                format!(
                    "{}|{}|{}|{}|{}|{}|{}",
                    Self::sanitize_log_field(&Self::current_clause_preview(
                        &snapshot.preview,
                        &snapshot.fixed_prefix,
                    )),
                    Self::sanitize_log_field(&snapshot.suffix),
                    Self::sanitize_log_field(&snapshot.raw_hiragana),
                    snapshot.corresponding_count,
                    if snapshot.is_split_derived {
                        "split"
                    } else {
                        "base"
                    },
                    if snapshot.is_direct_split_remainder {
                        "direct"
                    } else {
                        "-"
                    },
                    snapshot
                        .split_group_id
                        .map(|group_id| group_id.to_string())
                        .unwrap_or_else(|| "-".to_string())
                )
            })
            .collect::<Vec<_>>()
            .join(" ; ")
    }

    #[inline]
    fn debug_future_clause_snapshots(future_clause_snapshots: &[FutureClauseSnapshot]) -> String {
        future_clause_snapshots
            .iter()
            .map(|snapshot| {
                format!(
                    "{}|{}|{}|{}|{}|{}|{}|{}",
                    Self::sanitize_log_field(&snapshot.clause_preview),
                    Self::sanitize_log_field(&snapshot.suffix),
                    Self::sanitize_log_field(&snapshot.raw_hiragana),
                    snapshot.corresponding_count,
                    if snapshot.is_conservative {
                        "conservative"
                    } else {
                        "actual"
                    },
                    if snapshot.is_split_derived {
                        "split"
                    } else {
                        "base"
                    },
                    if snapshot.is_direct_split_remainder {
                        "direct"
                    } else {
                        "-"
                    },
                    snapshot
                        .split_group_id
                        .map(|group_id| group_id.to_string())
                        .unwrap_or_else(|| "-".to_string())
                )
            })
            .collect::<Vec<_>>()
            .join(" ; ")
    }

    #[inline]
    fn action_log_name(action: &ClientAction) -> &'static str {
        match action {
            ClientAction::StartComposition => "StartComposition",
            ClientAction::EndComposition => "EndComposition",
            ClientAction::ShowCandidateWindow => "ShowCandidateWindow",
            ClientAction::AppendText(_) => "AppendText",
            ClientAction::AppendTextRaw(_) => "AppendTextRaw",
            ClientAction::AppendTextDirect(_) => "AppendTextDirect",
            ClientAction::RemoveText => "RemoveText",
            ClientAction::MoveCursor(_) => "MoveCursor",
            ClientAction::EnsureClauseNavigationReady => "EnsureClauseNavigationReady",
            ClientAction::MoveClause(_) => "MoveClause",
            ClientAction::AdjustBoundary(_) => "AdjustBoundary",
            ClientAction::SetIMEMode(_) => "SetIMEMode",
            ClientAction::SetSelection(_) => "SetSelection",
            ClientAction::ShrinkText(_) => "ShrinkText",
            ClientAction::ShrinkTextRaw(_) => "ShrinkTextRaw",
            ClientAction::ShrinkTextDirect(_) => "ShrinkTextDirect",
            ClientAction::SetTextWithType(_) => "SetTextWithType",
            ClientAction::SetTemporaryLatin(_) => "SetTemporaryLatin",
            ClientAction::SetTemporaryLatinShiftPending(_) => "SetTemporaryLatinShiftPending",
        }
    }

    #[inline]
    fn log_clause_action_state(
        phase: &str,
        action: &ClientAction,
        preview: &str,
        suffix: &str,
        raw_input: &str,
        raw_hiragana: &str,
        fixed_prefix: &str,
        corresponding_count: i32,
        selection_index: i32,
        candidates: &Candidates,
        clause_snapshots: &[ClauseSnapshot],
        future_clause_snapshots: &[FutureClauseSnapshot],
    ) {
        let selected = Self::select_candidate(candidates, selection_index);
        let selected_text = selected
            .as_ref()
            .map(|candidate| Self::sanitize_log_field(&candidate.text))
            .unwrap_or_default();
        let selected_sub_text = selected
            .as_ref()
            .map(|candidate| Self::sanitize_log_field(&candidate.sub_text))
            .unwrap_or_default();
        let clauses = Self::sanitize_log_field(&Self::clause_texts_for_log(
            preview,
            fixed_prefix,
            clause_snapshots,
            future_clause_snapshots,
        ));
        let clauses_raw = Self::sanitize_log_field(&Self::clause_raw_texts_for_log(
            raw_hiragana,
            corresponding_count,
            clause_snapshots,
            future_clause_snapshots,
        ));
        let clause_input_lengths = Self::sanitize_log_field(&Self::clause_input_lengths_for_log(
            corresponding_count,
            clause_snapshots,
            future_clause_snapshots,
        ));

        diagnostic_log(format!(
            "kind=clause-action\tphase={phase}\taction={}\tcurrent_index={}\tpreview={}\tsuffix={}\traw_input={}\traw_hiragana={}\tfixed_prefix={}\tcorresponding_count={corresponding_count}\tselection_index={selection_index}\tselected_text={selected_text}\tselected_sub_text={selected_sub_text}\tclauses={clauses}\tclauses_raw={clauses_raw}\tclause_input_lengths={clause_input_lengths}\tcandidates={}\tclause_snapshots={}\tfuture_clause_snapshots={}",
            Self::action_log_name(action),
            clause_snapshots.len(),
            Self::sanitize_log_field(preview),
            Self::sanitize_log_field(suffix),
            Self::sanitize_log_field(raw_input),
            Self::sanitize_log_field(raw_hiragana),
            Self::sanitize_log_field(fixed_prefix),
            Self::sanitize_log_field(&Self::debug_candidates(candidates, selection_index)),
            Self::sanitize_log_field(&Self::debug_clause_snapshots(clause_snapshots)),
            Self::sanitize_log_field(&Self::debug_future_clause_snapshots(future_clause_snapshots)),
        ));
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
    fn is_clause_navigation_active(composition: &Composition) -> bool {
        !composition.clause_snapshots.is_empty()
            || !composition.future_clause_snapshots.is_empty()
            || composition.current_clause_is_split_derived
            || composition.current_clause_split_group_id.is_some()
    }

    #[inline]
    fn is_clause_navigation_state_active(state: &ClauseActionStateMut<'_>) -> bool {
        !state.clause_snapshots.is_empty()
            || !state.future_clause_snapshots.is_empty()
            || *state.current_clause_is_split_derived
            || state.current_clause_split_group_id.is_some()
    }

    #[inline]
    fn is_auto_split_derived_clause(is_split_derived: bool, split_group_id: Option<u64>) -> bool {
        is_split_derived && split_group_id.is_none()
    }

    #[inline]
    fn has_auto_split_derived_clause_state(state: &ClauseActionStateMut<'_>) -> bool {
        Self::is_auto_split_derived_clause(
            *state.current_clause_is_split_derived,
            *state.current_clause_split_group_id,
        ) || state.clause_snapshots.iter().any(|snapshot| {
            Self::is_auto_split_derived_clause(snapshot.is_split_derived, snapshot.split_group_id)
        }) || state.future_clause_snapshots.iter().any(|snapshot| {
            Self::is_auto_split_derived_clause(snapshot.is_split_derived, snapshot.split_group_id)
        })
    }

    #[inline]
    fn has_auto_split_derived_clause(composition: &Composition) -> bool {
        Self::is_auto_split_derived_clause(
            composition.current_clause_is_split_derived,
            composition.current_clause_split_group_id,
        ) || composition.clause_snapshots.iter().any(|snapshot| {
            Self::is_auto_split_derived_clause(snapshot.is_split_derived, snapshot.split_group_id)
        }) || composition.future_clause_snapshots.iter().any(|snapshot| {
            Self::is_auto_split_derived_clause(snapshot.is_split_derived, snapshot.split_group_id)
        })
    }

    #[inline]
    fn ensure_clause_navigation_ready<B: ClauseActionBackend>(
        state: &mut ClauseActionStateMut<'_>,
        backend: &mut B,
    ) -> Result<ClauseActionEffect> {
        if Self::is_clause_navigation_state_active(state)
            || state.candidates.texts.is_empty()
            || state.raw_hiragana.is_empty()
        {
            return Ok(ClauseActionEffect::skipped());
        }

        if !state.suffix.is_empty() {
            return Ok(ClauseActionEffect::skipped());
        }

        let navigation_candidates = backend.move_cursor(0)?;
        let Some(selected) = Self::select_candidate(&navigation_candidates, 0) else {
            return Ok(ClauseActionEffect::skipped());
        };

        if !Self::candidate_splits_raw_input(&selected, state.raw_input) {
            return Ok(ClauseActionEffect::skipped());
        }

        *state.candidates = navigation_candidates;
        *state.selection_index = selected.index;
        *state.corresponding_count = selected.corresponding_count;
        let display_suffix = Self::display_suffix_after_selected_clause(
            state.preview,
            state.fixed_prefix,
            state.suffix,
            &selected,
        );
        *state.preview = Self::merge_preview_with_prefix(state.fixed_prefix, &selected.text);
        *state.suffix = display_suffix;
        *state.raw_hiragana = selected.hiragana;
        *state.current_clause_is_split_derived = true;
        *state.current_clause_is_direct_split_remainder = false;
        *state.current_clause_has_split_left_neighbor = false;
        *state.current_clause_split_group_id = None;
        Self::rebuild_future_clause_snapshots_from_backend(state, backend)?;
        *state.suffix = Self::sync_current_clause_future_suffix(
            state.candidates,
            *state.selection_index,
            *state.corresponding_count,
            state.future_clause_snapshots,
        );

        Ok(ClauseActionEffect::applied(true))
    }

    #[inline]
    fn apply_move_clause<B: ClauseActionBackend>(
        state: &mut ClauseActionStateMut<'_>,
        backend: &mut B,
        direction: i32,
    ) -> Result<ClauseActionEffect> {
        if direction == Self::MOVE_CLAUSE_TO_LAST {
            let mut applied_any = false;
            loop {
                let before = MoveClauseProgressMarker::from_state(state);
                let effect = Self::apply_move_clause(state, backend, 1)?;
                if !effect.applied {
                    break;
                }
                let after = MoveClauseProgressMarker::from_state(state);
                if before == after {
                    break;
                }
                applied_any = true;
                if state.suffix.is_empty() {
                    break;
                }
            }

            return Ok(if applied_any {
                ClauseActionEffect::applied(true)
            } else {
                ClauseActionEffect::skipped()
            });
        }

        if direction > 0 {
            if state.suffix.is_empty() {
                return Ok(ClauseActionEffect::skipped());
            }

            let mut snapshot = Self::build_clause_snapshot(
                state.preview,
                state.suffix,
                state.raw_input,
                state.raw_hiragana,
                state.fixed_prefix,
                *state.corresponding_count,
                *state.selection_index,
                *state.current_clause_is_split_derived,
                *state.current_clause_has_split_left_neighbor,
                state.candidates,
            );
            snapshot.split_group_id = *state.current_clause_split_group_id;
            snapshot.is_direct_split_remainder = *state.current_clause_is_direct_split_remainder;
            let current_clause_preview =
                Self::current_clause_preview(state.preview, state.fixed_prefix);
            let current_corresponding_count = *state.corresponding_count;

            let _ = backend.move_cursor(Self::MOVE_CURSOR_PUSH_CLAUSE_SNAPSHOT)?;
            state.clause_snapshots.push(snapshot);

            *state.candidates = backend.shrink_text(current_corresponding_count)?;
            *state.selection_index = 0;
            *state.raw_input = state
                .raw_input
                .chars()
                .skip(current_corresponding_count.max(0) as usize)
                .collect();
            state.fixed_prefix.push_str(&current_clause_preview);

            if Self::future_snapshot_matches_server(state.future_clause_snapshots, state.candidates)
            {
                if let Some(restored_future) = state.future_clause_snapshots.pop() {
                    Self::sync_backend_current_clause_to_future_snapshot(
                        backend,
                        state.candidates,
                        &restored_future,
                    )?;
                    Self::restore_future_clause_snapshot(
                        state.preview,
                        state.suffix,
                        state.raw_input,
                        state.raw_hiragana,
                        state.corresponding_count,
                        state.selection_index,
                        state.current_clause_is_split_derived,
                        state.current_clause_is_direct_split_remainder,
                        state.current_clause_has_split_left_neighbor,
                        state.current_clause_split_group_id,
                        state.candidates,
                        state.fixed_prefix,
                        &restored_future,
                    );
                    *state.suffix = Self::sync_current_clause_future_suffix(
                        state.candidates,
                        *state.selection_index,
                        *state.corresponding_count,
                        state.future_clause_snapshots,
                    );
                    return Ok(ClauseActionEffect::applied(true));
                }
            } else {
                if !state.future_clause_snapshots.is_empty() {
                    state.future_clause_snapshots.clear();
                }

                if state.future_clause_snapshots.is_empty() {
                    let navigation_candidates = backend.move_cursor(0)?;
                    if let Some(navigation_selected) =
                        Self::select_candidate(&navigation_candidates, 0)
                    {
                        if Self::candidate_splits_raw_input(&navigation_selected, state.raw_input) {
                            *state.candidates = navigation_candidates;
                            *state.selection_index = navigation_selected.index;
                        }
                    }
                }

                let Some(selected) =
                    Self::select_candidate(state.candidates, *state.selection_index)
                else {
                    let _ = backend.move_cursor(Self::MOVE_CURSOR_POP_CLAUSE_SNAPSHOT)?;
                    if let Some(restored) = state.clause_snapshots.pop() {
                        *state.preview = restored.preview;
                        *state.suffix = restored.suffix;
                        *state.raw_input = restored.raw_input;
                        *state.raw_hiragana = restored.raw_hiragana;
                        *state.fixed_prefix = restored.fixed_prefix;
                        *state.corresponding_count = restored.corresponding_count;
                        *state.selection_index = restored.selection_index;
                        *state.current_clause_is_split_derived = restored.is_split_derived;
                        *state.current_clause_is_direct_split_remainder =
                            restored.is_direct_split_remainder;
                        *state.current_clause_has_split_left_neighbor =
                            restored.has_split_left_neighbor;
                        *state.current_clause_split_group_id = restored.split_group_id;
                        *state.candidates = restored.candidates;
                        return Ok(ClauseActionEffect::applied(true));
                    }
                    return Ok(ClauseActionEffect::skipped());
                };

                *state.current_clause_is_split_derived = false;
                *state.current_clause_is_direct_split_remainder = false;
                *state.current_clause_has_split_left_neighbor = false;
                *state.current_clause_split_group_id = None;
                *state.selection_index = selected.index;
                *state.corresponding_count = selected.corresponding_count;
                let display_suffix = Self::display_suffix_after_selected_clause(
                    state.preview,
                    state.fixed_prefix,
                    state.suffix,
                    &selected,
                );
                *state.preview =
                    Self::merge_preview_with_prefix(state.fixed_prefix, &selected.text);
                *state.suffix = display_suffix;
                *state.raw_hiragana = selected.hiragana;
                return Ok(ClauseActionEffect::applied(true));
            }

            Ok(ClauseActionEffect::skipped())
        } else if direction < 0 {
            if let Some(restored) = state.clause_snapshots.pop() {
                Self::push_current_future_clause_snapshot(
                    state.future_clause_snapshots,
                    state.preview,
                    state.suffix,
                    state.raw_input,
                    state.raw_hiragana,
                    state.fixed_prefix,
                    *state.corresponding_count,
                    *state.selection_index,
                    *state.current_clause_is_split_derived,
                    *state.current_clause_is_direct_split_remainder,
                    *state.current_clause_has_split_left_neighbor,
                    *state.current_clause_split_group_id,
                    state.candidates,
                );
                let _ = backend.move_cursor(Self::MOVE_CURSOR_POP_CLAUSE_SNAPSHOT)?;

                *state.preview = restored.preview;
                *state.suffix = restored.suffix;
                *state.raw_input = restored.raw_input;
                *state.raw_hiragana = restored.raw_hiragana;
                *state.fixed_prefix = restored.fixed_prefix;
                *state.corresponding_count = restored.corresponding_count;
                *state.selection_index = restored.selection_index;
                *state.current_clause_is_split_derived = restored.is_split_derived;
                *state.current_clause_is_direct_split_remainder =
                    restored.is_direct_split_remainder;
                *state.current_clause_has_split_left_neighbor = restored.has_split_left_neighbor;
                *state.current_clause_split_group_id = restored.split_group_id;
                *state.candidates = restored.candidates;
                Ok(ClauseActionEffect::applied(true))
            } else {
                Ok(ClauseActionEffect::skipped())
            }
        } else {
            Ok(ClauseActionEffect::skipped())
        }
    }

    #[inline]
    fn apply_adjust_boundary<B: ClauseActionBackend>(
        state: &mut ClauseActionStateMut<'_>,
        backend: &mut B,
        direction: i32,
    ) -> Result<ClauseActionEffect> {
        if direction == 0 {
            return Ok(ClauseActionEffect::skipped());
        }

        if Self::has_auto_split_derived_clause_state(state) {
            return Ok(ClauseActionEffect::skipped());
        }

        let fallback_candidates = state.candidates.clone();
        if !state.suffix.is_empty() || !state.future_clause_snapshots.is_empty() {
            Self::sync_backend_current_clause_to_target(
                backend,
                state.candidates,
                state.raw_hiragana,
                state.suffix,
                *state.corresponding_count,
            )?;
        }

        let _ = backend.move_cursor(direction)?;
        let boundary_candidates = backend.move_cursor(0)?;
        if boundary_candidates.texts.is_empty() {
            if direction < 0 {
                let _ = backend.move_cursor(1)?;
                if let Some(selected) = Self::select_split_left_candidate(
                    &fallback_candidates,
                    *state.corresponding_count,
                ) {
                    *state.candidates = fallback_candidates;
                    return Ok(Self::apply_boundary_candidate_selection(state, selected));
                }
            }
            return Ok(ClauseActionEffect::skipped());
        }

        *state.candidates = boundary_candidates;
        if let Some(selected) = Self::select_candidate(state.candidates, 0) {
            Ok(Self::apply_boundary_candidate_selection(state, selected))
        } else {
            Ok(ClauseActionEffect::skipped())
        }
    }

    #[inline]
    fn select_split_left_candidate(
        candidates: &Candidates,
        current_corresponding_count: i32,
    ) -> Option<CandidateSelection> {
        (0..candidates.texts.len())
            .filter_map(|index| Self::select_candidate(candidates, index as i32))
            .filter(|candidate| candidate.corresponding_count < current_corresponding_count)
            .max_by_key(|candidate| candidate.corresponding_count)
    }

    #[inline]
    fn apply_boundary_candidate_selection(
        state: &mut ClauseActionStateMut<'_>,
        selected: CandidateSelection,
    ) -> ClauseActionEffect {
        let split_group_id = (*state.current_clause_split_group_id)
            .or_else(|| {
                state.future_clause_snapshots.last().and_then(|snapshot| {
                    snapshot
                        .is_conservative
                        .then_some(snapshot.split_group_id)
                        .flatten()
                        .or_else(|| {
                            snapshot
                                .has_split_left_neighbor
                                .then_some(snapshot.split_group_id)
                                .flatten()
                        })
                })
            })
            .unwrap_or_else(|| {
                let group_id = *state.next_split_group_id;
                *state.next_split_group_id += 1;
                group_id
            });
        *state.selection_index = selected.index;
        *state.corresponding_count = selected.corresponding_count;
        *state.preview = Self::merge_preview_with_prefix(state.fixed_prefix, &selected.text);
        *state.raw_hiragana = selected.hiragana;
        *state.suffix = selected.sub_text.clone();
        *state.current_clause_split_group_id = Some(split_group_id);
        let allow_bootstrap_without_existing_future = state.future_clause_snapshots.is_empty()
            && !state.clause_snapshots.is_empty()
            && Self::current_raw_suffix(state.raw_hiragana, *state.corresponding_count).is_empty();
        Self::maybe_push_split_future_clause_snapshot(
            state.future_clause_snapshots,
            state.raw_input,
            state.raw_hiragana,
            *state.corresponding_count,
            &selected.sub_text,
            allow_bootstrap_without_existing_future,
            Some(split_group_id),
        );
        let split_group_still_active = state
            .future_clause_snapshots
            .iter()
            .any(|snapshot| snapshot.split_group_id == Some(split_group_id));
        *state.current_clause_is_split_derived =
            *state.current_clause_has_split_left_neighbor || split_group_still_active;
        *state.current_clause_is_direct_split_remainder = false;
        *state.current_clause_split_group_id = state
            .current_clause_is_split_derived
            .then_some(split_group_id);
        *state.suffix = Self::sync_current_clause_future_suffix(
            state.candidates,
            *state.selection_index,
            *state.corresponding_count,
            state.future_clause_snapshots,
        );
        Self::sync_clause_snapshot_suffixes(state.clause_snapshots, state.preview, state.suffix);

        ClauseActionEffect::applied(true)
    }

    #[inline]
    fn apply_set_selection(
        state: &mut ClauseActionStateMut<'_>,
        selection: &SetSelectionType,
    ) -> ClauseActionEffect {
        let desired_index = match selection {
            SetSelectionType::Up => *state.selection_index - 1,
            SetSelectionType::Down => *state.selection_index + 1,
            SetSelectionType::Number(number) => *number,
        };

        if let Some(selected) = Self::select_candidate(state.candidates, desired_index) {
            *state.selection_index = selected.index;
            *state.corresponding_count = selected.corresponding_count;
            *state.preview = Self::merge_preview_with_prefix(state.fixed_prefix, &selected.text);
            *state.raw_hiragana = selected.hiragana;
            *state.suffix = Self::sync_current_clause_future_suffix(
                state.candidates,
                *state.selection_index,
                *state.corresponding_count,
                state.future_clause_snapshots,
            );
            Self::sync_clause_snapshot_suffixes(
                state.clause_snapshots,
                state.preview,
                state.suffix,
            );

            ClauseActionEffect::applied(false)
        } else {
            ClauseActionEffect::skipped()
        }
    }

    #[inline]
    fn sync_clause_action_ui(
        &self,
        preview: &str,
        suffix: &str,
        candidates: &Candidates,
        selection_index: i32,
        ipc_service: &mut IPCService,
        update_pos: bool,
    ) -> Result<()> {
        self.set_text(preview, suffix)?;
        ipc_service.set_candidates(candidates.texts.clone())?;
        ipc_service.set_selection(selection_index)?;
        if update_pos {
            self.update_pos()?;
        }
        Ok(())
    }

    #[inline]
    fn sync_candidate_window_after_text_update(
        &self,
        ipc_service: &mut IPCService,
        app_config: &AppConfig,
        transition: &CompositionState,
    ) -> Result<()> {
        self.update_pos()?;
        if !app_config.general.show_candidate_window_after_space
            && *transition != CompositionState::None
        {
            ipc_service.show_window()?;
        }
        Ok(())
    }

    #[inline]
    fn current_raw_suffix(raw_hiragana: &str, corresponding_count: i32) -> String {
        raw_hiragana
            .chars()
            .skip(corresponding_count.max(0) as usize)
            .collect()
    }

    #[inline]
    fn current_raw_input_suffix(raw_input: &str, corresponding_count: i32) -> String {
        raw_input
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
    fn restore_raw_suffix_from_sub_text(
        sub_text: &str,
        future_snapshot: &FutureClauseSnapshot,
    ) -> Option<String> {
        let future_raw = future_snapshot.raw_hiragana.as_str();
        let future_display = Self::future_clause_display(future_snapshot);

        sub_text
            .strip_suffix(&future_display)
            .map(|prefix| format!("{prefix}{future_raw}"))
            .or_else(|| (sub_text == future_display).then(|| future_raw.to_string()))
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
        current_clause_is_split_derived: bool,
        current_clause_is_direct_split_remainder: bool,
        current_clause_has_split_left_neighbor: bool,
        current_clause_split_group_id: Option<u64>,
        candidates: &Candidates,
    ) {
        let mut snapshot = Self::build_future_clause_snapshot(
            preview,
            suffix,
            raw_input,
            raw_hiragana,
            fixed_prefix,
            corresponding_count,
            selection_index,
            candidates,
        );
        snapshot.is_split_derived = current_clause_is_split_derived;
        snapshot.is_direct_split_remainder = current_clause_is_direct_split_remainder;
        snapshot.has_split_left_neighbor = current_clause_has_split_left_neighbor;
        snapshot.split_group_id = current_clause_split_group_id;
        diagnostic_log(format!(
            "kind=future-cache\tevent=push-current\tpreview={}\tsuffix={}\traw_input={}\traw_hiragana={}\tfuture_clause_snapshots_before={}\tis_split_derived={}\tis_direct_split_remainder={}\tsplit_group_id={}\tpushed={}",
            Self::sanitize_log_field(preview),
            Self::sanitize_log_field(suffix),
            Self::sanitize_log_field(raw_input),
            Self::sanitize_log_field(raw_hiragana),
            future_clause_snapshots.len(),
            current_clause_is_split_derived,
            current_clause_is_direct_split_remainder,
            current_clause_split_group_id
                .map(|group_id| group_id.to_string())
                .unwrap_or_else(|| "-".to_string()),
            Self::sanitize_log_field(&format!(
                "{}|{}|{}|{}|{}|{}|{}",
                snapshot.clause_preview,
                snapshot.suffix,
                snapshot.raw_hiragana,
                snapshot.corresponding_count,
                snapshot.is_split_derived,
                snapshot.is_direct_split_remainder,
                snapshot
                    .split_group_id
                    .map(|group_id| group_id.to_string())
                    .unwrap_or_else(|| "-".to_string()),
            )),
        ));
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
        allow_bootstrap_without_existing_future: bool,
        current_clause_split_group_id: Option<u64>,
    ) {
        let raw_input_suffix: String = raw_input
            .chars()
            .skip(corresponding_count.max(0) as usize)
            .collect();
        let normalized_raw_suffix_hint = future_clause_snapshots
            .last()
            .and_then(|snapshot| Self::restore_raw_suffix_from_sub_text(raw_suffix_hint, snapshot))
            .unwrap_or_else(|| raw_suffix_hint.to_string());
        let has_matching_future_hint = !normalized_raw_suffix_hint.is_empty()
            && future_clause_snapshots.last().is_some_and(|snapshot| {
                Self::future_snapshot_matches_raw_suffix(snapshot, &normalized_raw_suffix_hint)
            });
        let mut raw_suffix =
            if has_matching_future_hint {
                normalized_raw_suffix_hint
            } else if let Some(snapshot) = future_clause_snapshots.iter().rev().find(|snapshot| {
                !raw_input_suffix.is_empty() && raw_input_suffix == snapshot.raw_input
            }) {
                snapshot.raw_hiragana.clone()
            } else {
                let raw_suffix = Self::current_raw_suffix(raw_hiragana, corresponding_count);
                if raw_suffix.is_empty()
                    && future_clause_snapshots.is_empty()
                    && allow_bootstrap_without_existing_future
                    && !normalized_raw_suffix_hint.is_empty()
                {
                    normalized_raw_suffix_hint
                } else {
                    raw_suffix
                }
            };
        if raw_suffix.is_empty() {
            return;
        }

        if future_clause_snapshots.is_empty() {
            if !allow_bootstrap_without_existing_future {
                return;
            }

            let split_preview = if raw_suffix_hint.is_empty() {
                raw_suffix.clone()
            } else {
                raw_suffix_hint.to_string()
            };
            let split_raw_input = if raw_input_suffix.is_empty() {
                raw_suffix.clone()
            } else {
                raw_input_suffix.clone()
            };
            let mut split_snapshot = Self::build_conservative_future_clause_snapshot(
                &split_preview,
                "",
                &split_raw_input,
                &raw_suffix,
                split_raw_input.chars().count() as i32,
            );
            split_snapshot.is_split_derived = current_clause_split_group_id.is_some();
            split_snapshot.is_direct_split_remainder = true;
            split_snapshot.split_group_id = current_clause_split_group_id;
            diagnostic_log(format!(
                "kind=future-cache\tevent=bootstrap-split\traw_suffix={}\traw_input_suffix={}\tpushed={}",
                Self::sanitize_log_field(&raw_suffix),
                Self::sanitize_log_field(&split_raw_input),
                Self::sanitize_log_field(&format!(
                    "{}|{}|{}|{}",
                    split_snapshot.clause_preview,
                    split_snapshot.suffix,
                    split_snapshot.raw_hiragana,
                    split_snapshot.corresponding_count,
                )),
            ));
            future_clause_snapshots.push(split_snapshot);
            return;
        }

        while let Some(snapshot) = future_clause_snapshots.last() {
            if Self::future_snapshot_matches_raw_suffix(snapshot, &raw_suffix) {
                break;
            }
            diagnostic_log(format!(
                "kind=future-cache\tevent=trim-stale-split\traw_suffix={}\tdropped={}",
                Self::sanitize_log_field(&raw_suffix),
                Self::sanitize_log_field(&format!(
                    "{}|{}|{}|{}",
                    snapshot.clause_preview,
                    snapshot.suffix,
                    snapshot.raw_hiragana,
                    snapshot.corresponding_count,
                )),
            ));
            future_clause_snapshots.pop();
        }

        if !raw_suffix_hint.is_empty() {
            if let Some(snapshot) = future_clause_snapshots.last() {
                if let Some(restored) =
                    Self::restore_raw_suffix_from_sub_text(raw_suffix_hint, snapshot).or_else(
                        || {
                            (raw_suffix_hint == Self::future_clause_display(snapshot)
                                || raw_suffix_hint == snapshot.raw_hiragana)
                                .then(|| snapshot.raw_hiragana.clone())
                        },
                    )
                {
                    raw_suffix = restored;
                }
            }
        }

        let existing_future = future_clause_snapshots.last().cloned();
        let Some(snapshot) = existing_future.as_ref() else {
            return;
        };

        let trailing_raw_hiragana = future_clause_snapshots
            .iter()
            .rev()
            .nth(1)
            .map(|snapshot| snapshot.raw_hiragana.clone());
        let trailing_raw_input = future_clause_snapshots
            .iter()
            .rev()
            .nth(1)
            .map(|snapshot| snapshot.raw_input.clone());

        if !snapshot.is_conservative
            && snapshot.is_direct_split_remainder
            && snapshot.split_group_id == current_clause_split_group_id
            && trailing_raw_hiragana.is_some()
        {
            let trailing_raw_hiragana = trailing_raw_hiragana.unwrap_or_default();
            let trailing_raw_input = trailing_raw_input.unwrap_or_default();
            let joined_preview = if trailing_raw_hiragana.is_empty() {
                raw_suffix.clone()
            } else {
                raw_suffix
                    .strip_suffix(&trailing_raw_hiragana)
                    .unwrap_or(&raw_suffix)
                    .to_string()
            };
            let joined_corresponding_count = if trailing_raw_input.is_empty() {
                raw_input_suffix.chars().count() as i32
            } else {
                raw_input_suffix
                    .strip_suffix(&trailing_raw_input)
                    .unwrap_or(&raw_input_suffix)
                    .chars()
                    .count() as i32
            };
            let mut replaced_snapshot = Self::build_conservative_future_clause_snapshot(
                &joined_preview,
                &snapshot.suffix,
                &raw_input_suffix,
                &raw_suffix,
                joined_corresponding_count,
            );
            replaced_snapshot.is_split_derived = true;
            replaced_snapshot.is_direct_split_remainder = true;
            replaced_snapshot.has_split_left_neighbor = true;
            replaced_snapshot.split_group_id = current_clause_split_group_id;
            diagnostic_log(format!(
                "kind=future-cache\tevent=replace-actual-direct-remainder\traw_suffix={}\traw_input_suffix={}\treplaced={}",
                Self::sanitize_log_field(&raw_suffix),
                Self::sanitize_log_field(&raw_input_suffix),
                Self::sanitize_log_field(&format!(
                    "{}|{}|{}|{}",
                    replaced_snapshot.clause_preview,
                    replaced_snapshot.suffix,
                    replaced_snapshot.raw_hiragana,
                    replaced_snapshot.corresponding_count,
                )),
            ));
            future_clause_snapshots.pop();
            future_clause_snapshots.push(replaced_snapshot);
            return;
        }

        if snapshot.is_conservative {
            let trailing_raw_hiragana = trailing_raw_hiragana.unwrap_or_default();
            let trailing_raw_input = trailing_raw_input.unwrap_or_default();
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
            let mut replaced_snapshot = Self::build_conservative_future_clause_snapshot(
                &split_preview,
                &snapshot.suffix,
                &raw_input_suffix,
                &raw_suffix,
                split_corresponding_count,
            );
            replaced_snapshot.is_split_derived = current_clause_split_group_id.is_some();
            replaced_snapshot.is_direct_split_remainder = true;
            replaced_snapshot.split_group_id = current_clause_split_group_id;
            diagnostic_log(format!(
                "kind=future-cache\tevent=replace-derived-split\traw_suffix={}\traw_input_suffix={}\tcurrent_clause_split_group_id={}\treplaced={}",
                Self::sanitize_log_field(&raw_suffix),
                Self::sanitize_log_field(&raw_input_suffix),
                current_clause_split_group_id
                    .map(|group_id| group_id.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                Self::sanitize_log_field(&format!(
                    "{}|{}|{}|{}",
                    replaced_snapshot.clause_preview,
                    replaced_snapshot.suffix,
                    replaced_snapshot.raw_hiragana,
                    replaced_snapshot.corresponding_count,
                )),
            ));
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
        let mut split_snapshot = Self::build_conservative_future_clause_snapshot(
            split_preview,
            &Self::future_clause_display(snapshot),
            &raw_input_suffix,
            &raw_suffix,
            split_corresponding_count,
        );
        split_snapshot.is_split_derived = current_clause_split_group_id.is_some();
        split_snapshot.is_direct_split_remainder = true;
        split_snapshot.split_group_id = current_clause_split_group_id;
        diagnostic_log(format!(
            "kind=future-cache\tevent=push-split\traw_suffix={}\traw_input_suffix={}\tpushed={}",
            Self::sanitize_log_field(&raw_suffix),
            Self::sanitize_log_field(&raw_input_suffix),
            Self::sanitize_log_field(&format!(
                "{}|{}|{}|{}",
                split_snapshot.clause_preview,
                split_snapshot.suffix,
                split_snapshot.raw_hiragana,
                split_snapshot.corresponding_count,
            )),
        ));
        future_clause_snapshots.push(split_snapshot);
    }

    #[inline]
    fn rebuild_future_clause_snapshots_from_backend<B: ClauseActionBackend>(
        state: &mut ClauseActionStateMut<'_>,
        backend: &mut B,
    ) -> Result<()> {
        if state.suffix.is_empty() {
            state.future_clause_snapshots.clear();
            return Ok(());
        }

        let mut temp_preview = state.preview.clone();
        let mut temp_suffix = state.suffix.clone();
        let mut temp_raw_input = state.raw_input.clone();
        let mut temp_raw_hiragana = state.raw_hiragana.clone();
        let mut temp_fixed_prefix = state.fixed_prefix.clone();
        let mut temp_corresponding_count = *state.corresponding_count;
        let mut temp_selection_index = *state.selection_index;
        let mut temp_candidates = state.candidates.clone();
        let mut temp_clause_snapshots = Vec::new();
        let mut temp_future_clause_snapshots = Vec::new();
        let mut temp_current_clause_is_split_derived = *state.current_clause_is_split_derived;
        let mut temp_current_clause_is_direct_split_remainder =
            *state.current_clause_is_direct_split_remainder;
        let mut temp_current_clause_has_split_left_neighbor =
            *state.current_clause_has_split_left_neighbor;
        let mut temp_current_clause_split_group_id = *state.current_clause_split_group_id;
        let mut temp_next_split_group_id = *state.next_split_group_id;
        let mut collected = Vec::new();

        loop {
            let effect = {
                let mut temp_state = ClauseActionStateMut {
                    preview: &mut temp_preview,
                    suffix: &mut temp_suffix,
                    raw_input: &mut temp_raw_input,
                    raw_hiragana: &mut temp_raw_hiragana,
                    fixed_prefix: &mut temp_fixed_prefix,
                    corresponding_count: &mut temp_corresponding_count,
                    selection_index: &mut temp_selection_index,
                    candidates: &mut temp_candidates,
                    clause_snapshots: &mut temp_clause_snapshots,
                    future_clause_snapshots: &mut temp_future_clause_snapshots,
                    current_clause_is_split_derived: &mut temp_current_clause_is_split_derived,
                    current_clause_is_direct_split_remainder:
                        &mut temp_current_clause_is_direct_split_remainder,
                    current_clause_has_split_left_neighbor:
                        &mut temp_current_clause_has_split_left_neighbor,
                    current_clause_split_group_id: &mut temp_current_clause_split_group_id,
                    next_split_group_id: &mut temp_next_split_group_id,
                };
                Self::apply_move_clause(&mut temp_state, backend, 1)?
            };
            if !effect.applied {
                break;
            }

            let mut snapshot = Self::build_future_clause_snapshot(
                &temp_preview,
                &temp_suffix,
                &temp_raw_input,
                &temp_raw_hiragana,
                &temp_fixed_prefix,
                temp_corresponding_count,
                temp_selection_index,
                &temp_candidates,
            );
            if collected.is_empty() {
                snapshot.is_split_derived = true;
                snapshot.is_direct_split_remainder = true;
                snapshot.has_split_left_neighbor = true;
                snapshot.split_group_id = *state.current_clause_split_group_id;
            } else {
                snapshot.is_split_derived = temp_current_clause_is_split_derived;
                snapshot.is_direct_split_remainder = temp_current_clause_is_direct_split_remainder;
                snapshot.has_split_left_neighbor = temp_current_clause_has_split_left_neighbor;
                snapshot.split_group_id = temp_current_clause_split_group_id;
            }
            collected.push(snapshot);

            if temp_suffix.is_empty() {
                break;
            }
        }

        for _ in 0..temp_clause_snapshots.len() {
            let _ = backend.move_cursor(Self::MOVE_CURSOR_POP_CLAUSE_SNAPSHOT)?;
        }

        state.future_clause_snapshots.clear();
        state
            .future_clause_snapshots
            .extend(collected.into_iter().rev());
        Ok(())
    }

    #[inline]
    fn restore_future_clause_snapshot(
        preview: &mut String,
        suffix: &mut String,
        raw_input: &mut String,
        raw_hiragana: &mut String,
        corresponding_count: &mut i32,
        selection_index: &mut i32,
        current_clause_is_split_derived: &mut bool,
        current_clause_is_direct_split_remainder: &mut bool,
        current_clause_has_split_left_neighbor: &mut bool,
        current_clause_split_group_id: &mut Option<u64>,
        candidates: &mut Candidates,
        fixed_prefix: &str,
        snapshot: &FutureClauseSnapshot,
    ) {
        *preview = Self::merge_preview_with_prefix(fixed_prefix, &snapshot.clause_preview);
        *suffix = snapshot.suffix.clone();
        *raw_input = snapshot.raw_input.clone();
        *raw_hiragana = snapshot.raw_hiragana.clone();
        *corresponding_count = snapshot.corresponding_count;
        *current_clause_is_split_derived = snapshot.is_split_derived;
        *current_clause_is_direct_split_remainder = snapshot.is_direct_split_remainder;
        *current_clause_has_split_left_neighbor = snapshot.has_split_left_neighbor;
        *current_clause_split_group_id = if *current_clause_is_split_derived {
            snapshot.split_group_id
        } else {
            None
        };
        *selection_index = Self::resolve_selection_index(
            &snapshot.candidates,
            &snapshot.selected_text,
            &snapshot.selected_sub_text,
            snapshot.corresponding_count,
            snapshot.selection_index,
        );
        *candidates = snapshot.candidates.clone();
        diagnostic_log(format!(
            "kind=future-cache\tevent=restore\tpreview={}\tsuffix={}\traw_input={}\traw_hiragana={}\tselection_index={}\tcorresponding_count={}\tis_split_derived={}\tis_direct_split_remainder={}\tsplit_group_id={}",
            Self::sanitize_log_field(preview),
            Self::sanitize_log_field(suffix),
            Self::sanitize_log_field(raw_input),
            Self::sanitize_log_field(raw_hiragana),
            *selection_index,
            *corresponding_count,
            *current_clause_is_split_derived,
            *current_clause_is_direct_split_remainder,
            current_clause_split_group_id
                .map(|group_id| group_id.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ));
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
            .map(|snapshot| {
                server_candidates.hiragana == snapshot.raw_hiragana
                    || server_candidates.hiragana.ends_with(&snapshot.raw_hiragana)
                    || snapshot.raw_hiragana.ends_with(&server_candidates.hiragana)
            })
            .unwrap_or(false)
    }

    #[inline]
    fn sync_backend_current_clause_to_future_snapshot<B: ClauseActionBackend>(
        backend: &mut B,
        candidates: &mut Candidates,
        snapshot: &FutureClauseSnapshot,
    ) -> Result<()> {
        Self::sync_backend_current_clause_to_target(
            backend,
            candidates,
            &snapshot.raw_hiragana,
            &snapshot.selected_sub_text,
            snapshot.corresponding_count,
        )
    }

    #[inline]
    fn sync_backend_current_clause_to_target<B: ClauseActionBackend>(
        backend: &mut B,
        candidates: &mut Candidates,
        target_raw_hiragana: &str,
        target_sub_text: &str,
        target_corresponding_count: i32,
    ) -> Result<()> {
        let mut last_signature = None;
        let max_steps = candidates.hiragana.chars().count().max(1);
        let target_raw_suffix =
            Self::current_raw_suffix(target_raw_hiragana, target_corresponding_count);

        for _ in 0..max_steps {
            if let Some(selected) = Self::select_candidate(candidates, 0) {
                let candidate_raw_suffix =
                    Self::current_raw_suffix(&candidates.hiragana, selected.corresponding_count);
                let exact_match = selected.corresponding_count == target_corresponding_count
                    && (selected.sub_text == target_sub_text
                        || candidates.hiragana == target_raw_hiragana
                        || candidate_raw_suffix == target_raw_suffix);
                if exact_match {
                    return Ok(());
                }
                if selected.corresponding_count < target_corresponding_count {
                    return Ok(());
                }
            }

            let _ = backend.move_cursor(-1)?;
            let next_candidates = backend.move_cursor(0)?;
            if next_candidates.texts.is_empty() {
                let _ = backend.move_cursor(1)?;
                return Ok(());
            }

            let signature = format!(
                "{}|{}|{}",
                next_candidates.hiragana,
                next_candidates
                    .corresponding_count
                    .first()
                    .copied()
                    .unwrap_or_default(),
                next_candidates
                    .sub_texts
                    .first()
                    .cloned()
                    .unwrap_or_default(),
            );
            if last_signature.as_ref() == Some(&signature) {
                *candidates = next_candidates;
                return Ok(());
            }

            *candidates = next_candidates;
            last_signature = Some(signature);
        }

        Ok(())
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
    fn commit_enter_actions(composition: &Composition) -> (CompositionState, Vec<ClientAction>) {
        if Self::is_clause_navigation_active(composition) {
            (CompositionState::None, vec![ClientAction::EndComposition])
        } else {
            Self::commit_current_clause_actions(composition)
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

    #[inline]
    fn candidate_preview_actions(app_config: &AppConfig) -> Vec<ClientAction> {
        let mut actions = Vec::with_capacity(2);
        if app_config.general.show_candidate_window_after_space {
            actions.push(ClientAction::ShowCandidateWindow);
        }
        actions.push(ClientAction::SetSelection(SetSelectionType::Down));
        actions
    }

    #[inline]
    fn clause_navigation_actions(composition: &Composition, direction: i32) -> Vec<ClientAction> {
        if Self::is_clause_navigation_active(composition) || !composition.suffix.is_empty() {
            return vec![
                ClientAction::EnsureClauseNavigationReady,
                ClientAction::MoveClause(direction),
            ];
        }

        if direction < 0 {
            vec![
                ClientAction::EnsureClauseNavigationReady,
                ClientAction::MoveClause(Self::MOVE_CLAUSE_TO_LAST),
            ]
        } else {
            vec![ClientAction::EnsureClauseNavigationReady]
        }
    }

    #[inline]
    fn plan_actions_for_user_action(
        composition: &Composition,
        action: &UserAction,
        mode: &InputMode,
        is_shift_pressed: bool,
        app_config: &AppConfig,
        start_temporary_latin: bool,
    ) -> Option<(CompositionState, Vec<ClientAction>)> {
        let result = match composition.state {
            CompositionState::None => match action {
                _ if (composition.temporary_latin || start_temporary_latin)
                    && Self::direct_text_for_action(action).is_some() =>
                {
                    let text = Self::direct_text_for_action(action)?;
                    let mut actions = vec![ClientAction::StartComposition];
                    if start_temporary_latin {
                        actions.push(ClientAction::SetTemporaryLatin(true));
                    }
                    actions.push(ClientAction::AppendTextDirect(text));
                    Some((CompositionState::Composing, actions))
                }
                UserAction::NumpadSymbol(symbol) if *mode == InputMode::Kana => {
                    let text =
                        Self::numpad_text_for_mode(*symbol, app_config.general.numpad_input, true)?;
                    Some((
                        CompositionState::Composing,
                        vec![
                            ClientAction::StartComposition,
                            ClientAction::AppendTextRaw(text),
                        ],
                    ))
                }
                UserAction::Input(char) if *mode == InputMode::Kana => Some((
                    CompositionState::Composing,
                    vec![
                        ClientAction::StartComposition,
                        ClientAction::AppendText(char.to_string()),
                    ],
                )),
                UserAction::Number {
                    value,
                    is_numpad: true,
                } if *mode == InputMode::Kana => {
                    let digit = char::from_digit(*value as u32, 10).unwrap_or('0');
                    let text =
                        Self::numpad_text_for_mode(digit, app_config.general.numpad_input, true)?;

                    Some((
                        CompositionState::Composing,
                        vec![
                            ClientAction::StartComposition,
                            ClientAction::AppendTextRaw(text),
                        ],
                    ))
                }
                UserAction::Number {
                    value,
                    is_numpad: false,
                } if *mode == InputMode::Kana => Some((
                    CompositionState::Composing,
                    vec![
                        ClientAction::StartComposition,
                        ClientAction::AppendText(value.to_string()),
                    ],
                )),
                UserAction::Space if *mode == InputMode::Kana => {
                    let mut use_halfwidth =
                        matches!(app_config.general.space_input, SpaceInputMode::AlwaysHalf);
                    if is_shift_pressed {
                        use_halfwidth = !use_halfwidth;
                    }
                    let space = if use_halfwidth { " " } else { "　" };
                    Some((
                        CompositionState::None,
                        vec![
                            ClientAction::StartComposition,
                            ClientAction::AppendText(space.to_string()),
                            ClientAction::EndComposition,
                        ],
                    ))
                }
                UserAction::ToggleInputMode => Some((
                    CompositionState::None,
                    vec![match mode {
                        InputMode::Kana => ClientAction::SetIMEMode(InputMode::Latin),
                        InputMode::Latin => ClientAction::SetIMEMode(InputMode::Kana),
                    }],
                )),
                UserAction::InputModeOn => Some((
                    CompositionState::None,
                    vec![ClientAction::SetIMEMode(InputMode::Kana)],
                )),
                UserAction::InputModeOff => Some((
                    CompositionState::None,
                    vec![ClientAction::SetIMEMode(InputMode::Latin)],
                )),
                _ => None,
            },
            CompositionState::Composing => match action {
                _ if (composition.temporary_latin || start_temporary_latin)
                    && Self::direct_text_for_action(action).is_some() =>
                {
                    let text = Self::direct_text_for_action(action)?;
                    let mut actions = vec![];
                    if start_temporary_latin {
                        actions.push(ClientAction::SetTemporaryLatin(true));
                    }
                    if Self::should_shrink_before_direct_append(composition, start_temporary_latin)
                    {
                        actions.push(ClientAction::ShrinkTextDirect(text));
                    } else {
                        actions.push(ClientAction::AppendTextDirect(text));
                    }
                    Some((CompositionState::Composing, actions))
                }
                UserAction::NumpadSymbol(symbol) if *mode == InputMode::Kana => {
                    let text =
                        Self::numpad_text_for_mode(*symbol, app_config.general.numpad_input, false)
                            .unwrap_or_else(|| symbol.to_string());
                    Some((
                        CompositionState::Composing,
                        vec![ClientAction::AppendTextRaw(text)],
                    ))
                }
                UserAction::Input(char) => Some((
                    CompositionState::Composing,
                    vec![ClientAction::AppendText(char.to_string())],
                )),
                UserAction::Number {
                    value,
                    is_numpad: true,
                } if *mode == InputMode::Kana => {
                    let digit = char::from_digit(*value as u32, 10).unwrap_or('0');
                    let text =
                        Self::numpad_text_for_mode(digit, app_config.general.numpad_input, false)
                            .unwrap_or_else(|| digit.to_string());
                    Some((
                        CompositionState::Composing,
                        vec![ClientAction::AppendTextRaw(text)],
                    ))
                }
                UserAction::Number { value, .. } => Some((
                    CompositionState::Composing,
                    vec![ClientAction::AppendText(value.to_string())],
                )),
                UserAction::Backspace => {
                    if composition.raw_input.chars().count() <= 1 {
                        Some((
                            CompositionState::None,
                            vec![ClientAction::RemoveText, ClientAction::EndComposition],
                        ))
                    } else {
                        Some((CompositionState::Composing, vec![ClientAction::RemoveText]))
                    }
                }
                UserAction::Enter => Some(Self::commit_enter_actions(composition)),
                UserAction::CommitAndNextClause => {
                    Some(Self::commit_current_clause_actions(composition))
                }
                UserAction::CommitFirstClause => {
                    Some(Self::commit_first_clause_actions(composition))
                }
                UserAction::AdjustClauseBoundary(direction) => {
                    if Self::has_auto_split_derived_clause(composition) {
                        Some((CompositionState::Composing, vec![]))
                    } else {
                        Some((
                            CompositionState::Composing,
                            vec![ClientAction::AdjustBoundary(*direction)],
                        ))
                    }
                }
                UserAction::Escape => Some((
                    CompositionState::None,
                    vec![ClientAction::RemoveText, ClientAction::EndComposition],
                )),
                UserAction::Navigation(direction) => match direction {
                    Navigation::Right => Some((
                        CompositionState::Composing,
                        Self::clause_navigation_actions(composition, 1),
                    )),
                    Navigation::Left => Some((
                        CompositionState::Composing,
                        Self::clause_navigation_actions(composition, -1),
                    )),
                    Navigation::Up => Some((
                        CompositionState::Previewing,
                        vec![ClientAction::SetSelection(SetSelectionType::Up)],
                    )),
                    Navigation::Down => Some((
                        CompositionState::Previewing,
                        vec![ClientAction::SetSelection(SetSelectionType::Down)],
                    )),
                },
                UserAction::ToggleInputMode => Some((
                    CompositionState::None,
                    vec![
                        ClientAction::EndComposition,
                        ClientAction::SetIMEMode(InputMode::Latin),
                    ],
                )),
                UserAction::InputModeOn => Some((
                    CompositionState::None,
                    vec![
                        ClientAction::EndComposition,
                        ClientAction::SetIMEMode(InputMode::Kana),
                    ],
                )),
                UserAction::InputModeOff => Some((
                    CompositionState::None,
                    vec![
                        ClientAction::EndComposition,
                        ClientAction::SetIMEMode(InputMode::Latin),
                    ],
                )),
                UserAction::Space | UserAction::Tab => Some((
                    CompositionState::Previewing,
                    Self::candidate_preview_actions(app_config),
                )),
                UserAction::Function(key) => match key {
                    Function::Six => Some((
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::Hiragana)],
                    )),
                    Function::Seven => Some((
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::Katakana)],
                    )),
                    Function::Eight => Some((
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::HalfKatakana)],
                    )),
                    Function::Nine => Some((
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::FullLatin)],
                    )),
                    Function::Ten => Some((
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::HalfLatin)],
                    )),
                },
                _ => None,
            },
            CompositionState::Previewing => match action {
                _ if (composition.temporary_latin || start_temporary_latin)
                    && Self::direct_text_for_action(action).is_some() =>
                {
                    let text = Self::direct_text_for_action(action)?;
                    let mut actions = vec![];
                    if start_temporary_latin {
                        actions.push(ClientAction::SetTemporaryLatin(true));
                    }
                    actions.push(ClientAction::ShrinkTextDirect(text));
                    Some((CompositionState::Composing, actions))
                }
                UserAction::NumpadSymbol(symbol) if *mode == InputMode::Kana => {
                    let text =
                        Self::numpad_text_for_mode(*symbol, app_config.general.numpad_input, false)
                            .unwrap_or_else(|| symbol.to_string());
                    Some((
                        CompositionState::Composing,
                        vec![ClientAction::ShrinkTextRaw(text)],
                    ))
                }
                UserAction::Input(char) => Some((
                    CompositionState::Composing,
                    vec![ClientAction::ShrinkText(char.to_string())],
                )),
                UserAction::Number {
                    value,
                    is_numpad: true,
                } if *mode == InputMode::Kana => {
                    let digit = char::from_digit(*value as u32, 10).unwrap_or('0');
                    let text =
                        Self::numpad_text_for_mode(digit, app_config.general.numpad_input, false)
                            .unwrap_or_else(|| digit.to_string());
                    Some((
                        CompositionState::Composing,
                        vec![ClientAction::ShrinkTextRaw(text)],
                    ))
                }
                UserAction::Number { value, .. } => Some((
                    CompositionState::Composing,
                    vec![ClientAction::ShrinkText(value.to_string())],
                )),
                UserAction::Backspace => {
                    if composition.raw_input.chars().count() <= 1 {
                        Some((
                            CompositionState::None,
                            vec![ClientAction::RemoveText, ClientAction::EndComposition],
                        ))
                    } else {
                        Some((CompositionState::Composing, vec![ClientAction::RemoveText]))
                    }
                }
                UserAction::Enter => Some(Self::commit_enter_actions(composition)),
                UserAction::CommitAndNextClause => {
                    Some(Self::commit_current_clause_actions(composition))
                }
                UserAction::CommitFirstClause => {
                    Some(Self::commit_first_clause_actions(composition))
                }
                UserAction::AdjustClauseBoundary(direction) => {
                    if Self::has_auto_split_derived_clause(composition) {
                        Some((CompositionState::Previewing, vec![]))
                    } else {
                        Some((
                            CompositionState::Previewing,
                            vec![ClientAction::AdjustBoundary(*direction)],
                        ))
                    }
                }
                UserAction::Escape => Some((
                    CompositionState::None,
                    vec![ClientAction::RemoveText, ClientAction::EndComposition],
                )),
                UserAction::Navigation(direction) => match direction {
                    Navigation::Right => Some((
                        CompositionState::Composing,
                        Self::clause_navigation_actions(composition, 1),
                    )),
                    Navigation::Left => Some((
                        CompositionState::Composing,
                        Self::clause_navigation_actions(composition, -1),
                    )),
                    Navigation::Up => Some((
                        CompositionState::Previewing,
                        vec![ClientAction::SetSelection(SetSelectionType::Up)],
                    )),
                    Navigation::Down => Some((
                        CompositionState::Previewing,
                        vec![ClientAction::SetSelection(SetSelectionType::Down)],
                    )),
                },
                UserAction::ToggleInputMode => Some((
                    CompositionState::None,
                    vec![
                        ClientAction::EndComposition,
                        ClientAction::SetIMEMode(InputMode::Latin),
                    ],
                )),
                UserAction::InputModeOn => Some((
                    CompositionState::None,
                    vec![
                        ClientAction::EndComposition,
                        ClientAction::SetIMEMode(InputMode::Kana),
                    ],
                )),
                UserAction::InputModeOff => Some((
                    CompositionState::None,
                    vec![
                        ClientAction::EndComposition,
                        ClientAction::SetIMEMode(InputMode::Latin),
                    ],
                )),
                UserAction::Space | UserAction::Tab => Some((
                    CompositionState::Previewing,
                    Self::candidate_preview_actions(app_config),
                )),
                UserAction::Function(key) => match key {
                    Function::Six => Some((
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::Hiragana)],
                    )),
                    Function::Seven => Some((
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::Katakana)],
                    )),
                    Function::Eight => Some((
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::HalfKatakana)],
                    )),
                    Function::Nine => Some((
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::FullLatin)],
                    )),
                    Function::Ten => Some((
                        CompositionState::Previewing,
                        vec![ClientAction::SetTextWithType(SetTextType::HalfLatin)],
                    )),
                },
                _ => None,
            },
            CompositionState::Selecting => None,
        };

        result
    }

    #[tracing::instrument]
    pub fn process_key(
        &self,
        context: Option<&ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<Option<(Vec<ClientAction>, CompositionState)>> {
        let Some(context) = context else {
            self.set_keyboard_disabled_state(true)?;
            return Ok(None);
        };
        let keyboard_disabled = keyboard_disabled_from_context(context);
        self.set_keyboard_disabled_state(keyboard_disabled)?;
        if keyboard_disabled {
            self.cancel_composition_for_disabled_context();
            return Ok(None);
        }

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

        let Some((transition, mut actions)) = Self::plan_actions_for_user_action(
            &composition,
            &action,
            &mode,
            is_shift_pressed,
            &app_config,
            start_temporary_latin,
        ) else {
            self.clear_temporary_latin_shift_pending_if_needed(should_clear_shift_pending)?;
            return Ok(None);
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
        let Some(context) = context else {
            self.set_keyboard_disabled_state(true)?;
            return Ok(None);
        };
        let keyboard_disabled = keyboard_disabled_from_context(context);
        self.set_keyboard_disabled_state(keyboard_disabled)?;
        if keyboard_disabled {
            self.cancel_composition_for_disabled_context();
            return Ok(None);
        }
        if !Self::is_shift_key(wparam) {
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
                self.set_keyboard_disabled_state(true)?;
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
                self.set_keyboard_disabled_state(true)?;
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
        self.cancel_composition_for_disabled_context();
    }

    fn cancel_composition_for_disabled_context(&self) {
        let _ = self.abort_composition();

        if let Ok(text_service) = self.borrow() {
            if let Ok(mut composition) = text_service.borrow_mut_composition() {
                *composition = Composition::default();
            }
        }

        let ipc_service = IMEState::get()
            .ok()
            .and_then(|state| state.ipc_service.clone());

        if let Some(mut ipc_service) = ipc_service {
            let _ = ipc_service.hide_window();
            let _ = ipc_service.set_candidates(vec![]);
            let _ = ipc_service.clear_text();

            if let Ok(mut ime_state) = IMEState::get() {
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
        let mut current_clause_is_split_derived = composition.current_clause_is_split_derived;
        let mut current_clause_is_direct_split_remainder =
            composition.current_clause_is_direct_split_remainder;
        let mut current_clause_has_split_left_neighbor =
            composition.current_clause_has_split_left_neighbor;
        let mut current_clause_split_group_id = composition.current_clause_split_group_id;
        let mut next_split_group_id = composition.next_split_group_id;
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
                    if app_config.general.show_candidate_window_after_space {
                        ipc_service.hide_window()?;
                    }
                }
                ClientAction::ShowCandidateWindow => {
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
                    current_clause_is_split_derived = false;
                    current_clause_is_direct_split_remainder = false;
                    current_clause_has_split_left_neighbor = false;
                    current_clause_split_group_id = None;
                    next_split_group_id = 0;
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

                    current_clause_is_split_derived = false;
                    current_clause_is_direct_split_remainder = false;
                    current_clause_has_split_left_neighbor = false;
                    current_clause_split_group_id = None;
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
                        self.sync_candidate_window_after_text_update(
                            &mut ipc_service,
                            &app_config,
                            &transition,
                        )?;
                    }
                }
                ClientAction::AppendTextRaw(text) => {
                    Self::clear_clause_caches(
                        &mut clause_snapshots,
                        &mut future_clause_snapshots,
                        &mut ipc_service,
                    )?;
                    current_clause_is_split_derived = false;
                    current_clause_is_direct_split_remainder = false;
                    current_clause_has_split_left_neighbor = false;
                    current_clause_split_group_id = None;
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
                        self.sync_candidate_window_after_text_update(
                            &mut ipc_service,
                            &app_config,
                            &transition,
                        )?;
                    }
                }
                ClientAction::AppendTextDirect(text) => {
                    Self::clear_clause_caches(
                        &mut clause_snapshots,
                        &mut future_clause_snapshots,
                        &mut ipc_service,
                    )?;
                    current_clause_is_split_derived = false;
                    current_clause_is_direct_split_remainder = false;
                    current_clause_has_split_left_neighbor = false;
                    current_clause_split_group_id = None;
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
                        self.sync_candidate_window_after_text_update(
                            &mut ipc_service,
                            &app_config,
                            &transition,
                        )?;
                    }
                }
                ClientAction::RemoveText => {
                    Self::clear_clause_caches(
                        &mut clause_snapshots,
                        &mut future_clause_snapshots,
                        &mut ipc_service,
                    )?;
                    current_clause_is_split_derived = false;
                    current_clause_is_direct_split_remainder = false;
                    current_clause_has_split_left_neighbor = false;
                    current_clause_split_group_id = None;
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
                        current_clause_is_split_derived = false;
                        current_clause_is_direct_split_remainder = false;
                        current_clause_has_split_left_neighbor = false;
                        current_clause_split_group_id = None;

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
                ClientAction::EnsureClauseNavigationReady => {
                    Self::log_clause_action_state(
                        "before",
                        action,
                        &preview,
                        &suffix,
                        &raw_input,
                        &raw_hiragana,
                        &fixed_prefix,
                        corresponding_count,
                        selection_index,
                        &candidates,
                        &clause_snapshots,
                        &future_clause_snapshots,
                    );
                    let effect = {
                        let mut state = ClauseActionStateMut {
                            preview: &mut preview,
                            suffix: &mut suffix,
                            raw_input: &mut raw_input,
                            raw_hiragana: &mut raw_hiragana,
                            fixed_prefix: &mut fixed_prefix,
                            corresponding_count: &mut corresponding_count,
                            selection_index: &mut selection_index,
                            candidates: &mut candidates,
                            clause_snapshots: &mut clause_snapshots,
                            future_clause_snapshots: &mut future_clause_snapshots,
                            current_clause_is_split_derived: &mut current_clause_is_split_derived,
                            current_clause_is_direct_split_remainder:
                                &mut current_clause_is_direct_split_remainder,
                            current_clause_has_split_left_neighbor:
                                &mut current_clause_has_split_left_neighbor,
                            current_clause_split_group_id: &mut current_clause_split_group_id,
                            next_split_group_id: &mut next_split_group_id,
                        };
                        Self::ensure_clause_navigation_ready(&mut state, &mut ipc_service)?
                    };

                    if effect.applied {
                        self.sync_clause_action_ui(
                            &preview,
                            &suffix,
                            &candidates,
                            selection_index,
                            &mut ipc_service,
                            effect.update_pos,
                        )?;
                        Self::log_clause_action_state(
                            "after",
                            action,
                            &preview,
                            &suffix,
                            &raw_input,
                            &raw_hiragana,
                            &fixed_prefix,
                            corresponding_count,
                            selection_index,
                            &candidates,
                            &clause_snapshots,
                            &future_clause_snapshots,
                        );
                    } else {
                        Self::log_clause_action_state(
                            "skip",
                            action,
                            &preview,
                            &suffix,
                            &raw_input,
                            &raw_hiragana,
                            &fixed_prefix,
                            corresponding_count,
                            selection_index,
                            &candidates,
                            &clause_snapshots,
                            &future_clause_snapshots,
                        );
                    }
                }
                ClientAction::MoveClause(direction) => {
                    Self::log_clause_action_state(
                        "before",
                        action,
                        &preview,
                        &suffix,
                        &raw_input,
                        &raw_hiragana,
                        &fixed_prefix,
                        corresponding_count,
                        selection_index,
                        &candidates,
                        &clause_snapshots,
                        &future_clause_snapshots,
                    );
                    let effect = {
                        let mut state = ClauseActionStateMut {
                            preview: &mut preview,
                            suffix: &mut suffix,
                            raw_input: &mut raw_input,
                            raw_hiragana: &mut raw_hiragana,
                            fixed_prefix: &mut fixed_prefix,
                            corresponding_count: &mut corresponding_count,
                            selection_index: &mut selection_index,
                            candidates: &mut candidates,
                            clause_snapshots: &mut clause_snapshots,
                            future_clause_snapshots: &mut future_clause_snapshots,
                            current_clause_is_split_derived: &mut current_clause_is_split_derived,
                            current_clause_is_direct_split_remainder:
                                &mut current_clause_is_direct_split_remainder,
                            current_clause_has_split_left_neighbor:
                                &mut current_clause_has_split_left_neighbor,
                            current_clause_split_group_id: &mut current_clause_split_group_id,
                            next_split_group_id: &mut next_split_group_id,
                        };
                        Self::apply_move_clause(&mut state, &mut ipc_service, *direction)?
                    };

                    if effect.applied {
                        self.sync_clause_action_ui(
                            &preview,
                            &suffix,
                            &candidates,
                            selection_index,
                            &mut ipc_service,
                            effect.update_pos,
                        )?;
                        Self::log_clause_action_state(
                            "after",
                            action,
                            &preview,
                            &suffix,
                            &raw_input,
                            &raw_hiragana,
                            &fixed_prefix,
                            corresponding_count,
                            selection_index,
                            &candidates,
                            &clause_snapshots,
                            &future_clause_snapshots,
                        );
                    } else {
                        Self::log_clause_action_state(
                            "skip",
                            action,
                            &preview,
                            &suffix,
                            &raw_input,
                            &raw_hiragana,
                            &fixed_prefix,
                            corresponding_count,
                            selection_index,
                            &candidates,
                            &clause_snapshots,
                            &future_clause_snapshots,
                        );
                    }
                }
                ClientAction::AdjustBoundary(direction) => {
                    Self::log_clause_action_state(
                        "before",
                        action,
                        &preview,
                        &suffix,
                        &raw_input,
                        &raw_hiragana,
                        &fixed_prefix,
                        corresponding_count,
                        selection_index,
                        &candidates,
                        &clause_snapshots,
                        &future_clause_snapshots,
                    );
                    let effect = {
                        let mut state = ClauseActionStateMut {
                            preview: &mut preview,
                            suffix: &mut suffix,
                            raw_input: &mut raw_input,
                            raw_hiragana: &mut raw_hiragana,
                            fixed_prefix: &mut fixed_prefix,
                            corresponding_count: &mut corresponding_count,
                            selection_index: &mut selection_index,
                            candidates: &mut candidates,
                            clause_snapshots: &mut clause_snapshots,
                            future_clause_snapshots: &mut future_clause_snapshots,
                            current_clause_is_split_derived: &mut current_clause_is_split_derived,
                            current_clause_is_direct_split_remainder:
                                &mut current_clause_is_direct_split_remainder,
                            current_clause_has_split_left_neighbor:
                                &mut current_clause_has_split_left_neighbor,
                            current_clause_split_group_id: &mut current_clause_split_group_id,
                            next_split_group_id: &mut next_split_group_id,
                        };
                        Self::apply_adjust_boundary(&mut state, &mut ipc_service, *direction)?
                    };

                    if effect.applied {
                        self.sync_clause_action_ui(
                            &preview,
                            &suffix,
                            &candidates,
                            selection_index,
                            &mut ipc_service,
                            effect.update_pos,
                        )?;
                        Self::log_clause_action_state(
                            "after",
                            action,
                            &preview,
                            &suffix,
                            &raw_input,
                            &raw_hiragana,
                            &fixed_prefix,
                            corresponding_count,
                            selection_index,
                            &candidates,
                            &clause_snapshots,
                            &future_clause_snapshots,
                        );
                    } else {
                        Self::log_clause_action_state(
                            "skip",
                            action,
                            &preview,
                            &suffix,
                            &raw_input,
                            &raw_hiragana,
                            &fixed_prefix,
                            corresponding_count,
                            selection_index,
                            &candidates,
                            &clause_snapshots,
                            &future_clause_snapshots,
                        );
                    }
                }
                ClientAction::SetIMEMode(mode) => {
                    self.start_composition()?;
                    self.update_pos()?;
                    self.end_composition()?;

                    {
                        let mut ime_state = IMEState::get()?;
                        ime_state.input_mode = mode.clone();
                    }

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
                    current_clause_is_split_derived = false;
                    current_clause_is_direct_split_remainder = false;
                    current_clause_has_split_left_neighbor = false;
                    current_clause_split_group_id = None;
                    next_split_group_id = 0;
                    ipc_service.clear_text()?;
                }
                ClientAction::SetSelection(selection) => {
                    Self::log_clause_action_state(
                        "before",
                        action,
                        &preview,
                        &suffix,
                        &raw_input,
                        &raw_hiragana,
                        &fixed_prefix,
                        corresponding_count,
                        selection_index,
                        &candidates,
                        &clause_snapshots,
                        &future_clause_snapshots,
                    );
                    let effect = {
                        let mut state = ClauseActionStateMut {
                            preview: &mut preview,
                            suffix: &mut suffix,
                            raw_input: &mut raw_input,
                            raw_hiragana: &mut raw_hiragana,
                            fixed_prefix: &mut fixed_prefix,
                            corresponding_count: &mut corresponding_count,
                            selection_index: &mut selection_index,
                            candidates: &mut candidates,
                            clause_snapshots: &mut clause_snapshots,
                            future_clause_snapshots: &mut future_clause_snapshots,
                            current_clause_is_split_derived: &mut current_clause_is_split_derived,
                            current_clause_is_direct_split_remainder:
                                &mut current_clause_is_direct_split_remainder,
                            current_clause_has_split_left_neighbor:
                                &mut current_clause_has_split_left_neighbor,
                            current_clause_split_group_id: &mut current_clause_split_group_id,
                            next_split_group_id: &mut next_split_group_id,
                        };
                        Self::apply_set_selection(&mut state, selection)
                    };

                    if effect.applied {
                        self.sync_clause_action_ui(
                            &preview,
                            &suffix,
                            &candidates,
                            selection_index,
                            &mut ipc_service,
                            effect.update_pos,
                        )?;
                        Self::log_clause_action_state(
                            "after",
                            action,
                            &preview,
                            &suffix,
                            &raw_input,
                            &raw_hiragana,
                            &fixed_prefix,
                            corresponding_count,
                            selection_index,
                            &candidates,
                            &clause_snapshots,
                            &future_clause_snapshots,
                        );
                    } else {
                        Self::log_clause_action_state(
                            "skip",
                            action,
                            &preview,
                            &suffix,
                            &raw_input,
                            &raw_hiragana,
                            &fixed_prefix,
                            corresponding_count,
                            selection_index,
                            &candidates,
                            &clause_snapshots,
                            &future_clause_snapshots,
                        );
                    }
                }
                ClientAction::ShrinkText(text) => {
                    fixed_prefix.clear();
                    Self::clear_clause_caches(
                        &mut clause_snapshots,
                        &mut future_clause_snapshots,
                        &mut ipc_service,
                    )?;
                    current_clause_is_split_derived = false;
                    current_clause_is_direct_split_remainder = false;
                    current_clause_has_split_left_neighbor = false;
                    current_clause_split_group_id = None;
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
                    current_clause_is_split_derived = false;
                    current_clause_is_direct_split_remainder = false;
                    current_clause_has_split_left_neighbor = false;
                    current_clause_split_group_id = None;
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
                    current_clause_is_split_derived = false;
                    current_clause_is_direct_split_remainder = false;
                    current_clause_has_split_left_neighbor = false;
                    current_clause_split_group_id = None;
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
                    let clause_raw_input = Self::current_clause_raw_input_preview(
                        &raw_input,
                        corresponding_count,
                        &future_clause_snapshots,
                    );
                    let clause_raw_hiragana = Self::current_clause_raw_hiragana_preview(
                        &raw_hiragana,
                        corresponding_count,
                        &future_clause_snapshots,
                    );
                    let converted_clause = Self::converted_clause_preview_text(
                        set_type,
                        &clause_raw_input,
                        &clause_raw_hiragana,
                    );

                    preview = Self::merge_preview_with_prefix(&fixed_prefix, &converted_clause);
                    Self::sync_clause_snapshot_suffixes(&mut clause_snapshots, &preview, &suffix);
                    self.set_text(&preview, &suffix)?;
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
        composition.current_clause_is_split_derived = current_clause_is_split_derived;
        composition.current_clause_is_direct_split_remainder =
            current_clause_is_direct_split_remainder;
        composition.current_clause_has_split_left_neighbor = current_clause_has_split_left_neighbor;
        composition.current_clause_split_group_id = current_clause_split_group_id;
        composition.next_split_group_id = next_split_group_id;
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
mod tests;
