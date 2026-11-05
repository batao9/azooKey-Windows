use super::input_mode::InputMode;

#[derive(Clone, Debug, PartialEq)]
pub enum ClientAction {
    StartComposition,
    EndComposition,
    ShowCandidateWindow,
    StartJevConversion,

    AppendText(String),
    AppendTextRaw(String),
    AppendTextDirect(String),
    /// Commit the rendered preview by moving the live TSF start boundary,
    /// after preparing the following input. Do not end/start within one key.
    CommitPreviewAndAppend {
        committed: String,
        text: String,
        kind: PreviewInputKind,
        temporary_latin: bool,
    },
    CommitTextDirect(String),
    RestoreReconversionOriginal,
    RemoveText,
    ShrinkText(String),
    ShrinkTextRaw(String),
    ShrinkTextDirect(String),

    SetTextWithType(SetTextType),

    MoveCursor(i32),
    EnsureClauseNavigationReady {
        prepare_future_clauses: bool,
    },
    MoveClause(i32),
    AdjustBoundary(i32),
    SetSelection(SetSelectionType),
    CommitLearning {
        scope: LearningCommitScope,
        kind: LearningCommitKind,
        was_temporary_latin: bool,
    },
    SetTemporaryLatin(bool),
    SetTemporaryLatinShiftPending(bool),

    SetIMEMode(InputMode),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PreviewInputKind {
    Romaji,
    Raw,
    Direct,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SetSelectionType {
    Up,
    Down,
    Number(i32),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LearningCommitKind {
    Normal,
    Partial,
}

impl LearningCommitKind {
    pub(crate) fn proto_value(self) -> i32 {
        match self {
            Self::Normal => 1,
            Self::Partial => 3,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LearningCommitScope {
    CurrentClause,
    Composition,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SetTextType {
    Hiragana,     // F6 / Ctrl+U
    Katakana,     // F7 / Ctrl+I
    HalfKatakana, // F8 / Ctrl+O
    FullLatin,    // F9 / Ctrl+P
    HalfLatin,    // F10 / Ctrl+T
}
