# 誤入力補正・製品実装ベンチマーク

計測日: 2026-08-18
対象: 96件の動的辞書 + `SmallTSU` + `DoubleNN` を一括でOFF/ONする製品実装
KKC revision: `38b59b5cbb97e356798a1d4fd6fed0658cd825ad`（性能計測時点）

順位評価後の選定結果は、この96件から複数ノード圧縮になる2件、正しい「中欧」に衝突する1件、KKCの枝刈り閾値を下回る1件を除いた92行である。文中接続対応では、末尾未確定 `n` と長音キー `-` を含む12行にcanonical/rawの2 variantを持たせるため、最終実装は104 lookup entryになる。本計測は構成が近い参考値だが、最終構成の保守的上限とは扱わない。

最終revision `d5e9ffa1be372be488ab74f736662897835d397c` は、通常の全ローマ字入力に同じfast pathを使い、混在入力時の入力区間判定と誤入力辞書の適用範囲を厳密化している。末尾の未確定 `n` と長音キーを照合する辞書生成の修正、辞書候補を学習対象外にする後処理は、辞書件数およびrewriteの候補生成経路を変えない。また、補正OFFのinput-only lookupではcomposition全体のsurfaceを生成しないようにし、既定OFFのhot pathへ不要なコピーを持ち込まない。

## 結論

clean miss 30物理キーでの3プロセス中央値は、完成入力のpaired p50が `+1.852 ms/request`、1キーごとに変換した累積のpaired p50が `+13.906 ms/30 keys`（単純平均 `+0.464 ms/key`）だった。通常入力の先頭候補はOFF/ONとも「私たちの生活の中で」で不変だった。

必須2例はON時に1位へ入り、OFF時には出現しなかった。`SmallTSU` の `きっって→切って` は4位、`DoubleNN` の `こんんいちは→こんにちは` は1位だった。

## 3プロセス中央値

各セルは、それぞれの独立プロセスで得たp50/p95の中央値。paired deltaは同じ反復のON-OFFを取ってからpercentileを求めている。単位はms。

| scenario | path | OFF p50 / p95 | ON p50 / p95 | paired delta p50 / p95 | ON期待順位 |
|---|---|---:|---:|---:|---:|
| clean miss 30 keys | full | 58.307 / 163.057 | 59.011 / 161.980 | +1.852 / +36.306 | - |
| clean miss 30 keys | realtime累積 | 1379.392 / 1795.933 | 1403.879 / 1823.687 | +13.906 / +453.140 | - |
| `しますた` | full | 36.414 / 66.044 | 37.414 / 72.824 | +0.334 / +17.248 | 1 |
| `しますた` | realtime累積 | 368.468 / 543.446 | 333.530 / 604.223 | -7.830 / +134.008 | 1 |
| `ごかくにおねがいします` | full | 50.398 / 90.009 | 51.535 / 85.522 | +1.572 / +29.040 | 1 |
| `ごかくにおねがいします` | realtime累積 | 976.509 / 1277.779 | 952.197 / 1297.841 | -22.301 / +277.511 | 1 |
| `SmallTSU` | full | 42.697 / 99.753 | 44.685 / 89.494 | +0.716 / +34.527 | 4 |
| `SmallTSU` | realtime累積 | 447.449 / 709.410 | 495.043 / 771.861 | +22.351 / +318.306 | 4 |
| `DoubleNN` | full | 47.278 / 82.694 | 49.167 / 88.269 | +0.086 / +39.548 | 1 |
| `DoubleNN` | realtime累積 | 424.821 / 596.197 | 439.303 / 659.779 | -5.681 / +197.399 | 1 |

## 方法と解釈

- Windows VM、Swift 6.1.2、release build。
- KKC `requestCandidates` を直接計測。Rust IPC、TSF client、候補window描画は含まない。
- converterと辞書の構築・importは計測外。
- fullは各scenarioを6回warmup後、compositionを毎回resetして100 samples/process。
- realtimeは各scenarioを3 rounds warmup後、空のcompositionから物理ローマ字キーを1文字ずつ追加して40 rounds/process。
- OFF/ONの実行順は反復ごとに交互にした。
- 独立processを3回実行し、3/3成功。
- p95のpaired deltaはVMスケジューラ由来の外れ値が大きく、規則自体のtail overheadとは断定しない。中心判断はpaired p50と候補不変性とする。
- installed IMEのend-to-end tail latencyは、依存revisionをremoteから取得できる状態にした後のインストール検証で別途確認する。

## 既定OFF hot pathのfollow-up

最終レビューで、revision `90f29d53122e99d667d05a6fe7909ff629b67a3e` は補正OFFのinput-only lookupでもcomposition全体のkatakana surfaceを生成していることが分かった。最終revision `d5e9ffa1be372be488ab74f736662897835d397c` ではsurface lookupがある場合だけ生成するよう戻し、既定OFFのinput-only pathに全文コピーを追加しない。

同じclean miss 30物理キーを補正OFFだけで修正前後それぞれ3独立process計測した。各値はprocessごとのp50をさらに中央値にしたもので、修正前/最終はfull `14.027 / 16.022 ms`、realtime累積 `101.561 / 108.675 ms`（差 `+7.114 ms/30 keys`、約 `+0.237 ms/key`）だった。独立VM run間のばらつきがこの差より大きく、timingだけから小さな改善・悪化は判定できない。一方、最終revisionでは問題の全surface変換とArray allocationがinput-only branchからコード上なくなっているため、脆いtiming assertionは追加していない。
