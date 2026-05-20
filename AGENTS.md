# AGENTS.md

このリポジトリの開発運用ルールを定義する。

## 基本ルール

- 開発は機能・修正ごとに独立したブランチで行う。
- `master` へ直接コミットしない。開発前に `master` を最新化し、そこから作業ブランチを作成する。
- 変更後は必要なローカル検証を行い、`origin` へ push して GitHub CI で確認する。
- VM 名、スナップショット名、SSH 接続先、鍵パスなどの環境依存値はリポジトリに固定しない。必要に応じて環境変数または `.local/` のローカルスクリプトで指定する。

## ブランチ構成

- `master`
  - メインブランチ。直接開発しない。
  - 開発前に最新化し、`master` から機能開発用またはバグ修正用のブランチを切る。
- `feature/<feature>`
  - 機能開発用ブランチ。
  - 対応する issue がある場合は `feature/<issue-number>-<feature>` の形式にする。
- `fix/<issue>`
  - バグ修正用ブランチ。
  - 対応する issue がある場合は `fix/<issue-number>-<issue>` の形式にする。

## 開発フロー

1. `master` を最新化する。
2. `master` から作業ブランチを作成する。
3. 実装し、変更範囲に応じた test を追加・更新する。
4. VM ビルド、VM test、インストール検証など、変更に必要な確認を行う。
5. `origin/<branch>` に push し、GitHub CI の結果を確認する。
6. 必要に応じて CI で生成したビルドを実機にインストールし、動作を確認する。
7. `origin/master` へ PR を出す。

## ビルド / 検証

- 正式なビルド判定は GitHub CI とする。
- ローカル確認として、可能なら Windows VM 上でビルド・test・インストール検証を行う。
- VM 操作用スクリプトは `scripts/` または `.local/` のものを使う。`.local/` に同じ用途のスクリプトがある場合は、環境固有の設定を含む可能性があるため `.local/` を優先する。
- 公開用スクリプトは環境依存値を持たない。`VM_NAME`、`SNAPSHOT_NAME`、`SSH_USER`、`SSH_PORT`、`SSH_KEY`、必要に応じて `SSH_HOST` や `VBOX_MANAGE` を環境変数で指定して実行する。

代表的な実行方法:

- VM ビルド: `scripts/vm_build.sh <branch>`
- `client` クレートの composition / clause adjustment / stateful test: `scripts/vm_test_client_composition.sh <branch> [cargo-test-filter|skip] [swift-test-filter|all|skip]`
- インストーラーの無人ステージング: `scripts/vm_stage_for_manual_test.sh <installer-path|latest>`

`.local/` 側に対応するスクリプトがある場合の例:

- VM ビルド: `.local/vm_build_master.sh <branch>`
- VM test: `.local/vm_test_client_composition.sh <branch> [cargo-test-filter|skip] [swift-test-filter|all|skip]`
- インストール検証: `.local/vm_stage_for_manual_test.sh <installer-path|latest>`

VM ビルドでは、指定ブランチと現在ブランチが一致していることを確認する。成果物は `.local/artifacts/` に回収する。可能なら検証前後に指定スナップショットへ復元し、VM をクリーンな状態に戻す。

インストール検証では、クリーンなスナップショットへ復元してから VM を起動し、インストーラーを転送してサイレント実行する。インストールログは `.local/logs/` に回収する。

## テスト方針

- 変更に応じて必要な test を実装する。バグ修正では、まず再現 test や回帰 test の追加を優先する。
- 実装変更後は、変更範囲に対応する test を実行し、pass を確認してから次の段階へ進む。
- `client` クレートの composition / clause adjustment / stateful test に関わる変更では、可能なら VM 上の test スクリプトを実行する。
- push 前の事前確認として、可能なら VM ビルドを実行し、ビルド成功を確認する。
