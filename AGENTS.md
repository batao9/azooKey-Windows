# AGENTS.md

このリポジトリの開発ブランチ運用ルールを定義する。

## 目的

- `origin` に push して GitHub CI でビルドする。
- 機能ごとに独立したブランチで開発する。
- 最終的に `upstream` へ機能差分だけで PR を出せる状態を保つ。
- `AGENTS.md` や workflow など fork 運用用の変更は、upstream PR に混ぜない。

## ブランチ構成

- `master`
  - `upstream/master` 同期専用。直接開発しない。
- `fork-base`
  - fork 運用専用変更を置く基底ブランチ。
  - 例: `AGENTS.md`, `.github/workflows`, `.gitignore`, ローカル運用スクリプト。
- `dev/<feature>`
  - 機能開発用ブランチ。`fork-base` から切る。
  - `origin` に push して CI ビルドを回す。
- `pr/<feature>`
  - upstream 提出専用ブランチ。`upstream/master` から切る。
  - `dev/<feature>` から機能コミットのみ cherry-pick して作る。

## 日常フロー

1. `master` を最新化して `fork-base` を更新する。
2. `fork-base` から `dev/<feature>` を作成する。
3. 実装して `origin/dev/<feature>` に push し、CI で検証する。
4. upstream 提出時は `upstream/master` から `pr/<feature>` を作る。
5. `dev/<feature>` の機能コミットのみを `pr/<feature>` に cherry-pick する。
6. `pr/<feature>` から upstream へ PR を作成する。

## upstream PR の禁止変更

`upstream` 向け PR に以下を含めない。

- `AGENTS.md`
- `.github/workflows/**`
- `.gitignore`
- その他 fork ローカル運用だけに必要なファイル

## PR 前チェック

upstream PR ブランチで、次を必ず実行する。

```bash
git diff --name-only upstream/master...HEAD
```

出力に禁止変更が含まれる場合は、PR を作成せずコミット構成を整理する。
upstream PRは必ずユーザーの許可を得てから実行する。