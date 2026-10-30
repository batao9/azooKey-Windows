# 配布データ

このディレクトリは、製品に組み込むデータとそのライセンス表示をGit管理する場所です。

- [`keyboard-typo-dictionary.tsv`](keyboard-typo-dictionary.tsv): 選定済みの誤入力補正辞書。Swift生成物と回帰テストの入力。
- [`keyboard-typo-rewrite-rules.tsv`](keyboard-typo-rewrite-rules.tsv): rewriteの採否・照合範囲・負例の契約。
- [`keyboard-typo-dictionary-NOTICE.md`](keyboard-typo-dictionary-NOTICE.md): JWTD由来データの帰属・CC BY-SA 3.0表示。
- [`keyboard-typo-rewrite-NOTICE.md`](keyboard-typo-rewrite-NOTICE.md): Mozc由来のBSD-3-Clause表示。

列の意味、生成・検証方法、サーバーの利用契約は [誤入力補正の仕様](../docs/keyboard-typo-correction.md) を参照してください。