import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FileText, RefreshCcw, Server } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { saveConfigWithToast } from "@/lib/config";

type DebugConfigState = {
    server_log_enabled: boolean;
};

const DEFAULT_DEBUG_CONFIG: DebugConfigState = {
    server_log_enabled: false,
};

const normalizeDebugConfig = (value?: Record<string, unknown>): DebugConfigState => ({
    server_log_enabled:
        typeof value?.server_log_enabled === "boolean"
            ? value.server_log_enabled
            : DEFAULT_DEBUG_CONFIG.server_log_enabled,
});

export const DebugSettings = () => {
    const [isRestartingServer, setIsRestartingServer] = useState(false);
    const [debugConfig, setDebugConfig] =
        useState<DebugConfigState>(DEFAULT_DEBUG_CONFIG);

    useEffect(() => {
        invoke<any>("get_config")
            .then((data) => {
                setDebugConfig(normalizeDebugConfig(data.debug));
            })
            .catch(() => {
                // Keep default values if config fetch fails.
            });
    }, []);

    const restartServer = async () => {
        if (isRestartingServer) {
            return;
        }

        setIsRestartingServer(true);
        try {
            await invoke("restart_server");
            toast("サーバーを再起動しました");
        } catch (error) {
            const message = error instanceof Error ? error.message : String(error);
            toast("サーバーの再起動に失敗しました", {
                description: message,
                duration: 10000,
            });
        } finally {
            setIsRestartingServer(false);
        }
    };

    const updateServerLogEnabled = async (enabled: boolean) => {
        const data = await saveConfigWithToast((config) => {
            config.debug = {
                ...DEFAULT_DEBUG_CONFIG,
                ...(config.debug ?? {}),
                server_log_enabled: enabled,
            };
        });

        if (data) {
            setDebugConfig(normalizeDebugConfig(data.debug));
        }
    };

    return (
        <section className="space-y-3">
            <h1 className="text-sm font-bold text-foreground">デバッグ用設定</h1>
            <div className="flex items-center gap-4 rounded-md border p-4">
                <FileText />
                <div className="flex-1 space-y-1">
                    <p className="text-sm font-medium leading-none">サーバーログ</p>
                    <p className="text-xs text-muted-foreground">
                        server.log と性能計測ログを記録します
                    </p>
                </div>
                <Switch
                    checked={debugConfig.server_log_enabled}
                    onCheckedChange={(checked) => void updateServerLogEnabled(checked)}
                />
            </div>
            <div className="flex items-center gap-4 rounded-md border p-4">
                <Server />
                <div className="flex-1 space-y-1">
                    <p className="text-sm font-medium leading-none">サーバー再起動</p>
                    <p className="text-xs text-muted-foreground">
                        変換サーバーを停止して起動し直します
                    </p>
                </div>
                <Button
                    variant="secondary"
                    onClick={() => void restartServer()}
                    disabled={isRestartingServer}
                >
                    <RefreshCcw />
                    {isRestartingServer ? "再起動中" : "サーバー再起動"}
                </Button>
            </div>
        </section>
    );
};
