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
use shared::{AppConfig, NumpadInputMode, SpaceInputMode};
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

    pub state: CompositionState,
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
            corresponding_count: candidates.corresponding_count.get(index).copied().unwrap_or(0),
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
    fn commit_current_clause_actions(composition: &Composition) -> (CompositionState, Vec<ClientAction>) {
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
    fn commit_first_clause_actions(composition: &Composition) -> (CompositionState, Vec<ClientAction>) {
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
        let is_alt_backquote = Self::is_alt_backquote(wparam, lparam);
        let app_config = AppConfig::read();

        // check shortcut keys
        if is_ctrl_pressed
            && !is_ctrl_space
            && !is_alt_backquote
            && !is_ctrl_enter
            && !is_ctrl_down
        {
            return Ok(None);
        }

        if is_ctrl_space || is_alt_backquote {
            let shortcuts = &app_config.shortcuts;

            if is_ctrl_space && !shortcuts.ctrl_space_toggle {
                return Ok(None);
            }

            if is_alt_backquote && !shortcuts.alt_backquote_toggle {
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

        let (transition, actions) = match composition.state {
            CompositionState::None => match action {
                UserAction::NumpadSymbol(symbol) if mode == InputMode::Kana => {
                    let Some(text) = Self::numpad_text_for_mode(
                        symbol,
                        app_config.general.numpad_input,
                        true,
                    ) else {
                        return Ok(None);
                    };

                    (
                        CompositionState::Composing,
                        vec![ClientAction::StartComposition, ClientAction::AppendTextRaw(text)],
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
                    let Some(text) = Self::numpad_text_for_mode(
                        digit,
                        app_config.general.numpad_input,
                        true,
                    ) else {
                        return Ok(None);
                    };

                    (
                        CompositionState::Composing,
                        vec![ClientAction::StartComposition, ClientAction::AppendTextRaw(text)],
                    )
                }
                UserAction::Number {
                    value,
                    is_numpad: false,
                } if mode == InputMode::Kana => {
                    (
                        CompositionState::Composing,
                        vec![
                            ClientAction::StartComposition,
                            ClientAction::AppendText(value.to_string()),
                        ],
                    )
                }
                UserAction::Space if mode == InputMode::Kana => {
                    let mut use_halfwidth = matches!(app_config.general.space_input, SpaceInputMode::AlwaysHalf);
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
                    return Ok(None);
                }
            },
            CompositionState::Composing => match action {
                UserAction::NumpadSymbol(symbol) if mode == InputMode::Kana => {
                    let text = Self::numpad_text_for_mode(
                        symbol,
                        app_config.general.numpad_input,
                        false,
                    )
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
                    let text = Self::numpad_text_for_mode(
                        digit,
                        app_config.general.numpad_input,
                        false,
                    )
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
                UserAction::Navigation(direction) => match direction {
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
                UserAction::Function(key) => match key {
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
                    return Ok(None);
                }
            },
            CompositionState::Previewing => match action {
                UserAction::NumpadSymbol(symbol) if mode == InputMode::Kana => {
                    let text = Self::numpad_text_for_mode(
                        symbol,
                        app_config.general.numpad_input,
                        false,
                    )
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
                    let text = Self::numpad_text_for_mode(
                        digit,
                        app_config.general.numpad_input,
                        false,
                    )
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
                UserAction::Navigation(direction) => match direction {
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
                UserAction::Function(key) => match key {
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
                    return Ok(None);
                }
            },
            _ => {
                return Ok(None);
            }
        };

        Ok(Some((actions, transition)))
    }

    #[tracing::instrument]
    pub fn handle_key(
        &self,
        context: Option<&ITfContext>,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<bool> {
        if let Some(context) = context {
            self.borrow_mut()?.context = Some(context.clone());
        } else {
            return Ok(false);
        };

        if let Some((actions, transition)) = self.process_key(context, wparam, lparam)? {
            self.handle_action(&actions, transition)?;
        } else {
            return Ok(false);
        }

        Ok(true)
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
        let mut selection_index = composition.selection_index;
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
                    preview.clear();
                    suffix.clear();
                    raw_input.clear();
                    raw_hiragana.clear();
                    fixed_prefix.clear();
                    clause_snapshots.clear();
                    ipc_service.hide_window()?;
                    ipc_service.set_candidates(vec![])?;
                    ipc_service.clear_text()?;
                }
                ClientAction::AppendText(text) => {
                    Self::clear_clause_snapshots(&mut clause_snapshots, &mut ipc_service)?;
                    raw_input.push_str(&text);

                    let text = match mode {
                        InputMode::Kana => convert_kana_symbol(
                            text,
                            &app_config.general,
                            &app_config.character_width,
                            &app_config.romaji_table.rows,
                        ),
                        InputMode::Latin => text.to_string(),
                    };

                    candidates = ipc_service.append_text(text.clone())?;
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
                    Self::clear_clause_snapshots(&mut clause_snapshots, &mut ipc_service)?;
                    raw_input.push_str(&text);

                    candidates = ipc_service.append_text(text.clone())?;
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
                    Self::clear_clause_snapshots(&mut clause_snapshots, &mut ipc_service)?;
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
                        suffix.clear();
                        raw_input.clear();
                        raw_hiragana.clear();
                        clause_snapshots.clear();

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

                        let snapshot = ClauseSnapshot {
                            preview: preview.clone(),
                            suffix: suffix.clone(),
                            raw_input: raw_input.clone(),
                            raw_hiragana: raw_hiragana.clone(),
                            fixed_prefix: fixed_prefix.clone(),
                            corresponding_count,
                            selection_index,
                            candidates: candidates.clone(),
                        };

                        let current_clause_preview = preview
                            .strip_prefix(&fixed_prefix)
                            .unwrap_or(&preview)
                            .to_string();
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
                        suffix = selected.sub_text.clone();
                        raw_hiragana = selected.hiragana;

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
                    preview.clear();
                    suffix.clear();
                    raw_input.clear();
                    raw_hiragana.clear();
                    fixed_prefix.clear();
                    clause_snapshots.clear();
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
                        suffix = selected.sub_text.clone();
                        raw_hiragana = selected.hiragana;

                        ipc_service.set_selection(selection_index)?;
                        self.set_text(&preview, &suffix)?;
                    }
                }
                ClientAction::ShrinkText(text) => {
                    fixed_prefix.clear();
                    Self::clear_clause_snapshots(&mut clause_snapshots, &mut ipc_service)?;
                    // shrink text
                    raw_input.push_str(&text);
                    raw_input = raw_input
                        .chars()
                        .skip(corresponding_count.max(0) as usize)
                        .collect();

                    ipc_service.shrink_text(corresponding_count.clone())?;
                    let text = match mode {
                        InputMode::Kana => convert_kana_symbol(
                            text,
                            &app_config.general,
                            &app_config.character_width,
                            &app_config.romaji_table.rows,
                        ),
                        InputMode::Latin => text.to_string(),
                    };
                    candidates = ipc_service.append_text(text)?;
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
                    Self::clear_clause_snapshots(&mut clause_snapshots, &mut ipc_service)?;
                    raw_input.push_str(&text);
                    raw_input = raw_input
                        .chars()
                        .skip(corresponding_count.max(0) as usize)
                        .collect();

                    ipc_service.shrink_text(corresponding_count.clone())?;
                    candidates = ipc_service.append_text(text.clone())?;
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
        composition.suffix = suffix.clone();
        composition.corresponding_count = corresponding_count;

        Ok(())
    }
}
