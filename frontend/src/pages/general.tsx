import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { ExternalLink, Keyboard, RefreshCcw, Table2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";

type WidthMode = "half" | "full";

type GeneralConfigState = {
    punctuation_style: string;
    symbol_style: string;
    space_input: string;
    numpad_input: string;
};

type CharacterWidthGroupsState = {
    alphabet: WidthMode;
    number: WidthMode;
    bracket: WidthMode;
    comma_period: WidthMode;
    middle_dot_corner_bracket: WidthMode;
    quote: WidthMode;
    colon_semicolon: WidthMode;
    hash_group: WidthMode;
    tilde: WidthMode;
    math_symbol: WidthMode;
    question_exclamation: WidthMode;
};

type RomajiRow = {
    input: string;
    output: string;
    next_input: string;
};

const DEFAULT_GENERAL_CONFIG: GeneralConfigState = {
    punctuation_style: "touten_kuten",
    symbol_style: "corner_bracket_middle_dot",
    space_input: "always_half",
    numpad_input: "direct_input",
};

const DEFAULT_WIDTH_GROUPS: CharacterWidthGroupsState = {
    alphabet: "half",
    number: "half",
    bracket: "full",
    comma_period: "full",
    middle_dot_corner_bracket: "full",
    quote: "full",
    colon_semicolon: "full",
    hash_group: "half",
    tilde: "full",
    math_symbol: "full",
    question_exclamation: "full",
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

const WIDTH_OPTIONS = [
    { value: "half", label: "半角" },
    { value: "full", label: "全角" },
];

const WIDTH_ROWS: Array<{
    key: keyof CharacterWidthGroupsState;
    label: string;
}> = [
    { key: "number", label: "数字" },
    { key: "bracket", label: "() {} []" },
    { key: "comma_period", label: "、 。" },
    { key: "middle_dot_corner_bracket", label: "･ ｢｣" },
    { key: "quote", label: "\" '" },
    { key: "colon_semicolon", label: ": ;" },
    { key: "hash_group", label: "# % & @ $ ^ _ | ` \\" },
    { key: "tilde", label: "~" },
    { key: "math_symbol", label: "< > = + - / *" },
    { key: "question_exclamation", label: "? !" },
];

const normalizeGeneralConfig = (value?: Record<string, unknown>): GeneralConfigState => ({
    punctuation_style:
        typeof value?.punctuation_style === "string"
            ? value.punctuation_style
            : DEFAULT_GENERAL_CONFIG.punctuation_style,
    symbol_style:
        typeof value?.symbol_style === "string"
            ? value.symbol_style
            : DEFAULT_GENERAL_CONFIG.symbol_style,
    space_input:
        value?.space_input === "always_full"
            ? "follow_input_mode"
            : typeof value?.space_input === "string"
              ? value.space_input
              : DEFAULT_GENERAL_CONFIG.space_input,
    numpad_input:
        value?.numpad_input === "direct_input"
            ? "direct_input"
            : value?.numpad_input === "always_half"
              ? "always_half"
              : value?.numpad_input === "follow_input_mode"
                ? "follow_input_mode"
                : DEFAULT_GENERAL_CONFIG.numpad_input,
});

const normalizeWidthGroups = (
    value?: Record<string, unknown>,
): CharacterWidthGroupsState => {
    const next = { ...DEFAULT_WIDTH_GROUPS };
    if (!value) {
        return next;
    }

    for (const [key, current] of Object.entries(next)) {
        const incoming = value[key];
        if (incoming === "half" || incoming === "full") {
            (next[key as keyof CharacterWidthGroupsState] as WidthMode) = incoming;
        } else {
            (next[key as keyof CharacterWidthGroupsState] as WidthMode) = current;
        }
    }

    return next;
};

const normalizeRomajiRows = (rows?: unknown): RomajiRow[] => {
    if (!Array.isArray(rows)) {
        return [];
    }

    return rows
        .map((row) => {
            if (!row || typeof row !== "object") {
                return null;
            }
            const record = row as Record<string, unknown>;
            if (typeof record.input !== "string" || typeof record.output !== "string") {
                return null;
            }
            return {
                input: record.input,
                output: record.output,
                next_input:
                    typeof record.next_input === "string" ? record.next_input : "",
            };
        })
        .filter((row): row is RomajiRow => row !== null);
};

const normalizeRomajiRowsForSave = (rows: RomajiRow[]): RomajiRow[] =>
    rows
        .map((row) => ({
            input: row.input.trim(),
            output: row.output.trim(),
            next_input: row.next_input.trim(),
        }))
        .filter((row) => row.input.length > 0 || row.output.length > 0 || row.next_input.length > 0);

export const General = () => {
    const [shortcutValue, setShortcutValue] = useState({
        ctrlSpaceToggle: true,
        altBackquoteToggle: true,
    });
    const [generalValue, setGeneralValue] = useState<GeneralConfigState>(
        DEFAULT_GENERAL_CONFIG,
    );
    const [widthGroups, setWidthGroups] =
        useState<CharacterWidthGroupsState>(DEFAULT_WIDTH_GROUPS);
    const [romajiRows, setRomajiRows] = useState<RomajiRow[]>([]);
    const [isRomajiEditorOpen, setIsRomajiEditorOpen] = useState(false);
    const [romajiDraftRows, setRomajiDraftRows] = useState<RomajiRow[]>([]);

    useEffect(() => {
        invoke<any>("get_config")
            .then((data) => {
                const shortcuts = data.shortcuts ?? {};
                setShortcutValue({
                    ctrlSpaceToggle: shortcuts.ctrl_space_toggle ?? true,
                    altBackquoteToggle: shortcuts.alt_backquote_toggle ?? true,
                });

                setGeneralValue(normalizeGeneralConfig(data.general));
                setWidthGroups(normalizeWidthGroups(data.character_width?.groups));
                setRomajiRows(normalizeRomajiRows(data.romaji_table?.rows));
            })
            .catch(() => {
                // Keep default values if config fetch fails
            });
    }, []);

    const updateConfig = async (updater: (config: any) => void) => {
        try {
            const data = await invoke<any>("get_config");
            updater(data);
            await invoke("update_config", { newConfig: data });
            return data;
        } catch (_error) {
            toast("設定の更新に失敗しました");
            return null;
        }
    };

    const handleCtrlSpaceToggle = async () => {
        const nextValue = !shortcutValue.ctrlSpaceToggle;
        const data = await updateConfig((config) => {
            config.shortcuts = config.shortcuts ?? {};
            config.shortcuts.ctrl_space_toggle = nextValue;
        });

        if (data) {
            setShortcutValue((prev) => ({ ...prev, ctrlSpaceToggle: nextValue }));
        }
    };

    const handleAltBackquoteToggle = async () => {
        const nextValue = !shortcutValue.altBackquoteToggle;
        const data = await updateConfig((config) => {
            config.shortcuts = config.shortcuts ?? {};
            config.shortcuts.alt_backquote_toggle = nextValue;
        });

        if (data) {
            setShortcutValue((prev) => ({ ...prev, altBackquoteToggle: nextValue }));
        }
    };

    const updateGeneralConfig = async (
        key: keyof GeneralConfigState,
        nextValue: string,
    ) => {
        const data = await updateConfig((config) => {
            config.general = config.general ?? {};
            config.general[key] = nextValue;
        });

        if (data) {
            setGeneralValue(normalizeGeneralConfig(data.general));
        }
    };

    const updateWidthGroup = async (
        key: keyof CharacterWidthGroupsState,
        nextValue: WidthMode,
    ) => {
        const data = await updateConfig((config) => {
            config.character_width = config.character_width ?? {};
            config.character_width.groups = config.character_width.groups ?? {};
            config.character_width.groups[key] = nextValue;
        });

        if (data) {
            setWidthGroups(normalizeWidthGroups(data.character_width?.groups));
        }
    };

    const openRomajiEditor = () => {
        setRomajiDraftRows(
            romajiRows.length > 0
                ? romajiRows
                : [{ input: "", output: "", next_input: "" }],
        );
        setIsRomajiEditorOpen(true);
    };

    const closeRomajiEditor = () => {
        setIsRomajiEditorOpen(false);
    };

    const setRomajiRowValue = (
        index: number,
        key: keyof RomajiRow,
        value: string,
    ) => {
        setRomajiDraftRows((prev) => {
            const next = [...prev];
            next[index] = { ...next[index], [key]: value };
            return next;
        });
    };

    const removeRomajiRow = (index: number) => {
        setRomajiDraftRows((prev) => {
            if (prev.length <= 1) {
                return [{ input: "", output: "", next_input: "" }];
            }
            return prev.filter((_, rowIndex) => rowIndex !== index);
        });
    };

    const addRomajiRow = () => {
        setRomajiDraftRows((prev) => [...prev, { input: "", output: "", next_input: "" }]);
    };

    const saveRomajiTable = async () => {
        const normalizedRows = normalizeRomajiRowsForSave(romajiDraftRows);

        if (normalizedRows.some((row) => !row.input || !row.output)) {
            toast("ローマ字テーブルに未入力の行があります");
            return;
        }

        const data = await updateConfig((config) => {
            config.romaji_table = config.romaji_table ?? {};
            config.romaji_table.rows = normalizedRows;
        });

        if (data) {
            const nextRows = normalizeRomajiRows(data.romaji_table?.rows);
            setRomajiRows(nextRows);
            setIsRomajiEditorOpen(false);
            toast("ローマ字テーブルを更新しました");
        }
    };

    const widthSummary = useMemo(() => {
        const visibleModes = WIDTH_ROWS.map((row) => widthGroups[row.key]);
        const fullCount = visibleModes.filter((mode) => mode === "full").length;
        const halfCount = visibleModes.length - fullCount;
        return `半角 ${halfCount} / 全角 ${fullCount}`;
    }, [widthGroups]);

    return (
        <>
            <div className="space-y-8">
                <section className="space-y-2">
                    <h1 className="text-sm font-bold text-foreground">バージョンと更新プログラム</h1>
                    <div className="flex items-center space-x-4 rounded-md border p-4">
                        <RefreshCcw />
                        <div className="flex-1 space-y-1">
                            <p className="text-sm font-medium leading-none">v0.1.0-alpha.1</p>
                        </div>
                        <Button variant="secondary">
                            <a
                                href="https://github.com/fkunn1326/azooKey-Windows/releases"
                                className="flex items-center gap-x-2"
                                target="_blank"
                                rel="noopener noreferrer"
                            >
                                <ExternalLink />
                                更新を確認する
                            </a>
                        </Button>
                    </div>
                </section>

                <section className="space-y-3">
                    <h1 className="text-sm font-bold text-foreground">基本設定</h1>
                    <div className="space-y-3 rounded-md border p-4">
                        <div className="grid grid-cols-[1fr_220px] items-center gap-4">
                            <p className="text-sm font-medium">句読点</p>
                            <Select
                                value={generalValue.punctuation_style}
                                onValueChange={(value) => void updateGeneralConfig("punctuation_style", value)}
                            >
                                <SelectTrigger className="w-full">
                                    <SelectValue placeholder="句読点を選択" />
                                </SelectTrigger>
                                <SelectContent>
                                    {PUNCTUATION_OPTIONS.map((option) => (
                                        <SelectItem key={option.value} value={option.value}>
                                            {option.label}
                                        </SelectItem>
                                    ))}
                                </SelectContent>
                            </Select>
                        </div>

                        <div className="grid grid-cols-[1fr_220px] items-center gap-4">
                            <p className="text-sm font-medium">記号</p>
                            <Select
                                value={generalValue.symbol_style}
                                onValueChange={(value) => void updateGeneralConfig("symbol_style", value)}
                            >
                                <SelectTrigger className="w-full">
                                    <SelectValue placeholder="記号を選択" />
                                </SelectTrigger>
                                <SelectContent>
                                    {SYMBOL_OPTIONS.map((option) => (
                                        <SelectItem key={option.value} value={option.value}>
                                            {option.label}
                                        </SelectItem>
                                    ))}
                                </SelectContent>
                            </Select>
                        </div>

                        <div className="grid grid-cols-[1fr_220px] items-center gap-4">
                            <p className="text-sm font-medium">スペースの入力</p>
                            <Select
                                value={generalValue.space_input}
                                onValueChange={(value) => void updateGeneralConfig("space_input", value)}
                            >
                                <SelectTrigger className="w-full">
                                    <SelectValue placeholder="スペースの入力を選択" />
                                </SelectTrigger>
                                <SelectContent>
                                    {SPACE_OPTIONS.map((option) => (
                                        <SelectItem key={option.value} value={option.value}>
                                            {option.label}
                                        </SelectItem>
                                    ))}
                                </SelectContent>
                            </Select>
                        </div>

                        <div className="grid grid-cols-[1fr_220px] items-center gap-4">
                            <p className="text-sm font-medium">テンキーからの入力</p>
                            <Select
                                value={generalValue.numpad_input}
                                onValueChange={(value) => void updateGeneralConfig("numpad_input", value)}
                            >
                                <SelectTrigger className="w-full">
                                    <SelectValue placeholder="テンキー入力を選択" />
                                </SelectTrigger>
                                <SelectContent>
                                    {NUMPAD_OPTIONS.map((option) => (
                                        <SelectItem key={option.value} value={option.value}>
                                            {option.label}
                                        </SelectItem>
                                    ))}
                                </SelectContent>
                            </Select>
                        </div>
                    </div>
                </section>

                <section className="space-y-3">
                    <h1 className="text-sm font-bold text-foreground">キー設定</h1>
                    <div className="space-y-3 rounded-md border p-4">
                        <div className="flex items-center gap-4">
                            <Table2 className="h-4 w-4" />
                            <div className="flex-1">
                                <p className="text-sm font-medium">ローマ字テーブル</p>
                                <p className="text-xs text-muted-foreground">
                                    登録件数: {romajiRows.length} 件
                                </p>
                            </div>
                            <Button variant="secondary" onClick={openRomajiEditor}>
                                編集
                            </Button>
                        </div>
                    </div>
                </section>

                <section className="space-y-3">
                    <h1 className="text-sm font-bold text-foreground">半角全角設定</h1>
                    <div className="space-y-3 rounded-md border p-4">
                        <p className="text-xs text-muted-foreground">
                            記号カテゴリごとに半角/全角を指定します（{widthSummary}）。
                        </p>
                        <div className="overflow-x-auto rounded-md border">
                            <table className="w-full min-w-[640px] text-sm">
                                <thead className="bg-muted/30 text-left text-xs text-muted-foreground">
                                    <tr>
                                        <th className="px-3 py-2 font-medium">文字グループ</th>
                                        <th className="px-3 py-2 font-medium">変換前文字列</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {WIDTH_ROWS.map((row) => (
                                        <tr key={row.key} className="border-t">
                                            <td className="px-3 py-2 font-medium">{row.label}</td>
                                            <td className="px-3 py-2">
                                                <div className="flex justify-end">
                                                    <Select
                                                        value={widthGroups[row.key]}
                                                        onValueChange={(value: WidthMode) =>
                                                            void updateWidthGroup(row.key, value)
                                                        }
                                                    >
                                                        <SelectTrigger className="w-28">
                                                            <SelectValue placeholder="幅" />
                                                        </SelectTrigger>
                                                        <SelectContent>
                                                            {WIDTH_OPTIONS.map((option) => (
                                                                <SelectItem key={option.value} value={option.value}>
                                                                    {option.label}
                                                                </SelectItem>
                                                            ))}
                                                        </SelectContent>
                                                    </Select>
                                                </div>
                                            </td>
                                        </tr>
                                    ))}
                                </tbody>
                            </table>
                        </div>
                    </div>
                </section>

                <section className="space-y-2">
                    <h1 className="text-sm font-bold text-foreground">入力モード切替ショートカット</h1>
                    <div className="flex items-center space-x-4 rounded-md border p-4">
                        <Keyboard />
                        <div className="flex-1 space-y-1">
                            <p className="text-sm font-medium leading-none">Ctrl + Space を有効化</p>
                            <p className="text-xs text-muted-foreground">
                                英数/かな切り替えのショートカットとして Ctrl + Space を使用します
                            </p>
                        </div>
                        <Switch checked={shortcutValue.ctrlSpaceToggle} onCheckedChange={handleCtrlSpaceToggle} />
                    </div>
                    <div className="flex items-center space-x-4 rounded-md border p-4">
                        <Keyboard />
                        <div className="flex-1 space-y-1">
                            <p className="text-sm font-medium leading-none">Alt + ` を有効化</p>
                            <p className="text-xs text-muted-foreground">
                                英数/かな切り替えのショートカットとして Alt + ` を使用します
                            </p>
                        </div>
                        <Switch checked={shortcutValue.altBackquoteToggle} onCheckedChange={handleAltBackquoteToggle} />
                    </div>
                </section>
            </div>

            {isRomajiEditorOpen && (
                <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4">
                    <div className="flex h-[80vh] w-full max-w-5xl flex-col rounded-lg border bg-background p-4 shadow-lg">
                        <div className="mb-3 flex items-center justify-between">
                            <div>
                                <h2 className="text-sm font-bold">ローマ字テーブル設定</h2>
                                <p className="text-xs text-muted-foreground">
                                    入力 / 出力 / 次の入力 を編集して保存します
                                </p>
                            </div>
                            <Button variant="outline" onClick={closeRomajiEditor}>
                                閉じる
                            </Button>
                        </div>

                        <div className="flex-1 overflow-auto rounded-md border">
                            <table className="w-full min-w-[860px] text-sm">
                                <thead className="sticky top-0 bg-muted/30 text-left text-xs text-muted-foreground">
                                    <tr>
                                        <th className="w-16 px-2 py-2 font-medium">#</th>
                                        <th className="px-2 py-2 font-medium">入力</th>
                                        <th className="px-2 py-2 font-medium">出力</th>
                                        <th className="px-2 py-2 font-medium">次の入力</th>
                                        <th className="w-20 px-2 py-2 font-medium">操作</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {romajiDraftRows.map((row, index) => (
                                        <tr key={`row-${index}`} className="border-t">
                                            <td className="px-2 py-2 text-xs text-muted-foreground">{index + 1}</td>
                                            <td className="px-2 py-2">
                                                <Input
                                                    value={row.input}
                                                    onChange={(event) =>
                                                        setRomajiRowValue(index, "input", event.target.value)
                                                    }
                                                    placeholder="例: ka"
                                                />
                                            </td>
                                            <td className="px-2 py-2">
                                                <Input
                                                    value={row.output}
                                                    onChange={(event) =>
                                                        setRomajiRowValue(index, "output", event.target.value)
                                                    }
                                                    placeholder="例: か"
                                                />
                                            </td>
                                            <td className="px-2 py-2">
                                                <Input
                                                    value={row.next_input}
                                                    onChange={(event) =>
                                                        setRomajiRowValue(index, "next_input", event.target.value)
                                                    }
                                                    placeholder="例: k"
                                                />
                                            </td>
                                            <td className="px-2 py-2">
                                                <Button
                                                    variant="outline"
                                                    size="sm"
                                                    onClick={() => removeRomajiRow(index)}
                                                >
                                                    削除
                                                </Button>
                                            </td>
                                        </tr>
                                    ))}
                                </tbody>
                            </table>
                        </div>

                        <div className="mt-3 flex items-center justify-between gap-2">
                            <Button variant="secondary" onClick={addRomajiRow}>
                                行を追加
                            </Button>
                            <div className="flex gap-2">
                                <Button variant="outline" onClick={closeRomajiEditor}>
                                    キャンセル
                                </Button>
                                <Button onClick={() => void saveRomajiTable()}>保存</Button>
                            </div>
                        </div>
                    </div>
                </div>
            )}
        </>
    );
};
