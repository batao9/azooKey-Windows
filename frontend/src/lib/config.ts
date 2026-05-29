import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";

type ConfigStartupNotice = {
    kind: string;
    message: string;
    backup_path?: string | null;
};

type UpdateConfigResponse = {
    saved: boolean;
    server_applied: boolean;
    message?: string | null;
};

type UpdateInstallResult = {
    status: string;
    exit_code?: number | null;
    needs_restart: boolean;
    message: string;
    completed_at: string;
    installer_path?: string | null;
    install_log_path?: string | null;
};

const SERVER_APPLY_WARNING =
    "設定を保存しましたが、IME への反映に失敗しました。再起動後に反映されます。";

export const showConfigStartupNoticeOnce = async () => {
    try {
        const notice = await invoke<ConfigStartupNotice | null>(
            "take_config_startup_notice",
        );
        if (!notice) {
            return;
        }

        toast(notice.message, {
            description: notice.backup_path
                ? `退避先: ${notice.backup_path}`
                : undefined,
            duration: 10000,
        });
    } catch (_error) {
        // Startup notice is best effort; normal settings loading still handles errors.
    }
};

export const showUpdateInstallResultOnce = async () => {
    try {
        const result = await invoke<UpdateInstallResult | null>(
            "take_update_install_result",
        );
        if (!result) {
            return;
        }

        toast(result.message, {
            description: result.needs_restart
                ? "再起動後に新しいバージョンが有効になります"
                : result.status === "failed"
                  ? result.install_log_path
                      ? `ログ: ${result.install_log_path}`
                      : undefined
                  : undefined,
            duration: result.status === "failed" || result.needs_restart ? 12000 : 6000,
        });
    } catch (_error) {
        // Update result notification is best effort.
    }
};

export const saveConfigWithToast = async (
    updater: (config: any) => void,
    failureMessage = "設定の更新に失敗しました",
) => {
    try {
        const data = await invoke<any>("get_config");
        updater(data);
        const result = await invoke<UpdateConfigResponse>("update_config", {
            newConfig: data,
        });

        if (result.saved && !result.server_applied) {
            toast(SERVER_APPLY_WARNING, {
                description: result.message ?? undefined,
                duration: 10000,
            });
        }

        return data;
    } catch (_error) {
        toast(failureMessage);
        return null;
    }
};
