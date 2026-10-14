import { ExternalLink } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { SettingsRow, SettingsSection } from "@/components/settings-section";
import { useUpdates } from "@/components/update-provider";

function ExternalLinkButton({ href, children }: { href: string; children: React.ReactNode }) {
    return (
        <Button variant="outline" asChild>
            <a href={href} target="_blank" rel="noopener noreferrer" onClick={(event) => {
                event.preventDefault();
                void openUrl(href).catch(() => toast("リンクを開けませんでした"));
            }}>{children}<ExternalLink className="size-4" /></a>
        </Button>
    );
}

export const About = () => {
    const { version, result, status, error, check, start } = useUpdates();
    const available = result?.update_available === true;
    return (
        <div className="settings-content">
            <SettingsSection title="バージョンと更新">
                <div className="space-y-3 p-4">
                    <p className="text-sm font-medium">{version ? `v${version}` : "バージョンを取得できませんでした"}</p>
                    <p className="settings-description" role="status">
                        {status === "checking" ? "更新を確認中…"
                            : status === "starting" ? "アップデートを開始中…"
                            : available ? `新しいバージョン v${result.latest_version} が利用できます。`
                            : error ? "更新を確認できませんでした。"
                            : result ? "最新版を利用中です。" : ""}
                    </p>
                    {error && <p className="text-xs text-destructive break-words" role="alert">{error}</p>}
                    <div className="flex flex-wrap items-center gap-2">
                        {available && <Button onClick={() => void start()} disabled={status !== "idle"}>アップデート</Button>}
                        {result?.release_url && <ExternalLinkButton href={result.release_url}>リリース内容を見る</ExternalLinkButton>}
                        <Button variant="outline" disabled={status !== "idle"} onClick={() => void check()}>更新を確認</Button>
                    </div>
                </div>
            </SettingsSection>
            <SettingsSection title="リンク">
                <SettingsRow label="GitHub">
                    <ExternalLinkButton href="https://github.com/batao9/azooKey-Windows">開く</ExternalLinkButton>
                </SettingsRow>
                <SettingsRow label="不具合報告・要望">
                    <ExternalLinkButton href="https://github.com/batao9/azooKey-Windows/issues">開く</ExternalLinkButton>
                </SettingsRow>
            </SettingsSection>
        </div>
    );
};
