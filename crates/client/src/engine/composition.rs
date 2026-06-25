use std::time::{Duration, Instant};

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
    ipc_service::{
        client_performance_log_enabled, current_input_trace_request_id, Candidates,
        ClientInputTraceGuard, IPCService, WindowRpcDelivery,
    },
    romaji_lookup::RomajiLookup,
    state::{keyboard_disabled_from_context, AppConfigSnapshot, IMEState},
    text_util::{to_half_katakana, to_katakana},
    user_action::{Function, Navigation},
};
#[cfg(test)]
use shared::RomajiRule;
use shared::{
    zenzai_cpu_backend_supported, AppConfig, NumpadInputMode, SpaceInputMode,
    LIVE_CONVERSION_READING_VERTICAL_ADJUSTMENT_MAX,
    LIVE_CONVERSION_READING_VERTICAL_ADJUSTMENT_MIN,
};
use windows::core::{w, PCWSTR};
use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    System::Registry::{RegGetValueW, HKEY, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ},
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, GetKeyboardType, VK_CONTROL, VK_LCONTROL, VK_LMENU, VK_LSHIFT,
            VK_MENU, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_SHIFT,
        },
        TextServices::{ITfComposition, ITfCompositionSink_Impl, ITfContext},
    },
};

use anyhow::{Context, Result};

const VK_CAPITAL_KEY_CODE: usize = 0x14;
const VK_TRANSLATED_CAPSLOCK_KEY_CODE: usize = 0xF0;
const CAPSLOCK_SCAN_CODE: isize = 0x3A;
const KEYBOARD_TYPE_ENHANCED_101_OR_102: i32 = 0x4;
const KEYBOARD_TYPE_JAPANESE: i32 = 0x7;

mod clause_state;
pub(super) use clause_state::{
    CandidateSelection, ClauseActionBackend, ClauseActionEffect, ClauseActionStateMut,
    ClauseCommand, ClauseState, ClauseTransitionInput, MoveClauseProgressMarker,
};

#[cfg(test)]
pub(crate) struct CompositionReducer;

#[cfg(test)]
impl CompositionReducer {
    pub(crate) fn plan_actions_for_user_action(
        composition: &Composition,
        action: &UserAction,
        mode: &InputMode,
        is_shift_pressed: bool,
        app_config: &AppConfig,
        start_temporary_latin: bool,
    ) -> Option<(CompositionState, Vec<ClientAction>)> {
        let romaji_lookup = RomajiLookup::from_rows(&app_config.romaji_table.rows);
        TextServiceFactory::plan_actions_for_user_action_with_lookup(
            composition,
            action,
            mode,
            is_shift_pressed,
            app_config,
            &romaji_lookup,
            start_temporary_latin,
        )
    }
}

#[derive(Default, Clone, PartialEq, Debug)]
pub enum CompositionState {
    #[default]
    None,
    Composing,
    Previewing,
    Selecting,
}

pub(crate) type ProcessKeyResult = Option<(Vec<ClientAction>, CompositionState, AppConfigSnapshot)>;

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

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum CapsLockKeyboardLayout {
    Japanese,
    English,
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct ClauseNavigationReadyUiSync {
    update_pos: bool,
    visible: Option<bool>,
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

    fn log_client_performance(
        request_id: u64,
        operation: &str,
        stage: &str,
        elapsed: Duration,
        details: impl Into<String>,
    ) {
        if !client_performance_log_enabled() {
            return;
        }

        if let Ok(Some(ipc_service)) = IMEState::ipc_service() {
            ipc_service.log_client_performance(
                request_id,
                operation,
                stage,
                elapsed,
                details.into(),
            );
        }
    }

    fn ensure_ipc_service_for_key_event(operation: &str) -> bool {
        match IMEState::ensure_ipc_service() {
            Ok(true) => {
                tracing::debug!(operation, "Initialized IPC service during key event");
                true
            }
            Ok(false) => true,
            Err(error) => {
                tracing::debug!(
                    ?error,
                    operation,
                    "IPC service is unavailable during key event"
                );
                false
            }
        }
    }

    #[inline]
    pub(crate) fn is_ctrl_pressed() -> bool {
        VK_CONTROL.is_pressed() || VK_LCONTROL.is_pressed() || VK_RCONTROL.is_pressed()
    }

    #[inline]
    pub(crate) fn is_alt_pressed() -> bool {
        VK_MENU.is_pressed() || VK_LMENU.is_pressed() || VK_RMENU.is_pressed()
    }

    #[inline]
    pub(crate) fn is_shift_pressed() -> bool {
        VK_SHIFT.is_pressed()
            || VK_LSHIFT.is_pressed()
            || VK_RSHIFT.is_pressed()
            || unsafe {
                [VK_SHIFT, VK_LSHIFT, VK_RSHIFT]
                    .iter()
                    .any(|key| GetAsyncKeyState(key.0 as i32) as u16 & 0x8000 != 0)
            }
    }

    #[inline]
    pub(crate) fn is_shift_key(wparam: WPARAM) -> bool {
        matches!(wparam.0, 0x10 | 0xA0 | 0xA1)
    }

    #[inline]
    fn is_shift_alphabet_shortcut(wparam: WPARAM, is_shift_pressed: bool) -> bool {
        is_shift_pressed && (0x41..=0x5A).contains(&wparam.0)
    }

    #[inline]
    fn is_eisu_shortcut(
        key_code: usize,
        lparam: LPARAM,
        is_shift_pressed: bool,
        is_ctrl_pressed: bool,
        is_alt_pressed: bool,
        keyboard_layout: CapsLockKeyboardLayout,
    ) -> bool {
        if is_ctrl_pressed || is_alt_pressed {
            return false;
        }

        match keyboard_layout {
            CapsLockKeyboardLayout::Japanese => {
                !is_shift_pressed
                    && (key_code == VK_CAPITAL_KEY_CODE
                        || Self::is_translated_capslock_key(key_code, lparam))
            }
            CapsLockKeyboardLayout::English => {
                is_shift_pressed
                    && (key_code == VK_CAPITAL_KEY_CODE
                        || Self::is_translated_capslock_key(key_code, lparam))
            }
        }
    }

    #[inline]
    fn is_translated_capslock_key(key_code: usize, lparam: LPARAM) -> bool {
        key_code == VK_TRANSLATED_CAPSLOCK_KEY_CODE
            && ((lparam.0 >> 16) & 0xff) == CAPSLOCK_SCAN_CODE
    }

    #[inline]
    fn caps_lock_keyboard_layout_from_keyboard_type(
        keyboard_type: Option<i32>,
    ) -> Option<CapsLockKeyboardLayout> {
        match keyboard_type {
            Some(KEYBOARD_TYPE_ENHANCED_101_OR_102) => Some(CapsLockKeyboardLayout::English),
            Some(KEYBOARD_TYPE_JAPANESE) => Some(CapsLockKeyboardLayout::Japanese),
            _ => None,
        }
    }

    #[inline]
    fn caps_lock_keyboard_layout_from_hardware_registry(
        layer_driver: Option<&str>,
        keyboard_identifier: Option<&str>,
    ) -> CapsLockKeyboardLayout {
        if let Some(layer_driver) = layer_driver.map(str::trim) {
            if layer_driver.eq_ignore_ascii_case("kbd101.dll") {
                return CapsLockKeyboardLayout::English;
            }

            if layer_driver.eq_ignore_ascii_case("kbd106.dll") {
                return CapsLockKeyboardLayout::Japanese;
            }
        }

        if let Some(keyboard_identifier) = keyboard_identifier.map(str::trim) {
            if keyboard_identifier.contains("101") {
                return CapsLockKeyboardLayout::English;
            }

            if keyboard_identifier.contains("106") {
                return CapsLockKeyboardLayout::Japanese;
            }
        }

        CapsLockKeyboardLayout::Japanese
    }

    #[inline]
    fn caps_lock_keyboard_layout_from_sources(
        keyboard_type: Option<i32>,
        layer_driver: Option<&str>,
        keyboard_identifier: Option<&str>,
    ) -> CapsLockKeyboardLayout {
        if let Some(layout) = Self::caps_lock_keyboard_layout_from_keyboard_type(keyboard_type) {
            return layout;
        }

        Self::caps_lock_keyboard_layout_from_hardware_registry(layer_driver, keyboard_identifier)
    }

    fn registry_string_value(root: HKEY, sub_key: PCWSTR, value: PCWSTR) -> Option<String> {
        let mut byte_len = 0u32;
        let result = unsafe {
            RegGetValueW(
                root,
                sub_key,
                value,
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut byte_len),
            )
        };
        if !result.is_ok() || byte_len == 0 {
            return None;
        }

        let mut buffer = vec![0u16; (byte_len as usize + 1) / 2];
        let result = unsafe {
            RegGetValueW(
                root,
                sub_key,
                value,
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut byte_len),
            )
        };
        if !result.is_ok() {
            return None;
        }

        let end = buffer
            .iter()
            .position(|ch| *ch == 0)
            .unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..end]))
    }

    pub(crate) fn current_caps_lock_keyboard_layout() -> CapsLockKeyboardLayout {
        let keyboard_type = unsafe { GetKeyboardType(0) };
        let layer_driver = Self::registry_string_value(
            HKEY_LOCAL_MACHINE,
            w!("SYSTEM\\CurrentControlSet\\Services\\i8042prt\\Parameters"),
            w!("LayerDriver JPN"),
        );
        let keyboard_identifier = Self::registry_string_value(
            HKEY_LOCAL_MACHINE,
            w!("SYSTEM\\CurrentControlSet\\Services\\i8042prt\\Parameters"),
            w!("OverrideKeyboardIdentifier"),
        );

        Self::caps_lock_keyboard_layout_from_sources(
            Some(keyboard_type),
            layer_driver.as_deref(),
            keyboard_identifier.as_deref(),
        )
    }

    #[inline]
    fn input_text_for_mode(input_char: char, mode: &InputMode) -> String {
        match mode {
            InputMode::Kana if input_char.is_ascii_uppercase() => {
                input_char.to_ascii_lowercase().to_string()
            }
            _ => input_char.to_string(),
        }
    }

    #[inline]
    fn ctrl_conversion_shortcut_function(
        key_code: usize,
        is_ctrl_pressed: bool,
        is_alt_pressed: bool,
    ) -> Option<Function> {
        if is_ctrl_pressed && !is_alt_pressed {
            Function::from_ctrl_shortcut_key_code(key_code)
        } else {
            None
        }
    }

    #[inline]
    fn set_text_type_for_function(function: Function) -> SetTextType {
        match function {
            Function::Six => SetTextType::Hiragana,
            Function::Seven => SetTextType::Katakana,
            Function::Eight => SetTextType::HalfKatakana,
            Function::Nine => SetTextType::FullLatin,
            Function::Ten => SetTextType::HalfLatin,
        }
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

    #[inline]
    fn has_romaji_table_context_with_lookup(
        raw_input_before: &str,
        symbol: char,
        romaji_lookup: &RomajiLookup,
    ) -> bool {
        romaji_lookup.has_romaji_table_context(raw_input_before, symbol)
    }

    #[inline]
    #[cfg(test)]
    fn should_apply_symbol_fallback(
        raw_input_before: &str,
        input: &str,
        romaji_rows: &[RomajiRule],
    ) -> bool {
        let romaji_lookup = RomajiLookup::from_rows(romaji_rows);
        Self::should_apply_symbol_fallback_with_lookup(raw_input_before, input, &romaji_lookup)
    }

    #[inline]
    fn should_apply_symbol_fallback_with_lookup(
        raw_input_before: &str,
        input: &str,
        romaji_lookup: &RomajiLookup,
    ) -> bool {
        let Some(symbols) = Self::single_symbol_candidates(input) else {
            return false;
        };

        !symbols.iter().any(|symbol| {
            Self::has_romaji_table_context_with_lookup(raw_input_before, *symbol, romaji_lookup)
        })
    }

    #[inline]
    fn has_multi_character_romaji_context_with_lookup(
        raw_input_before: &str,
        symbol: char,
        romaji_lookup: &RomajiLookup,
    ) -> bool {
        romaji_lookup.has_multi_character_romaji_context(raw_input_before, symbol)
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
    #[cfg(test)]
    fn single_symbol_romaji_output(input: &str, romaji_rows: &[RomajiRule]) -> Option<String> {
        let romaji_lookup = RomajiLookup::from_rows(romaji_rows);
        Self::single_symbol_romaji_output_with_lookup(input, &romaji_lookup)
    }

    #[inline]
    fn single_symbol_romaji_output_with_lookup(
        input: &str,
        romaji_lookup: &RomajiLookup,
    ) -> Option<String> {
        let symbols = Self::single_symbol_candidates(input)?;
        romaji_lookup.single_symbol_output(&symbols)
    }

    #[inline]
    #[cfg(test)]
    fn resolve_symbol_input_text(
        raw_input_before: &str,
        input: &str,
        app_config: &AppConfig,
    ) -> Option<String> {
        let romaji_lookup = RomajiLookup::from_rows(&app_config.romaji_table.rows);
        Self::resolve_symbol_input_text_with_lookup(
            raw_input_before,
            input,
            app_config,
            &romaji_lookup,
        )
    }

    #[inline]
    fn resolve_symbol_input_text_with_lookup(
        raw_input_before: &str,
        input: &str,
        app_config: &AppConfig,
        romaji_lookup: &RomajiLookup,
    ) -> Option<String> {
        let symbols = Self::single_symbol_candidates(input)?;
        let is_zenzai_enabled = Self::effective_zenzai_runtime_enabled(app_config);
        if is_zenzai_enabled {
            if symbols.iter().any(|symbol| {
                Self::has_multi_character_romaji_context_with_lookup(
                    raw_input_before,
                    *symbol,
                    romaji_lookup,
                )
            }) {
                return None;
            }

            if let Some(mapped) =
                Self::single_symbol_romaji_output_with_lookup(input, romaji_lookup)
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

        if Self::should_apply_symbol_fallback_with_lookup(raw_input_before, input, romaji_lookup) {
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
    fn select_navigation_candidate_for_current_preview(
        navigation_candidates: &Candidates,
        current_preview: &str,
        fixed_prefix: &str,
        selection_index: i32,
    ) -> Option<CandidateSelection> {
        let target_clause_preview = current_preview
            .strip_prefix(fixed_prefix)
            .unwrap_or(current_preview);

        if let Some(index) = (0..navigation_candidates.texts.len()).find(|index| {
            let text = navigation_candidates
                .texts
                .get(*index)
                .map(String::as_str)
                .unwrap_or_default();
            let sub_text = navigation_candidates
                .sub_texts
                .get(*index)
                .map(String::as_str)
                .unwrap_or_default();

            target_clause_preview.strip_prefix(text) == Some(sub_text)
        }) {
            return Self::select_candidate(navigation_candidates, index as i32);
        }

        if let Some(index) = navigation_candidates
            .texts
            .iter()
            .enumerate()
            .filter(|(_, text)| !text.is_empty() && target_clause_preview.starts_with(*text))
            .max_by_key(|(index, text)| {
                (
                    navigation_candidates
                        .corresponding_count
                        .get(*index)
                        .copied()
                        .unwrap_or(0),
                    text.len(),
                )
            })
            .map(|(index, _)| index)
        {
            return Self::select_candidate(navigation_candidates, index as i32);
        }

        Self::select_candidate(navigation_candidates, selection_index)
    }

    #[inline]
    fn split_at_char_count(value: &str, char_count: i32) -> (String, String) {
        let split_at = char_count.max(0) as usize;
        let mut prefix = String::new();
        let mut suffix = String::new();

        for (index, ch) in value.chars().enumerate() {
            if index < split_at {
                prefix.push(ch);
            } else {
                suffix.push(ch);
            }
        }

        (prefix, suffix)
    }

    #[inline]
    fn display_override_set_type(
        current_preview: &str,
        fixed_prefix: &str,
        raw_input: &str,
        raw_hiragana: &str,
    ) -> Option<SetTextType> {
        let target_clause_preview = current_preview
            .strip_prefix(fixed_prefix)
            .unwrap_or(current_preview);
        let set_types = [
            SetTextType::Hiragana,
            SetTextType::Katakana,
            SetTextType::HalfKatakana,
            SetTextType::FullLatin,
            SetTextType::HalfLatin,
        ];

        for set_type in set_types {
            if Self::converted_clause_preview_text(&set_type, raw_input, raw_hiragana)
                != target_clause_preview
            {
                continue;
            }

            return Some(set_type);
        }

        None
    }

    #[inline]
    fn display_override_split_for_selected_candidate(
        set_type: &SetTextType,
        raw_input: &str,
        raw_hiragana: &str,
        selected: &CandidateSelection,
        suffix_raw_input: Option<&str>,
        suffix_raw_hiragana: Option<&str>,
    ) -> (String, String) {
        let (clause_raw_input, suffix_raw_input) = suffix_raw_input
            .and_then(|suffix| {
                raw_input
                    .strip_suffix(suffix)
                    .map(|prefix| (prefix, suffix))
            })
            .map(|(prefix, suffix)| (prefix.to_string(), suffix.to_string()))
            .unwrap_or_else(|| Self::split_at_char_count(raw_input, selected.corresponding_count));
        let (clause_raw_hiragana, suffix_raw_hiragana) = suffix_raw_hiragana
            .and_then(|suffix| {
                raw_hiragana
                    .strip_suffix(suffix)
                    .map(|prefix| (prefix, suffix))
            })
            .map(|(prefix, suffix)| (prefix.to_string(), suffix.to_string()))
            .unwrap_or_else(|| {
                Self::split_at_char_count(raw_hiragana, selected.corresponding_count)
            });

        (
            Self::converted_clause_preview_text(set_type, &clause_raw_input, &clause_raw_hiragana),
            Self::converted_clause_preview_text(set_type, &suffix_raw_input, &suffix_raw_hiragana),
        )
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
            ClientAction::CommitTextDirect(_) => "CommitTextDirect",
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
        candidates: &Candidates,
    ) -> Result<()> {
        if clause_snapshots.is_empty() {
            return Ok(());
        }

        clause_snapshots.clear();
        let _ = ipc_service
            .move_cursor_with_context(Self::MOVE_CURSOR_CLEAR_CLAUSE_SNAPSHOTS, candidates)?;
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
        candidates: &Candidates,
    ) -> Result<()> {
        Self::clear_future_clause_snapshots(future_clause_snapshots);
        Self::clear_clause_snapshots(clause_snapshots, ipc_service, candidates)
    }

    #[cfg(test)]
    #[inline]
    fn ensure_clause_navigation_ready<B: ClauseActionBackend>(
        state: &mut ClauseActionStateMut<'_>,
        backend: &mut B,
    ) -> Result<ClauseActionEffect> {
        Ok(ClauseState::transition_with_backend(
            state,
            ClauseCommand::StartClauseNavigation,
            ClauseTransitionInput::default(),
            backend,
        )?
        .effect)
    }

    #[cfg(test)]
    #[inline]
    fn apply_move_clause<B: ClauseActionBackend>(
        state: &mut ClauseActionStateMut<'_>,
        backend: &mut B,
        direction: i32,
    ) -> Result<ClauseActionEffect> {
        Ok(ClauseState::transition_with_backend(
            state,
            ClauseCommand::MoveBy(direction),
            ClauseTransitionInput::default(),
            backend,
        )?
        .effect)
    }

    #[cfg(test)]
    #[inline]
    fn apply_adjust_boundary<B: ClauseActionBackend>(
        state: &mut ClauseActionStateMut<'_>,
        backend: &mut B,
        direction: i32,
    ) -> Result<ClauseActionEffect> {
        Ok(ClauseState::transition_with_backend(
            state,
            ClauseCommand::AdjustBoundary(direction),
            ClauseTransitionInput::default(),
            backend,
        )?
        .effect)
    }

    #[cfg(test)]
    #[inline]
    fn apply_set_selection(
        state: &mut ClauseActionStateMut<'_>,
        selection: &SetSelectionType,
    ) -> ClauseActionEffect {
        ClauseState::transition_without_backend(state, ClauseCommand::SetSelection(selection))
            .effect
    }

    #[inline]
    fn sync_clause_action_ui(
        &self,
        preview: &str,
        suffix: &str,
        candidates: &Candidates,
        selection_index: i32,
        ipc_service: &mut IPCService,
        visible: Option<bool>,
        update_pos: bool,
        reading: Option<&str>,
        candidate_list_visible: Option<bool>,
        reading_vertical_adjustment: Option<i32>,
    ) -> Result<()> {
        self.set_text(preview, suffix)?;
        self.sync_candidate_window_update(
            ipc_service,
            candidates,
            selection_index,
            visible,
            update_pos,
            reading,
            candidate_list_visible,
            reading_vertical_adjustment,
        )?;
        Ok(())
    }

    #[inline]
    fn live_conversion_reading<'a>(
        app_config: &AppConfig,
        candidates: &'a Candidates,
        transition: &CompositionState,
    ) -> Option<&'a str> {
        if app_config.general.show_live_conversion_reading
            && *transition != CompositionState::None
            && !candidates.hiragana.is_empty()
        {
            Some(candidates.hiragana.as_str())
        } else {
            None
        }
    }

    #[inline]
    fn live_conversion_reading_update<'a>(
        app_config: &AppConfig,
        candidates: &'a Candidates,
        transition: &CompositionState,
    ) -> Option<&'a str> {
        Self::live_conversion_reading(app_config, candidates, transition).or(Some(""))
    }

    #[inline]
    fn live_conversion_reading_vertical_adjustment(app_config: &AppConfig) -> i32 {
        app_config
            .general
            .live_conversion_reading_vertical_adjustment
            .clamp(
                LIVE_CONVERSION_READING_VERTICAL_ADJUSTMENT_MIN,
                LIVE_CONVERSION_READING_VERTICAL_ADJUSTMENT_MAX,
            )
    }

    #[inline]
    fn live_conversion_reading_vertical_adjustment_update(
        app_config: &AppConfig,
        reading: Option<&str>,
    ) -> Option<i32> {
        reading
            .is_some_and(|value| !value.is_empty())
            .then(|| Self::live_conversion_reading_vertical_adjustment(app_config))
    }

    #[inline]
    fn live_conversion_reading_vertical_adjustment_for_update(
        app_config: &AppConfig,
        candidates: &Candidates,
        transition: &CompositionState,
    ) -> Option<i32> {
        Self::live_conversion_reading_vertical_adjustment_update(
            app_config,
            Self::live_conversion_reading(app_config, candidates, transition),
        )
    }

    #[inline]
    fn sync_candidate_window_update(
        &self,
        ipc_service: &mut IPCService,
        candidates: &Candidates,
        selection_index: i32,
        visible: Option<bool>,
        update_pos: bool,
        reading: Option<&str>,
        candidate_list_visible: Option<bool>,
        reading_vertical_adjustment: Option<i32>,
    ) -> Result<()> {
        let reading = if visible == Some(false) && reading.is_none() {
            Some("")
        } else {
            reading
        };
        let trace_request_id = current_input_trace_request_id();
        let total_start = trace_request_id.map(|_| Instant::now());
        let result: Result<()> = (|| {
            let should_update_pos =
                self.should_update_candidate_window_position(update_pos, visible);
            let position = if should_update_pos {
                let position_start = trace_request_id.map(|_| Instant::now());
                let position = if visible == Some(true) {
                    self.candidate_window_position()?
                } else {
                    self.candidate_window_position_for_update()?
                };
                if let (Some(request_id), Some(position_start)) = (trace_request_id, position_start)
                {
                    Self::log_client_performance(
                        request_id,
                        "sync_candidate_window_update",
                        "candidate_window_position",
                        position_start.elapsed(),
                        format!(
                            "status=success;position_present={};candidate_count={}",
                            position.is_some(),
                            candidates.texts.len()
                        ),
                    );
                }
                position
            } else {
                if update_pos {
                    tracing::debug!(
                        "Skip candidate_window_position while candidate window is hidden"
                    );
                }
                None
            };
            let delivery = ipc_service.update_candidate_window_with_reading(
                visible,
                position,
                Some(candidates.texts.clone()),
                Some(selection_index),
                None,
                reading,
                candidate_list_visible,
                reading_vertical_adjustment,
            )?;
            self.remember_candidate_window_visibility_if_sent(delivery, visible);
            Ok(())
        })();

        if let (Some(request_id), Some(total_start)) = (trace_request_id, total_start) {
            let details = match &result {
                Ok(()) => format!(
                    "status=success;update_pos={update_pos};visible={visible:?};candidate_count={};selection_index={selection_index};reading_present={};candidate_list_visible={candidate_list_visible:?};reading_vertical_adjustment={reading_vertical_adjustment:?}",
                    candidates.texts.len(),
                    reading.is_some_and(|value| !value.is_empty())
                ),
                Err(error) => format!(
                    "status=error;update_pos={update_pos};visible={visible:?};candidate_count={};selection_index={selection_index};reading_present={};candidate_list_visible={candidate_list_visible:?};reading_vertical_adjustment={reading_vertical_adjustment:?};error={error:?}",
                    candidates.texts.len(),
                    reading.is_some_and(|value| !value.is_empty())
                ),
            };
            Self::log_client_performance(
                request_id,
                "sync_candidate_window_update",
                "total",
                total_start.elapsed(),
                details,
            );
        }

        result
    }

    #[inline]
    fn should_update_candidate_window_position(
        &self,
        update_pos: bool,
        visible: Option<bool>,
    ) -> bool {
        match self.borrow() {
            Ok(text_service) => text_service
                .candidate_window_visibility_state
                .should_update_position(update_pos, visible),
            Err(error) => {
                tracing::warn!(
                    "Assume candidate window position should update after visibility state borrow failed: {error:?}"
                );
                update_pos && visible != Some(false)
            }
        }
    }

    #[inline]
    fn delivered_candidate_window_visibility(
        delivery: WindowRpcDelivery,
        visible: Option<bool>,
    ) -> Option<bool> {
        delivery.was_sent().then_some(visible).flatten()
    }

    #[inline]
    pub(crate) fn remember_candidate_window_visibility_if_sent(
        &self,
        delivery: WindowRpcDelivery,
        visible: Option<bool>,
    ) {
        self.remember_candidate_window_visibility(Self::delivered_candidate_window_visibility(
            delivery, visible,
        ));
    }

    #[inline]
    pub(crate) fn remember_candidate_window_visibility(&self, visible: Option<bool>) {
        if visible.is_none() {
            return;
        }

        match self.borrow_mut() {
            Ok(mut text_service) => {
                text_service
                    .candidate_window_visibility_state
                    .apply_visibility_update(visible);
            }
            Err(error) => {
                tracing::warn!("Failed to remember candidate window visibility state: {error:?}");
            }
        }
    }

    #[inline]
    fn sync_candidate_window_after_text_update(
        &self,
        ipc_service: &mut IPCService,
        candidates: &Candidates,
        selection_index: i32,
        app_config: &AppConfig,
        transition: &CompositionState,
    ) -> Result<()> {
        let reading = Self::live_conversion_reading(app_config, candidates, transition);
        let reading_update = reading.or(Some(""));
        let reading_vertical_adjustment =
            Self::live_conversion_reading_vertical_adjustment_update(app_config, reading);
        let candidate_list_visible = if *transition != CompositionState::None {
            Some(!app_config.general.show_candidate_window_after_space)
        } else {
            Some(false)
        };
        let visible = if *transition == CompositionState::None {
            Some(false)
        } else if !app_config.general.show_candidate_window_after_space || reading.is_some() {
            Some(true)
        } else {
            None
        };
        let update_pos = *transition != CompositionState::None;
        self.sync_candidate_window_update(
            ipc_service,
            candidates,
            selection_index,
            visible,
            update_pos,
            reading_update,
            candidate_list_visible,
            reading_vertical_adjustment,
        )
    }

    #[inline]
    fn action_needs_context_update(action: &ClientAction) -> bool {
        matches!(
            action,
            ClientAction::AppendText(_)
                | ClientAction::AppendTextRaw(_)
                | ClientAction::AppendTextDirect(_)
                | ClientAction::ShrinkText(_)
                | ClientAction::ShrinkTextRaw(_)
                | ClientAction::ShrinkTextDirect(_)
        )
    }

    #[inline]
    fn current_raw_suffix(raw_hiragana: &str, corresponding_count: i32) -> String {
        raw_hiragana
            .chars()
            .skip(corresponding_count.max(0) as usize)
            .collect()
    }

    #[inline]
    fn append_result_indicates_server_reset(
        previous_raw_input: &str,
        previous_candidates: &Candidates,
        appended_text: &str,
        appended_candidates: &Candidates,
    ) -> bool {
        if previous_raw_input.is_empty()
            || previous_candidates.is_empty_composition()
            || appended_candidates.is_empty_composition()
        {
            return false;
        }

        let appended_len = appended_text.chars().count() as i32;
        appended_len > 0
            && appended_candidates
                .corresponding_count
                .iter()
                .copied()
                .max()
                .is_some_and(|count| count > 0 && count <= appended_len)
    }

    #[inline]
    fn has_client_composition_state(
        raw_input: &str,
        preview: &str,
        suffix: &str,
        fixed_prefix: &str,
        candidates: &Candidates,
    ) -> bool {
        !raw_input.is_empty()
            || !preview.is_empty()
            || !suffix.is_empty()
            || !fixed_prefix.is_empty()
            || !candidates.is_empty_composition()
    }

    #[inline]
    fn current_raw_input_suffix(raw_input: &str, corresponding_count: i32) -> String {
        let split_at = Self::byte_index_after_chars(raw_input, corresponding_count.max(0) as usize);
        let adjusted_split_at = Self::adjust_single_n_raw_input_boundary(raw_input, split_at);

        raw_input[adjusted_split_at..].to_string()
    }

    #[inline]
    fn byte_index_after_chars(text: &str, char_count: usize) -> usize {
        if char_count == 0 {
            return 0;
        }

        text.char_indices()
            .nth(char_count)
            .map(|(byte_index, _)| byte_index)
            .unwrap_or(text.len())
    }

    #[inline]
    fn adjust_single_n_raw_input_boundary(raw_input: &str, split_at: usize) -> usize {
        if split_at == 0 || split_at >= raw_input.len() || !raw_input.is_char_boundary(split_at) {
            return split_at;
        }

        let Some(consumed) = raw_input[..split_at].chars().next_back() else {
            return split_at;
        };
        let Some(next) = raw_input[split_at..].chars().next() else {
            return split_at;
        };
        if !Self::is_ascii_romaji_consonant(consumed) || !Self::is_ascii_romaji_vowel(next) {
            return split_at;
        }

        if raw_input[..split_at]
            .chars()
            .rev()
            .skip(1)
            .find(|ch| ch.is_ascii_alphabetic())
            .is_some_and(|ch| ch.eq_ignore_ascii_case(&'n'))
        {
            split_at - consumed.len_utf8()
        } else {
            split_at
        }
    }

    #[inline]
    fn is_ascii_romaji_vowel(ch: char) -> bool {
        matches!(ch.to_ascii_lowercase(), 'a' | 'i' | 'u' | 'e' | 'o')
    }

    #[inline]
    fn is_ascii_romaji_consonant(ch: char) -> bool {
        ch.is_ascii_alphabetic()
            && !Self::is_ascii_romaji_vowel(ch)
            && !ch.eq_ignore_ascii_case(&'n')
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
    fn trusted_raw_hiragana_suffix(raw_hiragana: &str, raw_suffix_hint: &str) -> Option<String> {
        (!raw_suffix_hint.is_empty() && raw_hiragana.ends_with(raw_suffix_hint))
            .then(|| raw_suffix_hint.to_string())
    }

    #[inline]
    fn common_suffix_char_count(left: &str, right: &str) -> usize {
        left.chars()
            .rev()
            .zip(right.chars().rev())
            .take_while(|(left, right)| left == right)
            .count()
    }

    #[inline]
    fn recover_single_n_raw_hiragana_suffix(
        full_raw_hiragana: &str,
        server_raw_hiragana: &str,
    ) -> Option<String> {
        if server_raw_hiragana.is_empty() {
            return None;
        }

        if full_raw_hiragana.ends_with(server_raw_hiragana) {
            return None;
        }

        let common_suffix_len =
            Self::common_suffix_char_count(full_raw_hiragana, server_raw_hiragana);
        let server_len = server_raw_hiragana.chars().count();
        if common_suffix_len == 0 || common_suffix_len + 1 != server_len {
            return None;
        }

        let full_len = full_raw_hiragana.chars().count();
        if full_len <= common_suffix_len {
            return None;
        }

        Some(
            full_raw_hiragana
                .chars()
                .skip(full_len - common_suffix_len - 1)
                .collect(),
        )
    }

    #[inline]
    fn has_recoverable_single_n_raw_hiragana_suffix(
        server_raw_hiragana: &str,
        snapshot_raw_hiragana: &str,
    ) -> bool {
        !server_raw_hiragana.is_empty()
            && server_raw_hiragana.chars().count() == snapshot_raw_hiragana.chars().count()
            && Self::common_suffix_char_count(server_raw_hiragana, snapshot_raw_hiragana) + 1
                == snapshot_raw_hiragana.chars().count()
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
        let raw_input_suffix = Self::current_raw_input_suffix(raw_input, corresponding_count);
        let normalized_raw_suffix_hint = future_clause_snapshots
            .last()
            .and_then(|snapshot| Self::restore_raw_suffix_from_sub_text(raw_suffix_hint, snapshot))
            .unwrap_or_else(|| raw_suffix_hint.to_string());
        let mut raw_suffix = if let Some(snapshot) = future_clause_snapshots
            .iter()
            .rev()
            .find(|snapshot| !raw_input_suffix.is_empty() && raw_input_suffix == snapshot.raw_input)
        {
            snapshot.raw_hiragana.clone()
        } else if let Some(raw_suffix) =
            Self::trusted_raw_hiragana_suffix(raw_hiragana, &normalized_raw_suffix_hint)
        {
            raw_suffix
        } else if let Some(snapshot) = future_clause_snapshots.last().filter(|snapshot| {
            Self::future_snapshot_matches_raw_suffix(snapshot, &normalized_raw_suffix_hint)
        }) {
            snapshot.raw_hiragana.clone()
        } else if !future_clause_snapshots.is_empty() && !allow_bootstrap_without_existing_future {
            Self::current_raw_suffix(raw_hiragana, corresponding_count)
        } else {
            String::new()
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
        let initial_suffix = state.suffix.clone();
        let initial_raw_input_suffix =
            Self::current_raw_input_suffix(state.raw_input, *state.corresponding_count);
        let initial_raw_hiragana = state.raw_hiragana.clone();
        let mut collected = Vec::new();

        loop {
            let (effect, made_progress) = {
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
                let before = MoveClauseProgressMarker::from_state(&temp_state);
                let effect = ClauseState::transition_with_backend(
                    &mut temp_state,
                    ClauseCommand::MoveRight,
                    ClauseTransitionInput::default(),
                    backend,
                )?
                .effect;
                let after = MoveClauseProgressMarker::from_state(&temp_state);
                (effect, before != after)
            };
            if !effect.applied || !made_progress {
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
                if temp_suffix.is_empty()
                    && !initial_suffix.is_empty()
                    && !initial_raw_input_suffix.is_empty()
                    && temp_raw_input == initial_raw_input_suffix
                {
                    if let Some(raw_hiragana_suffix) = Self::recover_single_n_raw_hiragana_suffix(
                        &initial_raw_hiragana,
                        &snapshot.raw_hiragana,
                    ) {
                        let mut repaired_snapshot = Self::build_conservative_future_clause_snapshot(
                            &initial_suffix,
                            "",
                            &initial_raw_input_suffix,
                            &raw_hiragana_suffix,
                            initial_raw_input_suffix.chars().count() as i32,
                        );
                        repaired_snapshot.is_split_derived = true;
                        repaired_snapshot.is_direct_split_remainder = true;
                        repaired_snapshot.has_split_left_neighbor = true;
                        repaired_snapshot.split_group_id = *state.current_clause_split_group_id;
                        snapshot = repaired_snapshot;
                    }
                }
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
            let previous_candidates = temp_candidates.clone();
            let _ = backend.move_cursor_with_context(
                Self::MOVE_CURSOR_POP_CLAUSE_SNAPSHOT,
                &previous_candidates,
            )?;
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
                    || server_candidates
                        .hiragana
                        .starts_with(&snapshot.raw_hiragana)
                    || server_candidates.hiragana.ends_with(&snapshot.raw_hiragana)
                    || snapshot.raw_hiragana.ends_with(&server_candidates.hiragana)
                    || Self::has_recoverable_single_n_raw_hiragana_suffix(
                        &server_candidates.hiragana,
                        &snapshot.raw_hiragana,
                    )
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

            let previous_candidates = candidates.clone();
            let _ = backend.move_cursor_with_context(-1, &previous_candidates)?;
            let next_candidates = backend.move_cursor(0)?;
            if next_candidates.texts.is_empty() {
                let _ = backend.move_cursor_with_context(1, &next_candidates)?;
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
        if ClauseState::is_active_for_composition(composition) {
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
    fn is_punctuation_commit_char(c: char) -> bool {
        matches!(c, ',' | '.' | '、' | '。' | '，' | '．')
    }

    #[inline]
    fn is_exclamation_commit_char(c: char) -> bool {
        matches!(c, '!' | '！')
    }

    #[inline]
    fn is_question_commit_char(c: char) -> bool {
        matches!(c, '?' | '？')
    }

    #[inline]
    fn punctuation_commit_target_enabled(c: char, app_config: &AppConfig) -> bool {
        (Self::is_punctuation_commit_char(c) && app_config.general.punctuation_commit_punctuation)
            || (Self::is_exclamation_commit_char(c)
                && app_config.general.punctuation_commit_exclamation)
            || (Self::is_question_commit_char(c) && app_config.general.punctuation_commit_question)
    }

    #[inline]
    fn punctuation_commit_text_for_action(
        action: &UserAction,
        mode: &InputMode,
        raw_input_before: &str,
        app_config: &AppConfig,
        romaji_lookup: &RomajiLookup,
    ) -> Option<String> {
        if !Self::punctuation_commit_action_target_enabled(
            action,
            mode,
            raw_input_before,
            app_config,
            romaji_lookup,
        ) {
            return None;
        }

        match action {
            UserAction::Input(ch) => {
                let input = ch.to_string();
                Some(Self::punctuation_commit_text_for_input(
                    &input,
                    app_config,
                    romaji_lookup,
                ))
            }
            UserAction::NumpadSymbol(symbol) => Some(
                Self::numpad_text_for_mode(*symbol, app_config.general.numpad_input, false)
                    .unwrap_or_else(|| symbol.to_string()),
            ),
            _ => None,
        }
    }

    #[inline]
    fn punctuation_commit_text_for_input(
        input: &str,
        app_config: &AppConfig,
        romaji_lookup: &RomajiLookup,
    ) -> String {
        if Self::effective_zenzai_runtime_enabled(app_config) {
            if let Some(mapped) =
                Self::single_symbol_romaji_output_with_lookup(input, romaji_lookup)
            {
                return mapped;
            }
        }

        convert_kana_symbol(
            input,
            &app_config.general,
            &app_config.character_width,
            &app_config.romaji_table.rows,
        )
    }

    #[inline]
    fn punctuation_commit_action_target_enabled(
        action: &UserAction,
        mode: &InputMode,
        raw_input_before: &str,
        app_config: &AppConfig,
        romaji_lookup: &RomajiLookup,
    ) -> bool {
        if !app_config.general.punctuation_commit || *mode != InputMode::Kana {
            return false;
        }

        match action {
            UserAction::Input(ch) => {
                Self::punctuation_commit_target_enabled(*ch, app_config)
                    && !Self::has_multi_character_romaji_context_with_lookup(
                        raw_input_before,
                        *ch,
                        romaji_lookup,
                    )
            }
            UserAction::NumpadSymbol(symbol) => {
                Self::punctuation_commit_target_enabled(*symbol, app_config)
                    && !Self::has_multi_character_romaji_context_with_lookup(
                        raw_input_before,
                        *symbol,
                        romaji_lookup,
                    )
            }
            _ => false,
        }
    }

    #[inline]
    fn punctuation_commit_actions(text: String) -> (CompositionState, Vec<ClientAction>) {
        (
            CompositionState::None,
            vec![
                ClientAction::EndComposition,
                ClientAction::CommitTextDirect(text),
            ],
        )
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
    fn commit_preview_then_append_actions(
        append_action: ClientAction,
        start_temporary_latin: bool,
    ) -> (CompositionState, Vec<ClientAction>) {
        let mut actions = Vec::with_capacity(4);
        actions.push(ClientAction::EndComposition);
        actions.push(ClientAction::StartComposition);
        if start_temporary_latin {
            actions.push(ClientAction::SetTemporaryLatin(true));
        }
        actions.push(append_action);
        (CompositionState::Composing, actions)
    }

    #[inline]
    fn clause_navigation_actions(composition: &Composition, direction: i32) -> Vec<ClientAction> {
        if ClauseState::is_active_for_composition(composition) || !composition.suffix.is_empty() {
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
    fn should_defer_clause_navigation_ready_sync(actions: &[ClientAction], index: usize) -> bool {
        matches!(
            (actions.get(index), actions.get(index + 1)),
            (
                Some(ClientAction::EnsureClauseNavigationReady),
                Some(ClientAction::MoveClause(direction)),
            ) if *direction == Self::MOVE_CLAUSE_TO_LAST
        )
    }

    #[inline]
    fn clause_navigation_ready_ui_sync(
        effect: ClauseActionEffect,
    ) -> Option<ClauseNavigationReadyUiSync> {
        effect.applied.then_some(ClauseNavigationReadyUiSync {
            update_pos: effect.update_pos,
            visible: Some(true),
        })
    }

    #[inline]
    fn deferred_clause_navigation_ready_ui_sync_after_move(
        deferred_sync: Option<ClauseNavigationReadyUiSync>,
        move_effect: ClauseActionEffect,
    ) -> Option<ClauseNavigationReadyUiSync> {
        if move_effect.server_reset {
            return None;
        }

        if move_effect.applied {
            deferred_sync.map(|sync| ClauseNavigationReadyUiSync {
                update_pos: move_effect.update_pos,
                visible: sync.visible,
            })
        } else {
            deferred_sync
        }
    }

    #[inline]
    #[cfg(test)]
    fn plan_actions_for_user_action(
        composition: &Composition,
        action: &UserAction,
        mode: &InputMode,
        is_shift_pressed: bool,
        app_config: &AppConfig,
        start_temporary_latin: bool,
    ) -> Option<(CompositionState, Vec<ClientAction>)> {
        let romaji_lookup = RomajiLookup::from_rows(&app_config.romaji_table.rows);
        Self::plan_actions_for_user_action_with_lookup(
            composition,
            action,
            mode,
            is_shift_pressed,
            app_config,
            &romaji_lookup,
            start_temporary_latin,
        )
    }

    #[inline]
    fn plan_actions_for_user_action_with_lookup(
        composition: &Composition,
        action: &UserAction,
        mode: &InputMode,
        is_shift_pressed: bool,
        app_config: &AppConfig,
        romaji_lookup: &RomajiLookup,
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
                        ClientAction::AppendText(Self::input_text_for_mode(*char, mode)),
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
                _ if !composition.temporary_latin
                    && !start_temporary_latin
                    && Self::punctuation_commit_action_target_enabled(
                        action,
                        mode,
                        &composition.raw_input,
                        app_config,
                        romaji_lookup,
                    ) =>
                {
                    let text = Self::punctuation_commit_text_for_action(
                        action,
                        mode,
                        &composition.raw_input,
                        app_config,
                        romaji_lookup,
                    )?;
                    Some(Self::punctuation_commit_actions(text))
                }
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
                    vec![ClientAction::AppendText(Self::input_text_for_mode(
                        *char, mode,
                    ))],
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
                UserAction::Backspace | UserAction::Delete => {
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
                UserAction::AdjustClauseBoundary(direction) => Some((
                    CompositionState::Composing,
                    vec![ClientAction::AdjustBoundary(*direction)],
                )),
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
                UserAction::Function(key) => Some((
                    CompositionState::Previewing,
                    vec![ClientAction::SetTextWithType(
                        Self::set_text_type_for_function(*key),
                    )],
                )),
                _ => None,
            },
            CompositionState::Previewing => match action {
                _ if !composition.temporary_latin
                    && !start_temporary_latin
                    && Self::punctuation_commit_action_target_enabled(
                        action,
                        mode,
                        &composition.raw_input,
                        app_config,
                        romaji_lookup,
                    ) =>
                {
                    let text = Self::punctuation_commit_text_for_action(
                        action,
                        mode,
                        &composition.raw_input,
                        app_config,
                        romaji_lookup,
                    )?;
                    Some(Self::punctuation_commit_actions(text))
                }
                _ if (composition.temporary_latin || start_temporary_latin)
                    && Self::direct_text_for_action(action).is_some() =>
                {
                    let text = Self::direct_text_for_action(action)?;
                    Some(Self::commit_preview_then_append_actions(
                        ClientAction::AppendTextDirect(text),
                        composition.temporary_latin || start_temporary_latin,
                    ))
                }
                UserAction::NumpadSymbol(symbol) if *mode == InputMode::Kana => {
                    let text =
                        Self::numpad_text_for_mode(*symbol, app_config.general.numpad_input, false)
                            .unwrap_or_else(|| symbol.to_string());
                    Some(Self::commit_preview_then_append_actions(
                        ClientAction::AppendTextRaw(text),
                        false,
                    ))
                }
                UserAction::Input(char) => Some(Self::commit_preview_then_append_actions(
                    ClientAction::AppendText(Self::input_text_for_mode(*char, mode)),
                    false,
                )),
                UserAction::Number {
                    value,
                    is_numpad: true,
                } if *mode == InputMode::Kana => {
                    let digit = char::from_digit(*value as u32, 10).unwrap_or('0');
                    let text =
                        Self::numpad_text_for_mode(digit, app_config.general.numpad_input, false)
                            .unwrap_or_else(|| digit.to_string());
                    Some(Self::commit_preview_then_append_actions(
                        ClientAction::AppendTextRaw(text),
                        false,
                    ))
                }
                UserAction::Number { value, .. } => Some(Self::commit_preview_then_append_actions(
                    ClientAction::AppendText(value.to_string()),
                    false,
                )),
                UserAction::Backspace | UserAction::Delete => {
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
                UserAction::AdjustClauseBoundary(direction) => Some((
                    CompositionState::Previewing,
                    vec![ClientAction::AdjustBoundary(*direction)],
                )),
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
                UserAction::Function(key) => Some((
                    CompositionState::Previewing,
                    vec![ClientAction::SetTextWithType(
                        Self::set_text_type_for_function(*key),
                    )],
                )),
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
    ) -> Result<ProcessKeyResult> {
        let standalone_trace = (current_input_trace_request_id().is_none()
            && client_performance_log_enabled())
        .then(ClientInputTraceGuard::begin);
        let trace_request_id = current_input_trace_request_id().or_else(|| {
            standalone_trace
                .as_ref()
                .map(ClientInputTraceGuard::request_id)
        });
        let total_start = trace_request_id.map(|_| Instant::now());
        let result: Result<ProcessKeyResult> = (|| {
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
            let is_alt_pressed = Self::is_alt_pressed();
            let is_shift_pressed = {
                let tracked_shift = self
                    .borrow()
                    .map(|text_service| text_service.shift_key_down)
                    .unwrap_or(false);
                tracked_shift || Self::is_shift_pressed()
            };
            let is_ctrl_space = is_ctrl_pressed && wparam.0 == 0x20;
            let is_capslock_key = wparam.0 == VK_CAPITAL_KEY_CODE
                || Self::is_translated_capslock_key(wparam.0, lparam);
            let is_eisu = if is_capslock_key {
                let keyboard_layout = Self::current_caps_lock_keyboard_layout();
                Self::is_eisu_shortcut(
                    wparam.0,
                    lparam,
                    is_shift_pressed,
                    is_ctrl_pressed,
                    is_alt_pressed,
                    keyboard_layout,
                )
            } else {
                false
            };
            let is_ctrl_enter = is_ctrl_pressed && wparam.0 == 0x0D;
            let is_ctrl_down = is_ctrl_pressed && wparam.0 == 0x28;
            let ctrl_conversion_function =
                Self::ctrl_conversion_shortcut_function(wparam.0, is_ctrl_pressed, is_alt_pressed);
            let is_shift_left = is_shift_pressed && wparam.0 == 0x25;
            let is_shift_right = is_shift_pressed && wparam.0 == 0x27;
            let is_shift_key = Self::is_shift_key(wparam);
            let is_alt_backquote = Self::is_alt_backquote(wparam, lparam);
            let config_snapshot_start = trace_request_id.map(|_| Instant::now());
            let config_snapshot = IMEState::app_config_snapshot()?;
            let app_config = config_snapshot.app_config();
            let romaji_lookup = config_snapshot.romaji_lookup();
            if let (Some(request_id), Some(config_snapshot_start)) =
                (trace_request_id, config_snapshot_start)
            {
                Self::log_client_performance(
                    request_id,
                    "process_key",
                    "config_snapshot",
                    config_snapshot_start.elapsed(),
                    format!(
                        "romaji_rows={};max_input_len={};max_multi_char_input_len={};wparam={}",
                        app_config.romaji_table.rows.len(),
                        romaji_lookup.max_input_len,
                        romaji_lookup.max_multi_char_input_len,
                        wparam.0
                    ),
                );
            }

            // check shortcut keys
            if is_ctrl_pressed
                && !is_ctrl_space
                && !is_alt_backquote
                && !is_ctrl_enter
                && !is_ctrl_down
                && ctrl_conversion_function.is_none()
            {
                self.clear_temporary_latin_shift_pending_if_needed(!Self::is_shift_key(wparam))?;
                return Ok(None);
            }

            if is_ctrl_space || is_alt_backquote || is_eisu {
                let shortcuts = &app_config.shortcuts;

                if is_ctrl_space && !shortcuts.ctrl_space_toggle {
                    self.clear_temporary_latin_shift_pending_if_needed(!Self::is_shift_key(
                        wparam,
                    ))?;
                    return Ok(None);
                }

                if is_alt_backquote && !shortcuts.alt_backquote_toggle {
                    self.clear_temporary_latin_shift_pending_if_needed(!Self::is_shift_key(
                        wparam,
                    ))?;
                    return Ok(None);
                }

                if is_eisu && !shortcuts.eisu_toggle {
                    self.clear_temporary_latin_shift_pending_if_needed(!Self::is_shift_key(
                        wparam,
                    ))?;
                    return Ok(None);
                }
            }

            #[allow(clippy::let_and_return)]
            let (composition, mode) = {
                let text_service = self.borrow()?;
                let composition = text_service.borrow_composition()?.clone();
                let mode = IMEState::input_mode()?;
                (composition, mode)
            };
            let start_temporary_latin = !composition.temporary_latin
                && mode == InputMode::Kana
                && Self::is_shift_alphabet_shortcut(wparam, is_shift_pressed);

            if composition.temporary_latin && is_shift_key && !is_shift_left && !is_shift_right {
                return Ok(Some((
                    vec![ClientAction::SetTemporaryLatinShiftPending(true)],
                    composition.state.clone(),
                    config_snapshot,
                )));
            }

            let should_clear_shift_pending =
                composition.temporary_latin_shift_pending && !is_shift_key;

            let action = if is_alt_backquote || is_eisu {
                UserAction::ToggleInputMode
            } else if is_ctrl_enter {
                UserAction::CommitFirstClause
            } else if is_ctrl_down {
                UserAction::CommitAndNextClause
            } else if let Some(function) = ctrl_conversion_function {
                UserAction::Function(function)
            } else if is_shift_left {
                UserAction::AdjustClauseBoundary(-1)
            } else if is_shift_right {
                UserAction::AdjustClauseBoundary(1)
            } else {
                UserAction::try_from(wparam.0)?
            };

            let Some((transition, mut actions)) = Self::plan_actions_for_user_action_with_lookup(
                &composition,
                &action,
                &mode,
                is_shift_pressed,
                app_config,
                romaji_lookup,
                start_temporary_latin,
            ) else {
                self.clear_temporary_latin_shift_pending_if_needed(should_clear_shift_pending)?;
                return Ok(None);
            };

            if composition.temporary_latin {
                let should_reset_on_confirm = matches!(
                    action,
                    UserAction::Enter
                        | UserAction::CommitAndNextClause
                        | UserAction::CommitFirstClause
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

            if !Self::ensure_ipc_service_for_key_event("process_key") {
                return Ok(None);
            }

            Ok(Some((actions, transition, config_snapshot)))
        })();

        if let (Some(request_id), Some(total_start)) = (trace_request_id, total_start) {
            let details = match &result {
                Ok(Some((actions, transition, _))) => format!(
                    "status=success;handled=true;actions={};transition={transition:?};wparam={}",
                    actions.len(),
                    wparam.0
                ),
                Ok(None) => format!("status=success;handled=false;wparam={}", wparam.0),
                Err(error) => format!("status=error;wparam={};error={error:?}", wparam.0),
            };
            Self::log_client_performance(
                request_id,
                "process_key",
                "total",
                total_start.elapsed(),
                details,
            );
        }

        result
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

        if !Self::ensure_ipc_service_for_key_event("process_key_up") {
            return Ok(None);
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
        let input_trace = client_performance_log_enabled().then(ClientInputTraceGuard::begin);
        let total_start = input_trace.as_ref().map(|_| Instant::now());
        let result: Result<bool> = (|| {
            if let Some(context) = context {
                self.borrow_mut()?.context = Some(context.clone());
            } else {
                self.set_keyboard_disabled_state(true)?;
                return Ok(false);
            };

            if let Some((actions, transition, config_snapshot)) =
                self.process_key(context, wparam, lparam)?
            {
                self.handle_action_with_config_snapshot(&actions, transition, config_snapshot)?;
                Ok(true)
            } else {
                Ok(false)
            }
        })();

        if let (Some(input_trace), Some(total_start)) = (input_trace.as_ref(), total_start) {
            let request_id = input_trace.request_id();
            let details = match &result {
                Ok(handled) => format!(
                    "status=success;handled={handled};wparam={};lparam={}",
                    wparam.0, lparam.0
                ),
                Err(error) => format!(
                    "status=error;wparam={};lparam={};error={error:?}",
                    wparam.0, lparam.0
                ),
            };
            Self::log_client_performance(
                request_id,
                "handle_key",
                "total",
                total_start.elapsed(),
                details,
            );
        }

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
        let input_trace = client_performance_log_enabled().then(ClientInputTraceGuard::begin);
        let total_start = input_trace.as_ref().map(|_| Instant::now());
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

        if let (Some(input_trace), Some(total_start)) = (input_trace.as_ref(), total_start) {
            let request_id = input_trace.request_id();
            let details = match &result {
                Ok(handled) => format!(
                    "status=success;handled={handled};wparam={};lparam={}",
                    wparam.0, lparam.0
                ),
                Err(error) => format!(
                    "status=error;wparam={};lparam={};error={error:?}",
                    wparam.0, lparam.0
                ),
            };
            Self::log_client_performance(
                request_id,
                "handle_key_up",
                "total",
                total_start.elapsed(),
                details,
            );
        }

        match result {
            Ok(handled) => Ok(handled),
            Err(error) => {
                tracing::error!("handle_key_up failed: {error:?}");
                self.recover_after_key_error();
                Ok(false)
            }
        }
    }

    #[tracing::instrument]
    pub fn handle_preserved_eisu_shortcut(&self, context: Option<&ITfContext>) -> Result<bool> {
        let result: Result<bool> = (|| {
            let Some(context) = context else {
                self.set_keyboard_disabled_state(true)?;
                return Ok(false);
            };

            self.borrow_mut()?.context = Some(context.clone());

            let keyboard_disabled = keyboard_disabled_from_context(context);
            self.set_keyboard_disabled_state(keyboard_disabled)?;
            if keyboard_disabled {
                self.cancel_composition_for_disabled_context();
                return Ok(false);
            }

            let is_shift_pressed = {
                let tracked_shift = self
                    .borrow()
                    .map(|text_service| text_service.shift_key_down)
                    .unwrap_or(false);
                tracked_shift || Self::is_shift_pressed()
            };
            let is_ctrl_pressed = Self::is_ctrl_pressed();
            let is_alt_pressed = Self::is_alt_pressed();
            let keyboard_layout = Self::current_caps_lock_keyboard_layout();
            if !Self::is_eisu_shortcut(
                VK_CAPITAL_KEY_CODE,
                LPARAM(0),
                is_shift_pressed,
                is_ctrl_pressed,
                is_alt_pressed,
                keyboard_layout,
            ) {
                return Ok(false);
            }

            let config_snapshot = IMEState::app_config_snapshot()?;
            let app_config = config_snapshot.app_config();
            if !app_config.shortcuts.eisu_toggle {
                return Ok(false);
            }

            #[allow(clippy::let_and_return)]
            let (composition, mode) = {
                let text_service = self.borrow()?;
                let composition = text_service.borrow_composition()?.clone();
                let mode = IMEState::input_mode()?;
                (composition, mode)
            };

            let Some((transition, mut actions)) = Self::plan_actions_for_user_action_with_lookup(
                &composition,
                &UserAction::ToggleInputMode,
                &mode,
                is_shift_pressed,
                app_config,
                config_snapshot.romaji_lookup(),
                false,
            ) else {
                return Ok(false);
            };

            if composition.temporary_latin
                && !actions
                    .iter()
                    .any(|action| matches!(action, ClientAction::SetTemporaryLatin(false)))
            {
                actions.insert(0, ClientAction::SetTemporaryLatin(false));
            }

            if !Self::ensure_ipc_service_for_key_event("handle_preserved_eisu_shortcut") {
                return Ok(false);
            }

            self.handle_action_with_config_snapshot(&actions, transition, config_snapshot)?;
            Ok(true)
        })();

        match result {
            Ok(handled) => Ok(handled),
            Err(error) => {
                tracing::error!("handle_preserved_eisu_shortcut failed: {error:?}");
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

        let ipc_service = IMEState::ipc_service().ok().flatten();

        if let Some(mut ipc_service) = ipc_service {
            if let Ok(delivery) =
                ipc_service.update_candidate_window(Some(false), None, Some(vec![]), Some(0), None)
            {
                self.remember_candidate_window_visibility_if_sent(delivery, Some(false));
            }
            let _ = ipc_service.clear_text();

            let _ = IMEState::set_ipc_service(ipc_service);
        }
    }

    #[tracing::instrument]
    pub fn handle_action(
        &self,
        actions: &[ClientAction],
        transition: CompositionState,
    ) -> Result<()> {
        let trace_request_id = current_input_trace_request_id();
        let config_snapshot_start = trace_request_id.map(|_| Instant::now());
        let config_snapshot = IMEState::app_config_snapshot()?;
        if let (Some(request_id), Some(config_snapshot_start)) =
            (trace_request_id, config_snapshot_start)
        {
            let app_config = config_snapshot.app_config();
            let romaji_lookup = config_snapshot.romaji_lookup();
            Self::log_client_performance(
                request_id,
                "handle_action",
                "config_snapshot",
                config_snapshot_start.elapsed(),
                format!(
                    "actions={};romaji_rows={};max_input_len={};max_multi_char_input_len={}",
                    actions.len(),
                    app_config.romaji_table.rows.len(),
                    romaji_lookup.max_input_len,
                    romaji_lookup.max_multi_char_input_len
                ),
            );
        }

        self.handle_action_with_config_snapshot(actions, transition, config_snapshot)
    }

    fn handle_action_with_config_snapshot(
        &self,
        actions: &[ClientAction],
        transition: CompositionState,
        config_snapshot: AppConfigSnapshot,
    ) -> Result<()> {
        let trace_request_id = current_input_trace_request_id();
        let total_start = trace_request_id.map(|_| Instant::now());
        let requested_transition = transition.clone();
        let result: Result<()> = (|| {
            #[allow(clippy::let_and_return)]
            let (composition, mode) = {
                let text_service = self.borrow()?;
                let composition = text_service.borrow_composition()?.clone();
                let mode = IMEState::input_mode()?;
                (composition, mode)
            };
            let app_config = config_snapshot.app_config();
            let romaji_lookup = config_snapshot.romaji_lookup();

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
            let mut ipc_service;
            let mut transition = transition;
            let mut deferred_clause_navigation_ready_ui_sync = None;

            macro_rules! reset_after_empty_server_composition {
                ($reason:expr) => {{
                    tracing::warn!(
                        reason = $reason,
                        "Reset stale client composition after server returned empty composition"
                    );

                    transition = CompositionState::None;
                    selection_index = 0;
                    corresponding_count = 0;
                    temporary_latin = false;
                    temporary_latin_shift_pending = false;
                    preview.clear();
                    suffix.clear();
                    raw_input.clear();
                    raw_hiragana.clear();
                    fixed_prefix.clear();
                    candidates = Candidates::default();
                    clause_snapshots.clear();
                    future_clause_snapshots.clear();
                    current_clause_is_split_derived = false;
                    current_clause_is_direct_split_remainder = false;
                    current_clause_has_split_left_neighbor = false;
                    current_clause_split_group_id = None;
                    next_split_group_id = 0;

                    self.discard_composition_text()?;
                    let delivery = ipc_service.update_candidate_window(
                        Some(false),
                        None,
                        Some(vec![]),
                        Some(0),
                        None,
                    )?;
                    self.remember_candidate_window_visibility_if_sent(delivery, Some(false));
                    ipc_service.clear_text()?;
                }};
            }

            macro_rules! reset_stale_composition_before_fresh_append {
                ($reason:expr) => {{
                    tracing::warn!(
                        reason = $reason,
                        "Reset stale client composition before applying fresh append"
                    );

                    transition = CompositionState::Composing;
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

                    self.discard_composition_text()?;
                    self.start_composition()?;
                }};
            }

            ipc_service = IMEState::ipc_service()?.context("ipc_service is None")?;

            for (action_index, action) in actions.iter().enumerate() {
                if Self::action_needs_context_update(action) {
                    IMEState::set_ipc_service(ipc_service.clone())?;
                    self.update_context(&preview)?;
                    ipc_service = IMEState::ipc_service()?.context("ipc_service is None")?;
                }

                match action {
                    ClientAction::StartComposition => {
                        self.start_composition()?;
                        if app_config.general.show_candidate_window_after_space {
                            let delivery = ipc_service.update_candidate_window(
                                Some(false),
                                None,
                                None,
                                None,
                                None,
                            )?;
                            self.remember_candidate_window_visibility_if_sent(
                                delivery,
                                Some(false),
                            );
                        }
                    }
                    ClientAction::ShowCandidateWindow => {
                        let position = self.candidate_window_position()?;
                        let delivery = ipc_service.update_candidate_window_with_reading(
                            Some(true),
                            position,
                            None,
                            None,
                            None,
                            Self::live_conversion_reading_update(
                                app_config,
                                &candidates,
                                &transition,
                            ),
                            Some(true),
                            Self::live_conversion_reading_vertical_adjustment_for_update(
                                app_config,
                                &candidates,
                                &transition,
                            ),
                        )?;
                        self.remember_candidate_window_visibility_if_sent(delivery, Some(true));
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
                        let delivery = ipc_service.update_candidate_window(
                            Some(false),
                            None,
                            Some(vec![]),
                            Some(0),
                            None,
                        )?;
                        self.remember_candidate_window_visibility_if_sent(delivery, Some(false));
                        ipc_service.clear_text()?;
                    }
                    ClientAction::AppendText(text) => {
                        Self::clear_clause_caches(
                            &mut clause_snapshots,
                            &mut future_clause_snapshots,
                            &mut ipc_service,
                            &candidates,
                        )?;
                        let resolved_symbol_text = match mode {
                            InputMode::Kana => Self::resolve_symbol_input_text_with_lookup(
                                &raw_input,
                                text,
                                app_config,
                                romaji_lookup,
                            ),
                            InputMode::Latin => None,
                        };
                        let text = match mode {
                            InputMode::Kana => {
                                resolved_symbol_text.unwrap_or_else(|| text.to_string())
                            }
                            InputMode::Latin => text.to_string(),
                        };

                        current_clause_is_split_derived = false;
                        current_clause_is_direct_split_remainder = false;
                        current_clause_has_split_left_neighbor = false;
                        current_clause_split_group_id = None;
                        let appended_candidates =
                            ipc_service.append_text_with_context(text.clone(), &candidates)?;
                        let session_changed = ipc_service.take_server_reset_recovered()
                            && Self::has_client_composition_state(
                                &raw_input,
                                &preview,
                                &suffix,
                                &fixed_prefix,
                                &candidates,
                            );
                        let server_reset_recovered = session_changed
                            || Self::append_result_indicates_server_reset(
                                &raw_input,
                                &candidates,
                                &text,
                                &appended_candidates,
                            );
                        candidates = appended_candidates;
                        if server_reset_recovered {
                            reset_stale_composition_before_fresh_append!(
                                "append_text recovered after server reset"
                            );
                            selection_index = 0;
                        }
                        raw_input.push_str(&text);
                        if let Some(selected) = Self::select_candidate(&candidates, selection_index)
                        {
                            selection_index = selected.index;
                            corresponding_count = selected.corresponding_count;
                            preview =
                                Self::merge_preview_with_prefix(&fixed_prefix, &selected.text);
                            suffix = selected.sub_text.clone();
                            raw_hiragana = selected.hiragana;

                            self.set_text(&preview, &suffix)?;
                            self.sync_candidate_window_after_text_update(
                                &mut ipc_service,
                                &candidates,
                                selection_index,
                                app_config,
                                &transition,
                            )?;
                        } else if candidates.is_empty_composition() {
                            reset_after_empty_server_composition!(
                                "append_text returned empty composition"
                            );
                        }
                    }
                    ClientAction::AppendTextRaw(text) => {
                        Self::clear_clause_caches(
                            &mut clause_snapshots,
                            &mut future_clause_snapshots,
                            &mut ipc_service,
                            &candidates,
                        )?;
                        current_clause_is_split_derived = false;
                        current_clause_is_direct_split_remainder = false;
                        current_clause_has_split_left_neighbor = false;
                        current_clause_split_group_id = None;
                        let appended_candidates =
                            ipc_service.append_text_with_context(text.clone(), &candidates)?;
                        let session_changed = ipc_service.take_server_reset_recovered()
                            && Self::has_client_composition_state(
                                &raw_input,
                                &preview,
                                &suffix,
                                &fixed_prefix,
                                &candidates,
                            );
                        let server_reset_recovered = session_changed
                            || Self::append_result_indicates_server_reset(
                                &raw_input,
                                &candidates,
                                text,
                                &appended_candidates,
                            );
                        candidates = appended_candidates;
                        if server_reset_recovered {
                            reset_stale_composition_before_fresh_append!(
                                "append_text_raw recovered after server reset"
                            );
                            selection_index = 0;
                        }
                        raw_input.push_str(text);
                        if let Some(selected) = Self::select_candidate(&candidates, selection_index)
                        {
                            selection_index = selected.index;
                            corresponding_count = selected.corresponding_count;
                            preview =
                                Self::merge_preview_with_prefix(&fixed_prefix, &selected.text);
                            suffix = selected.sub_text.clone();
                            raw_hiragana = selected.hiragana;

                            self.set_text(&preview, &suffix)?;
                            self.sync_candidate_window_after_text_update(
                                &mut ipc_service,
                                &candidates,
                                selection_index,
                                app_config,
                                &transition,
                            )?;
                        } else if candidates.is_empty_composition() {
                            reset_after_empty_server_composition!(
                                "append_text_raw returned empty composition"
                            );
                        }
                    }
                    ClientAction::AppendTextDirect(text) => {
                        Self::clear_clause_caches(
                            &mut clause_snapshots,
                            &mut future_clause_snapshots,
                            &mut ipc_service,
                            &candidates,
                        )?;
                        current_clause_is_split_derived = false;
                        current_clause_is_direct_split_remainder = false;
                        current_clause_has_split_left_neighbor = false;
                        current_clause_split_group_id = None;
                        let appended_candidates = ipc_service
                            .append_text_direct_with_context(text.clone(), &candidates)?;
                        let session_changed = ipc_service.take_server_reset_recovered()
                            && Self::has_client_composition_state(
                                &raw_input,
                                &preview,
                                &suffix,
                                &fixed_prefix,
                                &candidates,
                            );
                        let server_reset_recovered = session_changed
                            || Self::append_result_indicates_server_reset(
                                &raw_input,
                                &candidates,
                                text,
                                &appended_candidates,
                            );
                        candidates = appended_candidates;
                        if server_reset_recovered {
                            reset_stale_composition_before_fresh_append!(
                                "append_text_direct recovered after server reset"
                            );
                            selection_index = 0;
                        }
                        raw_input.push_str(text);
                        if let Some(selected) = Self::select_candidate(&candidates, selection_index)
                        {
                            selection_index = selected.index;
                            corresponding_count = selected.corresponding_count;
                            preview =
                                Self::merge_preview_with_prefix(&fixed_prefix, &selected.text);
                            suffix = selected.sub_text.clone();
                            raw_hiragana = selected.hiragana;

                            self.set_text(&preview, &suffix)?;
                            self.sync_candidate_window_after_text_update(
                                &mut ipc_service,
                                &candidates,
                                selection_index,
                                app_config,
                                &transition,
                            )?;
                        } else if candidates.is_empty_composition() {
                            reset_after_empty_server_composition!(
                                "append_text_direct returned empty composition"
                            );
                        }
                    }
                    ClientAction::CommitTextDirect(text) => {
                        self.start_composition()?;
                        self.set_text(text, "")?;
                        self.end_composition()?;
                    }
                    ClientAction::RemoveText => {
                        Self::clear_clause_caches(
                            &mut clause_snapshots,
                            &mut future_clause_snapshots,
                            &mut ipc_service,
                            &candidates,
                        )?;
                        current_clause_is_split_derived = false;
                        current_clause_is_direct_split_remainder = false;
                        current_clause_has_split_left_neighbor = false;
                        current_clause_split_group_id = None;
                        if ipc_service.take_server_reset_recovered()
                            && Self::has_client_composition_state(
                                &raw_input,
                                &preview,
                                &suffix,
                                &fixed_prefix,
                                &candidates,
                            )
                        {
                            reset_after_empty_server_composition!(
                                "server session changed before remove_text"
                            );
                            continue;
                        }
                        raw_input.pop();
                        candidates = ipc_service.remove_text_with_context(&candidates)?;
                        if ipc_service.take_server_reset_recovered()
                            && Self::has_client_composition_state(
                                &raw_input,
                                &preview,
                                &suffix,
                                &fixed_prefix,
                                &candidates,
                            )
                        {
                            reset_after_empty_server_composition!(
                                "remove_text detected server session change"
                            );
                            continue;
                        }
                        if let Some(selected) = Self::select_candidate(&candidates, selection_index)
                        {
                            selection_index = selected.index;
                            corresponding_count = selected.corresponding_count;

                            preview =
                                Self::merge_preview_with_prefix(&fixed_prefix, &selected.text);
                            suffix = selected.sub_text.clone();
                            raw_hiragana = selected.hiragana;

                            self.set_text(&preview, &suffix)?;
                            self.sync_candidate_window_update(
                                &mut ipc_service,
                                &candidates,
                                selection_index,
                                None,
                                false,
                                Self::live_conversion_reading_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
                                None,
                                Self::live_conversion_reading_vertical_adjustment_for_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
                            )?;
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
                            let delivery = ipc_service.update_candidate_window(
                                Some(false),
                                None,
                                Some(vec![]),
                                Some(0),
                                None,
                            )?;
                            self.remember_candidate_window_visibility_if_sent(
                                delivery,
                                Some(false),
                            );
                            ipc_service.clear_text()?;

                            preview.clear();
                            fixed_prefix.clear();
                        }
                    }
                    ClientAction::MoveCursor(offset) => {
                        if ipc_service.take_server_reset_recovered()
                            && Self::has_client_composition_state(
                                &raw_input,
                                &preview,
                                &suffix,
                                &fixed_prefix,
                                &candidates,
                            )
                        {
                            reset_after_empty_server_composition!(
                                "server session changed before move_cursor"
                            );
                            continue;
                        }
                        candidates = ipc_service.move_cursor_with_context(*offset, &candidates)?;
                        if ipc_service.take_server_reset_recovered()
                            && Self::has_client_composition_state(
                                &raw_input,
                                &preview,
                                &suffix,
                                &fixed_prefix,
                                &candidates,
                            )
                        {
                            reset_after_empty_server_composition!(
                                "move_cursor detected server session change"
                            );
                            continue;
                        }
                        if let Some(selected) = Self::select_candidate(&candidates, selection_index)
                        {
                            selection_index = selected.index;
                            corresponding_count = selected.corresponding_count;
                            preview =
                                Self::merge_preview_with_prefix(&fixed_prefix, &selected.text);
                            suffix = selected.sub_text.clone();
                            raw_hiragana = selected.hiragana;

                            self.set_text(&preview, &suffix)?;
                            self.sync_candidate_window_update(
                                &mut ipc_service,
                                &candidates,
                                selection_index,
                                None,
                                true,
                                Self::live_conversion_reading_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
                                None,
                                Self::live_conversion_reading_vertical_adjustment_for_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
                            )?;
                        } else if candidates.is_empty_composition() {
                            reset_after_empty_server_composition!(
                                "move_cursor returned empty composition"
                            );
                        }
                    }
                    ClientAction::EnsureClauseNavigationReady => {
                        if ipc_service.take_server_reset_recovered()
                            && Self::has_client_composition_state(
                                &raw_input,
                                &preview,
                                &suffix,
                                &fixed_prefix,
                                &candidates,
                            )
                        {
                            reset_after_empty_server_composition!(
                                "server session changed before clause_navigation_ready"
                            );
                            continue;
                        }
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
                            let mut state = ClauseState::from_composition_parts(
                                &mut preview,
                                &mut suffix,
                                &mut raw_input,
                                &mut raw_hiragana,
                                &mut fixed_prefix,
                                &mut corresponding_count,
                                &mut selection_index,
                                &mut candidates,
                                &mut clause_snapshots,
                                &mut future_clause_snapshots,
                                &mut current_clause_is_split_derived,
                                &mut current_clause_is_direct_split_remainder,
                                &mut current_clause_has_split_left_neighbor,
                                &mut current_clause_split_group_id,
                                &mut next_split_group_id,
                            );
                            let transition = ClauseState::transition_with_backend(
                                &mut state,
                                ClauseCommand::StartClauseNavigation,
                                ClauseTransitionInput::default(),
                                &mut ipc_service,
                            )?;
                            let effect = transition.effect;
                            ClauseState::write_back(state);
                            effect
                        };

                        if effect.server_reset {
                            reset_after_empty_server_composition!(
                                "clause_navigation_ready returned empty composition"
                            );
                        } else if effect.applied {
                            let defer_ui_sync = Self::should_defer_clause_navigation_ready_sync(
                                actions,
                                action_index,
                            );
                            let ready_ui_sync = Self::clause_navigation_ready_ui_sync(effect);
                            if defer_ui_sync {
                                deferred_clause_navigation_ready_ui_sync = ready_ui_sync;
                                Self::log_clause_action_state(
                                    "defer",
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
                                continue;
                            }

                            self.sync_clause_action_ui(
                                &preview,
                                &suffix,
                                &candidates,
                                selection_index,
                                &mut ipc_service,
                                ready_ui_sync.and_then(|sync| sync.visible),
                                effect.update_pos,
                                Self::live_conversion_reading_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
                                ready_ui_sync.and_then(|sync| sync.visible),
                                Self::live_conversion_reading_vertical_adjustment_for_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
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
                        if ipc_service.take_server_reset_recovered()
                            && Self::has_client_composition_state(
                                &raw_input,
                                &preview,
                                &suffix,
                                &fixed_prefix,
                                &candidates,
                            )
                        {
                            reset_after_empty_server_composition!(
                                "server session changed before move_clause"
                            );
                            continue;
                        }
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
                            let mut state = ClauseState::from_composition_parts(
                                &mut preview,
                                &mut suffix,
                                &mut raw_input,
                                &mut raw_hiragana,
                                &mut fixed_prefix,
                                &mut corresponding_count,
                                &mut selection_index,
                                &mut candidates,
                                &mut clause_snapshots,
                                &mut future_clause_snapshots,
                                &mut current_clause_is_split_derived,
                                &mut current_clause_is_direct_split_remainder,
                                &mut current_clause_has_split_left_neighbor,
                                &mut current_clause_split_group_id,
                                &mut next_split_group_id,
                            );
                            let transition = ClauseState::transition_with_backend(
                                &mut state,
                                ClauseCommand::MoveBy(*direction),
                                ClauseTransitionInput::default(),
                                &mut ipc_service,
                            )?;
                            let effect = transition.effect;
                            ClauseState::write_back(state);
                            effect
                        };

                        let deferred_ready_ui_sync =
                            Self::deferred_clause_navigation_ready_ui_sync_after_move(
                                deferred_clause_navigation_ready_ui_sync.take(),
                                effect,
                            );
                        if effect.server_reset {
                            reset_after_empty_server_composition!(
                                "move_clause returned empty composition"
                            );
                        } else if effect.applied {
                            self.sync_clause_action_ui(
                                &preview,
                                &suffix,
                                &candidates,
                                selection_index,
                                &mut ipc_service,
                                deferred_ready_ui_sync.and_then(|sync| sync.visible),
                                effect.update_pos,
                                Self::live_conversion_reading_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
                                deferred_ready_ui_sync.and_then(|sync| sync.visible),
                                Self::live_conversion_reading_vertical_adjustment_for_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
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
                        } else if let Some(sync) = deferred_ready_ui_sync {
                            self.sync_clause_action_ui(
                                &preview,
                                &suffix,
                                &candidates,
                                selection_index,
                                &mut ipc_service,
                                sync.visible,
                                sync.update_pos,
                                Self::live_conversion_reading_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
                                sync.visible,
                                Self::live_conversion_reading_vertical_adjustment_for_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
                            )?;
                            Self::log_clause_action_state(
                                "after-deferred",
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
                        if ipc_service.take_server_reset_recovered()
                            && Self::has_client_composition_state(
                                &raw_input,
                                &preview,
                                &suffix,
                                &fixed_prefix,
                                &candidates,
                            )
                        {
                            reset_after_empty_server_composition!(
                                "server session changed before adjust_boundary"
                            );
                            continue;
                        }
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
                            let mut state = ClauseState::from_composition_parts(
                                &mut preview,
                                &mut suffix,
                                &mut raw_input,
                                &mut raw_hiragana,
                                &mut fixed_prefix,
                                &mut corresponding_count,
                                &mut selection_index,
                                &mut candidates,
                                &mut clause_snapshots,
                                &mut future_clause_snapshots,
                                &mut current_clause_is_split_derived,
                                &mut current_clause_is_direct_split_remainder,
                                &mut current_clause_has_split_left_neighbor,
                                &mut current_clause_split_group_id,
                                &mut next_split_group_id,
                            );
                            let transition = ClauseState::transition_with_backend(
                                &mut state,
                                ClauseCommand::AdjustBoundary(*direction),
                                ClauseTransitionInput::default(),
                                &mut ipc_service,
                            )?;
                            let effect = transition.effect;
                            ClauseState::write_back(state);
                            effect
                        };

                        if effect.server_reset
                            || (ipc_service.take_server_reset_recovered()
                                && Self::has_client_composition_state(
                                    &raw_input,
                                    &preview,
                                    &suffix,
                                    &fixed_prefix,
                                    &candidates,
                                ))
                        {
                            reset_after_empty_server_composition!(
                                "adjust_boundary detected server reset"
                            );
                        } else if effect.applied {
                            self.sync_clause_action_ui(
                                &preview,
                                &suffix,
                                &candidates,
                                selection_index,
                                &mut ipc_service,
                                None,
                                effect.update_pos,
                                Self::live_conversion_reading_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
                                None,
                                Self::live_conversion_reading_vertical_adjustment_for_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
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
                        let position = self.candidate_window_position()?;
                        self.end_composition()?;

                        IMEState::set_input_mode(mode.clone())?;

                        // update the language bar
                        self.update_lang_bar()?;

                        let mode = match mode {
                            InputMode::Latin => "A",
                            InputMode::Kana => "あ",
                        };

                        ipc_service.update_candidate_window_with_reading(
                            None,
                            position,
                            None,
                            None,
                            Some(mode),
                            Some(""),
                            Some(false),
                            None,
                        )?;

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
                            let mut state = ClauseState::from_composition_parts(
                                &mut preview,
                                &mut suffix,
                                &mut raw_input,
                                &mut raw_hiragana,
                                &mut fixed_prefix,
                                &mut corresponding_count,
                                &mut selection_index,
                                &mut candidates,
                                &mut clause_snapshots,
                                &mut future_clause_snapshots,
                                &mut current_clause_is_split_derived,
                                &mut current_clause_is_direct_split_remainder,
                                &mut current_clause_has_split_left_neighbor,
                                &mut current_clause_split_group_id,
                                &mut next_split_group_id,
                            );
                            let transition = ClauseState::transition_without_backend(
                                &mut state,
                                ClauseCommand::SetSelection(selection),
                            );
                            let effect = transition.effect;
                            ClauseState::write_back(state);
                            effect
                        };

                        if effect.applied {
                            self.sync_clause_action_ui(
                                &preview,
                                &suffix,
                                &candidates,
                                selection_index,
                                &mut ipc_service,
                                None,
                                effect.update_pos,
                                Self::live_conversion_reading_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
                                None,
                                Self::live_conversion_reading_vertical_adjustment_for_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
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
                            &candidates,
                        )?;
                        current_clause_is_split_derived = false;
                        current_clause_is_direct_split_remainder = false;
                        current_clause_has_split_left_neighbor = false;
                        current_clause_split_group_id = None;
                        let shrunk_raw_input =
                            Self::current_raw_input_suffix(&raw_input, corresponding_count);
                        let resolved_symbol_text = match mode {
                            InputMode::Kana => Self::resolve_symbol_input_text_with_lookup(
                                &shrunk_raw_input,
                                text,
                                app_config,
                                romaji_lookup,
                            ),
                            InputMode::Latin => None,
                        };
                        let mut updated_raw_input = shrunk_raw_input.clone();
                        updated_raw_input.push_str(text);

                        let text = match mode {
                            InputMode::Kana => {
                                resolved_symbol_text.unwrap_or_else(|| text.to_string())
                            }
                            InputMode::Latin => text.to_string(),
                        };
                        let session_changed_before_shrink = ipc_service
                            .take_server_reset_recovered()
                            && Self::has_client_composition_state(
                                &raw_input,
                                &preview,
                                &suffix,
                                &fixed_prefix,
                                &candidates,
                            );
                        let shrunk_candidates = if session_changed_before_shrink {
                            Candidates::default()
                        } else {
                            ipc_service
                                .shrink_text_with_context(corresponding_count, &candidates)?
                        };
                        let mut fresh_append_after_server_reset = session_changed_before_shrink
                            || shrunk_candidates.is_empty_composition();
                        if fresh_append_after_server_reset {
                            let reset_reason = if session_changed_before_shrink {
                                "server session changed before shrink_text"
                            } else {
                                "shrink_text returned empty composition before append"
                            };
                            reset_stale_composition_before_fresh_append!(reset_reason);
                            updated_raw_input.clear();
                            updated_raw_input.push_str(&text);
                        }
                        candidates = ipc_service
                            .append_text_with_context(text.clone(), &shrunk_candidates)?;
                        if ipc_service.take_server_reset_recovered() {
                            if !fresh_append_after_server_reset {
                                reset_stale_composition_before_fresh_append!(
                                    "shrink_text append recovered after server reset"
                                );
                            }
                            updated_raw_input.clear();
                            updated_raw_input.push_str(&text);
                            fresh_append_after_server_reset = true;
                        }
                        raw_input = updated_raw_input;
                        selection_index = 0;

                        let recovered = if let Some(selected) =
                            Self::select_candidate(&candidates, selection_index)
                        {
                            let previous_preview = preview.clone();
                            selection_index = selected.index;
                            corresponding_count = selected.corresponding_count;
                            preview = selected.text.clone();
                            suffix = selected.sub_text.clone();
                            raw_hiragana = selected.hiragana;

                            if fresh_append_after_server_reset {
                                self.set_text(&preview, &suffix)?;
                            } else {
                                self.shift_start(&previous_preview, &selected.text)?;
                            }
                            self.sync_candidate_window_update(
                                &mut ipc_service,
                                &candidates,
                                selection_index,
                                None,
                                true,
                                Self::live_conversion_reading_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
                                None,
                                Self::live_conversion_reading_vertical_adjustment_for_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
                            )?;
                            true
                        } else if candidates.is_empty_composition() {
                            reset_after_empty_server_composition!(
                                "shrink_text returned empty composition"
                            );
                            false
                        } else {
                            true
                        };

                        if recovered {
                            transition = CompositionState::Composing;
                        }
                    }
                    ClientAction::ShrinkTextRaw(text) => {
                        fixed_prefix.clear();
                        Self::clear_clause_caches(
                            &mut clause_snapshots,
                            &mut future_clause_snapshots,
                            &mut ipc_service,
                            &candidates,
                        )?;
                        current_clause_is_split_derived = false;
                        current_clause_is_direct_split_remainder = false;
                        current_clause_has_split_left_neighbor = false;
                        current_clause_split_group_id = None;
                        let mut updated_raw_input =
                            Self::current_raw_input_suffix(&raw_input, corresponding_count);
                        updated_raw_input.push_str(text);

                        let session_changed_before_shrink = ipc_service
                            .take_server_reset_recovered()
                            && Self::has_client_composition_state(
                                &raw_input,
                                &preview,
                                &suffix,
                                &fixed_prefix,
                                &candidates,
                            );
                        let shrunk_candidates = if session_changed_before_shrink {
                            Candidates::default()
                        } else {
                            ipc_service
                                .shrink_text_with_context(corresponding_count, &candidates)?
                        };
                        let mut fresh_append_after_server_reset = session_changed_before_shrink
                            || shrunk_candidates.is_empty_composition();
                        if fresh_append_after_server_reset {
                            let reset_reason = if session_changed_before_shrink {
                                "server session changed before shrink_text_raw"
                            } else {
                                "shrink_text_raw returned empty composition before append"
                            };
                            reset_stale_composition_before_fresh_append!(reset_reason);
                            updated_raw_input.clear();
                            updated_raw_input.push_str(text);
                        }
                        candidates = ipc_service
                            .append_text_with_context(text.clone(), &shrunk_candidates)?;
                        if ipc_service.take_server_reset_recovered() {
                            if !fresh_append_after_server_reset {
                                reset_stale_composition_before_fresh_append!(
                                    "shrink_text_raw append recovered after server reset"
                                );
                            }
                            updated_raw_input.clear();
                            updated_raw_input.push_str(text);
                            fresh_append_after_server_reset = true;
                        }
                        raw_input = updated_raw_input;
                        selection_index = 0;

                        let recovered = if let Some(selected) =
                            Self::select_candidate(&candidates, selection_index)
                        {
                            let previous_preview = preview.clone();
                            selection_index = selected.index;
                            corresponding_count = selected.corresponding_count;
                            preview = selected.text.clone();
                            suffix = selected.sub_text.clone();
                            raw_hiragana = selected.hiragana;

                            if fresh_append_after_server_reset {
                                self.set_text(&preview, &suffix)?;
                            } else {
                                self.shift_start(&previous_preview, &selected.text)?;
                            }
                            self.sync_candidate_window_update(
                                &mut ipc_service,
                                &candidates,
                                selection_index,
                                None,
                                true,
                                Self::live_conversion_reading_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
                                None,
                                Self::live_conversion_reading_vertical_adjustment_for_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
                            )?;
                            true
                        } else if candidates.is_empty_composition() {
                            reset_after_empty_server_composition!(
                                "shrink_text_raw returned empty composition"
                            );
                            false
                        } else {
                            true
                        };

                        if recovered {
                            transition = CompositionState::Composing;
                        }
                    }
                    ClientAction::ShrinkTextDirect(text) => {
                        fixed_prefix.clear();
                        Self::clear_clause_caches(
                            &mut clause_snapshots,
                            &mut future_clause_snapshots,
                            &mut ipc_service,
                            &candidates,
                        )?;
                        current_clause_is_split_derived = false;
                        current_clause_is_direct_split_remainder = false;
                        current_clause_has_split_left_neighbor = false;
                        current_clause_split_group_id = None;
                        let mut updated_raw_input =
                            Self::current_raw_input_suffix(&raw_input, corresponding_count);
                        updated_raw_input.push_str(text);

                        let session_changed_before_shrink = ipc_service
                            .take_server_reset_recovered()
                            && Self::has_client_composition_state(
                                &raw_input,
                                &preview,
                                &suffix,
                                &fixed_prefix,
                                &candidates,
                            );
                        let shrunk_candidates = if session_changed_before_shrink {
                            Candidates::default()
                        } else {
                            ipc_service
                                .shrink_text_with_context(corresponding_count, &candidates)?
                        };
                        let mut fresh_append_after_server_reset = session_changed_before_shrink
                            || shrunk_candidates.is_empty_composition();
                        if fresh_append_after_server_reset {
                            let reset_reason = if session_changed_before_shrink {
                                "server session changed before shrink_text_direct"
                            } else {
                                "shrink_text_direct returned empty composition before append"
                            };
                            reset_stale_composition_before_fresh_append!(reset_reason);
                            updated_raw_input.clear();
                            updated_raw_input.push_str(text);
                        }
                        candidates = ipc_service
                            .append_text_direct_with_context(text.clone(), &shrunk_candidates)?;
                        if ipc_service.take_server_reset_recovered() {
                            if !fresh_append_after_server_reset {
                                reset_stale_composition_before_fresh_append!(
                                    "shrink_text_direct append recovered after server reset"
                                );
                            }
                            updated_raw_input.clear();
                            updated_raw_input.push_str(text);
                            fresh_append_after_server_reset = true;
                        }
                        raw_input = updated_raw_input;
                        selection_index = 0;

                        let recovered = if let Some(selected) =
                            Self::select_candidate(&candidates, selection_index)
                        {
                            let previous_preview = preview.clone();
                            selection_index = selected.index;
                            corresponding_count = selected.corresponding_count;
                            preview = selected.text.clone();
                            suffix = selected.sub_text.clone();
                            raw_hiragana = selected.hiragana;

                            if fresh_append_after_server_reset {
                                self.set_text(&preview, &suffix)?;
                            } else {
                                self.shift_start(&previous_preview, &selected.text)?;
                            }
                            self.sync_candidate_window_update(
                                &mut ipc_service,
                                &candidates,
                                selection_index,
                                None,
                                true,
                                Self::live_conversion_reading_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
                                None,
                                Self::live_conversion_reading_vertical_adjustment_for_update(
                                    app_config,
                                    &candidates,
                                    &transition,
                                ),
                            )?;
                            true
                        } else if candidates.is_empty_composition() {
                            reset_after_empty_server_composition!(
                                "shrink_text_direct returned empty composition"
                            );
                            false
                        } else {
                            true
                        };

                        if recovered {
                            transition = CompositionState::Composing;
                        }
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
                        Self::sync_clause_snapshot_suffixes(
                            &mut clause_snapshots,
                            &preview,
                            &suffix,
                        );
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
            composition.current_clause_has_split_left_neighbor =
                current_clause_has_split_left_neighbor;
            composition.current_clause_split_group_id = current_clause_split_group_id;
            composition.next_split_group_id = next_split_group_id;
            composition.suffix = suffix.clone();
            composition.corresponding_count = corresponding_count;
            composition.temporary_latin = temporary_latin;
            composition.temporary_latin_shift_pending = temporary_latin_shift_pending;

            drop(composition);
            drop(text_service);

            if let Err(error) = IMEState::set_ipc_service(ipc_service) {
                tracing::warn!(
                    ?error,
                    "Failed to persist updated IPC service into IMEState"
                );
            }

            Ok(())
        })();

        if let (Some(request_id), Some(total_start)) = (trace_request_id, total_start) {
            let details = match &result {
                Ok(()) => format!(
                    "status=success;actions={};requested_transition={requested_transition:?}",
                    actions.len()
                ),
                Err(error) => format!(
                    "status=error;actions={};requested_transition={requested_transition:?};error={error:?}",
                    actions.len()
                ),
            };
            Self::log_client_performance(
                request_id,
                "handle_action",
                "total",
                total_start.elapsed(),
                details,
            );
        }

        result
    }
}

#[cfg(test)]
mod tests;
