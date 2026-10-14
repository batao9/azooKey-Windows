import type { ReactNode } from "react";

export function SettingsSection({ title, children }: { title: string; children: ReactNode }) {
    return (
        <section className="settings-section">
            <h2>{title}</h2>
            <div className="settings-card">{children}</div>
        </section>
    );
}

export function SettingsRow({ label, description, htmlFor, children }: {
    label: string;
    description?: string;
    htmlFor?: string;
    children: ReactNode;
}) {
    return (
        <div className="settings-row">
            <div className="min-w-0 space-y-1">
                {htmlFor ? <label htmlFor={htmlFor} className="text-sm font-medium">{label}</label>
                    : <p className="text-sm font-medium">{label}</p>}
                {description && <p className="settings-description">{description}</p>}
            </div>
            <div className="settings-control">{children}</div>
        </div>
    );
}
