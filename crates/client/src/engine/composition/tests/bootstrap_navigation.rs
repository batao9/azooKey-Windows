use super::*;

fn with_clause_hints(mut candidates: Candidates, hints: &[(&str, &str, i32)]) -> Candidates {
    candidates.clauses = clause_hints(hints);
    candidates
}

#[derive(Default)]
struct BootstrapBackend {
    move_cursor_calls: Vec<i32>,
    shrink_text_calls: Vec<i32>,
}

impl BootstrapBackend {
    fn bootstrap_candidates() -> Candidates {
        with_clause_hints(
            candidates(
                &["かげんとういつ", "かげん"],
                &["", "とういつ"],
                "かげんとういつ",
                &[12, 5],
            ),
            &[("かげん", "かげん", 5), ("とういつ", "とういつ", 7)],
        )
    }

    fn boundary_candidates() -> Candidates {
        candidates(&["かげん"], &["とういつ"], "かげんとういつ", &[5])
    }

    fn tail_candidates() -> Candidates {
        candidates(&["とういつ"], &[""], "とういつ", &[7])
    }
}

impl ClauseActionBackend for BootstrapBackend {
    fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        self.move_cursor_calls.push(offset);
        Ok(match offset {
            -1 => Self::bootstrap_candidates(),
            0 => Self::boundary_candidates(),
            1 => Self::tail_candidates(),
            _ => Self::bootstrap_candidates(),
        })
    }

    fn shrink_text(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        self.shrink_text_calls.push(offset);
        Ok(Self::tail_candidates())
    }
}

#[derive(Default)]
struct DisplayPreservingBootstrapBackend {
    move_cursor_calls: Vec<i32>,
    shrink_text_calls: Vec<i32>,
}

impl DisplayPreservingBootstrapBackend {
    fn bootstrap_candidates() -> Candidates {
        with_clause_hints(
            candidates(
                &["いい加減統一しろ", "いい加減"],
                &["", "とういつしろ"],
                "いいかげんとういつしろ",
                &[17, 7],
            ),
            &[
                ("いい加減", "いいかげん", 7),
                ("統一しろ", "とういつしろ", 10),
            ],
        )
    }

    fn boundary_candidates() -> Candidates {
        candidates(
            &["いい加減"],
            &["とういつしろ"],
            "いいかげんとういつしろ",
            &[7],
        )
    }

    fn tail_candidates() -> Candidates {
        candidates(
            &["とういつしろ", "統一しろ"],
            &["", ""],
            "とういつしろ",
            &[10, 10],
        )
    }
}

impl ClauseActionBackend for DisplayPreservingBootstrapBackend {
    fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        self.move_cursor_calls.push(offset);
        Ok(match offset {
            -1 => Self::bootstrap_candidates(),
            0 => Self::boundary_candidates(),
            1 => Self::tail_candidates(),
            _ => Self::bootstrap_candidates(),
        })
    }

    fn shrink_text(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        self.shrink_text_calls.push(offset);
        Ok(Self::tail_candidates())
    }
}

#[derive(Default)]
struct SelectedCandidateBootstrapBackend {
    move_cursor_calls: Vec<i32>,
    shrink_text_calls: Vec<i32>,
}

impl SelectedCandidateBootstrapBackend {
    fn bootstrap_candidates() -> Candidates {
        with_clause_hints(
            candidates(
                &["加藤純一", "加藤"],
                &["", "じゅんいち"],
                "かとうじゅんいち",
                &[12, 5],
            ),
            &[("加藤", "かとう", 5), ("純一", "じゅんいち", 7)],
        )
    }

    fn boundary_candidates() -> Candidates {
        candidates(&["加藤"], &["じゅんいち"], "かとうじゅんいち", &[5])
    }

    fn tail_candidates() -> Candidates {
        candidates(
            &["じゅんいち", "淳一", "純一"],
            &["", "", ""],
            "じゅんいち",
            &[7, 7, 7],
        )
    }
}

impl ClauseActionBackend for SelectedCandidateBootstrapBackend {
    fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        self.move_cursor_calls.push(offset);
        Ok(match offset {
            -1 => Self::bootstrap_candidates(),
            0 => Self::boundary_candidates(),
            1 => Self::tail_candidates(),
            _ => Self::bootstrap_candidates(),
        })
    }

    fn shrink_text(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        self.shrink_text_calls.push(offset);
        Ok(Self::tail_candidates())
    }
}

#[derive(Default)]
struct AdjustBoundaryBackend {
    move_cursor_calls: Vec<i32>,
    shrink_text_calls: Vec<i32>,
    last_direction: i32,
}

impl AdjustBoundaryBackend {
    fn unsplit_candidates() -> Candidates {
        with_clause_hints(
            candidates(&["テスト文章"], &[""], "てすとぶんしょう", &[7]),
            &[("テスト", "てすと", 3), ("文章", "ぶんしょう", 4)],
        )
    }

    fn split_left_candidates() -> Candidates {
        candidates(&["テスト文書"], &["う"], "てすとぶんしょう", &[6])
    }

    fn split_right_candidates() -> Candidates {
        candidates(&["て"], &["スト文章"], "てすとぶんしょう", &[1])
    }
}

impl ClauseActionBackend for AdjustBoundaryBackend {
    fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        self.move_cursor_calls.push(offset);
        Ok(match offset {
            -1 | 1 => {
                self.last_direction = offset;
                Self::unsplit_candidates()
            }
            0 => match self.last_direction {
                -1 => Self::split_left_candidates(),
                1 => Self::split_right_candidates(),
                _ => Self::unsplit_candidates(),
            },
            _ => Self::unsplit_candidates(),
        })
    }

    fn shrink_text(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        self.shrink_text_calls.push(offset);
        Ok(Self::unsplit_candidates())
    }
}

#[derive(Default)]
struct MultiStageBootstrapBackend {
    move_cursor_calls: Vec<i32>,
    shrink_text_calls: Vec<i32>,
    current_stage: usize,
    stage_stack: Vec<usize>,
}

impl MultiStageBootstrapBackend {
    const FULL_RAW: &'static str = "aaabbbcccdddeee";
    const STAGE0_TAIL: &'static str =
        "モード見選択時の左右キー操作を初回だけ自動で設定しそのまま分節移動できるようにしました";
    const STAGE1_TAIL: &'static str =
        "見選択時の左右キー操作を初回だけ自動で設定しそのまま分節移動できるようにしました";
    const STAGE2_TAIL: &'static str =
        "左右キー操作を初回だけ自動で設定しそのまま分節移動できるようにしました";
    const STAGE3_TAIL: &'static str = "初回だけ自動で設定しそのまま分節移動できるようにしました";
    const STAGE4_TAIL: &'static str = "";

    fn bootstrap_candidates() -> Candidates {
        with_clause_hints(
            candidates(
            &[
                "文節",
                "文節モード見選択時の左右キー操作を初回だけ自動で設定しそのまま分節移動できるようにしました",
            ],
            &[Self::STAGE0_TAIL, ""],
            Self::FULL_RAW,
            &[3, 15],
            ),
            &[
                ("文節", "aaa", 3),
                ("モード", "bbb", 3),
                ("見選択時の", "ccc", 3),
                ("左右キー操作を", "ddd", 3),
                (
                    "初回だけ自動で設定しそのまま分節移動できるようにしました",
                    "eee",
                    3,
                ),
            ],
        )
    }

    fn stage_candidates(stage: usize) -> Candidates {
        match stage {
            0 => candidates(&["文節"], &[Self::STAGE0_TAIL], Self::FULL_RAW, &[3]),
            1 => candidates(&["モード"], &[Self::STAGE1_TAIL], "bbbcccdddeee", &[3]),
            2 => candidates(&["見選択時の"], &[Self::STAGE2_TAIL], "cccdddeee", &[3]),
            3 => candidates(&["左右キー操作を"], &[Self::STAGE3_TAIL], "dddeee", &[3]),
            4 => candidates(
                &["初回だけ自動で設定しそのまま分節移動できるようにしました"],
                &[Self::STAGE4_TAIL],
                "eee",
                &[3],
            ),
            _ => Candidates::default(),
        }
    }
}

impl ClauseActionBackend for MultiStageBootstrapBackend {
    fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        self.move_cursor_calls.push(offset);
        Ok(match offset {
            TextServiceFactory::MOVE_CURSOR_CLEAR_CLAUSE_SNAPSHOTS => {
                self.stage_stack.clear();
                Self::stage_candidates(self.current_stage)
            }
            TextServiceFactory::MOVE_CURSOR_PUSH_CLAUSE_SNAPSHOT => {
                self.stage_stack.push(self.current_stage);
                Self::stage_candidates(self.current_stage)
            }
            TextServiceFactory::MOVE_CURSOR_POP_CLAUSE_SNAPSHOT => {
                if let Some(stage) = self.stage_stack.pop() {
                    self.current_stage = stage;
                }
                Self::stage_candidates(self.current_stage)
            }
            -1 => Self::bootstrap_candidates(),
            0 | 1 => Self::stage_candidates(self.current_stage),
            _ => Candidates::default(),
        })
    }

    fn shrink_text(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        self.shrink_text_calls.push(offset);
        if self.current_stage < 4 {
            self.current_stage += 1;
        }
        Ok(Self::stage_candidates(self.current_stage))
    }
}

struct BootstrapState {
    preview: String,
    suffix: String,
    raw_input: String,
    raw_hiragana: String,
    fixed_prefix: String,
    corresponding_count: i32,
    selection_index: i32,
    candidates: Candidates,
    clause_snapshots: Vec<ClauseSnapshot>,
    future_clause_snapshots: Vec<FutureClauseSnapshot>,
    current_clause_is_split_derived: bool,
    current_clause_is_direct_split_remainder: bool,
    current_clause_has_split_left_neighbor: bool,
    current_clause_split_group_id: Option<u64>,
    next_split_group_id: u64,
    clause_navigation_backend_dirty: bool,
}

impl BootstrapState {
    fn new() -> Self {
        Self {
            preview: "かげんとういつ".to_string(),
            suffix: String::new(),
            raw_input: "kagentouitsu".to_string(),
            raw_hiragana: "かげんとういつ".to_string(),
            fixed_prefix: String::new(),
            corresponding_count: 12,
            selection_index: 0,
            candidates: BootstrapBackend::bootstrap_candidates(),
            clause_snapshots: Vec::new(),
            future_clause_snapshots: Vec::new(),
            current_clause_is_split_derived: false,
            current_clause_is_direct_split_remainder: false,
            current_clause_has_split_left_neighbor: false,
            current_clause_split_group_id: None,
            next_split_group_id: 1,
            clause_navigation_backend_dirty: false,
        }
    }

    fn state_mut(&mut self) -> ClauseActionStateMut<'_> {
        ClauseActionStateMut {
            preview: &mut self.preview,
            suffix: &mut self.suffix,
            raw_input: &mut self.raw_input,
            raw_hiragana: &mut self.raw_hiragana,
            fixed_prefix: &mut self.fixed_prefix,
            corresponding_count: &mut self.corresponding_count,
            selection_index: &mut self.selection_index,
            candidates: &mut self.candidates,
            clause_snapshots: &mut self.clause_snapshots,
            future_clause_snapshots: &mut self.future_clause_snapshots,
            current_clause_is_split_derived: &mut self.current_clause_is_split_derived,
            current_clause_is_direct_split_remainder: &mut self
                .current_clause_is_direct_split_remainder,
            current_clause_has_split_left_neighbor: &mut self
                .current_clause_has_split_left_neighbor,
            current_clause_split_group_id: &mut self.current_clause_split_group_id,
            next_split_group_id: &mut self.next_split_group_id,
            clause_navigation_backend_dirty: &mut self.clause_navigation_backend_dirty,
        }
    }
}

fn multi_stage_bootstrap_state() -> BootstrapState {
    BootstrapState {
        preview:
            "文節モード見選択時の左右キー操作を初回だけ自動で設定しそのまま分節移動できるようにしました"
                .to_string(),
        suffix: String::new(),
        raw_input: "aaabbbcccdddeee".to_string(),
        raw_hiragana: "aaabbbcccdddeee".to_string(),
        fixed_prefix: String::new(),
        corresponding_count: 15,
        selection_index: 0,
        candidates: MultiStageBootstrapBackend::bootstrap_candidates(),
        clause_snapshots: Vec::new(),
        future_clause_snapshots: Vec::new(),
        current_clause_is_split_derived: false,
        current_clause_is_direct_split_remainder: false,
        current_clause_has_split_left_neighbor: false,
        current_clause_split_group_id: None,
        next_split_group_id: 1,
        clause_navigation_backend_dirty: false,
    }
}

fn display_preserving_bootstrap_state() -> BootstrapState {
    BootstrapState {
        preview: "いい加減統一しろ".to_string(),
        suffix: String::new(),
        raw_input: "iikagentouitusiro".to_string(),
        raw_hiragana: "いいかげんとういつしろ".to_string(),
        fixed_prefix: String::new(),
        corresponding_count: 17,
        selection_index: 0,
        candidates: DisplayPreservingBootstrapBackend::bootstrap_candidates(),
        clause_snapshots: Vec::new(),
        future_clause_snapshots: Vec::new(),
        current_clause_is_split_derived: false,
        current_clause_is_direct_split_remainder: false,
        current_clause_has_split_left_neighbor: false,
        current_clause_split_group_id: None,
        next_split_group_id: 1,
        clause_navigation_backend_dirty: false,
    }
}

fn selected_candidate_bootstrap_state() -> BootstrapState {
    BootstrapState {
        preview: "加藤純一".to_string(),
        suffix: String::new(),
        raw_input: "katoujunnichi".to_string(),
        raw_hiragana: "かとうじゅんいち".to_string(),
        fixed_prefix: String::new(),
        corresponding_count: 12,
        selection_index: 0,
        candidates: SelectedCandidateBootstrapBackend::bootstrap_candidates(),
        clause_snapshots: Vec::new(),
        future_clause_snapshots: Vec::new(),
        current_clause_is_split_derived: false,
        current_clause_is_direct_split_remainder: false,
        current_clause_has_split_left_neighbor: false,
        current_clause_split_group_id: None,
        next_split_group_id: 1,
        clause_navigation_backend_dirty: false,
    }
}

fn adjust_boundary_state() -> BootstrapState {
    BootstrapState {
        preview: "テスト文章".to_string(),
        suffix: String::new(),
        raw_input: "てすとぶんしょう".to_string(),
        raw_hiragana: "てすとぶんしょう".to_string(),
        fixed_prefix: String::new(),
        corresponding_count: 7,
        selection_index: 0,
        candidates: AdjustBoundaryBackend::unsplit_candidates(),
        clause_snapshots: Vec::new(),
        future_clause_snapshots: Vec::new(),
        current_clause_is_split_derived: false,
        current_clause_is_direct_split_remainder: false,
        current_clause_has_split_left_neighbor: false,
        current_clause_split_group_id: None,
        next_split_group_id: 1,
        clause_navigation_backend_dirty: false,
    }
}

#[test]
fn move_clause_right_bootstraps_initial_navigation_to_the_first_clause() {
    let mut state = BootstrapState::new();
    let mut backend = BootstrapBackend::default();

    let effect = {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, 1)
            .expect("apply_move_clause")
    };

    assert!(effect.applied);
    assert_eq!(state.preview, "かげん");
    assert_eq!(state.suffix, "とういつ");
    assert_eq!(state.fixed_prefix, "");
    assert_eq!(state.raw_hiragana, "かげんとういつ");
    assert_eq!(state.corresponding_count, 5);
    assert_eq!(state.selection_index, 0);
    assert_eq!(state.candidates.texts, vec!["かげん"]);
    assert!(backend.move_cursor_calls.is_empty());
    assert!(backend.shrink_text_calls.is_empty());
}

#[test]
fn move_clause_left_bootstraps_initial_navigation_to_the_last_clause() {
    let mut state = BootstrapState::new();
    let mut backend = BootstrapBackend::default();

    let effect = {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, -1)
            .expect("apply_move_clause")
    };

    assert!(effect.applied);
    assert_eq!(state.preview, "かげんとういつ");
    assert_eq!(state.suffix, "");
    assert_eq!(state.fixed_prefix, "かげん");
    assert_eq!(
        TextServiceFactory::current_clause_preview(&state.preview, &state.fixed_prefix),
        "とういつ"
    );
    assert_eq!(state.raw_hiragana, "とういつ");
    assert_eq!(state.corresponding_count, 7);
    assert_eq!(state.selection_index, 0);
    assert_eq!(state.candidates.texts, vec!["とういつ"]);
    assert!(backend.move_cursor_calls.is_empty());
    assert!(backend.shrink_text_calls.is_empty());
    assert_eq!(state.clause_snapshots.len(), 1);
}

#[test]
fn move_clause_right_bootstrap_keeps_existing_clause_conversion() {
    let mut state = display_preserving_bootstrap_state();
    let mut backend = DisplayPreservingBootstrapBackend::default();

    let effect = {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, 1)
            .expect("apply_move_clause")
    };

    assert!(effect.applied);
    assert_eq!(state.suffix, "統一しろ");
    assert_eq!(
        TextServiceFactory::clause_texts_for_log(
            &state.preview,
            &state.fixed_prefix,
            &state.clause_snapshots,
            &state.future_clause_snapshots,
        ),
        "いい加減 / 統一しろ"
    );
}

#[test]
fn move_clause_left_then_left_keeps_bootstrapped_tail_conversion() {
    let mut state = display_preserving_bootstrap_state();
    let mut backend = DisplayPreservingBootstrapBackend::default();

    {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, -1)
            .expect("bootstrap to last clause");
    }

    let effect = {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, -1)
            .expect("move back to first clause")
    };

    assert!(effect.applied);
    assert_eq!(state.suffix, "統一しろ");
    assert_eq!(
        TextServiceFactory::clause_texts_for_log(
            &state.preview,
            &state.fixed_prefix,
            &state.clause_snapshots,
            &state.future_clause_snapshots,
        ),
        "いい加減 / 統一しろ"
    );
}

#[test]
fn move_clause_right_bootstrap_preserves_preview_conversion() {
    let mut state = display_preserving_bootstrap_state();
    let mut backend = DisplayPreservingBootstrapBackend::default();
    state.preview = "イイカゲントウイツシロ".to_string();

    let effect = {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, 1)
            .expect("apply_move_clause")
    };

    assert!(effect.applied);
    assert_eq!(
        TextServiceFactory::clause_texts_for_log(
            &state.preview,
            &state.fixed_prefix,
            &state.clause_snapshots,
            &state.future_clause_snapshots,
        ),
        "イイカゲン / トウイツシロ"
    );
}

#[test]
fn move_clause_left_bootstrap_keeps_selected_candidate_conversion() {
    let mut state = selected_candidate_bootstrap_state();
    let mut backend = SelectedCandidateBootstrapBackend::default();

    let effect = {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, -1)
            .expect("apply_move_clause")
    };

    assert!(effect.applied);
    assert_eq!(state.selection_index, 2);
    assert_eq!(
        TextServiceFactory::current_clause_preview(&state.preview, &state.fixed_prefix),
        "純一"
    );
    assert_eq!(
        TextServiceFactory::clause_texts_for_log(
            &state.preview,
            &state.fixed_prefix,
            &state.clause_snapshots,
            &state.future_clause_snapshots,
        ),
        "加藤 / 純一"
    );
}

#[test]
fn move_clause_right_bootstrap_keeps_selected_candidate_conversion() {
    let mut state = selected_candidate_bootstrap_state();
    let mut backend = SelectedCandidateBootstrapBackend::default();

    let effect = {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, 1)
            .expect("apply_move_clause")
    };

    assert!(effect.applied);
    assert_eq!(state.suffix, "純一");
    assert_eq!(
        TextServiceFactory::clause_texts_for_log(
            &state.preview,
            &state.fixed_prefix,
            &state.clause_snapshots,
            &state.future_clause_snapshots,
        ),
        "加藤 / 純一"
    );
}

#[test]
fn adjust_boundary_left_from_unsplit_state_skips_auto_clause_bootstrap() {
    let mut state = adjust_boundary_state();
    let mut backend = AdjustBoundaryBackend::default();

    let effect = {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_adjust_boundary(&mut state_mut, &mut backend, -1)
            .expect("apply_adjust_boundary")
    };

    assert!(effect.applied);
    assert_eq!(state.preview, "テスト文書");
    assert_eq!(state.suffix, "う");
    assert_eq!(state.fixed_prefix, "");
    assert_eq!(state.corresponding_count, 6);
    assert_eq!(state.selection_index, 0);
    assert_eq!(backend.move_cursor_calls, vec![-1, 0]);
    assert!(backend.shrink_text_calls.is_empty());
    assert!(state.clause_snapshots.is_empty());
    assert!(state.future_clause_snapshots.is_empty());
}

#[test]
fn adjust_boundary_right_from_unsplit_state_skips_auto_clause_bootstrap() {
    let mut state = adjust_boundary_state();
    let mut backend = AdjustBoundaryBackend::default();

    let effect = {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_adjust_boundary(&mut state_mut, &mut backend, 1)
            .expect("apply_adjust_boundary")
    };

    assert!(effect.applied);
    assert_eq!(state.preview, "て");
    assert_eq!(state.suffix, "スト文章");
    assert_eq!(state.fixed_prefix, "");
    assert_eq!(state.corresponding_count, 1);
    assert_eq!(state.selection_index, 0);
    assert_eq!(backend.move_cursor_calls, vec![1, 0]);
    assert!(backend.shrink_text_calls.is_empty());
    assert!(state.clause_snapshots.is_empty());
    assert!(state.future_clause_snapshots.is_empty());
}

#[test]
fn move_clause_left_then_left_keeps_selected_candidate_conversion() {
    let mut state = selected_candidate_bootstrap_state();
    let mut backend = SelectedCandidateBootstrapBackend::default();

    {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, -1)
            .expect("bootstrap to last clause");
    }

    let effect = {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, -1)
            .expect("move back to first clause")
    };

    assert!(effect.applied);
    assert_eq!(state.suffix, "純一");
    assert_eq!(
        TextServiceFactory::clause_texts_for_log(
            &state.preview,
            &state.fixed_prefix,
            &state.clause_snapshots,
            &state.future_clause_snapshots,
        ),
        "加藤 / 純一"
    );
}

#[test]
fn move_clause_right_rebuilds_multiple_following_clauses_without_resetting_text() {
    let mut state = multi_stage_bootstrap_state();
    let mut backend = MultiStageBootstrapBackend::default();

    let effect = {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, 1)
            .expect("apply_move_clause")
    };

    assert!(effect.applied);
    assert_eq!(state.future_clause_snapshots.len(), 4);
    assert!(backend.shrink_text_calls.is_empty());
    assert!(backend.move_cursor_calls.is_empty());
    assert_eq!(
        TextServiceFactory::clause_texts_for_log(
            &state.preview,
            &state.fixed_prefix,
            &state.clause_snapshots,
            &state.future_clause_snapshots,
        ),
        "文節 / モード / 見選択時の / 左右キー操作を / 初回だけ自動で設定しそのまま分節移動できるようにしました"
    );

    let effect = {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, 1)
            .expect("apply_move_clause")
    };

    assert!(effect.applied);
    assert!(backend.move_cursor_calls.is_empty());
    assert!(backend.shrink_text_calls.is_empty());
    assert_eq!(
        TextServiceFactory::current_clause_preview(&state.preview, &state.fixed_prefix),
        "モード"
    );
    assert_eq!(
        TextServiceFactory::clause_texts_for_log(
            &state.preview,
            &state.fixed_prefix,
            &state.clause_snapshots,
            &state.future_clause_snapshots,
        ),
        "文節 / モード / 見選択時の / 左右キー操作を / 初回だけ自動で設定しそのまま分節移動できるようにしました"
    );

    let effect = {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, -1)
            .expect("apply_move_clause")
    };

    assert!(effect.applied);
    assert_eq!(state.future_clause_snapshots.len(), 4);
    assert_eq!(
        TextServiceFactory::clause_texts_for_log(
            &state.preview,
            &state.fixed_prefix,
            &state.clause_snapshots,
            &state.future_clause_snapshots,
        ),
        "文節 / モード / 見選択時の / 左右キー操作を / 初回だけ自動で設定しそのまま分節移動できるようにしました"
    );
}

#[test]
fn move_clause_left_bootstraps_multiple_following_clauses_without_resetting_text() {
    let mut state = multi_stage_bootstrap_state();
    let mut backend = MultiStageBootstrapBackend::default();

    let effect = {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, -1)
            .expect("apply_move_clause")
    };

    assert!(effect.applied);
    assert_eq!(state.clause_snapshots.len(), 4);
    assert!(state.future_clause_snapshots.is_empty());
    assert!(backend.move_cursor_calls.is_empty());
    assert!(backend.shrink_text_calls.is_empty());
    assert!(!backend
        .move_cursor_calls
        .iter()
        .any(|offset| *offset == -1 || *offset == 0));
    assert_eq!(
        TextServiceFactory::current_clause_preview(&state.preview, &state.fixed_prefix),
        "初回だけ自動で設定しそのまま分節移動できるようにしました"
    );
    assert_eq!(
        TextServiceFactory::clause_texts_for_log(
            &state.preview,
            &state.fixed_prefix,
            &state.clause_snapshots,
            &state.future_clause_snapshots,
        ),
        "文節 / モード / 見選択時の / 左右キー操作を / 初回だけ自動で設定しそのまま分節移動できるようにしました"
    );
}

#[test]
fn move_clause_right_after_returning_to_the_first_clause_avoids_cursor_rewalk() {
    let mut state = multi_stage_bootstrap_state();
    let mut backend = MultiStageBootstrapBackend::default();

    {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, -1)
            .expect("bootstrap to last clause");
    }

    while !state.clause_snapshots.is_empty() {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, -1)
            .expect("move back toward first clause");
    }

    let effect = {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, 1)
            .expect("move right from first clause")
    };

    assert!(effect.applied);
    assert!(backend.shrink_text_calls.is_empty());
    assert!(!backend
        .move_cursor_calls
        .iter()
        .any(|offset| *offset == -1 || *offset == 0));
    assert_eq!(
        TextServiceFactory::current_clause_preview(&state.preview, &state.fixed_prefix),
        "モード"
    );
    assert_eq!(
        TextServiceFactory::clause_texts_for_log(
            &state.preview,
            &state.fixed_prefix,
            &state.clause_snapshots,
            &state.future_clause_snapshots,
        ),
        "文節 / モード / 見選択時の / 左右キー操作を / 初回だけ自動で設定しそのまま分節移動できるようにしました"
    );
}

#[test]
fn sync_raw_input_with_candidates_trims_stale_romaji_after_kana_backspace() {
    let mut raw_input = "aru".to_string();
    let candidates = with_clause_hints(
        candidates(&["あ"], &[""], "あ", &[1]),
        &[("あ", "あ", 1)],
    );

    TextServiceFactory::sync_raw_input_with_candidates(&mut raw_input, &candidates);

    assert_eq!(raw_input, "a");
}

#[test]
fn move_clause_right_uses_clause_hints_again_after_raw_input_resync() {
    let mut state = multi_stage_bootstrap_state();
    let mut backend = MultiStageBootstrapBackend::default();
    state.raw_input.push('x');

    assert!(TextServiceFactory::clause_bootstrap_hints(&state.state_mut()).is_none());

    TextServiceFactory::sync_raw_input_with_candidates(&mut state.raw_input, &state.candidates);

    let effect = {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, 1)
            .expect("bootstrap after raw_input resync")
    };

    assert!(effect.applied);
    assert!(backend.move_cursor_calls.is_empty());
    assert!(backend.shrink_text_calls.is_empty());
    assert_eq!(
        TextServiceFactory::clause_texts_for_log(
            &state.preview,
            &state.fixed_prefix,
            &state.clause_snapshots,
            &state.future_clause_snapshots,
        ),
        "文節 / モード / 見選択時の / 左右キー操作を / 初回だけ自動で設定しそのまま分節移動できるようにしました"
    );
}

#[test]
fn move_clause_left_uses_clause_hints_again_after_raw_input_resync() {
    let mut state = multi_stage_bootstrap_state();
    let mut backend = MultiStageBootstrapBackend::default();
    state.raw_input.push('x');

    TextServiceFactory::sync_raw_input_with_candidates(&mut state.raw_input, &state.candidates);

    let effect = {
        let mut state_mut = state.state_mut();
        TextServiceFactory::apply_move_clause(&mut state_mut, &mut backend, -1)
            .expect("bootstrap left after raw_input resync")
    };

    assert!(effect.applied);
    assert!(backend.move_cursor_calls.is_empty());
    assert!(backend.shrink_text_calls.is_empty());
    assert_eq!(
        TextServiceFactory::current_clause_preview(&state.preview, &state.fixed_prefix),
        "初回だけ自動で設定しそのまま分節移動できるようにしました"
    );
    assert_eq!(
        TextServiceFactory::clause_texts_for_log(
            &state.preview,
            &state.fixed_prefix,
            &state.clause_snapshots,
            &state.future_clause_snapshots,
        ),
        "文節 / モード / 見選択時の / 左右キー操作を / 初回だけ自動で設定しそのまま分節移動できるようにしました"
    );
}
