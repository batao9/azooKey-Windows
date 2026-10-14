import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { Textarea } from "@/components/ui/textarea";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { SettingsRow, SettingsSection } from "@/components/settings-section";
import { getConfigAfterPendingUpdates, saveConfigWithToast } from "@/lib/config";
import { createDebouncedSaver, type ConfigSaveState } from "@/lib/config-save-controller.js";

const PROFILE_SAVE_DELAY_MS = 500;

export const Zenzai = () => {
    const [value, setValue] = useState({ enable: false, profile: "", backend: "" });
    const [capability, setCapability] = useState({ cpu: false, cuda: false, vulkan: false });
    const [capabilityError, setCapabilityError] = useState(false);
    const [loadState, setLoadState] = useState<"loading" | "ready" | "error">("loading");
    const [loadAttempt, setLoadAttempt] = useState(0);
    const [restartRequired, setRestartRequired] = useState(false);
    const [profileSaveState, setProfileSaveState] = useState<ConfigSaveState>("saved");
    const profileEdited = useRef(false);
    const profileSaver = useMemo(() => createDebouncedSaver<string, any>({
        delayMs: PROFILE_SAVE_DELAY_MS,
        onStateChange: setProfileSaveState,
        save: (profile) => saveConfigWithToast((config) => { config.zenzai.profile = profile; }),
    }), []);

    useEffect(() => {
        let cancelled = false;
        profileSaver.resume();
        setLoadState("loading");
        getConfigAfterPendingUpdates().then((data) => {
            if (cancelled) return;
            const zenzai = data.zenzai;
            if (!zenzai || typeof zenzai.enable !== "boolean" || typeof zenzai.profile !== "string" || typeof zenzai.backend !== "string") {
                throw new Error("Zenzaiの設定が不正です");
            }
            setValue((previous) => ({
                enable: zenzai.enable,
                profile: profileEdited.current ? previous.profile : zenzai.profile,
                backend: zenzai.backend,
            }));
            setLoadState("ready");
        }).catch(() => { if (!cancelled) setLoadState("error"); });
        setCapabilityError(false);
        invoke<typeof capability>("check_capability").then((data) => {
            if (!cancelled) setCapability(data);
        }).catch(() => { if (!cancelled) setCapabilityError(true); });
        return () => {
            cancelled = true;
            void profileSaver.dispose();
        };
    }, [profileSaver, loadAttempt]);

    const handleZenzaiChange = async (enable: boolean) => {
        const data = await saveConfigWithToast((config) => { config.zenzai.enable = enable; });
        if (data) setValue((previous) => ({ ...previous, enable: data.zenzai.enable }));
    };
    const handleProfileChange = (event: React.ChangeEvent<HTMLTextAreaElement>) => {
        const profile = event.target.value;
        profileEdited.current = true;
        setValue((previous) => ({ ...previous, profile }));
        profileSaver.schedule(profile);
    };
    const handleBackendChange = async (backend: string) => {
        if (backend === value.backend) return;
        const data = await saveConfigWithToast((config) => { config.zenzai.backend = backend; });
        if (data) {
            setValue((previous) => ({ ...previous, backend: data.zenzai.backend }));
            setRestartRequired(true);
            toast("バックエンドが変更されました", {
                description: "変更を適用するには、PCを再起動してください",
                duration: 10000,
            });
        }
    };

    return (
        <SettingsSection title="Zenzai">
            {loadState !== "ready" ? <div className="p-4 space-y-2" role="status">
                <p className={loadState === "error" ? "text-sm text-destructive" : "settings-description"}>
                    {loadState === "error" ? "Zenzaiの設定を読み込めませんでした。" : "設定を読み込んでいます…"}
                </p>
                {loadState === "error" && <Button variant="outline" onClick={() => setLoadAttempt((attempt) => attempt + 1)}>再試行</Button>}
            </div> : <>
                <SettingsRow label="Zenzaiを使う" description="AIモデルを使って変換精度を高めます。" htmlFor="zenzai-enable">
                    <Switch id="zenzai-enable" checked={value.enable} onCheckedChange={(enable) => void handleZenzaiChange(enable)} />
                </SettingsRow>
                {value.enable && <div className="settings-subgroup">
                    <SettingsRow label="バックエンド" htmlFor="zenzai-backend">
                        <Select value={value.backend} disabled={capabilityError} onValueChange={(backend) => void handleBackendChange(backend)}>
                            <SelectTrigger id="zenzai-backend"><SelectValue placeholder="バックエンドを選択" /></SelectTrigger>
                            <SelectContent>
                                <SelectItem value="cpu" disabled={!capability.cpu}>CPU (非推奨){!capability.cpu && " — AVX 対応 CPU が必要"}</SelectItem>
                                <SelectItem value="cuda" disabled={!capability.cuda}>CUDA (NVIDIA GPU){!capability.cuda && " — CUDA Toolkit 12 が必要"}</SelectItem>
                                <SelectItem value="vulkan" disabled={!capability.vulkan}>Vulkan{!capability.vulkan && " — このPCでは未対応"}</SelectItem>
                            </SelectContent>
                        </Select>
                    </SettingsRow>
                    {capabilityError && <p className="px-4 pb-3 text-xs text-destructive" role="alert">バックエンドの対応状況を取得できませんでした。</p>}
                    {value.backend === "cpu" && <p className="settings-description px-4 pb-3">
                        CPUバックエンドには AVX 対応 CPU が必要です。未対応の場合は標準変換を使います。
                    </p>}
                    <SettingsRow label="変換プロファイル" description="名前・専門分野などを変換に反映します。" htmlFor="zenzai-profile">
                        <div className="w-full space-y-2">
                            <Textarea id="zenzai-profile" placeholder="例）山田太郎、数学科の学生。" value={value.profile}
                                aria-invalid={profileSaveState === "error"}
                                aria-describedby={profileSaveState === "error" ? "zenzai-profile-error" : undefined}
                                onChange={handleProfileChange} onBlur={() => { void profileSaver.flush(); }} />
                            {profileSaveState === "error" && <p id="zenzai-profile-error" className="text-xs text-destructive" role="alert">
                                保存できませんでした。入力内容は保持されています。
                            </p>}
                        </div>
                    </SettingsRow>
                </div>}
                {restartRequired && <p className="settings-description p-4" role="status">バックエンドの変更を適用するには、PCを再起動してください。</p>}
            </>}
        </SettingsSection>
    );
};
