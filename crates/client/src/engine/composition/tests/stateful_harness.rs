use std::collections::HashSet;

use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SimUnit {
    display: &'static str,
    raw_input: &'static str,
    raw_hiragana: &'static str,
    origin_id: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct SimCommittedClause {
    display: String,
    raw_hiragana: String,
    corresponding_count: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SimClause {
    units: Vec<SimUnit>,
    selected_candidate: usize,
    pending_remainder: bool,
    display_override: Option<SetTextType>,
}

impl SimClause {
    fn raw_input(&self) -> String {
        self.units
            .iter()
            .map(|unit| unit.raw_input)
            .collect::<Vec<_>>()
            .join("")
    }

    fn raw_hiragana(&self) -> String {
        self.units
            .iter()
            .map(|unit| unit.raw_hiragana)
            .collect::<Vec<_>>()
            .join("")
    }

    fn corresponding_count(&self) -> i32 {
        self.raw_input().chars().count() as i32
    }

    fn can_split_left(&self) -> bool {
        self.units.len() > 1
    }

    fn uniform_origin_id(&self) -> Option<usize> {
        let first = self.units.first()?.origin_id;
        self.units
            .iter()
            .all(|unit| unit.origin_id == first)
            .then_some(first)
    }

    fn candidate_texts(&self) -> Vec<String> {
        match (self.uniform_origin_id(), self.raw_hiragana().as_str()) {
            (Some(1), "かげん") => {
                vec!["加減".to_string(), "下限".to_string(), "かげん".to_string()]
            }
            (Some(2), "とういつ") => vec!["統一".to_string(), "とういつ".to_string()],
            _ => vec![self.raw_hiragana()],
        }
    }

    fn candidate_selected_text(&self) -> String {
        let candidate_texts = self.candidate_texts();
        candidate_texts
            .get(self.selected_candidate)
            .cloned()
            .unwrap_or_else(|| candidate_texts.first().cloned().unwrap_or_default())
    }

    fn selected_text(&self) -> String {
        self.display_override
            .as_ref()
            .map(|set_type| {
                TextServiceFactory::converted_clause_preview_text(
                    set_type,
                    &self.raw_input(),
                    &self.raw_hiragana(),
                )
            })
            .unwrap_or_else(|| self.candidate_selected_text())
    }

    fn clamp_selection(&mut self) {
        let max_index = self.candidate_texts().len().saturating_sub(1);
        self.selected_candidate = self.selected_candidate.min(max_index);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct SimSpecState {
    committed_clauses: Vec<SimCommittedClause>,
    clauses: Vec<SimClause>,
    current_index: usize,
}

#[derive(Clone, Debug)]
pub(super) struct ClauseHarness {
    pub(super) committed_clauses: Vec<SimCommittedClause>,
    pub(super) state: CompositionState,
    pub(super) preview: String,
    pub(super) suffix: String,
    pub(super) raw_input: String,
    pub(super) raw_hiragana: String,
    pub(super) fixed_prefix: String,
    pub(super) corresponding_count: i32,
    pub(super) selection_index: i32,
    pub(super) current_clause_is_split_derived: bool,
    pub(super) current_clause_is_direct_split_remainder: bool,
    pub(super) current_clause_has_split_left_neighbor: bool,
    pub(super) current_clause_split_group_id: Option<u64>,
    pub(super) candidates: Candidates,
    pub(super) clause_snapshots: Vec<ClauseSnapshot>,
    pub(super) future_clause_snapshots: Vec<FutureClauseSnapshot>,
    pub(super) next_split_group_id: u64,
}

#[derive(Copy, Clone, Debug)]
pub(super) enum HarnessUserAction {
    Left,
    Right,
    ShiftLeft,
    Space,
    Enter,
    SetTextType(SetTextType),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct HarnessStateKey {
    state: &'static str,
    preview: String,
    suffix: String,
    raw_input: String,
    raw_hiragana: String,
    fixed_prefix: String,
    corresponding_count: i32,
    selection_index: i32,
    current_clause_is_split_derived: bool,
    current_clause_is_direct_split_remainder: bool,
    current_clause_has_split_left_neighbor: bool,
    current_clause_split_group_id: Option<u64>,
    candidates: String,
    clause_snapshots: String,
    future_clause_snapshots: String,
    committed_clauses: String,
    clauses: String,
    clauses_raw: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct VisitedState {
    spec: SimSpecState,
    harness: HarnessStateKey,
}

#[derive(Clone, Debug)]
pub(super) struct ScenarioBackend {
    pub(super) spec: SimSpecState,
    pub(super) server: SimSpecState,
    pub(super) server_snapshots: Vec<SimSpecState>,
    pub(super) blocked_boundary: bool,
}

fn unit(
    display: &'static str,
    raw_input: &'static str,
    raw_hiragana: &'static str,
    origin_id: usize,
) -> SimUnit {
    SimUnit {
        display,
        raw_input,
        raw_hiragana,
        origin_id,
    }
}

fn clause(units: Vec<SimUnit>) -> SimClause {
    SimClause {
        units,
        selected_candidate: 0,
        pending_remainder: false,
        display_override: None,
    }
}

fn clause_with_selection(units: Vec<SimUnit>, selected_candidate: usize) -> SimClause {
    SimClause {
        units,
        selected_candidate,
        pending_remainder: false,
        display_override: None,
    }
}

fn pending_clause(units: Vec<SimUnit>) -> SimClause {
    SimClause {
        units,
        selected_candidate: 0,
        pending_remainder: true,
        display_override: None,
    }
}

pub(super) fn baseline_spec_state() -> SimSpecState {
    SimSpecState {
        committed_clauses: Vec::new(),
        clauses: vec![
            clause(vec![unit("い", "i", "い", 0), unit("い", "i", "い", 0)]),
            clause_with_selection(
                vec![
                    unit("か", "ka", "か", 1),
                    unit("げ", "ge", "げ", 1),
                    unit("ん", "n", "ん", 1),
                ],
                0,
            ),
            clause_with_selection(
                vec![
                    unit("と", "to", "と", 2),
                    unit("う", "u", "う", 2),
                    unit("い", "i", "い", 2),
                    unit("つ", "tu", "つ", 2),
                ],
                0,
            ),
            clause(vec![unit("し", "si", "し", 3), unit("ろ", "ro", "ろ", 3)]),
        ],
        current_index: 3,
    }
}

fn single_clause_spec_state() -> SimSpecState {
    SimSpecState {
        committed_clauses: Vec::new(),
        clauses: vec![clause(vec![
            unit("い", "i", "い", 0),
            unit("い", "i", "い", 0),
            unit("か", "ka", "か", 1),
            unit("げ", "ge", "げ", 1),
            unit("ん", "n", "ん", 1),
            unit("と", "to", "と", 2),
            unit("う", "u", "う", 2),
            unit("い", "i", "い", 2),
            unit("つ", "tu", "つ", 2),
            unit("し", "si", "し", 3),
            unit("ろ", "ro", "ろ", 3),
        ])],
        current_index: 0,
    }
}

fn fkey_two_clause_spec_state() -> SimSpecState {
    SimSpecState {
        committed_clauses: Vec::new(),
        clauses: vec![
            clause_with_selection(
                vec![
                    unit("か", "ka", "か", 1),
                    unit("げ", "ge", "げ", 1),
                    unit("ん", "n", "ん", 1),
                ],
                0,
            ),
            clause_with_selection(
                vec![
                    unit("と", "to", "と", 2),
                    unit("う", "u", "う", 2),
                    unit("い", "i", "い", 2),
                    unit("つ", "tu", "つ", 2),
                ],
                0,
            ),
        ],
        current_index: 0,
    }
}

fn candidates_owned(
    texts: Vec<String>,
    sub_texts: Vec<String>,
    hiragana: String,
    corresponding_count: Vec<i32>,
) -> Candidates {
    Candidates {
        texts,
        sub_texts,
        hiragana,
        corresponding_count,
        clauses: Vec::new(),
    }
}

fn spec_join_display(clauses: &[SimClause]) -> String {
    clauses
        .iter()
        .map(SimClause::selected_text)
        .collect::<Vec<_>>()
        .join("")
}

fn spec_join_raw_input(clauses: &[SimClause]) -> String {
    clauses
        .iter()
        .map(SimClause::raw_input)
        .collect::<Vec<_>>()
        .join("")
}

fn spec_join_raw_hiragana(clauses: &[SimClause]) -> String {
    clauses
        .iter()
        .map(SimClause::raw_hiragana)
        .collect::<Vec<_>>()
        .join("")
}

fn spec_display(spec: &SimSpecState) -> String {
    spec.committed_clauses
        .iter()
        .map(|clause| clause.display.clone())
        .chain(spec.clauses.iter().map(SimClause::selected_text))
        .collect::<Vec<_>>()
        .join(" / ")
}

fn spec_display_raw(spec: &SimSpecState) -> String {
    spec.committed_clauses
        .iter()
        .map(|clause| clause.raw_hiragana.clone())
        .chain(spec.clauses.iter().map(SimClause::raw_hiragana))
        .collect::<Vec<_>>()
        .join(" / ")
}

fn spec_clause_input_lengths(spec: &SimSpecState) -> String {
    spec.committed_clauses
        .iter()
        .map(|clause| clause.corresponding_count.to_string())
        .chain(
            spec.clauses
                .iter()
                .map(|clause| clause.corresponding_count().to_string()),
        )
        .collect::<Vec<_>>()
        .join(" / ")
}

fn spec_clause_origin_id(spec: &SimSpecState, clause_index: usize) -> Option<usize> {
    spec.clauses
        .get(clause_index)
        .and_then(SimClause::uniform_origin_id)
}

fn spec_clause_is_split_derived(spec: &SimSpecState, clause_index: usize) -> bool {
    let Some(origin_id) = spec_clause_origin_id(spec, clause_index) else {
        return false;
    };
    spec.clauses
        .get(clause_index.wrapping_sub(1))
        .and_then(SimClause::uniform_origin_id)
        .map(|candidate_origin| candidate_origin == origin_id)
        .unwrap_or(false)
        || spec
            .clauses
            .get(clause_index + 1)
            .and_then(SimClause::uniform_origin_id)
            .map(|candidate_origin| candidate_origin == origin_id)
            .unwrap_or(false)
}

fn spec_clause_has_split_left_neighbor(spec: &SimSpecState, clause_index: usize) -> bool {
    let Some(origin_id) = spec_clause_origin_id(spec, clause_index) else {
        return false;
    };
    spec.clauses
        .get(clause_index.wrapping_sub(1))
        .and_then(SimClause::uniform_origin_id)
        .map(|candidate_origin| candidate_origin == origin_id)
        .unwrap_or(false)
}

fn spec_clause_is_direct_split_remainder(spec: &SimSpecState, clause_index: usize) -> bool {
    spec_clause_has_split_left_neighbor(spec, clause_index)
}

fn spec_clause_split_group_id(spec: &SimSpecState, clause_index: usize) -> Option<u64> {
    spec_clause_is_split_derived(spec, clause_index)
        .then(|| spec_clause_origin_id(spec, clause_index))
        .flatten()
        .map(|origin_id| origin_id as u64 + 1)
}

fn candidates_for_clause(spec: &SimSpecState, clause_index: usize) -> Candidates {
    let current = &spec.clauses[clause_index];
    let texts = current.candidate_texts();
    let suffix = spec_join_display(&spec.clauses[(clause_index + 1)..]);
    candidates_owned(
        texts.clone(),
        vec![suffix; texts.len()],
        spec_join_raw_hiragana(&spec.clauses[clause_index..]),
        vec![current.corresponding_count(); texts.len()],
    )
}

fn split_units_by_offset(units: &[SimUnit], offset: i32) -> Option<(Vec<SimUnit>, Vec<SimUnit>)> {
    if offset <= 0 {
        return None;
    }

    let mut consumed = Vec::new();
    let mut remainder = Vec::new();
    let mut seen = 0;
    let mut split_found = false;

    for unit in units.iter().cloned() {
        if !split_found {
            let next_seen = seen + unit.raw_input.chars().count() as i32;
            if next_seen <= offset {
                consumed.push(unit);
                seen = next_seen;
                if seen == offset {
                    split_found = true;
                }
                continue;
            }
        }

        remainder.push(unit);
    }

    (split_found && !consumed.is_empty() && !remainder.is_empty()).then_some((consumed, remainder))
}

fn build_clause_snapshot_from_spec(spec: &SimSpecState, clause_index: usize) -> ClauseSnapshot {
    let fixed_prefix = spec_join_display(&spec.clauses[..clause_index]);
    let clause = &spec.clauses[clause_index];
    let preview =
        TextServiceFactory::merge_preview_with_prefix(&fixed_prefix, &clause.selected_text());
    let suffix = spec_join_display(&spec.clauses[(clause_index + 1)..]);
    let raw_input = spec_join_raw_input(&spec.clauses[clause_index..]);
    let raw_hiragana = spec_join_raw_hiragana(&spec.clauses[clause_index..]);
    let candidates = candidates_for_clause(spec, clause_index);

    let mut snapshot = TextServiceFactory::build_clause_snapshot(
        &preview,
        &suffix,
        &raw_input,
        &raw_hiragana,
        &fixed_prefix,
        clause.corresponding_count(),
        clause.selected_candidate as i32,
        spec_clause_is_split_derived(spec, clause_index),
        spec_clause_has_split_left_neighbor(spec, clause_index),
        &candidates,
    );
    snapshot.is_direct_split_remainder = spec_clause_is_direct_split_remainder(spec, clause_index);
    snapshot.split_group_id = spec_clause_split_group_id(spec, clause_index);
    snapshot
}

fn build_future_snapshot_from_spec(
    spec: &SimSpecState,
    clause_index: usize,
) -> FutureClauseSnapshot {
    let fixed_prefix = spec_join_display(&spec.clauses[..clause_index]);
    let clause = &spec.clauses[clause_index];
    let preview =
        TextServiceFactory::merge_preview_with_prefix(&fixed_prefix, &clause.selected_text());
    let suffix = spec_join_display(&spec.clauses[(clause_index + 1)..]);
    let raw_input = spec_join_raw_input(&spec.clauses[clause_index..]);
    let raw_hiragana = spec_join_raw_hiragana(&spec.clauses[clause_index..]);
    let candidates = candidates_for_clause(spec, clause_index);

    let mut snapshot = TextServiceFactory::build_future_clause_snapshot(
        &preview,
        &suffix,
        &raw_input,
        &raw_hiragana,
        &fixed_prefix,
        clause.corresponding_count(),
        clause.selected_candidate as i32,
        &candidates,
    );
    snapshot.is_split_derived = spec_clause_is_split_derived(spec, clause_index);
    snapshot.is_direct_split_remainder = spec_clause_is_direct_split_remainder(spec, clause_index);
    snapshot.has_split_left_neighbor = spec_clause_has_split_left_neighbor(spec, clause_index);
    snapshot.split_group_id = spec_clause_split_group_id(spec, clause_index);
    snapshot
}

fn build_harness_from_spec(spec: &SimSpecState, state: CompositionState) -> ClauseHarness {
    let fixed_prefix = spec_join_display(&spec.clauses[..spec.current_index]);
    let current = &spec.clauses[spec.current_index];
    let preview =
        TextServiceFactory::merge_preview_with_prefix(&fixed_prefix, &current.selected_text());
    let suffix = spec_join_display(&spec.clauses[(spec.current_index + 1)..]);
    let raw_input = spec_join_raw_input(&spec.clauses[spec.current_index..]);
    let raw_hiragana = spec_join_raw_hiragana(&spec.clauses[spec.current_index..]);
    let candidates = candidates_for_clause(spec, spec.current_index);
    let clause_snapshots = (0..spec.current_index)
        .map(|clause_index| build_clause_snapshot_from_spec(spec, clause_index))
        .collect::<Vec<_>>();
    let future_clause_snapshots = ((spec.current_index + 1)..spec.clauses.len())
        .rev()
        .map(|clause_index| build_future_snapshot_from_spec(spec, clause_index))
        .collect::<Vec<_>>();
    let next_split_group_id = (0..spec.clauses.len())
        .filter_map(|clause_index| spec_clause_split_group_id(spec, clause_index))
        .max()
        .unwrap_or(0)
        + 1;

    ClauseHarness {
        committed_clauses: spec.committed_clauses.clone(),
        state,
        preview,
        suffix,
        raw_input,
        raw_hiragana,
        fixed_prefix,
        corresponding_count: current.corresponding_count(),
        selection_index: current.selected_candidate as i32,
        current_clause_is_split_derived: spec_clause_is_split_derived(spec, spec.current_index),
        current_clause_is_direct_split_remainder: spec_clause_is_direct_split_remainder(
            spec,
            spec.current_index,
        ),
        current_clause_has_split_left_neighbor: spec_clause_has_split_left_neighbor(
            spec,
            spec.current_index,
        ),
        current_clause_split_group_id: spec_clause_split_group_id(spec, spec.current_index),
        candidates,
        clause_snapshots,
        future_clause_snapshots,
        next_split_group_id,
    }
}

fn build_logged_baseline_harness(spec: &SimSpecState, state: CompositionState) -> ClauseHarness {
    let first_candidates = candidates_for_clause(spec, 0);
    let second_candidates = candidates_for_clause(spec, 1);
    let third_candidates = candidates_for_clause(spec, 2);
    let current_candidates = candidates_for_clause(spec, 3);

    let mut first_snapshot = TextServiceFactory::build_clause_snapshot(
        "いい",
        "加減統一しろ",
        "iikagentouitusiro",
        "いいかげんとういつしろ",
        "",
        2,
        0,
        false,
        false,
        &first_candidates,
    );
    first_snapshot.split_group_id = None;

    let mut second_snapshot = TextServiceFactory::build_clause_snapshot(
        "いい加減",
        "統一しろ",
        "kagentouitusiro",
        "かげんとういつしろ",
        "いい",
        5,
        0,
        true,
        false,
        &second_candidates,
    );
    second_snapshot.split_group_id = Some(1);

    let mut third_snapshot = TextServiceFactory::build_clause_snapshot(
        "いい加減統一",
        "しろ",
        "touitusiro",
        "とういつしろ",
        "いい加減",
        6,
        0,
        true,
        true,
        &third_candidates,
    );
    third_snapshot.split_group_id = Some(1);

    ClauseHarness {
        committed_clauses: spec.committed_clauses.clone(),
        state,
        preview: "いい加減統一しろ".to_string(),
        suffix: String::new(),
        raw_input: "siro".to_string(),
        raw_hiragana: "しろ".to_string(),
        fixed_prefix: "いい加減統一".to_string(),
        corresponding_count: 4,
        selection_index: 0,
        current_clause_is_split_derived: true,
        current_clause_is_direct_split_remainder: true,
        current_clause_has_split_left_neighbor: true,
        current_clause_split_group_id: Some(1),
        candidates: current_candidates,
        clause_snapshots: vec![first_snapshot, second_snapshot, third_snapshot],
        future_clause_snapshots: Vec::new(),
        next_split_group_id: 2,
    }
}

fn baseline_spec_state_with_current_index(current_index: usize) -> SimSpecState {
    let mut spec = baseline_spec_state();
    spec.current_index = current_index.min(spec.clauses.len().saturating_sub(1));
    spec
}

pub(super) fn harness_visible_clauses(harness: &ClauseHarness) -> String {
    let mut clauses = harness
        .committed_clauses
        .iter()
        .map(|clause| clause.display.clone())
        .collect::<Vec<_>>();
    let base = TextServiceFactory::clause_texts_for_log(
        &harness.preview,
        &harness.fixed_prefix,
        &harness.clause_snapshots,
        &harness.future_clause_snapshots,
    );
    if !harness.future_clause_snapshots.is_empty() || harness.suffix.is_empty() {
        if !base.is_empty() {
            clauses.extend(base.split(" / ").map(str::to_string));
        }
        return clauses.join(" / ");
    }

    if harness.current_clause_is_direct_split_remainder {
        if harness.clause_snapshots.is_empty() {
            let current = format!(
                "{}{}",
                TextServiceFactory::current_clause_preview(&harness.preview, &harness.fixed_prefix),
                harness.suffix
            );
            clauses.push(current);
            return clauses.join(" / ");
        }

        let mut parts: Vec<_> = base.split(" / ").map(str::to_string).collect();
        if let Some(last) = parts.last_mut() {
            last.push_str(&harness.suffix);
        }
        clauses.extend(parts);
        return clauses.join(" / ");
    }

    if base.is_empty() {
        clauses.push(harness.suffix.clone());
    } else {
        clauses.extend(base.split(" / ").map(str::to_string));
        clauses.push(harness.suffix.clone());
    }
    clauses.join(" / ")
}

pub(super) fn harness_clause_input_lengths(harness: &ClauseHarness) -> String {
    let mut clause_lengths = harness
        .committed_clauses
        .iter()
        .map(|clause| clause.corresponding_count.to_string())
        .collect::<Vec<_>>();
    let base = TextServiceFactory::clause_input_lengths_for_log(
        harness.corresponding_count,
        &harness.clause_snapshots,
        &harness.future_clause_snapshots,
    );
    if !harness.future_clause_snapshots.is_empty() || harness.suffix.is_empty() {
        if !base.is_empty() {
            clause_lengths.extend(base.split(" / ").map(str::to_string));
        }
        return clause_lengths.join(" / ");
    }

    let raw_input_suffix: String = harness
        .raw_input
        .chars()
        .skip(harness.corresponding_count.max(0) as usize)
        .collect();
    let trailing_len = if raw_input_suffix.is_empty() {
        harness.suffix.chars().count()
    } else {
        raw_input_suffix.chars().count()
    };

    if harness.current_clause_is_direct_split_remainder {
        if harness.clause_snapshots.is_empty() {
            clause_lengths.push((harness.corresponding_count + trailing_len as i32).to_string());
            return clause_lengths.join(" / ");
        }

        let mut parts: Vec<_> = base.split(" / ").map(str::to_string).collect();
        if let Some(last) = parts.last_mut() {
            let current = last.parse::<i32>().unwrap_or_default();
            *last = (current + trailing_len as i32).to_string();
        }
        clause_lengths.extend(parts);
        return clause_lengths.join(" / ");
    }

    if base.is_empty() {
        clause_lengths.push(trailing_len.to_string());
    } else {
        clause_lengths.extend(base.split(" / ").map(str::to_string));
        clause_lengths.push(trailing_len.to_string());
    }
    clause_lengths.join(" / ")
}

pub(super) fn harness_raw_clauses(harness: &ClauseHarness) -> String {
    let mut clauses = harness
        .committed_clauses
        .iter()
        .map(|clause| clause.raw_hiragana.clone())
        .collect::<Vec<_>>();
    let base = TextServiceFactory::clause_raw_texts_for_log(
        &harness.raw_hiragana,
        harness.corresponding_count,
        &harness.clause_snapshots,
        &harness.future_clause_snapshots,
    );
    if !harness.future_clause_snapshots.is_empty() || harness.suffix.is_empty() {
        if !base.is_empty() {
            clauses.extend(base.split(" / ").map(str::to_string));
        }
        return clauses.join(" / ");
    }

    let trailing = harness.suffix.clone();
    let current_raw = harness
        .raw_hiragana
        .strip_suffix(&trailing)
        .unwrap_or(&harness.raw_hiragana)
        .to_string();
    if harness.current_clause_is_direct_split_remainder {
        if harness.clause_snapshots.is_empty() {
            clauses.push(format!("{current_raw}{trailing}"));
            return clauses.join(" / ");
        }

        let mut parts: Vec<_> = base.split(" / ").map(str::to_string).collect();
        if let Some(last) = parts.last_mut() {
            *last = format!("{current_raw}{trailing}");
        }
        clauses.extend(parts);
        return clauses.join(" / ");
    }
    let adjusted_base = if harness.clause_snapshots.is_empty() {
        current_raw
    } else {
        let mut parts: Vec<_> = base.split(" / ").map(str::to_string).collect();
        if let Some(last) = parts.last_mut() {
            *last = current_raw;
        }
        parts.join(" / ")
    };

    if adjusted_base.is_empty() {
        clauses.push(trailing);
    } else {
        clauses.extend(adjusted_base.split(" / ").map(str::to_string));
        clauses.push(trailing);
    }
    clauses.join(" / ")
}

fn harness_key(harness: &ClauseHarness) -> HarnessStateKey {
    HarnessStateKey {
        state: match harness.state {
            CompositionState::None => "None",
            CompositionState::Composing => "Composing",
            CompositionState::Previewing => "Previewing",
            CompositionState::Selecting => "Selecting",
        },
        preview: harness.preview.clone(),
        suffix: harness.suffix.clone(),
        raw_input: harness.raw_input.clone(),
        raw_hiragana: harness.raw_hiragana.clone(),
        fixed_prefix: harness.fixed_prefix.clone(),
        corresponding_count: harness.corresponding_count,
        selection_index: harness.selection_index,
        current_clause_is_split_derived: harness.current_clause_is_split_derived,
        current_clause_is_direct_split_remainder: harness.current_clause_is_direct_split_remainder,
        current_clause_has_split_left_neighbor: harness.current_clause_has_split_left_neighbor,
        current_clause_split_group_id: harness.current_clause_split_group_id,
        candidates: TextServiceFactory::debug_candidates(
            &harness.candidates,
            harness.selection_index,
        ),
        clause_snapshots: TextServiceFactory::debug_clause_snapshots(&harness.clause_snapshots),
        future_clause_snapshots: TextServiceFactory::debug_future_clause_snapshots(
            &harness.future_clause_snapshots,
        ),
        committed_clauses: harness
            .committed_clauses
            .iter()
            .map(|clause| {
                format!(
                    "{}|{}|{}",
                    clause.display, clause.raw_hiragana, clause.corresponding_count
                )
            })
            .collect::<Vec<_>>()
            .join(" ; "),
        clauses: harness_visible_clauses(harness),
        clauses_raw: harness_raw_clauses(harness),
    }
}

fn harness_as_composition(harness: &ClauseHarness) -> Composition {
    Composition {
        preview: harness.preview.clone(),
        suffix: harness.suffix.clone(),
        raw_input: harness.raw_input.clone(),
        raw_hiragana: harness.raw_hiragana.clone(),
        fixed_prefix: harness.fixed_prefix.clone(),
        corresponding_count: harness.corresponding_count,
        selection_index: harness.selection_index,
        candidates: harness.candidates.clone(),
        clause_snapshots: harness.clause_snapshots.clone(),
        future_clause_snapshots: harness.future_clause_snapshots.clone(),
        current_clause_is_split_derived: harness.current_clause_is_split_derived,
        current_clause_is_direct_split_remainder: harness.current_clause_is_direct_split_remainder,
        current_clause_has_split_left_neighbor: harness.current_clause_has_split_left_neighbor,
        current_clause_split_group_id: harness.current_clause_split_group_id,
        next_split_group_id: harness.next_split_group_id,
        state: harness.state.clone(),
        ..Composition::default()
    }
}

fn committed_clause_from_snapshot(
    snapshot: &ClauseSnapshot,
    next_raw_hiragana: Option<&str>,
) -> SimCommittedClause {
    SimCommittedClause {
        display: TextServiceFactory::current_clause_preview(
            &snapshot.preview,
            &snapshot.fixed_prefix,
        ),
        raw_hiragana: TextServiceFactory::clause_raw_preview(
            &snapshot.raw_hiragana,
            next_raw_hiragana,
            snapshot.corresponding_count,
        ),
        corresponding_count: snapshot.corresponding_count,
    }
}

fn committed_clause_from_future_snapshot(
    snapshot: &FutureClauseSnapshot,
    next_raw_hiragana: Option<&str>,
) -> SimCommittedClause {
    SimCommittedClause {
        display: snapshot.clause_preview.clone(),
        raw_hiragana: TextServiceFactory::clause_raw_preview(
            &snapshot.raw_hiragana,
            next_raw_hiragana,
            snapshot.corresponding_count,
        ),
        corresponding_count: snapshot.corresponding_count,
    }
}

fn committed_current_clause_from_harness(harness: &ClauseHarness) -> SimCommittedClause {
    SimCommittedClause {
        display: TextServiceFactory::current_clause_preview(
            &harness.preview,
            &harness.fixed_prefix,
        ),
        raw_hiragana: TextServiceFactory::current_clause_raw_hiragana_preview(
            &harness.raw_hiragana,
            harness.corresponding_count,
            &harness.future_clause_snapshots,
        ),
        corresponding_count: harness.corresponding_count,
    }
}

impl ScenarioBackend {
    fn new(spec: SimSpecState) -> Self {
        Self {
            server: spec.clone(),
            spec,
            server_snapshots: Vec::new(),
            blocked_boundary: false,
        }
    }

    fn from_baseline_fixture() -> Self {
        Self {
            server: baseline_spec_state(),
            spec: baseline_spec_state(),
            server_snapshots: vec![
                baseline_spec_state_with_current_index(0),
                baseline_spec_state_with_current_index(1),
                baseline_spec_state_with_current_index(2),
            ],
            blocked_boundary: false,
        }
    }

    fn current_candidates(&self) -> Candidates {
        if self.server.clauses.get(self.server.current_index).is_none() {
            return Candidates::default();
        }
        candidates_for_clause(&self.server, self.server.current_index)
    }

    fn sync_current_selection(&mut self, selection_index: i32) {
        if let Some(current) = self.spec.clauses.get_mut(self.spec.current_index) {
            let max_index = current.candidate_texts().len().saturating_sub(1) as i32;
            let previous = current.selected_candidate;
            current.selected_candidate = selection_index.clamp(0, max_index) as usize;
            current.clamp_selection();
            if current.selected_candidate != previous {
                current.display_override = None;
            }
        }
    }

    fn split_spec_left(&mut self) {
        let current_index = self.spec.current_index;
        let can_split = self
            .spec
            .clauses
            .get(current_index)
            .map(SimClause::can_split_left)
            .unwrap_or(false);
        if !can_split {
            self.blocked_boundary = true;
            return;
        }

        self.blocked_boundary = false;
        let moved_unit = self.spec.clauses[current_index]
            .units
            .pop()
            .expect("split source unit");
        self.spec.clauses[current_index].selected_candidate = 0;
        self.spec.clauses[current_index].display_override = None;
        self.spec.clauses[current_index].clamp_selection();

        if let Some(next_clause) = self.spec.clauses.get_mut(current_index + 1) {
            if next_clause.pending_remainder
                || next_clause.uniform_origin_id() == Some(moved_unit.origin_id)
            {
                next_clause.units.insert(0, moved_unit);
                next_clause.selected_candidate = 0;
                next_clause.clamp_selection();
                return;
            }
        }

        self.spec
            .clauses
            .insert(current_index + 1, pending_clause(vec![moved_unit]));
    }

    fn split_server_left(&mut self) {
        let current_index = self.server.current_index;
        let can_split = self
            .server
            .clauses
            .get(current_index)
            .map(SimClause::can_split_left)
            .unwrap_or(false);
        if !can_split {
            self.blocked_boundary = true;
            return;
        }

        self.blocked_boundary = false;
        let moved_unit = self.server.clauses[current_index]
            .units
            .pop()
            .expect("server split source unit");
        self.server.clauses[current_index].selected_candidate = 0;
        self.server.clauses[current_index].clamp_selection();

        if current_index == 0 && self.server.clauses.len() > 1 {
            self.server
                .clauses
                .insert(current_index + 1, pending_clause(vec![moved_unit]));
            return;
        }

        let mut collapsed_units = vec![moved_unit];
        let trailing_clauses: Vec<_> = self.server.clauses.drain((current_index + 1)..).collect();
        for clause in trailing_clauses {
            collapsed_units.extend(clause.units);
        }
        self.server
            .clauses
            .insert(current_index + 1, pending_clause(collapsed_units));
    }

    fn move_spec_right(&mut self) {
        if self.spec.current_index + 1 >= self.spec.clauses.len() {
            return;
        }

        self.spec.current_index += 1;
        if let Some(current) = self.spec.clauses.get_mut(self.spec.current_index) {
            current.pending_remainder = false;
        }
    }

    fn move_spec_left(&mut self) {
        if self.spec.current_index > 0 {
            self.spec.current_index -= 1;
        }
    }

    fn step_spec_selection(&mut self, delta: i32) {
        let Some(current) = self.spec.clauses.get_mut(self.spec.current_index) else {
            return;
        };
        let max_index = current.candidate_texts().len().saturating_sub(1) as i32;
        let next = (current.selected_candidate as i32 + delta).clamp(0, max_index) as usize;
        current.selected_candidate = next;
        current.display_override = None;
        current.clamp_selection();
    }

    fn commit_spec_all_clauses(&mut self) {
        if self.spec.clauses.is_empty() || self.spec.current_index >= self.spec.clauses.len() {
            return;
        }

        let committed = self.spec.clauses.drain(..).collect::<Vec<_>>();
        self.spec
            .committed_clauses
            .extend(committed.into_iter().map(|clause| SimCommittedClause {
                display: clause.selected_text(),
                raw_hiragana: clause.raw_hiragana(),
                corresponding_count: clause.corresponding_count(),
            }));

        if self.spec.clauses.is_empty() {
            self.spec.current_index = 0;
        } else {
            self.spec.current_index = 0;
            if let Some(current) = self.spec.clauses.get_mut(0) {
                current.pending_remainder = false;
            }
        }
    }

    fn apply_expected_user_action(&mut self, op: HarnessUserAction) {
        match op {
            HarnessUserAction::Left => self.move_spec_left(),
            HarnessUserAction::Right => self.move_spec_right(),
            HarnessUserAction::ShiftLeft => self.split_spec_left(),
            HarnessUserAction::Space => {}
            HarnessUserAction::Enter => self.commit_spec_all_clauses(),
            HarnessUserAction::SetTextType(set_type) => {
                if let Some(current) = self.spec.clauses.get_mut(self.spec.current_index) {
                    current.display_override = Some(set_type);
                }
            }
        }
    }
}

impl ClauseActionBackend for ScenarioBackend {
    fn move_cursor(&mut self, offset: i32) -> anyhow::Result<Candidates> {
        match offset {
            value if value == TextServiceFactory::MOVE_CURSOR_CLEAR_CLAUSE_SNAPSHOTS => {
                self.server_snapshots.clear();
                self.blocked_boundary = false;
                Ok(self.current_candidates())
            }
            value if value == TextServiceFactory::MOVE_CURSOR_PUSH_CLAUSE_SNAPSHOT => {
                self.server_snapshots.push(self.server.clone());
                Ok(self.current_candidates())
            }
            value if value == TextServiceFactory::MOVE_CURSOR_POP_CLAUSE_SNAPSHOT => {
                if let Some(restored) = self.server_snapshots.pop() {
                    self.server = restored;
                }
                self.blocked_boundary = false;
                Ok(self.current_candidates())
            }
            0 => {
                if self.blocked_boundary {
                    Ok(Candidates::default())
                } else {
                    Ok(self.current_candidates())
                }
            }
            -1 => {
                self.split_server_left();
                Ok(self.current_candidates())
            }
            1 => {
                self.blocked_boundary = false;
                Ok(self.current_candidates())
            }
            _ => Ok(self.current_candidates()),
        }
    }

    fn shrink_text(&mut self, _offset: i32) -> anyhow::Result<Candidates> {
        let Some(current) = self.server.clauses.get(self.server.current_index).cloned() else {
            return Ok(Candidates::default());
        };

        if let Some((_, remainder_units)) = split_units_by_offset(&current.units, _offset) {
            self.server.clauses[self.server.current_index] = clause(remainder_units);
            self.server.clauses[self.server.current_index].pending_remainder = false;
            self.blocked_boundary = false;
            return Ok(self.current_candidates());
        }

        if self.server.current_index + 1 >= self.server.clauses.len() {
            return Ok(Candidates::default());
        }

        self.server.current_index += 1;
        if let Some(current) = self.server.clauses.get_mut(self.server.current_index) {
            current.pending_remainder = false;
        }
        self.blocked_boundary = false;
        Ok(self.current_candidates())
    }
}

fn op_name(op: HarnessUserAction) -> &'static str {
    match op {
        HarnessUserAction::Left => "Left",
        HarnessUserAction::Right => "Right",
        HarnessUserAction::ShiftLeft => "ShiftLeft",
        HarnessUserAction::Space => "Space",
        HarnessUserAction::Enter => "Enter",
        HarnessUserAction::SetTextType(SetTextType::Hiragana) => "F6",
        HarnessUserAction::SetTextType(SetTextType::Katakana) => "F7",
        HarnessUserAction::SetTextType(SetTextType::HalfKatakana) => "F8",
        HarnessUserAction::SetTextType(SetTextType::FullLatin) => "F9",
        HarnessUserAction::SetTextType(SetTextType::HalfLatin) => "F10",
    }
}

pub(super) fn history_string(history: &[HarnessUserAction]) -> String {
    history
        .iter()
        .map(|op| op_name(*op))
        .collect::<Vec<_>>()
        .join(" -> ")
}

fn harness_user_action(op: HarnessUserAction) -> UserAction {
    match op {
        HarnessUserAction::Left => UserAction::Navigation(Navigation::Left),
        HarnessUserAction::Right => UserAction::Navigation(Navigation::Right),
        HarnessUserAction::ShiftLeft => UserAction::AdjustClauseBoundary(-1),
        HarnessUserAction::Space => UserAction::Space,
        HarnessUserAction::Enter => UserAction::Enter,
        HarnessUserAction::SetTextType(SetTextType::Hiragana) => {
            UserAction::Function(Function::Six)
        }
        HarnessUserAction::SetTextType(SetTextType::Katakana) => {
            UserAction::Function(Function::Seven)
        }
        HarnessUserAction::SetTextType(SetTextType::HalfKatakana) => {
            UserAction::Function(Function::Eight)
        }
        HarnessUserAction::SetTextType(SetTextType::FullLatin) => {
            UserAction::Function(Function::Nine)
        }
        HarnessUserAction::SetTextType(SetTextType::HalfLatin) => {
            UserAction::Function(Function::Ten)
        }
    }
}

fn assert_harness_matches_spec(
    harness: &ClauseHarness,
    spec: &SimSpecState,
    history: &[HarnessUserAction],
) {
    assert_eq!(
        harness_visible_clauses(harness),
        spec_display(spec),
        "history: {}\nharness clauses: {}\nexpected clauses: {}\nharness lengths: {}\nexpected lengths: {}\nclause_snapshots: {}\nfuture_clause_snapshots: {}\npreview={} fixed_prefix={} suffix={} raw_input={} raw_hiragana={} corresponding_count={} selection_index={}",
        history_string(history),
        harness_visible_clauses(harness),
        spec_display(spec),
        harness_clause_input_lengths(harness),
        spec_clause_input_lengths(spec),
        TextServiceFactory::debug_clause_snapshots(&harness.clause_snapshots),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
        harness.preview,
        harness.fixed_prefix,
        harness.suffix,
        harness.raw_input,
        harness.raw_hiragana,
        harness.corresponding_count,
        harness.selection_index,
    );
    assert_eq!(
        harness_clause_input_lengths(harness),
        spec_clause_input_lengths(spec),
        "history: {}\nharness clauses: {}\nexpected clauses: {}\nharness raw clauses: {}\nexpected raw clauses: {}\nharness lengths: {}\nexpected lengths: {}\nclause_snapshots: {}\nfuture_clause_snapshots: {}\npreview={} fixed_prefix={} suffix={} raw_input={} raw_hiragana={} corresponding_count={} selection_index={}",
        history_string(history),
        harness_visible_clauses(harness),
        spec_display(spec),
        harness_raw_clauses(harness),
        spec_display_raw(spec),
        harness_clause_input_lengths(harness),
        spec_clause_input_lengths(spec),
        TextServiceFactory::debug_clause_snapshots(&harness.clause_snapshots),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
        harness.preview,
        harness.fixed_prefix,
        harness.suffix,
        harness.raw_input,
        harness.raw_hiragana,
        harness.corresponding_count,
        harness.selection_index,
    );
    assert_eq!(
        harness_raw_clauses(harness).replace(" / ", ""),
        spec_display_raw(spec).replace(" / ", ""),
        "history: {}\nharness clauses: {}\nexpected clauses: {}\nharness raw clauses: {}\nexpected raw clauses: {}\nharness lengths: {}\nexpected lengths: {}\nclause_snapshots: {}\nfuture_clause_snapshots: {}\npreview={} fixed_prefix={} suffix={} raw_input={} raw_hiragana={} corresponding_count={} selection_index={}",
        history_string(history),
        harness_visible_clauses(harness),
        spec_display(spec),
        harness_raw_clauses(harness),
        spec_display_raw(spec),
        harness_clause_input_lengths(harness),
        spec_clause_input_lengths(spec),
        TextServiceFactory::debug_clause_snapshots(&harness.clause_snapshots),
        TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
        harness.preview,
        harness.fixed_prefix,
        harness.suffix,
        harness.raw_input,
        harness.raw_hiragana,
        harness.corresponding_count,
        harness.selection_index,
    );
    if let Some(current_clause) = spec.clauses.get(spec.current_index) {
        assert_eq!(
            TextServiceFactory::current_clause_preview(&harness.preview, &harness.fixed_prefix),
            current_clause.selected_text(),
            "history: {}\nharness clauses: {}\nexpected clauses: {}\nharness lengths: {}\nexpected lengths: {}\nclause_snapshots: {}\nfuture_clause_snapshots: {}\npreview={} fixed_prefix={} suffix={} raw_input={} raw_hiragana={} corresponding_count={} selection_index={}",
            history_string(history),
            harness_visible_clauses(harness),
            spec_display(spec),
            harness_clause_input_lengths(harness),
            spec_clause_input_lengths(spec),
            TextServiceFactory::debug_clause_snapshots(&harness.clause_snapshots),
            TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
            harness.preview,
            harness.fixed_prefix,
            harness.suffix,
            harness.raw_input,
            harness.raw_hiragana,
            harness.corresponding_count,
            harness.selection_index,
        );
        assert_eq!(
            harness.selection_index,
            current_clause.selected_candidate as i32,
            "history: {}\nharness clauses: {}\nexpected clauses: {}\nharness lengths: {}\nexpected lengths: {}\nclause_snapshots: {}\nfuture_clause_snapshots: {}\npreview={} fixed_prefix={} suffix={} raw_input={} raw_hiragana={} corresponding_count={} selection_index={}",
            history_string(history),
            harness_visible_clauses(harness),
            spec_display(spec),
            harness_clause_input_lengths(harness),
            spec_clause_input_lengths(spec),
            TextServiceFactory::debug_clause_snapshots(&harness.clause_snapshots),
            TextServiceFactory::debug_future_clause_snapshots(&harness.future_clause_snapshots),
            harness.preview,
            harness.fixed_prefix,
            harness.suffix,
            harness.raw_input,
            harness.raw_hiragana,
            harness.corresponding_count,
            harness.selection_index,
        );
    }
}

fn apply_user_action(
    harness: &mut ClauseHarness,
    backend: &mut ScenarioBackend,
    op: HarnessUserAction,
    history: &[HarnessUserAction],
) {
    let composition = harness_as_composition(harness);
    let app_config = AppConfig::default();
    let (transition, actions) = TextServiceFactory::plan_actions_for_user_action(
        &composition,
        &harness_user_action(op),
        &crate::engine::input_mode::InputMode::Kana,
        false,
        &app_config,
        false,
    )
    .unwrap_or_else(|| {
        panic!(
            "no actions planned for history: {}",
            history_string(history)
        )
    });

    for action in actions {
        match action {
            ClientAction::MoveClause(direction) => {
                backend.sync_current_selection(harness.selection_index);
                let effect = {
                    let mut state = ClauseActionStateMut {
                        preview: &mut harness.preview,
                        suffix: &mut harness.suffix,
                        raw_input: &mut harness.raw_input,
                        raw_hiragana: &mut harness.raw_hiragana,
                        fixed_prefix: &mut harness.fixed_prefix,
                        corresponding_count: &mut harness.corresponding_count,
                        selection_index: &mut harness.selection_index,
                        candidates: &mut harness.candidates,
                        clause_snapshots: &mut harness.clause_snapshots,
                        future_clause_snapshots: &mut harness.future_clause_snapshots,
                        current_clause_is_split_derived: &mut harness
                            .current_clause_is_split_derived,
                        current_clause_is_direct_split_remainder: &mut harness
                            .current_clause_is_direct_split_remainder,
                        current_clause_has_split_left_neighbor: &mut harness
                            .current_clause_has_split_left_neighbor,
                        current_clause_split_group_id: &mut harness.current_clause_split_group_id,
                        next_split_group_id: &mut harness.next_split_group_id,
                    };
                    TextServiceFactory::apply_move_clause(&mut state, backend, direction)
                        .expect("apply_move_clause")
                };
                assert!(
                    effect.applied,
                    "MoveClause unexpectedly skipped: {}\nharness clauses: {}\nspec clauses: {}\nharness raw clauses: {}\nspec raw clauses: {}\nfuture_clause_snapshots: {}\npreview={} fixed_prefix={} suffix={} raw_input={} raw_hiragana={} corresponding_count={} selection_index={}",
                    history_string(history),
                    harness_visible_clauses(harness),
                    spec_display(&backend.spec),
                    harness_raw_clauses(harness),
                    spec_display_raw(&backend.spec),
                    TextServiceFactory::debug_future_clause_snapshots(
                        &harness.future_clause_snapshots
                    ),
                    harness.preview,
                    harness.fixed_prefix,
                    harness.suffix,
                    harness.raw_input,
                    harness.raw_hiragana,
                    harness.corresponding_count,
                    harness.selection_index,
                );
            }
            ClientAction::AdjustBoundary(direction) => {
                let effect = {
                    let mut state = ClauseActionStateMut {
                        preview: &mut harness.preview,
                        suffix: &mut harness.suffix,
                        raw_input: &mut harness.raw_input,
                        raw_hiragana: &mut harness.raw_hiragana,
                        fixed_prefix: &mut harness.fixed_prefix,
                        corresponding_count: &mut harness.corresponding_count,
                        selection_index: &mut harness.selection_index,
                        candidates: &mut harness.candidates,
                        clause_snapshots: &mut harness.clause_snapshots,
                        future_clause_snapshots: &mut harness.future_clause_snapshots,
                        current_clause_is_split_derived: &mut harness
                            .current_clause_is_split_derived,
                        current_clause_is_direct_split_remainder: &mut harness
                            .current_clause_is_direct_split_remainder,
                        current_clause_has_split_left_neighbor: &mut harness
                            .current_clause_has_split_left_neighbor,
                        current_clause_split_group_id: &mut harness.current_clause_split_group_id,
                        next_split_group_id: &mut harness.next_split_group_id,
                    };
                    TextServiceFactory::apply_adjust_boundary(&mut state, backend, direction)
                        .expect("apply_adjust_boundary")
                };
                assert!(
                    effect.applied,
                    "AdjustBoundary unexpectedly skipped: {}\nharness clauses: {}\nspec clauses: {}\nserver clauses: {}\nharness raw clauses: {}\nspec raw clauses: {}\nserver raw clauses: {}\ncandidates: {}\nserver candidates: {}\nfuture_clause_snapshots: {}\npreview={} fixed_prefix={} suffix={} raw_input={} raw_hiragana={} corresponding_count={} selection_index={} spec_current_index={} server_current_index={}",
                    history_string(history),
                    harness_visible_clauses(harness),
                    spec_display(&backend.spec),
                    spec_display(&backend.server),
                    harness_raw_clauses(harness),
                    spec_display_raw(&backend.spec),
                    spec_display_raw(&backend.server),
                    TextServiceFactory::debug_candidates(
                        &harness.candidates,
                        harness.selection_index
                    ),
                    TextServiceFactory::debug_candidates(
                        &backend.current_candidates(),
                        backend
                            .server
                            .clauses
                            .get(backend.server.current_index)
                            .map(|clause| clause.selected_candidate as i32)
                            .unwrap_or_default()
                    ),
                    TextServiceFactory::debug_future_clause_snapshots(
                        &harness.future_clause_snapshots
                    ),
                    harness.preview,
                    harness.fixed_prefix,
                    harness.suffix,
                    harness.raw_input,
                    harness.raw_hiragana,
                    harness.corresponding_count,
                    harness.selection_index,
                    backend.spec.current_index,
                    backend.server.current_index,
                );
                backend.sync_current_selection(harness.selection_index);
            }
            ClientAction::SetSelection(selection) => {
                let effect = {
                    let mut state = ClauseActionStateMut {
                        preview: &mut harness.preview,
                        suffix: &mut harness.suffix,
                        raw_input: &mut harness.raw_input,
                        raw_hiragana: &mut harness.raw_hiragana,
                        fixed_prefix: &mut harness.fixed_prefix,
                        corresponding_count: &mut harness.corresponding_count,
                        selection_index: &mut harness.selection_index,
                        candidates: &mut harness.candidates,
                        clause_snapshots: &mut harness.clause_snapshots,
                        future_clause_snapshots: &mut harness.future_clause_snapshots,
                        current_clause_is_split_derived: &mut harness
                            .current_clause_is_split_derived,
                        current_clause_is_direct_split_remainder: &mut harness
                            .current_clause_is_direct_split_remainder,
                        current_clause_has_split_left_neighbor: &mut harness
                            .current_clause_has_split_left_neighbor,
                        current_clause_split_group_id: &mut harness.current_clause_split_group_id,
                        next_split_group_id: &mut harness.next_split_group_id,
                    };
                    TextServiceFactory::apply_set_selection(&mut state, &selection)
                };
                assert!(
                    effect.applied,
                    "SetSelection unexpectedly skipped: {}\nharness clauses: {}\nspec clauses: {}\nharness raw clauses: {}\nspec raw clauses: {}\nfuture_clause_snapshots: {}\npreview={} fixed_prefix={} suffix={} raw_input={} raw_hiragana={} corresponding_count={} selection_index={}",
                    history_string(history),
                    harness_visible_clauses(harness),
                    spec_display(&backend.spec),
                    harness_raw_clauses(harness),
                    spec_display_raw(&backend.spec),
                    TextServiceFactory::debug_future_clause_snapshots(
                        &harness.future_clause_snapshots
                    ),
                    harness.preview,
                    harness.fixed_prefix,
                    harness.suffix,
                    harness.raw_input,
                    harness.raw_hiragana,
                    harness.corresponding_count,
                    harness.selection_index,
                );
                backend.sync_current_selection(harness.selection_index);
            }
            ClientAction::StartComposition => {
                harness.state = CompositionState::Composing;
            }
            ClientAction::EndComposition => {
                let snapshot_count = harness.clause_snapshots.len();
                for index in 0..snapshot_count {
                    let next_raw_hiragana = harness
                        .clause_snapshots
                        .get(index + 1)
                        .map(|next| next.raw_hiragana.as_str())
                        .or_else(|| {
                            (!harness.raw_hiragana.is_empty())
                                .then_some(harness.raw_hiragana.as_str())
                        });
                    harness
                        .committed_clauses
                        .push(committed_clause_from_snapshot(
                            &harness.clause_snapshots[index],
                            next_raw_hiragana,
                        ));
                }
                if !harness.preview.is_empty() {
                    harness
                        .committed_clauses
                        .push(committed_current_clause_from_harness(harness));
                }
                let ordered_future = harness
                    .future_clause_snapshots
                    .iter()
                    .rev()
                    .collect::<Vec<_>>();
                for (index, snapshot) in ordered_future.iter().enumerate() {
                    let next_raw_hiragana = ordered_future
                        .get(index + 1)
                        .map(|next| next.raw_hiragana.as_str());
                    harness
                        .committed_clauses
                        .push(committed_clause_from_future_snapshot(
                            snapshot,
                            next_raw_hiragana,
                        ));
                }
                harness.preview.clear();
                harness.suffix.clear();
                harness.raw_input.clear();
                harness.raw_hiragana.clear();
                harness.fixed_prefix.clear();
                harness.corresponding_count = 0;
                harness.selection_index = 0;
                harness.candidates = Candidates::default();
                harness.clause_snapshots.clear();
                harness.future_clause_snapshots.clear();
                harness.current_clause_is_split_derived = false;
                harness.current_clause_is_direct_split_remainder = false;
                harness.current_clause_has_split_left_neighbor = false;
                harness.current_clause_split_group_id = None;
                harness.state = CompositionState::None;
            }
            ClientAction::ShrinkText(text) => {
                assert!(
                    text.is_empty(),
                    "unsupported non-empty ShrinkText in harness: {text:?}, history: {}",
                    history_string(history)
                );
                backend.sync_current_selection(harness.selection_index);

                let snapshot_count = harness.clause_snapshots.len();
                for index in 0..snapshot_count {
                    let next_raw_hiragana = harness
                        .clause_snapshots
                        .get(index + 1)
                        .map(|next| next.raw_hiragana.as_str())
                        .or_else(|| {
                            (!harness.raw_hiragana.is_empty())
                                .then_some(harness.raw_hiragana.as_str())
                        });
                    harness
                        .committed_clauses
                        .push(committed_clause_from_snapshot(
                            &harness.clause_snapshots[index],
                            next_raw_hiragana,
                        ));
                }
                harness
                    .committed_clauses
                    .push(committed_current_clause_from_harness(harness));

                harness.fixed_prefix.clear();
                harness.clause_snapshots.clear();
                harness.future_clause_snapshots.clear();
                harness.current_clause_is_split_derived = false;
                harness.current_clause_is_direct_split_remainder = false;
                harness.current_clause_has_split_left_neighbor = false;
                harness.current_clause_split_group_id = None;
                harness.raw_input = harness
                    .raw_input
                    .chars()
                    .skip(harness.corresponding_count.max(0) as usize)
                    .collect();

                harness.candidates = backend
                    .shrink_text(harness.corresponding_count)
                    .expect("harness shrink_text");
                harness.selection_index = 0;

                if let Some(selected) = TextServiceFactory::select_candidate(
                    &harness.candidates,
                    harness.selection_index,
                ) {
                    harness.selection_index = selected.index;
                    harness.corresponding_count = selected.corresponding_count;
                    harness.preview = selected.text.clone();
                    harness.suffix = selected.sub_text.clone();
                    harness.raw_hiragana = selected.hiragana;
                } else {
                    harness.preview.clear();
                    harness.suffix.clear();
                    harness.raw_hiragana.clear();
                    harness.corresponding_count = 0;
                }
            }
            ClientAction::SetTextWithType(set_type) => {
                let clause_raw_input = TextServiceFactory::current_clause_raw_input_preview(
                    &harness.raw_input,
                    harness.corresponding_count,
                    &harness.future_clause_snapshots,
                );
                let clause_raw_hiragana = TextServiceFactory::current_clause_raw_hiragana_preview(
                    &harness.raw_hiragana,
                    harness.corresponding_count,
                    &harness.future_clause_snapshots,
                );
                let converted_clause = TextServiceFactory::converted_clause_preview_text(
                    &set_type,
                    &clause_raw_input,
                    &clause_raw_hiragana,
                );
                harness.preview = TextServiceFactory::merge_preview_with_prefix(
                    &harness.fixed_prefix,
                    &converted_clause,
                );
                TextServiceFactory::sync_clause_snapshot_suffixes(
                    &mut harness.clause_snapshots,
                    &harness.preview,
                    &harness.suffix,
                );
            }
            unsupported => panic!(
                "unsupported client action in harness: {unsupported:?}, history: {}",
                history_string(history)
            ),
        }
    }

    backend.apply_expected_user_action(op);
    harness.state = transition;
}

pub(super) fn run_to_baseline() -> (ClauseHarness, ScenarioBackend, Vec<HarnessUserAction>) {
    let harness = build_harness_from_spec(&baseline_spec_state(), CompositionState::Composing);
    let backend = ScenarioBackend::from_baseline_fixture();
    let history = Vec::new();
    assert_harness_matches_spec(&harness, &backend.spec, &history);
    (harness, backend, history)
}

fn run_to_logged_baseline() -> (ClauseHarness, ScenarioBackend, Vec<HarnessUserAction>) {
    let harness =
        build_logged_baseline_harness(&baseline_spec_state(), CompositionState::Composing);
    let backend = ScenarioBackend::from_baseline_fixture();
    let history = Vec::new();
    (harness, backend, history)
}

fn run_to_fkey_baseline() -> (ClauseHarness, ScenarioBackend, Vec<HarnessUserAction>) {
    let spec = fkey_two_clause_spec_state();
    let harness = build_harness_from_spec(&spec, CompositionState::Composing);
    let backend = ScenarioBackend::new(spec);
    let history = Vec::new();
    (harness, backend, history)
}

pub(super) fn run_from_baseline(
    extra_actions: &[HarnessUserAction],
) -> (ClauseHarness, ScenarioBackend, Vec<HarnessUserAction>) {
    let (mut harness, mut backend, mut history) = run_to_baseline();
    for op in extra_actions {
        history.push(*op);
        apply_user_action(&mut harness, &mut backend, *op, &history);
        assert_harness_matches_spec(&harness, &backend.spec, &history);
    }

    (harness, backend, history)
}

pub(super) fn run_from_logged_baseline(
    extra_actions: &[HarnessUserAction],
) -> (ClauseHarness, ScenarioBackend, Vec<HarnessUserAction>) {
    let (mut harness, mut backend, mut history) = run_to_logged_baseline();
    for op in extra_actions {
        history.push(*op);
        apply_user_action(&mut harness, &mut backend, *op, &history);
    }

    (harness, backend, history)
}

pub(super) fn run_from_fkey_baseline(
    extra_actions: &[HarnessUserAction],
) -> (ClauseHarness, ScenarioBackend, Vec<HarnessUserAction>) {
    let (mut harness, mut backend, mut history) = run_to_fkey_baseline();
    assert_harness_matches_spec(&harness, &backend.spec, &history);

    for op in extra_actions {
        history.push(*op);
        apply_user_action(&mut harness, &mut backend, *op, &history);
        assert_harness_matches_spec(&harness, &backend.spec, &history);
    }

    (harness, backend, history)
}

pub(super) fn fkey_cases() -> [(SetTextType, &'static str); 5] {
    [
        (SetTextType::Hiragana, "かげん"),
        (SetTextType::Katakana, "カゲン"),
        (SetTextType::HalfKatakana, "ｶｹﾞﾝ"),
        (SetTextType::FullLatin, "ｋａｇｅｎ"),
        (SetTextType::HalfLatin, "kagen"),
    ]
}

fn is_available(harness: &ClauseHarness, spec: &SimSpecState, op: HarnessUserAction) -> bool {
    match op {
        HarnessUserAction::Left => spec.current_index > 0,
        HarnessUserAction::Right => spec.current_index + 1 < spec.clauses.len(),
        HarnessUserAction::ShiftLeft => spec.clauses[spec.current_index].can_split_left(),
        HarnessUserAction::Space => {
            matches!(
                harness.state,
                CompositionState::Composing | CompositionState::Previewing
            ) && harness.selection_index < harness.candidates.texts.len().saturating_sub(1) as i32
        }
        HarnessUserAction::Enter => {
            matches!(
                harness.state,
                CompositionState::Composing | CompositionState::Previewing
            ) && !spec.clauses.is_empty()
        }
        HarnessUserAction::SetTextType(_) => {
            matches!(
                harness.state,
                CompositionState::Composing | CompositionState::Previewing
            ) && !spec.clauses.is_empty()
        }
    }
}

fn explore_histories(
    harness: ClauseHarness,
    backend: ScenarioBackend,
    remaining_depth: usize,
    history: &mut Vec<HarnessUserAction>,
    visited: &mut HashSet<VisitedState>,
) {
    assert_harness_matches_spec(&harness, &backend.spec, history);
    let visited_state = VisitedState {
        spec: backend.spec.clone(),
        harness: harness_key(&harness),
    };
    if !visited.insert(visited_state) {
        return;
    }
    if remaining_depth == 0 {
        return;
    }

    let ops = [
        HarnessUserAction::Left,
        HarnessUserAction::Right,
        HarnessUserAction::ShiftLeft,
        HarnessUserAction::Space,
    ];
    for op in ops {
        if !is_available(&harness, &backend.spec, op) {
            continue;
        }

        let mut next_harness = harness.clone();
        let mut next_backend = backend.clone();
        history.push(op);
        apply_user_action(&mut next_harness, &mut next_backend, op, history);
        explore_histories(
            next_harness,
            next_backend,
            remaining_depth - 1,
            history,
            visited,
        );
        history.pop();
    }
}

pub(super) fn assert_histories_match_up_to_depth_eight() {
    let (harness, backend, _) = run_to_baseline();
    let mut history = Vec::new();
    let mut visited = HashSet::new();

    explore_histories(harness, backend, 8, &mut history, &mut visited);
}
