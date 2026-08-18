# 誤入力補正・文中順位評価

計測日: 2026-08-18

対象: 92選定行（104 lookup entry）の誤入力辞書 + `SmallTSU` + `DoubleNN`

環境: Windows VM、Swift 6.1.2、release build、実配布用 `zenz.gguf`、CPU inference（GPU layer 0）

## 結論

辞書補正17ケースは、Zenzai OFFでは17/17件が1位だった。Zenzai ONの直接出力では2/17件だけが1位で、残り15件はZenzai候補の直後となる2位だった。製品の候補統合時に後述のconfidence判定を適用すると、17/17件が1位になった。

`SmallTSU` と `DoubleNN` はZenzai OFFでは1位、Zenzai ONでは直接出力・統合後とも2位だった。rewrite由来であることを`Candidate`から判別できないため、辞書補正と同じ昇格処理は適用していない。

正しい入力との衝突を確認する負例8ケースは、Zenzai OFF、Zenzai ON直接出力、統合後のすべてで期待候補が1位だった。誤入力辞書由来の候補は8/8件で生成されなかった。

| 対象 | 件数 | Zenzai OFF 1位 | Zenzai ON直接 1位 / 2位以内 | 製品統合後 1位 / 2位以内 |
|---|---:|---:|---:|---:|
| 辞書補正 | 17 | 17 | 2 / 17 | 17 / 17 |
| SmallTSU・DoubleNN | 2 | 2 | 0 / 2 | 0 / 2 |
| 負例（自然候補） | 8 | 8 | 8 / 8 | 8 / 8 |

## スコアリングと統合方針

以前の全候補共通CID・固定スコアはやめ、各訂正語と完全一致するバンドル辞書エントリの `lcid`、`rcid`、`mid`、語彙スコアを使う。これにより、補正語の前後は通常辞書と同じ品詞接続コストで評価される。複数辞書ノードを1つに圧縮する必要があった2件は採用対象から外した。

語彙スコアには根拠Tierに応じて `required=1.0`、`core=1.5`、`candidate=2.5` の補正ペナルティを加える。ペナルティは候補を無条件に優先するためではなく、同じ品詞・語彙スケール上で通常候補と競合させるための初期値である。

Zenzaiの `Candidate.value` は通常KKCと同じ比較可能なスコアではなく、分節が異なる候補間の固定marginも信頼できなかった。このため、Zenzai候補と通常N-bestの数値を直接比較しない。製品統合では次の条件をすべて満たすときだけ、通常N-bestの1位をZenzaiの1位より前へ移す。

1. 通常N-bestの1位に誤入力辞書metadataがある。
2. Zenzaiの1位に、誤入力span全体を1ノードで消費する別の正規語彙候補がない。
3. 元のZenzai 1位は削除せず2位以降へ残す。

2の判定により、正しい「中欧」のような語を固定補正で上書きしない。実測中に `ちゅおう→中央` が「中欧」と衝突したため、この辞書項目自体も採用候補から除外した。

## 文中接続ケース

順位は1始まり。`ON直接`はZenzai converterの直接出力、`ON統合`は通常N-best補完とconfidence判定を含む製品候補順である。

| ケース | 物理キー列 | 期待候補 | OFF | ON直接 | ON統合 |
|---|---|---|---:|---:|---:|
| 必須・文全体 | `iikagentouitusimasuta` | いい加減統一しました | 1 | 2 | 1 |
| 必須・左文脈あり | `simasuta` | しました | 1 | 2 | 1 |
| 必須・文全体 | `syoruinogokakunionegaisimasu` | 書類のご確認お願いします | 1 | 2 | 1 |
| 必須・左文脈あり | `gokakunionegaisimasu` | ご確認お願いします | 1 | 2 | 1 |
| core | `jyuuyounakikakkedatta` | 重要なきっかけだった | 1 | 2 | 1 |
| core | `isurekawosentakusuru` | いずれかを選択する | 1 | 2 | 1 |
| core | `zubetekakuninsuru` | すべて確認する | 1 | 2 | 1 |
| core | `atarasiifashonwosyoukaisuru` | 新しいファッションを紹介する | 1 | 2 | 1 |
| core・長音接続 | `shopingumo-runiiku` | ショッピングモールに行く | 1 | 2 | 1 |
| NN候補 | `sonnakotohanai` | そんなことはない | 1 | 2 | 1 |
| NN候補 | `minnadehanasu` | みんなで話す | 1 | 2 | 1 |
| M候補 | `simbasiniiku` | 新橋に行く | 1 | 2 | 1 |
| M候補 | `tempurawotaberu` | 天ぷらを食べる | 1 | 2 | 1 |
| Yu候補 | `tyugokuniiku` | 中国に行く | 1 | 1 | 1 |
| Yu候補 | `kyukeiwotoru` | 休憩を取る | 1 | 1 | 1 |
| NI候補 | `renyoukeiwotukau` | 連用形を使う | 1 | 2 | 1 |
| NI候補 | `senyousyawotukau` | 専用車を使う | 1 | 2 | 1 |
| SmallTSU | `panwokixtuxtutekureru` | パンを切ってくれる | 1 | 2 | 2 |
| DoubleNN | `konnnnitihatoiu` | こんにちはという | 1 | 2 | 2 |

必須例の候補上位は次のようになった。

| 入力 | Zenzai ON直接 | 製品統合後 |
|---|---|---|
| いい加減統一しますた | 1. いい加減統一します田 / 2. いい加減統一しました | 1. いい加減統一しました / 2. いい加減統一します田 |
| しますた（左文脈「昨日、作業を」） | 1. します田 / 2. しました | 1. しました / 2. します田 |
| 書類のごかくにお願いします | 1. 書類のご確にお願いします / 2. 書類のご確認お願いします | 1. 書類のご確認お願いします / 2. 書類のご確にお願いします |
| ごかくにお願いします（左文脈「資料を添付しましたので、」） | 1. ごかくにお願いします / 2. ご確認お願いします | 1. ご確認お願いします / 2. ごかくにお願いします |

末尾未確定 `n` と長音キー `-` を含む12選定行は、composition末で使うraw variantに加えて、後続文字が入った文中で使うcanonical variantも生成する。これにより、単語末だけでなく「ファッションを」「ショッピングモールに」の接続まで同じ辞書項目で処理できる。

## 負例

| 物理キー列 | 期待候補 | OFF | ON直接 | ON統合 | 誤入力辞書候補 |
|---|---|---:|---:|---:|---:|
| `bikkukameraniiku` | ビックカメラに行く | 1 | 1 | 1 | なし |
| `nakatasann` | 中田さん | 1 | 1 | 1 | なし |
| `nyu-suwoyomu` | ニュースを読む | 1 | 1 | 1 | なし |
| `nyuuinsuru` | 入院する | 1 | 1 | 1 | なし |
| `syukudaiwosuru` | 宿題をする | 1 | 1 | 1 | なし |
| `syuzyutuwoukeru` | 手術を受ける | 1 | 1 | 1 | なし |
| `kyu-baniiku` | キューバに行く | 1 | 1 | 1 | なし |
| `tyuounoryokou` | 中欧の旅行 | 1 | 1 | 1 | なし |

## 測定方法と制約

- Windows用のmapped romaji tableで物理キーを1文字ずつ `ComposingText` へ入力した。
- learningと日本語・英語predictionは無効。ケースごとに `stopComposition()` し、同一の辞書、入力列、左文脈をOFF/ONで使用した。
- Zenzai ONは `inferenceLimit=1`、rich candidates有効、実配布モデル、CPU inferenceで実行した。
- 27ケース × 3経路の81結果を取得し、VMテストは約29.2秒で完走した。この時間は順位確認用test全体であり、レイテンシ指標ではない。
- 計測時に残っていた `ちゅげん→中原` は生成スコアがKKCの枝刈り閾値未満で、全経路でlookup前に除外されていた。最終選定から削除し、validatorで同種の到達不能entryを拒否する。評価ケースの候補集合と順位には影響しない。
- 本評価は選定した文例に対する順位回帰で、実利用コーパス全体のTop-1精度や誤補正率を推定するものではない。
- SmallTSU・DoubleNNをZenzai ONでも1位へ昇格させるには、KKCからrewrite由来metadataを返す必要がある。出自を判別せず通常N-best 1位を常に優先すると、Zenzaiが選んだ自然な候補まで上書きするため現時点では行わない。
