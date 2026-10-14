# Tauri + React + Typescript

This template should help get you started developing with Tauri, React and Typescript in Vite.

## Settings UI checks

```sh
npm run typecheck
npm test
npm run build
```

The browser regression check uses mocked Tauri commands and an existing Playwright installation (no new app dependency):

```sh
PLAYWRIGHT_MODULE=<absolute-path-to-playwright-module> node --test tests/settings-ui.browser.mjs
```

Optional environment variables: `PLAYWRIGHT_CHROMIUM_EXECUTABLE` for an existing Chromium binary, and `SETTINGS_UI_EVIDENCE_DIR` for screenshots. The check covers fixed editor actions at 800×600 and smaller sizes, validation, save failures, unsaved navigation/Back, mocked native-close events, modal focus, profile saving, stored-key check success/failures (without passing a secret to the renderer), external links, update badges, and dark mode. Native window close, Credential Manager, and Windows display scaling still require VM checks.

The Jev key check uses the authenticated `GET /v1/models` endpoint documented at https://docs.typesafe.ai/models. It does not send input, history, or inference requests; successful authentication does not guarantee available credits or successful conversion. Windows backend checks (local HTTP fixtures only): `cargo test -p frontend --lib`.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
