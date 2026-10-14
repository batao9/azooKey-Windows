import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCcw } from "lucide-react";
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
import { getConfigAfterPendingUpdates, saveConfigWithToast } from "@/lib/config";
import { SettingsRow, SettingsSection } from "@/components/settings-section";

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

    const [loadState, setLoadState] = useState<"loading" | "ready" | "error">("loading");
    const [loadAttempt, setLoadAttempt] = useState(0);
    useEffect(() => {
        let cancelled = false;
        setLoadState("loading");
        getConfigAfterPendingUpdates()
            .then((data) => {
                if (cancelled) return;
                setDebugConfig(normalizeDebugConfig(data.debug));
                setLoadState("ready");
            })
            .catch(() => { if (!cancelled) setLoadState("error"); });
        return () => { cancelled = true; };
    }, [loadAttempt]);

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
        <>
            <SettingsSection title="ログ">
                {loadState !== "ready" ? <div className="p-4 space-y-2" role="status">
                    <p className={loadState === "error" ? "text-sm text-destructive" : "settings-description"}>
                        {loadState === "error" ? "デバッグ設定を読み込めませんでした。" : "設定を読み込んでいます…"}
                    </p>
                    {loadState === "error" && <Button variant="outline" onClick={() => setLoadAttempt((attempt) => attempt + 1)}>再試行</Button>}
                </div> : <>
                    <SettingsRow label="サーバーログを記録" description="server.log と性能計測ログを記録します。" htmlFor="server-log-enabled">
                        <Switch id="server-log-enabled" checked={debugConfig.server_log_enabled}
                            onCheckedChange={(checked) => void updateServerLogEnabled(checked)} />
                    </SettingsRow>
                    {debugConfig.server_log_enabled && <div className="settings-subgroup">
                        <SettingsRow label="ログレベル" htmlFor="server-log-level">
                            <Select value={debugConfig.server_log_level} onValueChange={(value) => void updateServerLogLevel(value as ServerLogLevel)}>
                                <SelectTrigger id="server-log-level"><SelectValue /></SelectTrigger>
                                <SelectContent>
                                    <SelectItem value="off">Off</SelectItem>
                                    <SelectItem value="error">Error</SelectItem>
                                    <SelectItem value="warn">Warn</SelectItem>
                                    <SelectItem value="info">Info</SelectItem>
                                    <SelectItem value="debug">Debug</SelectItem>
                                </SelectContent>
                            </Select>
                        </SettingsRow>
                        {debugConfig.server_log_level === "debug" && <p className="px-4 pb-3 text-xs text-destructive" role="status">
                            Debugではキー入力も記録されます。機密情報の入力に注意してください。
                        </p>}
                    </div>}
                </>}
            </SettingsSection>
            <SettingsSection title="障害からの復旧">
                <SettingsRow label="クラッシュトレース" description="直前の候補生成状態を小さなファイルに残します。" htmlFor="crash-trace">
                    <Switch id="crash-trace" checked={debugConfig.server_crash_trace_enabled} disabled={loadState !== "ready"}
                        onCheckedChange={(checked) => void updateCrashTraceEnabled(checked)} />
                </SettingsRow>
                <SettingsRow label="変換サーバー">
                    <Button variant="outline" onClick={() => void restartServer()} disabled={isRestartingServer}>
                        <RefreshCcw />{isRestartingServer ? "再起動中…" : "再起動"}
                    </Button>
                </SettingsRow>
            </SettingsSection>
        </>
    );
};
