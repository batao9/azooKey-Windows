import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { Bug, FileText } from "lucide-react";

import { Switch } from "@/components/ui/switch";

type DeveloperOptionsState = {
    enable: boolean;
    logging: boolean;
};

const DEFAULT_DEVELOPER_OPTIONS: DeveloperOptionsState = {
    enable: false,
    logging: false,
};

const normalizeDeveloperOptions = (
    value?: Record<string, unknown>,
): DeveloperOptionsState => {
    const enable = typeof value?.enable === "boolean" ? value.enable : false;
    return {
        enable,
        logging:
            enable && typeof value?.logging === "boolean"
                ? value.logging
                : false,
    };
};

export const DeveloperOptions = () => {
    const [value, setValue] = useState<DeveloperOptionsState>(
        DEFAULT_DEVELOPER_OPTIONS,
    );

    useEffect(() => {
        invoke<any>("get_config")
            .then((data) => {
                setValue(normalizeDeveloperOptions(data.developer_options));
            })
            .catch(() => {
                toast("開発者オプションの読み込みに失敗しました");
            });
    }, []);

    const updateConfig = async (
        updater: (developerOptions: DeveloperOptionsState) => DeveloperOptionsState,
    ) => {
        try {
            const data = await invoke<any>("get_config");
            const current = normalizeDeveloperOptions(data.developer_options);
            const next = updater(current);
            data.developer_options = next.enable
                ? next
                : { enable: false, logging: false };
            await invoke("update_config", { newConfig: data });
            setValue(data.developer_options);
        } catch (_error) {
            toast("開発者オプションの更新に失敗しました");
        }
    };

    const handleDeveloperOptionsChange = (checked: boolean) => {
        updateConfig((current) => ({
            enable: checked,
            logging: checked ? current.logging : false,
        }));
    };

    const handleLoggingChange = (checked: boolean) => {
        updateConfig((current) => ({
            enable: current.enable,
            logging: current.enable ? checked : false,
        }));
    };

    return (
        <div className="space-y-8">
            <section className="space-y-2">
                <h1 className="text-sm font-bold text-foreground">開発者オプション</h1>
                <div className="flex items-center space-x-4 rounded-md border p-4">
                    <Bug />
                    <div className="flex-1 space-y-1">
                        <p className="text-sm font-medium leading-none">
                            開発者オプションを有効化
                        </p>
                    </div>
                    <Switch
                        checked={value.enable}
                        onCheckedChange={handleDeveloperOptionsChange}
                    />
                </div>

                {value.enable && (
                    <div className="flex items-center space-x-4 rounded-md border p-4">
                        <FileText />
                        <div className="flex-1 space-y-1">
                            <p className="text-sm font-medium leading-none">
                                ログを有効化
                            </p>
                        </div>
                        <Switch
                            checked={value.logging}
                            onCheckedChange={handleLoggingChange}
                        />
                    </div>
                )}
            </section>
        </div>
    );
};
