import { Textarea } from "@/components/ui/textarea";
import { Switch } from "@/components/ui/switch";
import { Bot, User, Cpu } from "lucide-react";
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from "@/components/ui/select"
import { useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner"
import { invoke } from '@tauri-apps/api/core';
import {
    Tooltip,
    TooltipContent,
    TooltipProvider,
    TooltipTrigger,
} from "@/components/ui/tooltip"
import {
    getConfigAfterPendingUpdates,
    saveConfigWithToast,
} from "@/lib/config";
import {
    createDebouncedSaver,
    type ConfigSaveState,
} from "@/lib/config-save-controller.js";

const PROFILE_SAVE_DELAY_MS = 500;

const PROFILE_SAVE_LABELS: Record<ConfigSaveState, string> = {
    dirty: "未保存の変更があります",
    saving: "保存中…",
    saved: "保存済み",
    error: "保存できませんでした。入力内容は画面に保持されています。",
};

const ToolTipSelectItem = ({
    name,
    value,
    disabled,
    tooltip
}: {
    name: string;
    value: string;
    disabled: boolean;
    tooltip: string;
}) => {
    return (
        <TooltipProvider>
            <Tooltip>
                <TooltipTrigger>
                    <SelectItem value={value} disabled={disabled}>
                        {name}
                    </SelectItem>
                </TooltipTrigger>
                {disabled && <TooltipContent side="left">
                    {tooltip}
                </TooltipContent>}
            </Tooltip>
        </TooltipProvider>
    )
}

export const Zenzai = () => {
    const [value, setValue] = useState({
        enable: false,
        profile: "",
        backend: "",
    });

    const [capability, setCapability] = useState({
        cpu: true,
        cuda: false,
        vulkan: false,
    });
    const [profileSaveState, setProfileSaveState] =
        useState<ConfigSaveState>("saved");
    const profileEdited = useRef(false);
    const profileSaver = useMemo(
        () =>
            createDebouncedSaver<string, any>({
                delayMs: PROFILE_SAVE_DELAY_MS,
                onStateChange: setProfileSaveState,
                save: (profile) =>
                    saveConfigWithToast((config) => {
                        config.zenzai.profile = profile;
                    }),
            }),
        [],
    );

    // Load config on component mount
    useEffect(() => {
        profileSaver.resume();
        getConfigAfterPendingUpdates()
            .then((data) => {
                const zenzai = data.zenzai;
                setValue((previous) => ({
                    enable: zenzai.enable,
                    profile: profileEdited.current
                        ? previous.profile
                        : zenzai.profile,
                    backend: zenzai.backend,
                }));
            })
            .catch(() => {
                // Keep default values if config fetch fails
            });

        invoke("check_capability").then((capability: any) => {
            setCapability({
                cpu: capability["cpu"],
                cuda: capability["cuda"],
                vulkan: capability["vulkan"],
            });
        })

        return () => {
            void profileSaver.dispose();
        };
    }, [profileSaver]);

    const updateConfig = (updater: (config: any) => void) =>
        saveConfigWithToast(updater);

    const handleZenzaiChange = async () => {
        const data = await updateConfig((data) => {
            data.zenzai.enable = !value.enable;
        });
        
        if (data) {
            setValue((prev) => ({ ...prev, enable: data.zenzai.enable }));
        }
    };

    const handleProfileChange = (event: React.ChangeEvent<HTMLTextAreaElement>) => {
        const newProfile = event.target.value;
        profileEdited.current = true;
        setValue((prev) => ({ ...prev, profile: newProfile }));
        profileSaver.schedule(newProfile);
    };

    const handleBackendChange = async (backend: string) => {
        const data = await updateConfig((data) => {
            data.zenzai.backend = backend;
        });
        
        if (data) {
            setValue((prev) => ({ ...prev, backend }));
            toast("バックエンドが変更されました", {
                description: "変更を適用するには、PCを再起動してください",
                duration: 10000,
            });
        }
    };

    return (
        <div className="space-y-8">
            <section className="space-y-2">
                <h1 className="text-sm font-bold text-foreground">Zenzai</h1>
                <div className="flex items-center space-x-4 rounded-md border p-4">
                    <Bot />
                    <div className="flex-1 space-y-1">
                        <p className="text-sm font-medium leading-none">
                            Zenzaiを有効化
                        </p>
                        <p className="text-xs text-muted-foreground">
                            Zenzaiを有効にして、変換精度を向上させます。
                        </p>
                        <p className="text-xs text-muted-foreground">
                            CPUバックエンドは AVX 対応 CPU が必要です。未対応環境では
                            標準変換へ自動フォールバックします
                        </p>
                    </div>
                    <Switch checked={value.enable} onCheckedChange={handleZenzaiChange} />
                </div>
                <div className="space-y-4 rounded-md border p-4">
                    <div className="flex items-center space-x-4 ">
                        <User />
                        <div className="flex-1 space-y-1">
                            <p className="text-sm font-medium leading-none">
                                変換プロファイル
                            </p>
                            <p className="text-xs text-muted-foreground">
                                Zenzaiで利用されるユーザー情報を設定します
                            </p>
                        </div>
                    </div>
                    <Textarea
                        placeholder="例）山田太郎、数学科の学生。"
                        value={value.profile}
                        disabled={!value.enable}
                        aria-invalid={profileSaveState === "error"}
                        onChange={handleProfileChange}
                        onBlur={() => {
                            void profileSaver.flush();
                        }}
                    />
                    <p
                        className={
                            profileSaveState === "error"
                                ? "text-xs text-destructive"
                                : "text-xs text-muted-foreground"
                        }
                        aria-live="polite"
                    >
                        {PROFILE_SAVE_LABELS[profileSaveState]}
                    </p>
                </div>
                <div className="flex items-center space-x-4 rounded-md border p-4">
                    <Cpu />
                    <div className="flex-1 space-y-1">
                        <p className="text-sm font-medium leading-none">
                            バックエンド
                        </p>
                        <p className="text-xs text-muted-foreground">
                            Zenzaiを利用するバックエンドを選択します
                        </p>
                    </div>
                    <Select disabled={!value.enable} value={value.backend} onValueChange={handleBackendChange}>
                        <SelectTrigger className="w-48">
                            <SelectValue placeholder="バックエンドを選択" />
                        </SelectTrigger>
                        <SelectContent className="flex flex-col">
                            <ToolTipSelectItem name="CPU (非推奨)" value="cpu" disabled={!capability.cpu} tooltip="AVX 対応 CPU が必要です" />
                            <ToolTipSelectItem name="CUDA (NVIDIA GPU)" value="cuda" disabled={!capability.cuda} tooltip="CUDA Toolkit 12をインストールする必要があります" />
                            <ToolTipSelectItem name="Vulkan" value="vulkan" disabled={!capability.vulkan} tooltip="お使いのPCはVulkanに対応していません" />
                        </SelectContent>
                    </Select>
                </div>
            </section>
        </div>
    )
}
