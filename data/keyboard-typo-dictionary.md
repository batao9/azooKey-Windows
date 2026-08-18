# PCキーボード誤入力辞書（rewrite併用選定版）

`keyboard-typo-dictionary.tsv` は、ローマ字QWERTY入力で生じる打鍵ミスと、ローマ字かな変換時の曖昧性を補う96件の候補です。Mozc型rewriteの `SmallTSU` と `DoubleNN` を同時に有効化する前提で、両規則が処理できる候補は辞書から除いています。

この辞書は、全般設定末尾の実験的機能「誤入力の補正機能」を有効にした場合だけサーバーへ読み込まれます。デフォルトはOFFです。Windows VM上の製品実装では、96件辞書とSmallTSU・DoubleNNを一括でOFF/ONして計測しました。30物理キーのclean missにおける3プロセスのpaired p50中央値は、フル変換`+1.852 ms/request`、1キーごとの変換累積`+13.906 ms/30 keys`（約`+0.464 ms/key`）でした。先頭候補はOFF/ONで不変です。

測定条件とp50/p95の全結果は `keyboard-typo-performance.md` に記録しています。

## 構成

| 所有者 | 状態 | 辞書件数 | 方針 |
|---|---|---:|---|
| 通常辞書 | 採用候補 | 39 | 必須seed 2件とJWTD根拠のcore 37件 |
| `SmallTSU` | rewrite有効 | 0 | rewriteで完全に処理できる候補は辞書へ重複登録しない |
| `DoubleNN` | rewrite有効 | 0 | 母音branchもこの規則内で完結させ、汎用`NN`は有効化しない |
| `NN` | rewriteは実験止まり | 10 | `みんあ→みんな` など固定spanだけ辞書候補化 |
| `M` | rewriteは実験止まり | 10 | 未確定`m`を含むraw composition専用候補 |
| `Yu` | rewriteは見送り | 28 | 意味がほぼ固定できる語全体だけ辞書候補化 |
| `NI` | rewriteは見送り | 9 | 正しい別単語との衝突を避けた語全体だけ辞書候補化 |

rewriteの状態、調査値、実装時に必要な負例は `keyboard-typo-rewrite-rules.tsv` に分離しています。候補数を性能・順位評価後に減らしても、規則の採否とは独立して変更できます。

## 有効にするrewrite

### SmallTSU

ローマ字入力から生じた `([^っ])っっ([^っ])` を、促音1文字の経路へ補正します。3文字以上の連続促音や直接かな入力には適用しません。元入力の経路は残します。

以前の辞書案から次の8件を移管しました。

`なっって`、`なっった`、`よっって`、`であっった`、`しまっって`、`なかっった`、`あっった`、`だっった`

### DoubleNN

ローマ字入力から生じた `([^ん])んん` を、過剰な `ん` が1つの経路へ補正します。`こんんいちは` のように後続が母音の場合は、この規則内で `こんにちは` まで生成します。汎用 `NN` を連鎖的に有効化する必要はありません。

以前の辞書案から `ほとんんど→ほとんど` と `オープニンング→オープニング` を移管しました。後者もKKC内部のひらがな読みへ正規化して重複判定します。

両規則とも `roman2kana_only` とし、直接かな入力、末尾の未完成pattern、3連続の `んんん` / `っっっ` では発火させません。実装時にはBackspaceやカーソル編集で発火状態が解除されることも別途検証します。

## 辞書の利用契約

- KKC lattice上の1つの補正spanが `typed_reading` 全体を消費した場合だけ照合する。composition全体との一致は要求しない。
- 原入力から生成した通常候補を削除せず、補正候補をpenalty付きで追加する。
- `corrected_reading` を再変換し、`expected_candidate` は回帰テストの最低期待値として使う。
- 辞書由来の補正候補は学習対象外とし、機能をOFFにした後に学習メモリから再出現させない。
- 辞書エントリは誤入力補正用metadataで識別し、ローマ字互換区間の原surface lookup、または誤り訂正を介さないraw input lookupだけに許可する。直接かな・再変換・別の補正で生成した経路には適用しない。
- 単純な部分文字列置換、無制限な前方一致、入力途中での強制置換には使わない。
- 1回の入力で適用する辞書補正は最大1件とする。
- `M` 由来候補、物理キー列が `n` で終わる候補、長音キー `-` を含む候補は、未確定ASCIIを含む `raw_lattice_span_exact` とする。通常のかな辞書lookupへ混ぜず、canonicalな `typed_reading` は保持したまま、生成時だけ末尾 `ん→n`、長音 `ー→-` としてlatticeへ照合する。

この契約により、`ごかくにおねがいします` の先頭spanを補正して `ご確認お願いします` を生成しつつ、ユーザーが意図した原入力の候補も残せます。

## 広いrewriteから辞書へ移した候補

### NN

汎用 `ん[あいうえお]` rewriteは、`原案`、`簡易`、`店員`、`運営`、`千円` など正しい語境界にも広く発火します。このため規則は有効化せず、`みんあ→みんな`、`こんいちは→こんにちは`、`そんあ→そんな`、`ちゃんえる→チャンネル` など10件だけを固定候補にしました。

NN・NIの `typed_reading` はローマ字composerの中間キーを表します。例えばraw key `minna` 自体は正しくても、中間キーが `みんあ` になる経路を辞書候補で `みんな` へ接続します。そのため、このTierでは `typed_keys` と `corrected_keys` が同一です。

### M

汎用 `m[ばびぶべぼぱぴぷぺぽ]→ん[...]` はJWTDで全文exactが0件で、`10cmばかり` を壊す発火例がありました。規則は有効化せず、`しmばし→新橋`、`てmぷら→天ぷら`、`せmぱい→先輩` など10件をraw composition専用の実験候補にしています。

実装時にroman composerがこれらの中間キーを外部へ公開しない場合、この10件は効果がないため削除対象です。

### Yu

汎用 `[きしちにひり]ゅ[^う]` rewriteは、`宿題`、`手術`、`キューバ`、`キュリー` などの正しい入力にも発火します。規則は見送り、`ちゅごく→中国`、`きゅけい→休憩`、`りゅがく→留学`、`にゅいん→入院` など28件を固定候補にしました。

`しゅかん→しゅうかん` は「主観」、`しゅしょく→しゅうしょく` は「主食」、`しゅりょう→しゅうりょう` は「狩猟」、`しゅちゅう→しゅうちゅう` は「手中」、`ちゅもん→ちゅうもん` は固有名「朱蒙」と衝突するため採用していません。

### NI

汎用 `にゃ/にゅ/にょ→んや/んゆ/んよ` rewriteは、`ニュース`、`入院`、`にゃん`、`セニョリータ` へ広く発火します。規則は見送り、`せにょう→専用`、`きにょうび→金曜日`、`れにょうけい→連用形` など9件を固定候補にしました。

`きにゅう→きんゆう` は「記入」、`かにゅう→かんゆう` は「加入」、`しにょう→しんよう` は「屎尿」、`しにゅう→しんゆう` は「刺入」、`せにゅう→せんゆう` は「施入」と衝突するため採用していません。

## 選定Tierと列

| `selection_tier` | 意味 |
|---|---|
| `required` | 製品要件の `しますた→しました`、`ごかくに→ご確認` |
| `core` | JWTD trainで5件・5ページ以上、訂正方向98%以上を満たした打鍵ミス |
| `candidate` | 無効化したrewriteから回収した固定語。低頻度または手動seedを含み、実性能・順位評価後に削減可能 |

| 列 | 意味 |
|---|---|
| `origin_rule` | `none`、`NN`、`M`、`Yu`、`NI` の候補由来 |
| `typed_reading` | 補正spanが完全一致させる読み、またはcomposer中間キー |
| `corrected_reading` | KKCへ追加する訂正後の読み |
| `expected_candidate` | 回帰テストで期待する代表候補 |
| `typed_keys` / `corrected_keys` | 差分を説明する代表的なローマ字キー列 |
| `mechanism` | 想定した打鍵ミスまたはrewrite fallbackの種類 |
| `source` | `product_seed`、`JWTD_v2_train`、`curated_seed` |
| `train_count` / `page_count` | JWTD trainでの差分数と異なるWikipediaページ数 |
| `reverse_count` | JWTD trainで逆向きに編集された件数 |
| `test_count` | JWTD testで同じ向きに現れた件数 |
| `match_policy` | 通常は `lattice_span_exact`。M由来、末尾 `n`、長音キー `-` を含む候補は `raw_lattice_span_exact` |
| `builtin_collision_count` | compiled LOUDS内で誤入力読みと完全一致するsurface数。0以外は順位・penaltyの重点評価対象 |

## 現在の検証値

- 辞書候補96件: JWTD由来55件、手動candidate 39件、製品seed 2件。
- JWTD由来候補はtrainで合計982差分、testで合計18差分。
- compiled LOUDS本体との完全一致衝突は5 reading・8 surface。各件数をTSVへ記録し、validatorで実データと照合している。
- 残した衝突は `いすれ` 4 surface、`ことろ`、`ずべて`、`ちゅおう`（中欧）、`しゅまつ`（朱抹）各1 surface。原入力を残してpenaltyを付けた場合の順位評価対象とする。
- `タクス→タスク`（託す）、`しゅちゅう→集中`（手中）、`しにゅう→親友`（刺入）、`せにゅう→専有`（施入）、`ちゅもん→注文`（朱蒙）は候補から除外した。
- SmallTSU・DoubleNNが完全に処理できる辞書候補は0件。
- 打鍵列とmechanism、rewrite状態、candidate由来、matching layerを検証スクリプトで固定している。
- Windows VMのrelease buildで96件を全件走査し、ON時は96/96件で期待候補の順位が改善した。OFF時から存在した1件（6位）を除く95件はOFF時に候補外で、ON時は全96件が1位だった。内訳は通常39件、NN 10件、M 10件、Yu 28件、NI 9件。

これらは候補抽出・構造検証と採用候補への到達性の値で、実利用コーパス全体のTop-N精度や誤補正率ではありません。96件構成のconverter局所レイテンシには件数を50件へ削る必要があるほどの悪化は見つかりませんでした。Windows VMの結合テストでは、設定のデフォルトOFF、ON/OFF切替、必須2例、SmallTSU・DoubleNN、直接かな入力への非適用を確認しています。通常入力のTop-1不変性、誤補正率、installed IMEのend-to-end tail latencyは継続評価します。

## 出典とライセンス

JWTD v2 の統計は、Yu Tanaka、Yugo Murawaki、Daisuke Kawahara、Sadao Kurohashiによる Japanese Wikipedia Typo Dataset v2 と、日本語Wikipediaの編集履歴に基づきます。

- Yu Tanaka et al., [Building a Japanese Typo Dataset from Wikipedia's Revision History](https://aclanthology.org/2020.acl-srw.31/)
- 田中佑ほか, [日本語Wikipediaの編集履歴に基づく入力誤りデータセットと訂正システムの改良](https://www.anlp.jp/proceedings/annual_meeting/2021/pdf_dir/E8-3.pdf)
- Google Mozc, [KeyCorrector](https://github.com/google/mozc/blob/851c3fe33060d2a6090363e4d7ec44fafde2c03d/src/converter/key_corrector.cc)

このTSV、rewrite調査表、本文中のJWTD由来の集計・選定データは [CC BY-SA 3.0](https://creativecommons.org/licenses/by-sa/3.0/) で提供します。リポジトリ内のプログラム本体に適用されるMIT Licenseとは別です。検証コードにはリポジトリのMIT Licenseが適用されます。

JWTD由来データの帰属表示は `keyboard-typo-dictionary-NOTICE.md`、MozcのBSD-3-Clause表示は `keyboard-typo-rewrite-NOTICE.md` に保持し、インストーラーにも同梱します。GitHub Typo Corpusは公式配布先から元データを取得できなかったため、この候補の根拠には使用していません。MSR Spelling Correction Dataは非商用条件のため、配布辞書の派生元には使用していません。
