# AGENTS.md

このリポジトリの開発運用ルールを定義する。

## 基本ルール

- `origin` に push して GitHub CI でビルドする。
- 機能ごとに独立したブランチで開発する。

## ブランチ構成

- `master`
  - fork 運用専用変更を置く基底ブランチ。
  - 例: `AGENTS.md`, `.github/workflows`, `.gitignore`, ローカル運用スクリプト。
- `pr-base`
  - `upstream/master` 同期専用。直接開発しない。
- `dev/<feature>`
  - 機能開発用ブランチ。`master` から切る。
  - `origin` に push して CI ビルドを回す。

## 開発フロー

1. `master` から `dev/<feature>` を作成する。
2. 実装して `origin/dev/<feature>` に push し、CI で検証する。

## ビルド / 検証

- `origin` に push して GitHub CI でビルドする。
