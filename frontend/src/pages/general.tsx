import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { RefreshCcw, ExternalLink, Keyboard } from "lucide-react";
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";

type SymbolWidthEntry = {
    key: string;
    fullwidth: string;
    defaultFullwidth: boolean;
    group: "digit" | "symbol";
};

const SYMBOL_WIDTH_ENTRIES: SymbolWidthEntry[] = [
    { key: "0", fullwidth: "０", defaultFullwidth: false, group: "digit" },
    { key: "1", fullwidth: "１", defaultFullwidth: false, group: "digit" },
    { key: "2", fullwidth: "２", defaultFullwidth: false, group: "digit" },
    { key: "3", fullwidth: "３", defaultFullwidth: false, group: "digit" },
    { key: "4", fullwidth: "４", defaultFullwidth: false, group: "digit" },
    { key: "5", fullwidth: "５", defaultFullwidth: false, group: "digit" },
    { key: "6", fullwidth: "６", defaultFullwidth: false, group: "digit" },
    { key: "7", fullwidth: "７", defaultFullwidth: false, group: "digit" },
    { key: "8", fullwidth: "８", defaultFullwidth: false, group: "digit" },
    { key: "9", fullwidth: "９", defaultFullwidth: false, group: "digit" },
    { key: "!", fullwidth: "！", defaultFullwidth: true, group: "symbol" },
    { key: "\"", fullwidth: "”", defaultFullwidth: true, group: "symbol" },
    { key: "#", fullwidth: "＃", defaultFullwidth: true, group: "symbol" },
    { key: "$", fullwidth: "＄", defaultFullwidth: true, group: "symbol" },
    { key: "%", fullwidth: "％", defaultFullwidth: true, group: "symbol" },
    { key: "&", fullwidth: "＆", defaultFullwidth: true, group: "symbol" },
    { key: "'", fullwidth: "’", defaultFullwidth: true, group: "symbol" },
    { key: "(", fullwidth: "（", defaultFullwidth: true, group: "symbol" },
    { key: ")", fullwidth: "）", defaultFullwidth: true, group: "symbol" },
    { key: "*", fullwidth: "＊", defaultFullwidth: true, group: "symbol" },
    { key: "+", fullwidth: "＋", defaultFullwidth: true, group: "symbol" },
    { key: ",", fullwidth: "、", defaultFullwidth: true, group: "symbol" },
    { key: "-", fullwidth: "ー", defaultFullwidth: true, group: "symbol" },
    { key: ".", fullwidth: "。", defaultFullwidth: true, group: "symbol" },
    { key: "/", fullwidth: "・", defaultFullwidth: true, group: "symbol" },
    { key: ":", fullwidth: "：", defaultFullwidth: true, group: "symbol" },
    { key: ";", fullwidth: "；", defaultFullwidth: true, group: "symbol" },
    { key: "<", fullwidth: "＜", defaultFullwidth: true, group: "symbol" },
    { key: "=", fullwidth: "＝", defaultFullwidth: true, group: "symbol" },
    { key: ">", fullwidth: "＞", defaultFullwidth: true, group: "symbol" },
    { key: "?", fullwidth: "？", defaultFullwidth: true, group: "symbol" },
    { key: "@", fullwidth: "＠", defaultFullwidth: true, group: "symbol" },
    { key: "[", fullwidth: "「", defaultFullwidth: true, group: "symbol" },
    { key: "\\", fullwidth: "￥", defaultFullwidth: true, group: "symbol" },
    { key: "]", fullwidth: "」", defaultFullwidth: true, group: "symbol" },
    { key: "^", fullwidth: "＾", defaultFullwidth: true, group: "symbol" },
    { key: "_", fullwidth: "＿", defaultFullwidth: true, group: "symbol" },
    { key: "`", fullwidth: "｀", defaultFullwidth: true, group: "symbol" },
    { key: "{", fullwidth: "｛", defaultFullwidth: true, group: "symbol" },
    { key: "|", fullwidth: "｜", defaultFullwidth: true, group: "symbol" },
    { key: "}", fullwidth: "｝", defaultFullwidth: true, group: "symbol" },
    { key: "~", fullwidth: "～", defaultFullwidth: true, group: "symbol" },
];

const SYMBOL_WIDTH_DEFAULTS = SYMBOL_WIDTH_ENTRIES.reduce<Record<string, boolean>>(
    (acc, entry) => {
        acc[entry.key] = entry.defaultFullwidth;
        return acc;
    },
    {},
);

const DIGIT_ENTRIES = SYMBOL_WIDTH_ENTRIES.filter((entry) => entry.group === "digit");
const SYMBOL_ENTRIES = SYMBOL_WIDTH_ENTRIES.filter((entry) => entry.group === "symbol");

const buildSymbolWidthState = (symbolFullwidth?: Record<string, unknown>) => {
    const nextState = { ...SYMBOL_WIDTH_DEFAULTS };
    if (!symbolFullwidth) {
        return nextState;
    }

    for (const entry of SYMBOL_WIDTH_ENTRIES) {
        const value = symbolFullwidth[entry.key];
        if (typeof value === "boolean") {
            nextState[entry.key] = value;
        }
    }

    return nextState;
};

const displayKey = (key: string) => {
    if (key === "\"") {
        return "\\\"";
    }
    if (key === "\\") {
        return "\\\\";
    }
    return key;
};

export const General = () => {
    const [shortcutValue, setShortcutValue] = useState({
        ctrlSpaceToggle: true,
        altBackquoteToggle: true,
    });
    const [symbolWidthValue, setSymbolWidthValue] =
        useState<Record<string, boolean>>(SYMBOL_WIDTH_DEFAULTS);

    useEffect(() => {
        invoke<any>("get_config")
            .then((data) => {
                const shortcuts = data.shortcuts ?? {};
                setShortcutValue({
                    ctrlSpaceToggle: shortcuts.ctrl_space_toggle ?? true,
                    altBackquoteToggle: shortcuts.alt_backquote_toggle ?? true,
                });
                setSymbolWidthValue(
                    buildSymbolWidthState(data.character_width?.symbol_fullwidth),
                );
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
        const data = await updateConfig((data) => {
            data.shortcuts = data.shortcuts ?? {};
            data.shortcuts.ctrl_space_toggle = nextValue;
        });

        if (data) {
            setShortcutValue((prev) => ({ ...prev, ctrlSpaceToggle: nextValue }));
        }
    };

    const handleAltBackquoteToggle = async () => {
        const nextValue = !shortcutValue.altBackquoteToggle;
        const data = await updateConfig((data) => {
            data.shortcuts = data.shortcuts ?? {};
            data.shortcuts.alt_backquote_toggle = nextValue;
        });

        if (data) {
            setShortcutValue((prev) => ({ ...prev, altBackquoteToggle: nextValue }));
        }
    };

    const updateSymbolWidth = async (nextValue: Record<string, boolean>) => {
        const data = await updateConfig((data) => {
            data.character_width = data.character_width ?? {};
            data.character_width.symbol_fullwidth = nextValue;
        });

        if (data) {
            setSymbolWidthValue(
                buildSymbolWidthState(data.character_width?.symbol_fullwidth),
            );
        }
    };

    const handleSymbolWidthToggle = async (key: string) => {
        const nextValue = {
            ...symbolWidthValue,
            [key]: !Boolean(symbolWidthValue[key]),
        };
        await updateSymbolWidth(nextValue);
    };

    const handleSymbolPreset = async (preset: "half" | "full" | "default") => {
        const nextValue = SYMBOL_WIDTH_ENTRIES.reduce<Record<string, boolean>>(
            (acc, entry) => {
                if (preset === "half") {
                    acc[entry.key] = false;
                } else if (preset === "full") {
                    acc[entry.key] = true;
                } else {
                    acc[entry.key] = entry.defaultFullwidth;
                }
                return acc;
            },
            {},
        );

        await updateSymbolWidth(nextValue);
    };

    return (
        <div className="space-y-8">
            <section className="space-y-2">
                <h1 className="text-sm font-bold text-foreground">バージョンと更新プログラム</h1>
                <div className="flex items-center space-x-4 rounded-md border p-4">
                    <RefreshCcw />
                    <div className="flex-1 space-y-1">
                        <p className="text-sm font-medium leading-none">
                            v0.1.0-alpha.1
                        </p>
                    </div>
                    <Button  variant="secondary">
                        <a href="https://github.com/fkunn1326/azooKey-Windows/releases" className="flex items-center gap-x-2" target="_blank" rel="noopener noreferrer">
                            <ExternalLink />
                            更新を確認する
                        </a>
                    </Button>
                </div>
            </section>
            <section className="space-y-2">
                <h1 className="text-sm font-bold text-foreground">入力モード切替ショートカット</h1>
                <div className="flex items-center space-x-4 rounded-md border p-4">
                    <Keyboard />
                    <div className="flex-1 space-y-1">
                        <p className="text-sm font-medium leading-none">
                            Ctrl + Space を有効化
                        </p>
                        <p className="text-xs text-muted-foreground">
                            英数/かな切り替えのショートカットとして Ctrl + Space を使用します
                        </p>
                    </div>
                    <Switch checked={shortcutValue.ctrlSpaceToggle} onCheckedChange={handleCtrlSpaceToggle} />
                </div>
                <div className="flex items-center space-x-4 rounded-md border p-4">
                    <Keyboard />
                    <div className="flex-1 space-y-1">
                        <p className="text-sm font-medium leading-none">
                            Alt + ` を有効化
                        </p>
                        <p className="text-xs text-muted-foreground">
                            英数/かな切り替えのショートカットとして Alt + ` を使用します
                        </p>
                    </div>
                    <Switch checked={shortcutValue.altBackquoteToggle} onCheckedChange={handleAltBackquoteToggle} />
                </div>
            </section>
            <section className="space-y-3">
                <h1 className="text-sm font-bold text-foreground">数字・記号の入力幅</h1>
                <div className="space-y-3 rounded-md border p-4">
                    <p className="text-xs text-muted-foreground">
                        日本語入力時に、数字・記号ごとに全角/半角を切り替えます。
                    </p>
                    <div className="flex flex-wrap gap-2">
                        <Button variant="secondary" size="sm" onClick={() => void handleSymbolPreset("half")}>
                            すべて半角
                        </Button>
                        <Button variant="secondary" size="sm" onClick={() => void handleSymbolPreset("full")}>
                            すべて全角
                        </Button>
                        <Button variant="outline" size="sm" onClick={() => void handleSymbolPreset("default")}>
                            既定に戻す
                        </Button>
                    </div>
                </div>

                <div className="space-y-2">
                    <p className="text-xs font-semibold text-muted-foreground">数字</p>
                    <div className="grid gap-2 md:grid-cols-2">
                        {DIGIT_ENTRIES.map((entry) => (
                            <div key={entry.key} className="flex items-center space-x-4 rounded-md border p-3">
                                <div className="min-w-12 text-center">
                                    <p className="font-mono text-sm">{displayKey(entry.key)}</p>
                                </div>
                                <div className="flex-1 space-y-1">
                                    <p className="text-sm font-medium leading-none">全角時: {entry.fullwidth}</p>
                                    <p className="text-xs text-muted-foreground">入力「{displayKey(entry.key)}」</p>
                                </div>
                                <Switch
                                    checked={Boolean(symbolWidthValue[entry.key])}
                                    onCheckedChange={() => void handleSymbolWidthToggle(entry.key)}
                                />
                            </div>
                        ))}
                    </div>
                </div>

                <div className="space-y-2">
                    <p className="text-xs font-semibold text-muted-foreground">記号</p>
                    <div className="grid gap-2 md:grid-cols-2">
                        {SYMBOL_ENTRIES.map((entry) => (
                            <div key={entry.key} className="flex items-center space-x-4 rounded-md border p-3">
                                <div className="min-w-12 text-center">
                                    <p className="font-mono text-sm">{displayKey(entry.key)}</p>
                                </div>
                                <div className="flex-1 space-y-1">
                                    <p className="text-sm font-medium leading-none">全角時: {entry.fullwidth}</p>
                                    <p className="text-xs text-muted-foreground">入力「{displayKey(entry.key)}」</p>
                                </div>
                                <Switch
                                    checked={Boolean(symbolWidthValue[entry.key])}
                                    onCheckedChange={() => void handleSymbolWidthToggle(entry.key)}
                                />
                            </div>
                        ))}
                    </div>
                </div>
            </section>
            {/* <section className="space-y-2">
                <h1 className="text-sm font-bold text-foreground">診断とフィードバック</h1>
                <div className="flex items-center space-x-4 rounded-md border p-4">
                    <FileChartColumn />
                    <div className="flex-1 space-y-1">
                        <p className="text-sm font-medium leading-none">
                            診断データ
                        </p>
                        <p className="text-xs text-muted-foreground">
                            診断データを保存し、バグの修正に役立てます
                        </p>
                    </div>
                    <Switch />
                </div>
            </section> */}
        </div>
    )
}
