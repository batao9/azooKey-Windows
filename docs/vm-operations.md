# Windows VM の運用

## 状態の契約

通常の検証は現在のディスクに対して行う。**終了時は検証した版をインストールしたまま通常シャットダウン**し、ユーザーへ渡す。復元、アンインストール、保存状態破棄、スナップショット追加は検証の暗黙の後処理にしない。ユーザーが「起動したまま」「指定baselineへ戻す」等を指示した場合はそれを優先する。

ビルド用VMと手動確認用VMを分離する。ビルドのために別のVMを停止しない。既存設定・辞書・学習・資格情報を使う手動VMと、破壊的な移行・アンインストール試験用の disposable VM を区別する。

## 設定と入口

Python 3.11以降、OpenSSH、VirtualBox CLI を使う Linux/WSL ホスト用。
`scripts/vm-config.example.json` を参考に **`.local/vm.json`** を作る。
鍵は秘密の内容ではなくパスを指定する。VM識別子は名前よりUUIDを推奨する。
ホスト、ポート、Windowsユーザー、鍵、VBoxManageのパス、baseline名はこのローカルファイルだけに置く。
設定例のVM/snapshot識別子はプレースホルダーであり、実際の値は公開しない。ゲストに既存の依存・辞書キャッシュを使う場合は、build profileの`guest_cache_root`へそのWindows絶対パスを設定する。省略時はゲストの`LOCALAPPDATA`配下の`Azookey\vm-cache`を使う。controllerはこの設定をhelperへ渡すため、キャッシュ用cloneの実パスを公開スクリプトへ埋め込まない。
controller経由ではVirtualBoxの実行ファイルもprofileから渡す。旧helperを直接使う場合は`VBOX_MANAGE`を明示するか`VBoxManage`を`PATH`に置く。

```sh
python3 scripts/vm.py --profile build status
python3 scripts/vm.py --profile build doctor
python3 scripts/vm.py --profile build build <branch>
python3 scripts/vm.py --profile build test <branch> composition all
python3 scripts/vm.py --profile manual stage .local/artifacts/<exact-installer>.exe
```

`stage` / `install` は同じ入口。隣の `<installer>.build-info.json` または `build-info.json` が必須。`latest` という曖昧な成果物識別子ではなく、ビルド結果の具体的なファイルを渡す。SHA256で転送物を検証し、インストール後の version / channel / revision / installerGeneration を照合する。

ローカルの旧入口 `.local/vm_build_master.sh`、`.local/vm_test_client_composition.sh`、`.local/vm_stage_for_manual_test.sh` は互換ラッパー。実装の正本をコピーして保持しない。公開build/test helperはcontrollerから呼ばれ、`VM_LIFECYCLE_EXTERNAL=1` のとき起動後の終了処理はcontrollerが所有する。

公開対象はcontroller/helper・回帰テスト・設定テンプレート・一般的な運用手順とする。実設定、VM/snapshot一覧、実行ログ、スクリーンショット、調査・引き継ぎレポート、成果物とそのbuild-infoは`.local/`に保存する。これらには実パス・アカウント・UUID等が含まれるため、Gitへ追加しない。個人共通のハーネス設定もこのリポジトリへコピーしない。

## 基本操作

| 操作 | コマンド末尾 | 結果・注意 |
|---|---|---|
| 状態確認 | `status` | 電源状態・現在のsnapshot識別子。ディスク復元はしない |
| 起動 | `start [--gui]` | savedなら再開。SSH readinessと実時刻を確認。saved stateは捨てない |
| スリープからの復帰要求 | `wake` | runningのguestへCtrlキー1回だけを送る。未解決予約中も利用でき、再起動・時刻変更はしない |
| 通常停止 | `shutdown` | 強制停止はしない。完了しなければ失敗として状態を残す |
| 環境診断 | `doctor` | 必要なら起動し、Rust/Swift/Node/VS/電源設定を報告 |
| build環境準備 | `prepare` | repoのRust pinとx86 target/clippy/rustfmtを公式経由で導入 |
| 通常PowerShell | `exec --script <file> [--timeout <seconds>]` | 明示したWindowsユーザーと鍵でSSH実行。実行中は一時的にスリープ抑止 |
| UI用PowerShell | `exec --script <file> --interactive` | ログオン中のユーザーの通常権限タスク。インストールには使わない |
| 転送 | `upload <local-file> <C:\absolute\remote>` | SSH設定を共通化 |
| 回収 | `download <C:\absolute\remote> <local-file>` | 同上 |
| 画面取得 | `screenshot .local/ui-evidence/<name>.png` | 起動中の実画面を保存。内容も確認する |
| 引き渡し | `handoff` | インストール識別情報を記録し、通常シャットダウン |
| 明示的な復元 | `restore <snapshot-name-or-UUID>` | 電源offが前提。現在のディスク変更を巻き戻す操作 |
| baseline保存 | `snapshot <new-name>` | 電源offが前提。既存snapshotを削除・上書きしない |
| VMログ取得 | `diagnostics` | 現在のVirtualBoxログをrun directoryへ保存 |
| 未解決操作の点検 | `operation-status` | 前回の予約、実行中のbuild/install/interactive taskを確認。起動や時計変更はしない |
| 点検後の予約解除 | `resolve-operation --reason <確認根拠>` | 操作の終了を確認した後の明示的な解除。実行中の対象taskやbuild/install処理は拒否。interactive task以外のprocessは別途点検 |
| 停止不能時の保守 | `recover-poweroff --reason <根拠>` | 所有するbuild/disposable VMだけ。通常停止を試し、失敗時は明示的な強制停止。中断対象と根拠を記録し、電源off確認後に予約を解除 |
| 導入メディアの保守 | `detach-install-media --controller <name> --port <n> [--device <n>]` | 電源offが前提。ISO/VISOだけを取り外し、ディスクは拒否 |

`doctor`の成功は診断の実行成功であり、すべてのツールが揃っている意味ではない。`rust_ready` 等の結果を読む。手動VMにはビルドツールがない場合もある。

PowerShellスクリプトは native command の `$LASTEXITCODE` を確認し、失敗を明示して終了する。GUI taskにはログオンセッションが必要。通常のSSHプロセスのWindowHandleやクリップボードを、ユーザーの実画面の証拠にしない。
`exec --script` の入力はUTF-8（BOMは任意）。controllerはWindows PowerShell 5.1で日本語リテラルを保持するため、BOM付きUTF-8へ正規化して転送する。

## ネイティブUI / IME検証

```sh
python3 scripts/vm.py --profile manual stage <installer> --leave-running
python3 scripts/vm.py --profile manual exec --script <focused-ui-probe.ps1> --interactive
python3 scripts/vm.py --profile manual screenshot .local/ui-evidence/<scenario>.png
python3 scripts/vm.py --profile manual handoff
```

- スクリプト終了値だけで合格にせず、フォーカス・有効IME・設定値・候補表示を画像やログで確認する。
- 最終文字列に加えて入力開始からの編集・移動・候補選択・確定・フォーカス履歴を記録する。
- 保存確認はdisk保存とserver反映を分ける。APIキー等の実値を読み出してレポートへ出さない。
- インストーラーは同一Windowsユーザーの昇格済みSSHから実行する。通常権限のinteractiveタスクを流用しない。
- `stage`は`/RESTARTEXITCODE=3010`で`reboot_required`を記録する。インストール識別子の一致は実行中DLLの更新証拠ではない。再起動要求がある場合は通常停止→cold boot後にUI/IMEを検証する。`runtime_verification`は標準stageでは`not_checked`。
- インストール前のsettingsはguest内へ退避する。結果はbytes変更の有無を記録するが、移行や通常の再serializeがあり得るため変更だけで内容消失とは断定しない。
- 資格情報について、`cmdkey`出力全体の対象名部分一致を存在・保全証拠にしない。標準stageはcredential保全を未検証と明示する。

## ログ・排他・再開

各操作は `.local/logs/vm/<run-id>/result.json` と `commands.log` を作る。
`task_status` は `succeeded` / `failed` / `blocked`、実行中は `running`。
before/after状態、guest結果、コマンドの終了値と証拠パスを合わせて判断する。

VMの実UUIDに対するホスト共通ロックで、worktreeやprofile名が違っても同じVMを同時操作しない。`status`はロック中も利用できる。旧スクリプトの直接実行はcontrollerの排他を通らないので、通常作業ではcontrollerを使う。
同じVMを対象とする一連の状態確認・準備・実行は順次行う。`status`と`operation-status`も同じVirtualBox objectへアクセスし、並列起動するとobject/lockの過渡エラーになり得るため、独立した読み取りとして並列化しない。`status`のロック例外は実行中操作の監視用であり、失敗した状態取得を正常状態と見なすものではない。
実行前にホスト共通の永続予約も作り、通信断・timeout・controller中断でguest処理の終了が不明な場合は残す。後続の変更操作は予約が解決されるまで拒否する。`operation-status`、`diagnostics`、`screenshot`は予約中も利用できる。保存されたrun directoryとguestのログ・task・processを点検し、終了確認後に`resolve-operation --reason`を使う。汎用スクリプトが起動した独自processの終了は、検出リストだけで判断しない。

timeout・中断はguest内処理の終了を意味しない。結果の `running` や timeout を確認したら、起動中のinstaller/cargo/taskとログを点検してから再試行する。失敗を `tee` の成功や別のコマンドの終了値で隠さない。CLI出力をパイプする場合は `set -o pipefail` を使う。

## baseline更新（明示的な保守）

更新の判断材料は「復元のたびに必要な補修」と「保存メモリに依存せず再起動できること」。例: Rust pin未導入、BuildTools検出漏れ、古い時刻、インストールメディアへの起動、検証中のスリープ。

1. 現行snapshotのUUIDと現在状態を記録。作業中の別ユーザー/agentがいないことを確認する。
2. 現在状態を起動し `doctor`。build VMは `prepare`、必要な限定的保守を `exec --script` で行う。証明書検証は無効化しない。
3. 通常シャットダウン→cold boot→SSH/ツール確認→必要なbuild/testを実行する。保存メモリ再開だけを合格にしない。
4. 手動VMは対象installerを検証し、通常シャットダウン。テスト用データやデバッグ起動をbaselineへ混入させない。
5. `snapshot <new-name>` でpowered-off baselineを追加し、旧baselineは残す。新しいUUIDを記録する。
6. 新baselineからの復元・cold boot・診断を確認してから `.local/vm.json` のsnapshot参照を更新する。
7. 最終的な両VMの電源状態、インストール識別子、snapshot UUID、未確認事項を報告する。

これは保守として依頼された場合の手順であり、通常検証ごとにsnapshotを増やす理由にしない。

## 検証

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_vm*.py'
```

mockによる排他・接続設定・失敗伝播・復元しない既定値の検証と、実VMでの起動/build/test/install/終了状態の検証を分けて記録する。ローカルVM合格は正式なGitHub CI合格の代わりではない。
