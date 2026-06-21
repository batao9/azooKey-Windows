use crate::extension::VKeyExt;
use anyhow::{Context, Result};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyboardState, ToUnicode, VK_CONTROL, VK_LCONTROL, VK_RCONTROL, VK_SHIFT,
};

#[derive(Debug)]
pub enum UserAction {
    Input(char),
    Backspace,
    Delete,
    Enter,
    CommitFirstClause,
    CommitAndNextClause,
    Space,
    Tab,
    Escape,
    Unknown,
    Navigation(Navigation),
    AdjustClauseBoundary(i32),
    Function(Function),
    NumpadSymbol(char),
    Number { value: i8, is_numpad: bool },
    ToggleInputMode,
    InputModeOn,
    InputModeOff,
}

#[derive(Debug)]
pub enum Navigation {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Function {
    Six,
    Seven,
    Eight,
    Nine,
    Ten,
}

impl Function {
    pub(crate) fn from_ctrl_shortcut_key_code(key_code: usize) -> Option<Self> {
        match key_code {
            0x55 => Some(Self::Six),   // Ctrl+U
            0x49 => Some(Self::Seven), // Ctrl+I
            0x4F => Some(Self::Eight), // Ctrl+O
            0x50 => Some(Self::Nine),  // Ctrl+P
            0x54 => Some(Self::Ten),   // Ctrl+T
            _ => None,
        }
    }
}

#[inline]
fn is_ctrl_pressed() -> bool {
    VK_CONTROL.is_pressed() || VK_LCONTROL.is_pressed() || VK_RCONTROL.is_pressed()
}

#[cfg(test)]
mod tests {
    use super::{Function, UserAction};

    #[test]
    fn ctrl_shortcut_keys_map_to_conversion_functions() {
        assert_eq!(
            Function::from_ctrl_shortcut_key_code(0x55),
            Some(Function::Six)
        );
        assert_eq!(
            Function::from_ctrl_shortcut_key_code(0x49),
            Some(Function::Seven)
        );
        assert_eq!(
            Function::from_ctrl_shortcut_key_code(0x4F),
            Some(Function::Eight)
        );
        assert_eq!(
            Function::from_ctrl_shortcut_key_code(0x50),
            Some(Function::Nine)
        );
        assert_eq!(
            Function::from_ctrl_shortcut_key_code(0x54),
            Some(Function::Ten)
        );
        assert_eq!(Function::from_ctrl_shortcut_key_code(0x41), None);
    }

    #[test]
    fn delete_key_maps_to_delete_action() {
        let action = UserAction::try_from(0x2E).expect("VK_DELETE should map");

        assert!(matches!(action, UserAction::Delete));
    }
}

fn clear_dead_key_state(key_state: &[u8; 256]) {
    let mut unicode = [0u16; 8];
    // Use VK_SPACE to clear dead-key state left by ToUnicode.
    unsafe { ToUnicode(0x20, 0, Some(key_state), &mut unicode, 0) };
}

impl TryFrom<usize> for UserAction {
    type Error = anyhow::Error;
    fn try_from(key_code: usize) -> Result<UserAction> {
        let action = match key_code {
            0x08 => UserAction::Backspace, // VK_BACK
            0x09 => UserAction::Tab,       // VK_TAB
            0x0D => UserAction::Enter,     // VK_RETURN
            0x20 => {
                if is_ctrl_pressed() {
                    UserAction::ToggleInputMode
                } else {
                    UserAction::Space
                }
            } // VK_SPACE
            0x1B => UserAction::Escape,    // VK_ESCAPE
            0x2E => UserAction::Delete,    // VK_DELETE

            0x25 => UserAction::Navigation(Navigation::Left), // VK_LEFT
            0x26 => UserAction::Navigation(Navigation::Up),   // VK_UP
            0x27 => UserAction::Navigation(Navigation::Right), // VK_RIGHT
            0x28 => UserAction::Navigation(Navigation::Down), // VK_DOWN

            0x30..=0x39 | 0x60..=0x69 if !VK_SHIFT.is_pressed() => {
                match key_code {
                    0x30 => UserAction::Number {
                        value: 0,
                        is_numpad: false,
                    }, // VK_0
                    0x31 => UserAction::Number {
                        value: 1,
                        is_numpad: false,
                    }, // VK_1
                    0x32 => UserAction::Number {
                        value: 2,
                        is_numpad: false,
                    }, // VK_2
                    0x33 => UserAction::Number {
                        value: 3,
                        is_numpad: false,
                    }, // VK_3
                    0x34 => UserAction::Number {
                        value: 4,
                        is_numpad: false,
                    }, // VK_4
                    0x35 => UserAction::Number {
                        value: 5,
                        is_numpad: false,
                    }, // VK_5
                    0x36 => UserAction::Number {
                        value: 6,
                        is_numpad: false,
                    }, // VK_6
                    0x37 => UserAction::Number {
                        value: 7,
                        is_numpad: false,
                    }, // VK_7
                    0x38 => UserAction::Number {
                        value: 8,
                        is_numpad: false,
                    }, // VK_8
                    0x39 => UserAction::Number {
                        value: 9,
                        is_numpad: false,
                    }, // VK_9
                    0x60 => UserAction::Number {
                        value: 0,
                        is_numpad: true,
                    }, // VK_NUMPAD0
                    0x61 => UserAction::Number {
                        value: 1,
                        is_numpad: true,
                    }, // VK_NUMPAD1
                    0x62 => UserAction::Number {
                        value: 2,
                        is_numpad: true,
                    }, // VK_NUMPAD2
                    0x63 => UserAction::Number {
                        value: 3,
                        is_numpad: true,
                    }, // VK_NUMPAD3
                    0x64 => UserAction::Number {
                        value: 4,
                        is_numpad: true,
                    }, // VK_NUMPAD4
                    0x65 => UserAction::Number {
                        value: 5,
                        is_numpad: true,
                    }, // VK_NUMPAD5
                    0x66 => UserAction::Number {
                        value: 6,
                        is_numpad: true,
                    }, // VK_NUMPAD6
                    0x67 => UserAction::Number {
                        value: 7,
                        is_numpad: true,
                    }, // VK_NUMPAD7
                    0x68 => UserAction::Number {
                        value: 8,
                        is_numpad: true,
                    }, // VK_NUMPAD8
                    0x69 => UserAction::Number {
                        value: 9,
                        is_numpad: true,
                    }, // VK_NUMPAD9
                    _ => UserAction::Unknown,
                }
            }

            0x75 => UserAction::Function(Function::Six), // VK_F6
            0x76 => UserAction::Function(Function::Seven), // VK_F7
            0x77 => UserAction::Function(Function::Eight), // VK_F8
            0x78 => UserAction::Function(Function::Nine), // VK_F9
            0x79 => UserAction::Function(Function::Ten), // VK_F10
            0x6A => UserAction::NumpadSymbol('*'),       // VK_MULTIPLY
            0x6B => UserAction::NumpadSymbol('+'),       // VK_ADD
            0x6C => UserAction::NumpadSymbol(','),       // VK_SEPARATOR
            0x6D => UserAction::NumpadSymbol('-'),       // VK_SUBTRACT
            0x6E => UserAction::NumpadSymbol('.'),       // VK_DECIMAL
            0x6F => UserAction::NumpadSymbol('/'),       // VK_DIVIDE
            0x16 => UserAction::InputModeOn,             // VK_IME_ON
            0x1A => UserAction::InputModeOff,            // VK_IME_OFF

            0xF3 | 0xF4 => UserAction::ToggleInputMode, // Zenkaku/Hankaku

            _ => {
                let key_state = {
                    let mut key_state = [0u8; 256];
                    unsafe {
                        GetKeyboardState(&mut key_state)?;
                    }
                    key_state
                };
                let (unicode, unicode_result) = {
                    let mut unicode = [0u16; 1];
                    let result =
                        unsafe { ToUnicode(key_code as u32, 0, Some(&key_state), &mut unicode, 0) };
                    (unicode[0], result)
                };

                if unicode != 0 {
                    if unicode_result < 0 {
                        clear_dead_key_state(&key_state);
                    }
                    UserAction::Input(char::from_u32(unicode as u32).context("Invalid char")?)
                } else {
                    UserAction::Unknown
                }
            }
        };

        Ok(action)
    }
}
