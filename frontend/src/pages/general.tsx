import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { RefreshCcw, ExternalLink, Keyboard } from "lucide-react";
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";

export const General = () => {
    const [shortcutValue, setShortcutValue] = useState({
        ctrlSpaceToggle: true,
        altBackquoteToggle: true,
    });

    useEffect(() => {
        invoke<any>("get_config")
            .then((data) => {
                const shortcuts = data.shortcuts ?? {};
                setShortcutValue({
                    ctrlSpaceToggle: shortcuts.ctrl_space_toggle ?? true,
                    altBackquoteToggle: shortcuts.alt_backquote_toggle ?? true,
                });
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
