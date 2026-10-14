import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
    AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent,
    AlertDialogDescription, AlertDialogFooter, AlertDialogHeader,
    AlertDialogTitle, AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import {
    Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { SettingsRow, SettingsSection } from "@/components/settings-section";
import { DebugSettings } from "@/components/debug-settings";
import { RomajiEditor, normalizeRomajiRows, type RomajiRow } from "@/components/romaji-editor";
import { Zenzai } from "@/pages/zenzai";
import { getConfigAfterPendingUpdates, saveConfigWithToast } from "@/lib/config";

type WidthMode = "half" | "full";
type LearningMode = "enabled" | "read_only" | "disabled";
type ReconversionKey = "convert" | "shift_convert" | "space" | "win_slash" | "disabled";
type GeneralConfigState = {
    punctuation_style: string;
    symbol_style: string;
    space_input: string;
    numpad_input: string;
    punctuation_commit: boolean;
    punctuation_commit_punctuation: boolean;
    punctuation_commit_exclamation: boolean;
    punctuation_commit_question: boolean;
    show_candidate_window_after_space: boolean;
    show_live_conversion_reading: boolean;
    live_conversion_reading_vertical_adjustment: number;
    jev_conversion: boolean;
    experimental_typo_correction: boolean;
};
type CharacterWidthGroupsState = Record<
    "alphabet" | "number" | "bracket" | "comma_period" | "middle_dot_corner_bracket" |
    "quote" | "colon_semicolon" | "hash_group" | "tilde" | "math_symbol" | "question_exclamation",
    WidthMode
>;

const DEFAULT_GENERAL_CONFIG: GeneralConfigState = {
    punctuation_style: "touten_kuten",
    symbol_style: "corner_bracket_middle_dot",
    space_input: "always_half",
    numpad_input: "direct_input",
    punctuation_commit: false,
    punctuation_commit_punctuation: true,
    punctuation_commit_exclamation: true,
    punctuation_commit_question: true,
    show_candidate_window_after_space: false,
    show_live_conversion_reading: true,
    live_conversion_reading_vertical_adjustment: 4,
    jev_conversion: false,
    experimental_typo_correction: false,
};
const LIVE_CONVERSION_READING_VERTICAL_ADJUSTMENT_MIN = -12;
const LIVE_CONVERSION_READING_VERTICAL_ADJUSTMENT_MAX = 12;
const clampLiveConversionReadingVerticalAdjustment = (value: unknown): number => {
    const number = typeof value === "number" ? value
        : typeof value === "string" ? Number.parseInt(value, 10) : Number.NaN;
    return Number.isFinite(number)
        ? Math.min(LIVE_CONVERSION_READING_VERTICAL_ADJUSTMENT_MAX,
            Math.max(LIVE_CONVERSION_READING_VERTICAL_ADJUSTMENT_MIN, Math.round(number)))
        : DEFAULT_GENERAL_CONFIG.live_conversion_reading_vertical_adjustment;
};
const DEFAULT_WIDTH_GROUPS: CharacterWidthGroupsState = {
    alphabet: "half", number: "half", bracket: "full", comma_period: "full",
    middle_dot_corner_bracket: "full", quote: "full", colon_semicolon: "full",
    hash_group: "half", tilde: "full", math_symbol: "full", question_exclamation: "full",
};
const PUNCTUATION_OPTIONS = [
    { value: "touten_kuten", label: "、。" },
    { value: "fullwidth_comma_fullwidth_period", label: "，．" },
    { value: "touten_fullwidth_period", label: "、．" },
    { value: "fullwidth_comma_kuten", label: "，。" },
];
const SYMBOL_OPTIONS = [
    { value: "corner_bracket_middle_dot", label: "「」・" },
    { value: "square_bracket_backslash", label: "［］／" },
    { value: "corner_bracket_backslash", label: "「」／" },
    { value: "square_bracket_middle_dot", label: "［］・" },
];
const SPACE_OPTIONS = [
    { value: "always_half", label: "常に半角" },
    { value: "follow_input_mode", label: "入力モードに従う" },
];
const NUMPAD_OPTIONS = [
    { value: "direct_input", label: "直接入力" },
    { value: "always_half", label: "常に半角" },
    { value: "follow_input_mode", label: "入力モードに従う" },
];
const RECONVERSION_KEY_OPTIONS: { value: ReconversionKey; label: string }[] = [
    { value: "convert", label: "変換" },
    { value: "shift_convert", label: "Shift + 変換" },
    { value: "space", label: "Space" },
    { value: "win_slash", label: "Win + /" },
    { value: "disabled", label: "無効" },
];
const normalizeReconversionKey = (value: unknown): ReconversionKey =>
    RECONVERSION_KEY_OPTIONS.some((option) => option.value === value)
        ? value as ReconversionKey : "win_slash";
const LEARNING_MODE_OPTIONS: { value: LearningMode; label: string }[] = [
    { value: "enabled", label: "学習して履歴を使う" },
    { value: "read_only", label: "既存の履歴のみ使う" },
    { value: "disabled", label: "学習も履歴の使用もしない" },
];
const WIDTH_OPTIONS = [
    { value: "half", label: "半角" },
    { value: "full", label: "全角" },
];
const WIDTH_ROWS: { key: keyof CharacterWidthGroupsState; label: string }[] = [
    { key: "number", label: "数字" },
    { key: "bracket", label: "() {} []" },
    { key: "comma_period", label: "、。" },
    { key: "middle_dot_corner_bracket", label: "･ ｢｣" },
    { key: "quote", label: "\" '" },
    { key: "colon_semicolon", label: ": ;" },
    { key: "hash_group", label: "# % & @ $ ^ _ | ` \\" },
    { key: "tilde", label: "~" },
    { key: "math_symbol", label: "< > = + - / *" },
    { key: "question_exclamation", label: "? !" },
];
const normalizeGeneralConfig = (value?: Record<string, unknown>): GeneralConfigState => {
    const next = { ...DEFAULT_GENERAL_CONFIG };
    for (const key of Object.keys(next) as (keyof GeneralConfigState)[]) {
        if (typeof value?.[key] === typeof next[key]) {
            Object.assign(next, { [key]: value![key] });
        }
    }
    if (next.space_input === "always_full") next.space_input = "follow_input_mode";
    if (!["direct_input", "always_half", "follow_input_mode"].includes(next.numpad_input)) {
        next.numpad_input = DEFAULT_GENERAL_CONFIG.numpad_input;
    }
    next.live_conversion_reading_vertical_adjustment =
        clampLiveConversionReadingVerticalAdjustment(value?.live_conversion_reading_vertical_adjustment);
    return next;
};
const normalizeLearningMode = (value: unknown): LearningMode =>
    value === "enabled" || value === "read_only" || value === "disabled" ? value : "enabled";
const normalizeWidthGroups = (value?: Record<string, unknown>): CharacterWidthGroupsState => {
    const next = { ...DEFAULT_WIDTH_GROUPS };
    for (const key of Object.keys(next) as (keyof CharacterWidthGroupsState)[]) {
        if (value?.[key] === "half" || value?.[key] === "full") next[key] = value[key];
    }
    return next;
};

export const General = ({ page }: { page: "input" | "conversion" | "keys" | "advanced" }) => {
    const [shortcutValue, setShortcutValue] = useState({
        ctrlSpaceToggle: true, altBackquoteToggle: true, eisuToggle: false,
        reconversionKey: "win_slash" as ReconversionKey,
    });
    const [generalValue, setGeneralValue] = useState(DEFAULT_GENERAL_CONFIG);
    const [widthGroups, setWidthGroups] = useState(DEFAULT_WIDTH_GROUPS);
    const [learningMode, setLearningMode] = useState<LearningMode>("enabled");
    const [romajiRows, setRomajiRows] = useState<RomajiRow[]>([]);
    const [loadState, setLoadState] = useState<"loading" | "ready" | "error">("loading");
    const [loadAttempt, setLoadAttempt] = useState(0);
    const [jevWarningOpen, setJevWarningOpen] = useState(false);
    const [jevApiKey, setJevApiKey] = useState("");
    const [jevApiKeySaved, setJevApiKeySaved] = useState<boolean | null>(null);
    const [jevApiKeyBusy, setJevApiKeyBusy] = useState(false);
    const [jevApiKeyChecking, setJevApiKeyChecking] = useState(false);
    const liveConversionReadingAdjustmentSaveRef = useRef<{
        saving: boolean; pendingValue: number | null;
    }>({ saving: false, pendingValue: null });

    useEffect(() => {
        let cancelled = false;
        setLoadState("loading");
        getConfigAfterPendingUpdates().then((data) => {
            if (cancelled) return;
            const shortcuts = data.shortcuts ?? {};
            setShortcutValue({
                ctrlSpaceToggle: shortcuts.ctrl_space_toggle ?? true,
                altBackquoteToggle: shortcuts.alt_backquote_toggle ?? true,
                eisuToggle: shortcuts.eisu_toggle ?? false,
                reconversionKey: normalizeReconversionKey(shortcuts.reconversion_key),
            });
            setGeneralValue(normalizeGeneralConfig(data.general));
            setLearningMode(normalizeLearningMode(data.learning?.mode));
            setWidthGroups(normalizeWidthGroups(data.character_width?.groups));
            setRomajiRows(normalizeRomajiRows(data.romaji_table?.rows));
            setLoadState("ready");
        }).catch(() => { if (!cancelled) setLoadState("error"); });
        if (page === "advanced") {
            invoke<boolean>("has_jev_api_key").then((saved) => {
                if (!cancelled) setJevApiKeySaved(saved);
            }).catch(() => {
                if (!cancelled) toast("Jev APIキーの状態を取得できませんでした");
            });
        }
        return () => { cancelled = true; };
    }, [page, loadAttempt]);

    const updateGeneralConfig = async <K extends keyof GeneralConfigState>(key: K, nextValue: GeneralConfigState[K]) => {
        const data = await saveConfigWithToast((config) => {
            config.general = config.general ?? {};
            config.general[key] = nextValue;
        });
        if (data) setGeneralValue(normalizeGeneralConfig(data.general));
    };
    const updateShortcut = async (key: string, nextValue: boolean | ReconversionKey) => {
        const data = await saveConfigWithToast((config) => {
            config.shortcuts = config.shortcuts ?? {};
            config.shortcuts[key] = nextValue;
        });
        if (data) {
            setShortcutValue({
                ctrlSpaceToggle: data.shortcuts.ctrl_space_toggle,
                altBackquoteToggle: data.shortcuts.alt_backquote_toggle,
                eisuToggle: data.shortcuts.eisu_toggle,
                reconversionKey: normalizeReconversionKey(data.shortcuts.reconversion_key),
            });
        }
    };
    const updateWidthGroup = async (key: keyof CharacterWidthGroupsState, nextValue: WidthMode) => {
        const data = await saveConfigWithToast((config) => {
            config.character_width = config.character_width ?? {};
            config.character_width.groups = config.character_width.groups ?? {};
            config.character_width.groups[key] = nextValue;
        });
        if (data) setWidthGroups(normalizeWidthGroups(data.character_width?.groups));
    };
    const updateLearningMode = async (nextValue: LearningMode) => {
        const data = await saveConfigWithToast((config) => {
            config.learning = config.learning ?? {};
            config.learning.mode = nextValue;
        });
        if (data) setLearningMode(normalizeLearningMode(data.learning?.mode));
    };
    const resetLearningHistory = async () => {
        try {
            const result = await invoke<{ reset: boolean; server_applied: boolean; message?: string | null }>("reset_learning_history");
            if (result.reset && result.server_applied) toast("学習履歴を削除しました");
            else toast("学習履歴を削除できませんでした", { description: result.message ?? undefined });
        } catch (error) {
            toast("学習履歴を削除できませんでした", { description: String(error) });
        }
    };
    const saveJevApiKey = async () => {
        setJevApiKeyBusy(true);
        try {
            await invoke("save_jev_api_key", { apiKey: jevApiKey.trim() });
            setJevApiKey("");
            setJevApiKeySaved(true);
            toast("Jev APIキーを保存しました");
        } catch { toast("Jev APIキーを保存できませんでした"); }
        finally { setJevApiKeyBusy(false); }
    };
    const deleteJevApiKey = async () => {
        setJevApiKeyBusy(true);
        try {
            await invoke("delete_jev_api_key");
            setJevApiKey("");
            setJevApiKeySaved(false);
            toast("Jev APIキーを削除しました");
        } catch { toast("Jev APIキーを削除できませんでした"); }
        finally { setJevApiKeyBusy(false); }
    };
    const checkJevApiKey = async () => {
        if (jevApiKeyBusy) return;
        setJevApiKeyBusy(true);
        setJevApiKeyChecking(true);
        try {
            await invoke("check_jev_api_key");
            toast("保存済みAPIキーは有効です");
        } catch (error) {
            toast("保存済みAPIキーを確認できませんでした", { description: String(error) });
        } finally {
            setJevApiKeyBusy(false);
            setJevApiKeyChecking(false);
        }
    };
    const flushLiveConversionReadingVerticalAdjustmentSave = async () => {
        const saveState = liveConversionReadingAdjustmentSaveRef.current;
        if (saveState.saving) return;
        saveState.saving = true;
        try {
            while (saveState.pendingValue !== null) {
                const nextValue = saveState.pendingValue;
                saveState.pendingValue = null;
                const data = await saveConfigWithToast((config) => {
                    config.general = config.general ?? {};
                    config.general.live_conversion_reading_vertical_adjustment = nextValue;
                });
                if (data && saveState.pendingValue === null) setGeneralValue(normalizeGeneralConfig(data.general));
            }
        } finally { saveState.saving = false; }
        if (saveState.pendingValue !== null) void flushLiveConversionReadingVerticalAdjustmentSave();
    };
    const updateLiveConversionReadingVerticalAdjustment = (nextValue: number) => {
        liveConversionReadingAdjustmentSaveRef.current.pendingValue = clampLiveConversionReadingVerticalAdjustment(nextValue);
        void flushLiveConversionReadingVerticalAdjustmentSave();
    };

    if (loadState !== "ready") return (
        <div className="settings-content" role="status">
            <p className={loadState === "error" ? "text-sm text-destructive" : "settings-description"}>
                {loadState === "error" ? "設定を読み込めませんでした。" : "設定を読み込んでいます…"}
            </p>
            {loadState === "error" && <Button variant="outline" onClick={() => setLoadAttempt((attempt) => attempt + 1)}>再試行</Button>}
        </div>
    );

    return (
        <div className="settings-content">
            {page === "input" && <>
                <SettingsSection title="文字の入力">
                    {([
                        ["punctuation_style", "句読点", PUNCTUATION_OPTIONS],
                        ["symbol_style", "記号", SYMBOL_OPTIONS],
                        ["space_input", "スペース", SPACE_OPTIONS],
                        ["numpad_input", "テンキー", NUMPAD_OPTIONS],
                    ] as const).map(([key, label, options]) => (
                        <SettingsRow key={key} label={label} htmlFor={key}>
                            <Select value={generalValue[key]} onValueChange={(value) => void updateGeneralConfig(key, value)}>
                                <SelectTrigger id={key}><SelectValue /></SelectTrigger>
                                <SelectContent>{options.map((option) => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}</SelectContent>
                            </Select>
                        </SettingsRow>
                    ))}
                </SettingsSection>
                <SettingsSection title="半角・全角">
                    {WIDTH_ROWS.map(({ key, label }) => (
                        <SettingsRow key={key} label={label} htmlFor={`width-${key}`}>
                            <Select value={widthGroups[key]} onValueChange={(value: WidthMode) => void updateWidthGroup(key, value)}>
                                <SelectTrigger id={`width-${key}`}><SelectValue /></SelectTrigger>
                                <SelectContent>{WIDTH_OPTIONS.map((option) => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}</SelectContent>
                            </Select>
                        </SettingsRow>
                    ))}
                </SettingsSection>
            </>}
            {page === "conversion" && <>
                <Zenzai />
                <SettingsSection title="確定">
                    <SettingsRow label="記号入力で変換を確定" htmlFor="punctuation-commit">
                        <Switch id="punctuation-commit" checked={generalValue.punctuation_commit} onCheckedChange={(value) => void updateGeneralConfig("punctuation_commit", value)} />
                    </SettingsRow>
                    {generalValue.punctuation_commit && <div className="settings-subgroup">
                        {([
                            ["punctuation_commit_punctuation", "句読点"],
                            ["punctuation_commit_exclamation", "！"],
                            ["punctuation_commit_question", "？"],
                        ] as const).map(([key, label]) => (
                            <SettingsRow key={key} label={label} htmlFor={key}>
                                <Switch id={key} checked={generalValue[key]} onCheckedChange={(value) => void updateGeneralConfig(key, value)} />
                            </SettingsRow>
                        ))}
                    </div>}
                </SettingsSection>
                <SettingsSection title="候補と読みの表示">
                    <SettingsRow label="Space / Tab まで候補を非表示" htmlFor="candidate-delay">
                        <Switch id="candidate-delay" checked={generalValue.show_candidate_window_after_space} onCheckedChange={(value) => void updateGeneralConfig("show_candidate_window_after_space", value)} />
                    </SettingsRow>
                    <SettingsRow label="ライブ変換中の読みを表示" htmlFor="live-reading">
                        <Switch id="live-reading" checked={generalValue.show_live_conversion_reading} onCheckedChange={(value) => void updateGeneralConfig("show_live_conversion_reading", value)} />
                    </SettingsRow>
                    {generalValue.show_live_conversion_reading && <div className="settings-subgroup">
                        <SettingsRow label="読み表示の高さ" htmlFor="reading-height">
                            <div className="w-full space-y-1">
                                <div className="flex items-center gap-2 text-xs">
                                    <span>下</span>
                                    <input id="reading-height" type="range" min={LIVE_CONVERSION_READING_VERTICAL_ADJUSTMENT_MIN} max={LIVE_CONVERSION_READING_VERTICAL_ADJUSTMENT_MAX} step={1}
                                        value={generalValue.live_conversion_reading_vertical_adjustment} className="min-w-0 w-full accent-primary"
                                        aria-valuetext={`${generalValue.live_conversion_reading_vertical_adjustment}px`}
                                        onChange={(event) => {
                                            const nextValue = clampLiveConversionReadingVerticalAdjustment(event.currentTarget.value);
                                            setGeneralValue((current) => ({ ...current, live_conversion_reading_vertical_adjustment: nextValue }));
                                        }}
                                        onPointerUp={(event) => updateLiveConversionReadingVerticalAdjustment(event.currentTarget.valueAsNumber)}
                                        onKeyUp={(event) => {
                                            if (["ArrowDown", "ArrowLeft", "ArrowRight", "ArrowUp", "End", "Home", "PageDown", "PageUp"].includes(event.key)) {
                                                updateLiveConversionReadingVerticalAdjustment(event.currentTarget.valueAsNumber);
                                            }
                                        }}
                                        onBlur={(event) => updateLiveConversionReadingVerticalAdjustment(event.currentTarget.valueAsNumber)} />
                                    <span>上</span>
                                </div>
                                <p className="text-right text-xs tabular-nums">{generalValue.live_conversion_reading_vertical_adjustment > 0 ? "+" : ""}{generalValue.live_conversion_reading_vertical_adjustment}px</p>
                            </div>
                        </SettingsRow>
                    </div>}
                </SettingsSection>
                <SettingsSection title="変換学習">
                    <SettingsRow label="学習と履歴の使用" htmlFor="learning-mode">
                        <Select value={learningMode} onValueChange={(value: LearningMode) => void updateLearningMode(value)}>
                            <SelectTrigger id="learning-mode"><SelectValue /></SelectTrigger>
                            <SelectContent>{LEARNING_MODE_OPTIONS.map((option) => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}</SelectContent>
                        </Select>
                    </SettingsRow>
                    <SettingsRow label="学習履歴">
                        <AlertDialog>
                            <AlertDialogTrigger asChild><Button variant="outline">履歴を削除</Button></AlertDialogTrigger>
                            <AlertDialogContent>
                                <AlertDialogHeader>
                                    <AlertDialogTitle>学習履歴を削除しますか？</AlertDialogTitle>
                                    <AlertDialogDescription>これまでの変換学習データを削除します。この操作は取り消せません。</AlertDialogDescription>
                                </AlertDialogHeader>
                                <AlertDialogFooter>
                                    <AlertDialogCancel>キャンセル</AlertDialogCancel>
                                    <AlertDialogAction onClick={() => void resetLearningHistory()}>削除</AlertDialogAction>
                                </AlertDialogFooter>
                            </AlertDialogContent>
                        </AlertDialog>
                    </SettingsRow>
                </SettingsSection>
            </>}
            {page === "keys" && <>
                <SettingsSection title="英数・かな切り替え">
                    {([
                        ["ctrl_space_toggle", "ctrlSpaceToggle", "Ctrl + Space"],
                        ["alt_backquote_toggle", "altBackquoteToggle", "Alt + `"],
                        ["eisu_toggle", "eisuToggle", "英数 (CapsLock)"],
                    ] as const).map(([key, stateKey, label]) => (
                        <SettingsRow key={key} label={label} htmlFor={key}>
                            <Switch id={key} checked={shortcutValue[stateKey]} onCheckedChange={(value) => void updateShortcut(key, value)} />
                        </SettingsRow>
                    ))}
                </SettingsSection>
                <SettingsSection title="再変換">
                    <SettingsRow label="再変換キー" htmlFor="reconversion-key">
                        <Select value={shortcutValue.reconversionKey} onValueChange={(value: ReconversionKey) => void updateShortcut("reconversion_key", value)}>
                            <SelectTrigger id="reconversion-key"><SelectValue /></SelectTrigger>
                            <SelectContent>{RECONVERSION_KEY_OPTIONS.map((option) => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}</SelectContent>
                        </Select>
                    </SettingsRow>
                </SettingsSection>
                <SettingsSection title="ローマ字入力">
                    <SettingsRow label="ローマ字テーブル" description={`${romajiRows.length} 件`}>
                        <RomajiEditor rows={romajiRows} onSaved={setRomajiRows} />
                    </SettingsRow>
                </SettingsSection>
            </>}
            {page === "advanced" && <>
                <SettingsSection title="実験的機能">
                    <SettingsRow label="誤入力の補正" description="意図しない変換結果になる場合があります。" htmlFor="typo-correction">
                        <Switch id="typo-correction" checked={generalValue.experimental_typo_correction} onCheckedChange={(value) => void updateGeneralConfig("experimental_typo_correction", value)} />
                    </SettingsRow>
                    <SettingsRow label="Jev による候補選択" description="読み・候補・左文脈を外部へ送信します。ONの間はZenzaiを使いません。" htmlFor="jev-conversion">
                        <Switch id="jev-conversion" checked={generalValue.jev_conversion} onCheckedChange={(value) => {
                            if (value) setJevWarningOpen(true);
                            else void updateGeneralConfig("jev_conversion", false);
                        }} />
                    </SettingsRow>
                    {(generalValue.jev_conversion || jevApiKeySaved) && <div className="settings-subgroup">
                        <SettingsRow label="TypeSafe APIキー" description="Windowsの資格情報マネージャーに保存します。" htmlFor="jev-api-key">
                            <div className="w-full space-y-2">
                                <Input id="jev-api-key" type="password" autoComplete="off" spellCheck={false} value={jevApiKey}
                                    disabled={jevApiKeyBusy || jevApiKeySaved === null}
                                    placeholder={jevApiKeySaved ? "設定済み（変更する場合は入力）" : "APIキーを入力"}
                                    onChange={(event) => setJevApiKey(event.target.value)} />
                                <div className="settings-actions">
                                    <Button disabled={!jevApiKey.trim() || jevApiKeyBusy || jevApiKeySaved === null} onClick={() => void saveJevApiKey()}>キーを保存</Button>
                                    {jevApiKeySaved && <>
                                        <Button variant="outline" disabled={jevApiKeyBusy} onClick={() => void checkJevApiKey()}>
                                            {jevApiKeyChecking ? "確認中…" : "保存済みキーを確認"}
                                        </Button>
                                        <Button variant="outline" disabled={jevApiKeyBusy} onClick={() => void deleteJevApiKey()}>削除</Button>
                                    </>}
                                </div>
                                {jevApiKeySaved && <p className="settings-description">モデル一覧APIで認証を確認します。入力内容は送信しません。</p>}
                            </div>
                        </SettingsRow>
                    </div>}
                </SettingsSection>
                <DebugSettings />
            </>}
            <AlertDialog open={jevWarningOpen} onOpenChange={setJevWarningOpen}>
                <AlertDialogContent>
                    <AlertDialogHeader>
                        <AlertDialogTitle>Jevによる候補選択を有効にしますか？</AlertDialogTitle>
                        <AlertDialogDescription>
                            有効にするとZenzaiとリアルタイム変換を使わなくなります。デスクトップアプリで変換キーを押すと、入力した読み・変換候補・左文脈がJevのサーバー（TypeSafe）へ送信されます。Windows検索などのAppContainerでは通常KKCを使います。
                        </AlertDialogDescription>
                    </AlertDialogHeader>
                    <AlertDialogFooter>
                        <AlertDialogCancel>キャンセル</AlertDialogCancel>
                        <AlertDialogAction onClick={() => void updateGeneralConfig("jev_conversion", true)}>有効にする</AlertDialogAction>
                    </AlertDialogFooter>
                </AlertDialogContent>
            </AlertDialog>
        </div>
    );
};
