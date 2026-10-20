# Windows VM の運用

## 状態の契約

**ビルド・検証は毎回、profileのベーススナップショットへ復元してから開始する。成果物・ログ・画面をホストへ回収し、終了後もベースへ復元して電源offにする。** VMの現在ディスク・保存メモリは使い捨てで、設定やインストール状態を残すことを既定にしない。復元はスナップショットを新規作成・更新する操作ではない。

ビルド用VMと手動確認用VMを分離し、両方を使い捨ての検証環境として扱う。必要な設定・辞書・資格情報のfixtureはベースに用意し、検証中の変更は復元で破棄する。ビルドのために別のVMや他の実行中ownerを停止しない。

インストール→UI/IME probe→画面・ログ回収は一つの検証セッションとし、`stage --leave-running` または `start` でクリーン開始し、同じworktreeから継続して `handoff` / `shutdown` で復元する。セッション内の`exec`等のたびにインストールを消さない。単独の`exec`/`doctor`は前後を復元する。`upload`はクリーンなセッションを開始または継続し、`download`は回収後に、セッション中なら継続、単独なら復元する。
再起動が必要な更新試験は、セッション中の`reboot`で現在のインストールを保持してcold bootする。通常の`shutdown`→`start`はベース復元を含むため、その代用にしない。

**通常の使い捨て検証でVMやSSHの動作がおかしい場合は長時間調査・待機せずリセットする。** 失敗・通信断・timeout・build/testの無出力停止では、ホスト側helperと子SSHを停止し、VMを強制停止してベースへ復元する。失敗したbuild/test/installを自動再実行したり、復元できたことを試験成功と扱ったりしない。

**Windows Update・baseline作成などの明示的な保守は現在のディスクを保持する。** `start --keep-current` / `prepare`のセッションでは、失敗時も強制停止・自動復元しない。既知の終了済みエラーなら保守セッションを継続でき、完了が不明ならtask/commandと保守modeを予約に残す。次のexec/doctor等でそれを通常検証に置き換えない。点検・終了確認後の`resolve-operation`、または明示した`reset`で処置する。保守中の`handoff`も復元せず通常停止する。

## 設定と入口

Python 3.11以降、OpenSSH、VirtualBox CLI を使う Linux/WSL ホスト用。
`scripts/vm-config.example.json` を参考に **`.local/vm.json`** を作る。
鍵は秘密の内容ではなくパスを指定する。VM識別子は名前よりUUIDを推奨する。
ホスト、ポート、Windowsユーザー、鍵、VBoxManageのパス、baseline名はこのローカルファイルだけに置く。
`build` / `manual`は役割のprofile名で、VirtualBox上の表示名ではない。`vm`にUUIDを使えば、VM表示名の変更で公開scriptやprofile役割名を変更する必要はない。実際の表示名とsnapshot一覧・検証結果はローカルのinventory/reportに記録する。commandとprofile役割の適合はVM操作より前に確認し、実行不能な組み合わせでは既存セッションを変更しない。
設定例のVM/snapshot識別子はプレースホルダーであり、実際の値は公開しない。ゲストに既存の依存・辞書キャッシュを使う場合は、build profileの`guest_cache_root`へそのWindows絶対パスを設定する。省略時はゲストの`LOCALAPPDATA`配下の`Azookey\vm-cache`を使う。controllerはこの設定をhelperへ渡すため、キャッシュ用cloneの実パスを公開スクリプトへ埋め込まない。
キャッシュは事前作成必須ではない。Swift testのCPU runtimeがキャッシュにも転送sourceにもない場合は、固定版をその実行のsource treeへ取得し、linkとtest runnerの両方で同じruntimeを使う。既存の外部キャッシュは書き換えない。
buildとSwift testは共通のruntime helperを使い、固定版packageの依存DLLを含めて検証する。buildは検証済みruntimeをpackaging treeへ配置するため、不完全なcache/sourceをそのままinstallerへ取り込まない。
controller経由ではVirtualBoxの実行ファイルもprofileから渡す。旧helperを直接使う場合は`VBOX_MANAGE`を明示するか`VBoxManage`を`PATH`に置く。

SSH準備完了は認証済みコマンドで確認する。NATの転送ポートだけが先に開き、ゲストOS起動中はbannerが返らない場合がある。省電力についても`powercfg /a`の対応状態と設定値を分けて確認する。スリープ非対応ならGUIに項目が出ないことがあり、保存されているタイムアウト値だけからSSH停止原因を断定しない。

```sh
python3 scripts/vm.py --profile build status
python3 scripts/vm.py --profile build doctor
python3 scripts/vm.py --profile build build <branch>
python3 scripts/vm.py --profile build test <branch> composition all
python3 scripts/vm.py --profile manual stage .local/artifacts/<exact-installer>.exe
```

`stage` / `install` は同じ入口。隣の `<installer>.build-info.json` または `build-info.json` が必須。`latest` という曖昧な成果物識別子ではなく、ビルド結果の具体的なファイルを渡す。SHA256で転送物を検証し、インストール後の version / channel / revision / installerGeneration を照合する。このidentityは検証時の版であり、終了後に復元したベースの版とは区別する。
ファイル・metadata・identity・SHA256のローカル事前検査は、VMのクリーン開始より先に行う。不正な成果物では、開いているセッションを復元・破棄しない。

ローカルの旧入口 `.local/vm_build_master.sh`、`.local/vm_test_client_composition.sh`、`.local/vm_stage_for_manual_test.sh` は互換ラッパー。公開build/test helperはcontrollerから呼ぶ。`VM_LIFECYCLE_EXTERNAL=1`では前後の復元・異常時停止をcontrollerだけが所有し、helper側のrestoreフラグは0にして二重操作を防ぐ。

公開対象はcontroller/helper・回帰テスト・設定テンプレート・一般的な運用手順とする。実設定、VM/snapshot一覧、実行ログ、スクリーンショット、調査・引き継ぎレポート、成果物とそのbuild-infoは`.local/`に保存する。これらには実パス・アカウント・UUID等が含まれるため、Gitへ追加しない。個人共通のハーネス設定もこのリポジトリへコピーしない。

## 基本操作

| 操作 | コマンド末尾 | 結果・注意 |
|---|---|---|
| 状態確認 | `status` | 電源状態・現在のsnapshot識別子。ディスク復元はしない |
| 検証セッション開始 | `start [--gui]` | ベースへ復元してcold boot。SSH readinessと実時刻を確認 |
| スリープからの復帰要求 | `wake` | runningのguestへCtrlキー1回だけを送る。未解決予約中も利用でき、再起動・時刻変更はしない |
| セッション終了 | `shutdown` | 短い通常停止猶予の後、必要なら強制停止しベースへ復元 |
| 即時リセット | `reset` | SSHなしで強制停止／保存メモリ破棄→ベース復元→poweroff確認。別worktreeの待機中セッションを意図的に破棄する場合だけ`--discard-session` |
| 保守用停止 | `shutdown --keep-current` | Windowsに通常停止を一度要求して最大120秒観測。未確認なら現ディスク・予約を保持 |
| 保守用cold boot | `start --keep-current` | 現ディスクを保持して起動。savedの場合は保存メモリだけを破棄してcold boot |
| 環境診断 | `doctor` | 必要なら起動し、Rust/Swift/Node/VS/電源設定を報告 |
| build環境準備 | `prepare` | 明示的な保守セッションでRust pin等を導入。後続のdoctor/execも変更を保持 |
| セッション内cold boot | `reboot` | 検証／保守セッションのインストール・変更を保持して再起動 |
| 通常PowerShell | `exec --script <file> [--timeout <seconds>]` | 明示したWindowsユーザーと鍵でSSH実行。実行中は一時的にスリープ抑止 |
| UI用PowerShell | `exec --script <file> --interactive` | ログオン中のユーザーの通常権限タスク。インストールには使わない |
| 転送 | `upload <local-file> <C:\absolute\remote>` | SSH設定を共通化 |
| 回収 | `download <C:\absolute\remote> <local-file>` | 同上 |
| 画面取得 | `screenshot .local/ui-evidence/<name>.png` | 起動中の実画面を保存。内容も確認する |
| 検証終了 | `handoff` | identity記録後、通常検証はベース復元。保守中は現在ディスクを保持して通常停止 |
| 指定snapshotへ復元 | `restore <snapshot-name-or-UUID>` | 必要なら強制停止／保存メモリ破棄し復元 |
| baseline保存 | `snapshot <new-name>` | 電源offが前提。予約後にcaptureし、新しいpowered-off identity確認まで解除しない |
| VMログ取得 | `diagnostics` | 現在のVirtualBoxログをrun directoryへ保存 |
| 未解決操作の点検 | `operation-status` | 前回の予約、実行中のbuild/install/interactive taskを確認。起動や時計変更はしない |
| 点検後の予約解除 | `resolve-operation --reason <確認根拠>` | 操作の終了を確認した後の明示的な解除。実行中の対象taskやbuild/install処理は拒否。interactive task以外のprocessは別途点検 |
| 旧回復入口 | `recover-poweroff --reason <根拠>` | 即時リセットの互換入口。manual VMも対象 |
| 導入メディアの保守 | `detach-install-media --controller <name> --port <n> [--device <n>]` | 電源offが前提。ISO/VISOだけを取り外し、ディスクは拒否 |

`doctor`の成功は診断の実行成功であり、すべてのツールが揃っている意味ではない。`rust_ready` 等の結果を読む。手動VMにはビルドツールがない場合もある。

PowerShellスクリプトは native command の `$LASTEXITCODE` を確認し、失敗を明示して終了する。GUI taskにはログオンセッションが必要。通常のSSHプロセスのWindowHandleやクリップボードを、ユーザーの実画面の証拠にしない。
`exec --script` の入力はUTF-8（BOMは任意）。controllerはWindows PowerShell 5.1で日本語リテラルを保持するため、BOM付きUTF-8へ正規化して転送する。
読み込み・UTF-8検証・BOM付きコピーの作成はlifecycleより前に行い、そのコピーを転送する。欠落・不正encodingなどのローカル入力エラーでは、開いている検証セッションを復元しない。
upload/downloadの絶対パス構文・ローカル入出力先もlifecycleより前に検査する。転送元の欠落や出力先directoryの作成失敗で既存セッションを破棄しない。

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
- `stage`は`/RESTARTEXITCODE=3010`で`reboot_required`を記録する。インストール識別子の一致は実行中DLLの更新証拠ではない。3010の確認は`stage --leave-running`→`reboot`→UI/IME probe→`handoff`で行う。`runtime_verification`は標準stageでは`not_checked`。
- installerログは完了が既知なら復元前に一度だけ、15秒以内の転送でホストへ回収し、成功/未回収を`evidence`に記録する。通信・完了が不明なら回収を反復せず即時復元する。ログ回収中の通信timeoutもrun失敗とし、既知のインストール結果を別に記録して即時復元する。
- インストール前のsettingsはguest内へ退避する。結果はbytes変更の有無を記録するが、移行や通常の再serializeがあり得るため変更だけで内容消失とは断定しない。
- 資格情報について、`cmdkey`出力全体の対象名部分一致を存在・保全証拠にしない。標準stageはcredential保全を未検証と明示する。

## ログ・排他・再開

各操作は `.local/logs/vm/<run-id>/result.json` と `commands.log` を作る。`resets`に開始前／終了後／失敗回復の根拠・中断対象・復元結果を別々に残す。通常検証の終了後はディスクを破棄するため、必要な成果物・画面・guestログをセッション中にホストへ回収する。保守中は失敗時の現在ディスクと予約も証拠として保持する。
`task_status` は `succeeded` / `failed` / `blocked`、実行中は `running`。
before/after状態、guest結果、コマンドの終了値と証拠パスを合わせて判断する。

VMの実UUIDに対するホスト共通ロックで、worktreeやprofile名が違っても同じVMを同時操作しない。`status`はロック中も利用できる。旧スクリプトの直接実行はcontrollerの排他を通らないので、通常作業ではcontrollerを使う。
同じVMを対象とする一連の状態確認・準備・実行は順次行う。`status`と`operation-status`も同じVirtualBox objectへアクセスし、並列起動するとobject/lockの過渡エラーになり得るため、独立した読み取りとして並列化しない。`status`のロック例外は実行中操作の監視用であり、失敗した状態取得を正常状態と見なすものではない。
実行前にホスト共通の永続予約も作る。通常の失敗回復はVMごと復元して予約を解消する。controllerの強制終了などで予約が残った場合も、次のクリーンrunまたは`reset`で復元する。復元の成功・電源off・対象snapshotを確認できなければ、最初のsnapshot照会の失敗でも予約を残して失敗とする。別worktreeのUI/保守セッションはshutdownでも暗黙に継続・破棄しない。所有者が終了させるか、明示した`reset --discard-session`で破棄する。実行中controllerのロックはリセットでも迂回しない。
未解決のscript/interactive/reset予約を`start/shutdown --keep-current`や`snapshot`で保守セッションに置き換えない。先に`reset`、または終了を確認した`resolve-operation`で解決する。
build/test helperが確定した通常の失敗で終了した場合は、失敗結果を保持しつつ予約を解除する。timeout・signal・SSH切断や完了を断定できない高位exit statusは保守的に未解決として残し、mutationを自動再実行しない。
installの既知の事前検証・終了済みinstallerの失敗も予約を解除する。installer起動後に完了が不明な場合はguest status124で未解決を保つ。interactiveの起動拒否はtaskの不存在/非実行状態を確認し、残った非実行taskを解除できた場合だけ予約を解除する。状態照会やcleanupの失敗、queued/runningは成功と見なさない。
`resolve-operation`によるinteractive予約の解除も、taskの不存在、またはReady/Disabledからの削除成功と再照会による不存在確認を必要とする。削除・再照会が失敗した場合は予約を保持する。保守作業の完了解決後は予約を単純に消さず、保守セッションへ戻す。次のexec/doctorで現在ディスクを復元してしまうことを防ぐ。

低水準のhelperが不確実な終了を報告した時点では予約を保持する。通常検証は外側のlifecycleが停止・復元して解消するが、保守はそのまま保持する。`resolve-operation`は終了が既知の場合だけの手動解除。保守用shutdownのSSH応答喪失だけで要求を再送せず、host側のpoweroff観測で完了を判断する。snapshotのtimeoutも再captureせず、実体・CLI終了・電源状態を点検する。失敗を `tee` や復元成功で隠さない。CLI出力をパイプする場合は `set -o pipefail` を使う。

待機の既定値はcold bootのSSH readiness90秒、通常のPowerShell照会15秒、task登録・点検30秒、`exec`120秒、build/testの無出力120秒、使い捨て終了時のACPI猶予10秒、保守用通常停止120秒、VirtualBoxコマンド20秒。scriptの完了点検では無関係なTask Scheduler照会をせず、予約にtaskがある場合だけその対象を点検する。SSH/SCPはBatchMode・接続5秒・keepalive5秒×2回で対話待ちと切断待ちを抑える。profileの`ready_timeout`/`shutdown_timeout`/`maintenance_shutdown_timeout`/`idle_timeout`は必要時だけ調整し、無期限待ちを導入しない。

## baseline更新（明示的な保守）

更新の判断材料は「復元のたびに必要な補修」と「保存メモリに依存せず再起動できること」。例: Rust pin未導入、BuildTools検出漏れ、古い時刻、インストールメディアへの起動、検証中のスリープ。

1. 現行snapshotのUUIDと現在状態を記録。作業中の別ユーザー/agentがいないことを確認する。
2. build VMは`prepare`、手動VMは`start --keep-current`で明示的な保守セッションを開始する。必要な限定的保守を`exec --script`で行う。証明書検証は無効化しない。
   `prepare`が継続するのは既存の保守セッションだけ。通常の検証セッションからはベースへ戻して開始し、インストール済み試験データを新baselineへ持ち込まない。
3. `prepare`でクリーンな保守セッションを開始し、後続のdoctor/execで変更を保持して確認する。cold boot確認には`reboot`または`shutdown --keep-current`→`start --keep-current`を使う。ベース捕捉前の停止も`shutdown --keep-current`を使う（通常のshutdown/startはベースへ戻す）。保存メモリ再開だけを合格にしない。
4. 手動VMのベースへ入れる版・設定を明示して準備し、`shutdown --keep-current`。通常の試験データやデバッグ起動をbaselineへ混入させない。
5. `snapshot <new-name>` でpowered-off baselineを追加し、旧baselineは残す。新しいUUIDを記録する。
6. `restore <new-name>`→`start --keep-current`→doctorで新baselineのcold boot・診断を確認し、`shutdown --keep-current`後に `.local/vm.json` のsnapshot参照を更新する。最後に`reset`して新baselineへ戻す。
7. 最終的な両VMの電源状態、インストール識別子、snapshot UUID、未確認事項を報告する。

これは保守として依頼された場合の手順であり、通常検証ごとにsnapshotを増やす理由にしない。

## 検証

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_vm*.py'
```

mockによる排他・接続設定・失敗伝播・前後復元・異常時回復の検証と、実VMでの起動/build/test/install/終了状態の検証を分けて記録する。ローカルVM合格は正式なGitHub CI合格の代わりではない。
