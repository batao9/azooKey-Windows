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
2. VMによってビルドを行い、動作を検証する。
3. 動作確認後 `origin/dev/<feature>` に push し、CI で検証する。
4. `origin/master` へ PR を出す。

## ビルド / 検証

- `origin` に push して GitHub CI でビルドする。

## ローカルVMビルド（任意）

- 正式なビルド判定は GitHub CI とする。
- push 前の事前確認として、可能であればローカル VM でのビルドを実行する。
- ローカル VM ビルドの実行インターフェースは `.local/vm_build_master.sh <branch>` とする。
- ローカル VM ビルドスクリプトは、少なくとも以下を満たすこと。
  - 指定ブランチと現在ブランチが一致しない場合は失敗させる。
  - 作業ツリーがクリーンでない場合は失敗させる。
  - 成果物は `.local/artifacts/` に回収する。
  - 成功時・失敗時ともに、終了時に既定スナップショットへ復元してクリーン状態へ戻す。
