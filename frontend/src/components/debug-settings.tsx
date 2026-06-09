import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FileText, RefreshCcw, Server, ShieldAlert, SlidersHorizontal } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { saveConfigWithToast } from "@/lib/config";

type ServerLogLevel = "off" | "error" | "warn" | "info" | "debug";

type DebugConfigState = {
    server_log_enabled: boolean;
    server_log_level: ServerLogLevel;
    server_crash_trace_enabled: boolean;
};

const DEFAULT_DEBUG_CONFIG: DebugConfigState = {
    server_log_enabled: false,
    server_log_level: "warn",
    server_crash_trace_enabled: true,
};

const SERVER_LOG_LEVELS = new Set<ServerLogLevel>([
    "off",
    "error",
    "warn",
    "info",
    "debug",
]);

const normalizeServerLogLevel = (value: unknown): ServerLogLevel =>
    typeof value === "string" && SERVER_LOG_LEVELS.has(value as ServerLogLevel)
        ? (value as ServerLogLevel)
        : DEFAULT_DEBUG_CONFIG.server_log_level;

const normalizeDebugConfig = (value?: Record<string, unknown>): DebugConfigState => ({
    server_log_enabled:
        typeof value?.server_log_enabled === "boolean"
            ? value.server_log_enabled
            : DEFAULT_DEBUG_CONFIG.server_log_enabled,
    server_log_level: normalizeServerLogLevel(value?.server_log_level),
    server_crash_trace_enabled:
        typeof value?.server_crash_trace_enabled === "boolean"
            ? value.server_crash_trace_enabled
            : DEFAULT_DEBUG_CONFIG.server_crash_trace_enabled,
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

    const updateDebugConfig = async (patch: Partial<DebugConfigState>) => {
        const data = await saveConfigWithToast((config) => {
            config.debug = {
                ...DEFAULT_DEBUG_CONFIG,
                ...(config.debug ?? {}),
                ...patch,
            };
        });

        if (data) {
            setDebugConfig(normalizeDebugConfig(data.debug));
        }
    };

    const updateServerLogEnabled = async (enabled: boolean) => {
        await updateDebugConfig({ server_log_enabled: enabled });
    };

    const updateServerLogLevel = async (level: ServerLogLevel) => {
        await updateDebugConfig({ server_log_level: level });
    };

    const updateCrashTraceEnabled = async (enabled: boolean) => {
        await updateDebugConfig({ server_crash_trace_enabled: enabled });
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
                <SlidersHorizontal />
                <div className="flex-1 space-y-1">
                    <p className="text-sm font-medium leading-none">ログレベル</p>
                    <p className="text-xs text-muted-foreground">
                        Debug でキー入力と性能計測ログを記録します
                    </p>
                </div>
                <Select
                    value={debugConfig.server_log_level}
                    onValueChange={(value) => void updateServerLogLevel(value as ServerLogLevel)}
                >
                    <SelectTrigger className="w-36">
                        <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                        <SelectItem value="off">Off</SelectItem>
                        <SelectItem value="error">Error</SelectItem>
                        <SelectItem value="warn">Warn</SelectItem>
                        <SelectItem value="info">Info</SelectItem>
                        <SelectItem value="debug">Debug</SelectItem>
                    </SelectContent>
                </Select>
            </div>
            <div className="flex items-center gap-4 rounded-md border p-4">
                <ShieldAlert />
                <div className="flex-1 space-y-1">
                    <p className="text-sm font-medium leading-none">クラッシュトレース</p>
                    <p className="text-xs text-muted-foreground">
                        直前の候補生成状態だけを小さなファイルに残します
                    </p>
                </div>
                <Switch
                    checked={debugConfig.server_crash_trace_enabled}
                    onCheckedChange={(checked) => void updateCrashTraceEnabled(checked)}
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
