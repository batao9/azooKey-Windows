// Run: PLAYWRIGHT_MODULE=<installed playwright module> node --test tests/settings-ui.browser.mjs
// No production dependency is needed. Tauri commands are mocked for browser regression checks.
import test from "node:test";
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { createServer } from "vite";

const root = fileURLToPath(new URL("../", import.meta.url));
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || "playwright");

function installMock() {
    const config = {
        general: {}, shortcuts: {}, character_width: {}, learning: { mode: "enabled" }, debug: {},
        zenzai: { enable: true, profile: "old profile", backend: "vulkan" },
        romaji_table: { rows: [{ input: "ka", output: "か", next_input: "" }] },
        user_dictionary: { entries: Array.from({ length: 25 }, (_, i) => ({ reading: `よみ${i}`, word: `単語${i}` })) },
    };
    window.settingsTest = { config, checks: 0, writes: 0, failCheck: false, failSave: false, failLoad: sessionStorage.getItem("failSettingsLoad") === "true", available: true, holdSave: false, jevApiKeySaved: false, keyChecks: 0, keyCheckArgs: [], holdKeyCheck: false, keyCheckError: null, links: [], callbacks: {}, listeners: {}, nextCallback: 0, destroys: 0 };
    window.isTauri = true;
    window.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: "main" } },
        transformCallback: (callback) => {
            const id = ++window.settingsTest.nextCallback;
            window.settingsTest.callbacks[id] = callback;
            return id;
        },
        invoke: async (command, args) => {
            const state = window.settingsTest;
            switch (command) {
                case "plugin:app|version": return "0.1.0-batao.15";
                case "plugin:event|listen": state.listeners[args.handler] = args; return args.handler;
                case "plugin:event|unlisten": delete state.listeners[args.eventId]; return null;
                case "plugin:window|destroy": state.destroys++; return null;
                case "take_config_startup_notice":
                case "take_update_install_result": return null;
                case "get_config":
                    if (state.failLoad) throw new Error("mock load failure");
                    return structuredClone(config);
                case "update_config":
                    state.writes++;
                    while (state.holdSave) await new Promise((resolve) => setTimeout(resolve, 20));
                    if (state.failSave) throw new Error("mock save failure");
                    Object.assign(config, structuredClone(args.newConfig));
                    return { saved: true, server_applied: true, changed: true, config: structuredClone(config) };
                case "check_for_updates":
                    state.checks++;
                    if (state.failCheck) throw new Error("mock update failure");
                    return { current_version: "0.1.0-batao.15", latest_version: "0.1.0-batao.16", release_url: "https://github.com/batao9/azooKey-Windows/releases/tag/v0.1.0-batao.16", update_available: state.available };
                case "start_update": return {};
                case "check_capability": return { cpu: true, cuda: false, vulkan: true };
                case "has_jev_api_key": return state.jevApiKeySaved;
                case "check_jev_api_key":
                    state.keyChecks++;
                    state.keyCheckArgs.push(args ?? null);
                    while (state.holdKeyCheck) await new Promise((resolve) => setTimeout(resolve, 20));
                    if (state.keyCheckError) throw state.keyCheckError;
                    return null;
                case "get_default_romaji_rows": return [{ input: "ka", output: "か", next_input: "" }, { input: "kk", output: "っ", next_input: "k" }];
                case "plugin:opener|open_url": state.links.push(args.url); return null;
                case "restart_server": return null;
                case "reset_learning_history": return { reset: true, server_applied: true };
                default: throw new Error(`Unmocked command: ${command}`);
            }
        },
    };
}

async function assertInViewport(locator, page) {
    const rect = await locator.boundingBox();
    const { width, height } = page.viewportSize();
    assert.ok(rect && rect.x >= 0 && rect.y >= 0 && rect.x + rect.width <= width + 1 && rect.y + rect.height <= height + 1, "control must remain inside the viewport");
}

test("settings layout, editing safety, navigation and update badge", async () => {
    const server = await createServer({ root, configFile: path.join(root, "vite.config.ts"), server: { host: "127.0.0.1", port: 0 }, logLevel: "error" });
    await server.listen();
    const address = server.httpServer.address();
    const base = `http://127.0.0.1:${address.port}`;
    const browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE }).catch(async (error) => {
        await server.close();
        throw error;
    });
    const context = await browser.newContext({ viewport: { width: 800, height: 600 } });
    await context.addInitScript(installMock);
    const page = await context.newPage();
    const runtimeErrors = [];
    page.on("pageerror", (error) => runtimeErrors.push(error.message));
    const evidence = process.env.SETTINGS_UI_EVIDENCE_DIR;
    const screenshot = async (name) => {
        if (!evidence) return;
        await mkdir(evidence, { recursive: true });
        await page.screenshot({ path: path.join(evidence, `${name}.png`) });
    };
    const emitClose = () => page.evaluate(async () => {
        const state = window.settingsTest;
        const listeners = Object.values(state.listeners).filter((listener) => listener.event === "tauri://close-requested");
        if (listeners.length !== 1) throw new Error("native close listener must not leak across navigation or StrictMode");
        await Promise.all(listeners.map((listener) => state.callbacks[listener.handler]({ event: listener.event, id: listener.handler, payload: {} })));
    });
    try {
        await page.goto(`${base}/dictionary`);
        await page.getByRole("textbox", { name: "1 行目の読み", exact: true }).waitFor();
        await page.getByLabel("更新があります", { exact: true }).waitFor();
        assert.equal(await page.evaluate(() => window.settingsTest.checks), 1, "startup check runs once, even in StrictMode");
        await page.getByRole("button", { name: "設定メニューを開閉", exact: true }).click();
        const rail = page.locator('[data-slot="sidebar"][data-state="collapsed"]');
        await rail.waitFor();
        for (const name of ["入力と文字", "変換と候補", "キーとローマ字", "ユーザ辞書", "詳細設定", /Azookeyについて/]) {
            const link = page.getByRole("link", { name, exact: typeof name === "string" });
            await assertInViewport(link, page);
            assert.ok(await link.locator("svg").isVisible(), "collapsed sidebar retains each navigation icon");
            assert.ok(await link.locator("span").first().isHidden(), "collapsed sidebar hides labels, not links");
        }
        const badge = page.getByLabel("更新があります", { exact: true });
        await assertInViewport(badge, page);
        assert.equal((await badge.boundingBox()).width, 8, "collapsed About keeps a red update dot");
        await page.getByRole("link", { name: "変換と候補", exact: true }).hover();
        await page.getByRole("tooltip", { name: "変換と候補", exact: true }).waitFor();
        await page.getByRole("link", { name: "変換と候補", exact: true }).click();
        await page.getByRole("heading", { name: "Zenzai", exact: true }).waitFor();
        assert.equal(await page.locator(".settings-content > section").first().getByRole("heading").innerText(), "Zenzai");
        assert.ok((await page.locator('[data-slot="sidebar-container"]').boundingBox()).width <= 64, "collapsed sidebar is a narrow icon rail");
        await screenshot("sidebar-icons");
        await page.getByRole("link", { name: "入力と文字", exact: true }).click();
        for (const label of ["数字", "() {} []", "、。", "･ ｢｣", "\" '", ": ;", "# % & @ $ ^ _ | ` \\", "~", "< > = + - / *", "? !"]) {
            await page.getByLabel(label, { exact: true }).waitFor();
        }
        await page.getByRole("button", { name: "設定メニューを開閉", exact: true }).click();
        await page.locator('[data-slot="sidebar"][data-state="expanded"]').waitFor();
        await page.getByRole("link", { name: "ユーザ辞書", exact: true }).click();
        await page.getByRole("button", { name: "行を追加", exact: true }).click();
        const reading = page.getByRole("textbox", { name: "26 行目の読み", exact: true });
        await reading.fill("ついか");
        await page.getByRole("button", { name: "保存", exact: true }).click();
        await page.getByText("単語を入力してください", { exact: true }).waitFor();
        const word = page.getByRole("textbox", { name: "26 行目の単語", exact: true });
        assert.ok(await word.evaluate((element) => element === document.activeElement));
        await word.fill("追加");
        const scrollBefore = await reading.evaluate((element) => element.closest("table").parentElement.scrollTop);
        assert.ok(scrollBefore > 0);
        await assertInViewport(page.getByRole("button", { name: "保存", exact: true }), page);
        await assertInViewport(page.getByRole("button", { name: "行を追加", exact: true }), page);
        await screenshot("dictionary-bottom");

        await page.evaluate(() => { window.settingsTest.holdSave = true; });
        await page.getByRole("button", { name: "保存", exact: true }).click();
        await page.getByText("保存中", { exact: true }).waitFor();
        assert.ok(await reading.isDisabled());
        assert.ok(await page.getByRole("button", { name: "行を追加", exact: true }).isDisabled());
        await page.evaluate(() => { window.settingsTest.holdSave = false; });
        await page.getByText("ユーザ辞書を保存しました", { exact: true }).waitFor();
        assert.equal(await page.evaluate(() => window.settingsTest.writes), 1);
        assert.equal(await reading.evaluate((element) => element.closest("table").parentElement.scrollTop), scrollBefore);

        await page.getByRole("link", { name: "入力と文字", exact: true }).click();
        await page.getByRole("link", { name: "ユーザ辞書", exact: true }).click();
        await word.fill("変更");
        await page.evaluate(() => { window.settingsTest.failSave = true; });
        await page.getByRole("button", { name: "保存", exact: true }).click();
        await page.getByText("ユーザ辞書の保存に失敗しました", { exact: true }).waitFor();
        await page.getByText("未保存の変更", { exact: true }).waitFor();
        assert.equal(await word.inputValue(), "変更");
        await page.evaluate(() => history.back());
        await page.getByRole("alertdialog").waitFor();
        await page.getByRole("button", { name: "編集を続ける", exact: true }).click();
        assert.ok(page.url().endsWith("/dictionary"), "Back must not silently discard a draft");
        await page.getByRole("link", { name: "キーとローマ字", exact: true }).click();
        await page.getByRole("alertdialog").waitFor();
        await page.getByRole("button", { name: "編集を続ける", exact: true }).click();
        assert.ok(page.url().endsWith("/dictionary"));
        await page.getByRole("link", { name: "キーとローマ字", exact: true }).click();
        await page.getByRole("button", { name: "破棄して移動", exact: true }).click();
        await page.evaluate(() => { window.settingsTest.failSave = false; });

        await page.getByRole("button", { name: "編集", exact: true }).click();
        await page.getByRole("dialog").waitFor();
        await page.getByRole("button", { name: "行を追加", exact: true }).click();
        await page.getByRole("textbox", { name: "2行目 入力", exact: true }).fill("ki");
        await page.getByRole("button", { name: "保存", exact: true }).click();
        await page.getByText("出力を記入してください", { exact: true }).waitFor();
        const output = page.getByRole("textbox", { name: "2行目 出力", exact: true });
        assert.ok(await output.evaluate((element) => element === document.activeElement));
        await output.fill("き");
        await emitClose();
        await page.getByRole("alertdialog").waitFor();
        assert.equal(await page.evaluate(() => window.settingsTest.destroys), 0, "dirty Romaji must prevent native close");
        await page.keyboard.press("Tab");
        assert.ok(await page.evaluate(() => !!document.activeElement.closest('[role="alertdialog"]')));
        await page.getByRole("button", { name: "編集を続ける", exact: true }).click();
        await page.locator('[role="alertdialog"]').waitFor({ state: "detached" });
        assert.equal(await output.inputValue(), "き");
        await emitClose();
        await page.getByRole("alertdialog").waitFor();
        await page.keyboard.press("Escape");
        // Wait for the closing focus scope/exit animation before testing the underlying modal.
        await page.locator('[role="alertdialog"]').waitFor({ state: "detached" });
        assert.equal(await output.inputValue(), "き");
        // Radix removes the closed focus scope in a deferred cleanup after DOM detachment.
        await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
        await page.getByRole("button", { name: "保存", exact: true }).focus();
        await page.keyboard.press("Tab");
        assert.ok(await page.evaluate(() => !!document.activeElement.closest('[role="dialog"]')), "Tab must stay inside the modal");
        await page.keyboard.press("Escape");
        await page.getByRole("alertdialog").waitFor();
        await page.getByRole("button", { name: "編集を続ける", exact: true }).click();
        await page.getByRole("button", { name: "初期値に戻す", exact: true }).click();
        await emitClose();
        await page.getByRole("alertdialog").getByRole("button", { name: "破棄して閉じる", exact: true }).waitFor();
        await page.getByRole("alertdialog").getByRole("button", { name: "編集を続ける", exact: true }).click();
        await page.getByRole("alertdialog").getByText("初期値に戻しますか？", { exact: true }).waitFor();
        await page.getByRole("alertdialog").getByRole("button", { name: "初期値に戻す", exact: true }).click();
        await page.getByRole("textbox", { name: "2行目 入力", exact: true }).waitFor();
        assert.equal(await page.getByRole("textbox", { name: "2行目 入力", exact: true }).inputValue(), "kk");
        await screenshot("romaji-editor");
        await page.getByRole("button", { name: "保存", exact: true }).click();
        await page.getByRole("dialog").waitFor({ state: "hidden" });

        await page.getByRole("link", { name: "変換と候補", exact: true }).click();
        await page.getByLabel("変換プロファイル", { exact: true }).fill("latest profile");
        await page.getByRole("link", { name: /Azookeyについて/ }).click();
        await page.waitForFunction(() => window.settingsTest.config.zenzai.profile === "latest profile");
        await page.getByText("不具合報告・要望", { exact: true }).waitFor();
        assert.equal(await page.getByText(/Discord/).count(), 0);
        const github = page.locator('a[href="https://github.com/batao9/azooKey-Windows"]');
        const issues = page.locator('a[href="https://github.com/batao9/azooKey-Windows/issues"]');
        await github.click();
        await issues.click();
        assert.deepEqual(await page.evaluate(() => window.settingsTest.links), ["https://github.com/batao9/azooKey-Windows", "https://github.com/batao9/azooKey-Windows/issues"]);
        await page.evaluate(() => { window.settingsTest.failCheck = true; });
        await page.getByRole("button", { name: "更新を確認", exact: true }).click();
        await page.getByText("Error: mock update failure", { exact: true }).waitFor();
        assert.equal(await page.getByLabel("更新があります", { exact: true }).count(), 1);
        await screenshot("about-update");
        await page.evaluate(() => { window.settingsTest.failCheck = false; window.settingsTest.available = false; });
        await page.getByRole("button", { name: "更新を確認", exact: true }).click();
        await page.getByText("最新版を利用中です。", { exact: true }).waitFor();
        assert.equal(await page.getByLabel("更新があります", { exact: true }).count(), 0);

        await page.getByRole("link", { name: "入力と文字", exact: true }).click();
        await page.getByLabel("句読点", { exact: true }).waitFor();
        await screenshot("input");
        await page.getByRole("link", { name: "変換と候補", exact: true }).click();
        await page.getByLabel("ライブ変換中の読みを表示", { exact: true }).waitFor();
        await screenshot("conversion");
        await page.evaluate(() => { window.settingsTest.jevApiKeySaved = true; });
        await page.getByRole("link", { name: "詳細設定", exact: true }).click();
        await page.getByLabel("誤入力の補正", { exact: true }).waitFor();
        const keyInput = page.getByLabel("TypeSafe APIキー", { exact: true });
        const keyCheck = page.getByRole("button", { name: "保存済みキーを確認", exact: true });
        await keyCheck.waitFor();
        assert.equal(await keyInput.inputValue(), "", "saved secret must not enter the renderer, even with Jev off");
        await keyInput.fill("unsaved-draft-for-test");
        const writesBeforeKeyCheck = await page.evaluate(() => window.settingsTest.writes);
        await page.evaluate(() => { window.settingsTest.holdKeyCheck = true; });
        await keyCheck.click();
        await page.getByRole("button", { name: "確認中…", exact: true }).waitFor();
        assert.ok(await keyInput.isDisabled());
        assert.ok(await page.getByRole("button", { name: "キーを保存", exact: true }).isDisabled());
        assert.ok(await page.getByRole("button", { name: "削除", exact: true }).isDisabled());
        await page.evaluate(() => { window.settingsTest.holdKeyCheck = false; });
        await page.getByText("保存済みAPIキーは有効です", { exact: true }).waitFor();
        await keyCheck.waitFor();
        assert.equal(await keyInput.inputValue(), "unsaved-draft-for-test");
        for (const error of ["APIキーが無効です。", "APIキーの確認がタイムアウトしました。", "APIキーの確認中にネットワークエラーが発生しました。"]) {
            await page.evaluate((error) => { window.settingsTest.keyCheckError = error; }, error);
            await keyCheck.click();
            await page.getByText(error, { exact: true }).waitFor();
            assert.ok(await keyCheck.isEnabled(), "failed checks must unlock retry");
        }
        assert.deepEqual(await page.evaluate(() => window.settingsTest.keyCheckArgs), [{}, {}, {}, {}], "check must use stored key, never pass unsaved key or context");
        assert.equal(await page.evaluate(() => window.settingsTest.writes), writesBeforeKeyCheck, "checking must not modify settings");
        await screenshot("advanced");

        await page.getByRole("link", { name: "ユーザ辞書", exact: true }).click();
        for (const viewport of [{ width: 640, height: 480 }, { width: 400, height: 360 }]) {
            await page.setViewportSize(viewport);
            await page.getByRole("button", { name: "行を追加", exact: true }).click();
            await assertInViewport(page.getByRole("button", { name: "行を追加", exact: true }), page);
            await assertInViewport(page.getByRole("button", { name: "保存", exact: true }), page);
            await page.getByRole("button", { name: "設定メニューを開閉", exact: true }).click();
            await page.getByRole("link", { name: "キーとローマ字", exact: true }).click();
            await page.getByRole("button", { name: "破棄して移動", exact: true }).click();
            await page.getByRole("button", { name: "編集", exact: true }).click();
            await assertInViewport(page.getByRole("button", { name: "行を追加", exact: true }), page);
            await assertInViewport(page.getByRole("button", { name: "保存", exact: true }), page);
            await screenshot(`romaji-${viewport.width}`);
            await page.getByRole("button", { name: "キャンセル", exact: true }).click();
            await page.getByRole("button", { name: "設定メニューを開閉", exact: true }).click();
            await page.getByRole("link", { name: "ユーザ辞書", exact: true }).click();
        }
        await page.evaluate(() => { sessionStorage.setItem("failSettingsLoad", "true"); });
        await page.reload();
        await page.getByText("辞書設定の読み込みに失敗しました。", { exact: true }).waitFor();
        assert.ok(await page.getByRole("button", { name: "保存", exact: true }).isDisabled());
        await page.evaluate(() => sessionStorage.removeItem("failSettingsLoad"));
        await page.setViewportSize({ width: 800, height: 600 });
        await page.emulateMedia({ colorScheme: "dark" });
        await page.reload();
        await page.getByRole("textbox", { name: "1 行目の読み", exact: true }).waitFor();
        assert.ok(await page.locator("html").evaluate((element) => element.classList.contains("dark")));
        await screenshot("dictionary-dark");
        await emitClose();
        assert.equal(await page.evaluate(() => window.settingsTest.destroys), 1, "a clean editor closes normally");
        await page.getByRole("textbox", { name: "1 行目の読み", exact: true }).fill("みほぞん");
        await emitClose();
        await page.getByRole("alertdialog").waitFor();
        assert.equal(await page.evaluate(() => window.settingsTest.destroys), 1, "native close must not bypass dirty confirmation");
        await page.getByRole("button", { name: "編集を続ける", exact: true }).click();
        assert.equal(await page.getByRole("textbox", { name: "1 行目の読み", exact: true }).inputValue(), "みほぞん");
        await emitClose();
        await page.getByRole("alertdialog").waitFor();
        await page.keyboard.press("Escape");
        await page.getByRole("alertdialog").waitFor({ state: "hidden" });
        await emitClose();
        await page.getByRole("button", { name: "破棄して閉じる", exact: true }).click();
        assert.equal(await page.evaluate(() => window.settingsTest.destroys), 2);
        assert.deepEqual(runtimeErrors, []);
    } finally {
        await browser.close();
        await server.close();
    }
});
