use super::input_mode::InputMode;

#[derive(Debug, PartialEq)]
pub enum ClientAction {
    StartComposition,
    EndComposition,

    AppendText(String),
    AppendTextRaw(String),
    AppendTextDirect(String),
    RemoveText,
    ShrinkText(String),
    ShrinkTextRaw(String),
    ShrinkTextDirect(String),

    SetTextWithType(SetTextType),

    MoveCursor(i32),
    MoveClause(i32),
    AdjustBoundary(i32),
    SetSelection(SetSelectionType),
    SetTemporaryLatin(bool),
    SetTemporaryLatinShiftPending(bool),

    SetIMEMode(InputMode),
}

#[derive(Debug, PartialEq)]
pub enum SetSelectionType {
    Up,
    Down,
    Number(i32),
}

#[derive(Debug, PartialEq)]
pub enum SetTextType {
    Hiragana,     // F6
    Katakana,     // F7
    HalfKatakana, // F8
    FullLatin,    // F9
    HalfLatin,    // F10
}
