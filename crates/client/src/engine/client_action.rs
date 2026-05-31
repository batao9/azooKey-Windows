use super::input_mode::InputMode;

#[derive(Debug, PartialEq)]
pub enum ClientAction {
    StartComposition,
    EndComposition,
    ShowCandidateWindow,

    AppendText(String),
    AppendTextRaw(String),
    AppendTextDirect(String),
    CommitTextDirect(String),
    RemoveText,
    ShrinkText(String),
    ShrinkTextRaw(String),
    ShrinkTextDirect(String),

    SetTextWithType(SetTextType),

    MoveCursor(i32),
    EnsureClauseNavigationReady,
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

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SetTextType {
    Hiragana,     // F6 / Ctrl+U
    Katakana,     // F7 / Ctrl+I
    HalfKatakana, // F8 / Ctrl+O
    FullLatin,    // F9 / Ctrl+P
    HalfLatin,    // F10 / Ctrl+T
}
