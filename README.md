# azooKey for Windows

[AzooKeyKanaKanjiConverter](https://github.com/azooKey/AzooKeyKanaKanjiConverter)を利用したWindows版IMEです。

> [!WARNING]
> 現在開発中であるため、安定性や機能に関しては保証できません。使用する際は自己責任でお願いします。

# インストール方法
[Release](https://github.com/fkunn1326/azooKey-Windows/releases)から`azookey-setup.exe`をダウンロードし、インストーラーを実行してください。

# 機能

- [x] ライブ変換
- [x] Zenzaiを使用したニューラルかな漢字変換

- [ ] 学習機能
- [x] 辞書登録機能（MVP）
- [ ] テーマ変更機能
- [ ] 辞書のインポート/エクスポート機能
- [ ] いい感じ変換
- [ ] 個人最適化システム
- [ ] 予測変換

# 設定

## 全般設定

### 基本設定
- 句読点
- 記号
- スペースの入力
- テンキーからの入力

### キー設定
- ローマ字テーブル:
  - 設定画面から行テーブル形式で編集できます（入力 / 出力 / 次の入力）。

### 半角全角設定
日本語入力時の文字幅はカテゴリごとに `半角 / 全角` を設定できます。

### 変換の優先順位
記号・句読点の変換は次の優先順位で適用されます。

1. ローマ字テーブル（1文字入力ルールのみ）
2. 基本設定（句読点 / 記号）
3. 半角全角設定（カテゴリ設定）

## 辞書

- 設定画面の「辞書」から、`読み` と `単語` を追加・編集・削除できます。
- MVPではインポート/エクスポートには未対応です。
- 登録件数は最大 `50` 件です。
- 本実装は動的ユーザ辞書方式です（静的 `user.louds*` は未対応）。

### 入力モード切替ショートカット
- `半角/全角`: 入力モード切り替え（英数/ひらがな）
- `VK_IME_ON` (`0x16`): ひらがな入力へ切替
- `VK_IME_OFF` (`0x1A`): 英数入力へ切替

英語キーボードでは以下のショートカットも設定可能です。
- `Ctrl + Space`: 入力モード切替（英数/ひらがなかな）
- `` Alt + ` ``: 入力モード切替（英数/ひらがなかな）

### 変換中ショートカット
- `Ctrl + Enter`: 先頭文節のみを確定
- `Ctrl + ↓`: 現在文節を確定して次文節へ移動
- `Shift + ← / →`: 文節境界を前後に調整
- `Shift + A〜Z`: 一時英字モードで未確定入力（確定操作または `Shift` 単独押下で解除）

## Zenzai

### 変換プロファイル
設定で変換プロファイルを指定すると、プロファイルに応じた変換候補が表示されます。

### バックエンド
以下の3種類のバックエンドをサポートしています。

- **CPU**: 動作が非常に遅いため、非推奨です。
- **CUDA**: NvidiaのGPU専用。[CUDA Toolkit 12系](https://developer.nvidia.com/cuda-downloads)をインストールする必要があります。
- **Vulkan**: GPUのドライバーに標準で含まれているため、追加のインストールは不要です。

# コミュニティ

## 開発を支援する
- [GitHub Sponsors (Miwa)](https://github.com/sponsors/ensan-hcl): 変換エンジンの開発者
- [Patreon (fkunn1326)](https://www.patreon.com/c/fkunn1326): Windowsに移植した人

## 開発に参加する

### 開発環境のセットアップ

- [Rust](https://www.rust-lang.org/tools/install)
- [Swift for Windows](https://www.swift.org/install/windows/) (Swift 6.0以上)
- [protoc](https://protobuf.dev/installation/) 
- [node.js](https://nodejs.org/en/download/)
- [inno setup](https://jrsoftware.org/isinfo.php)

### ビルド

#### リポジトリのクローン
```
git clone https://github.com/fkunn1326/azookey-Windows --recursive
```
`--recursive`オプションを付けて、サブモジュールも一緒にクローンしてください。

#### cargo-makeのインストール
```
cargo install --force cargo-make
```

#### ビルド
```
cargo make build [--debug/--release]
```
`--debug`オプションを付けるとデバッグビルド、`--release`オプションを付けるとリリースビルドになります。必ずどちらかを指定してください。

`build`フォルダーが作成され、ビルドされた実行ファイルが格納されます。

`launcher.exe`を管理者権限で実行すると、azookeyの変換エンジンが起動します。

また、IMEを登録する際は以下のように`regsvr32.exe`を使用して登録する必要があります。
```c
regsvr32.exe "path/to/build/azookey_windows.dll" /s
regsvr32.exe "path/to/build/x86/azookey_windows.dll" /s
```
逆に登録を解除する場合は`/u`オプションを付けて実行してください。

#### 開発時のヒント
- 開発は仮想マシンまたは専用のPCで行うことを推奨します。IMEがクラッシュするとWindowsがフリーズする可能性があります。
- IMEを解除する際、IMEを使用中のアプリケーション（メモ帳など）を終了しないと、解除できないことがあります。

# 関連

- [azooKey/azooKey](https://github.com/azooKey/azooKey): iOS / iPadOS向けの日本語キーボードアプリ
- [7ka-Hiira/fcitx5-hazkey](https://github.com/7ka-Hiira/fcitx5-hazkey): fcitx5向けのLinux版azooKey
- [azooKey/AzookeyKanakanjiConverter](https://github.com/azooKey/AzooKeyKanaKanjiConverter): azooKeyの変換エンジン

# 参考
本プロジェクトの開発にあたり、以下のリソースを参考にしました。ありがとうございます！
- [OMAMA-Taioan/khiin-rs](https://github.com/OMAMA-Taioan/khiin-rs/tree/master/windows)
- [google/mozc](https://github.com/google/mozc/tree/master/src/win32/tip)
- [microsoft/Windows-classic-samples](https://github.com/microsoft/Windows-classic-samples/tree/main/Samples/Win7Samples/winui/input/tsf/textservice)
- [dec32/ajemi](https://github.com/dec32/ajemi)
- https://zenn.dev/mkpoli/scraps/6dc57fcd0335cf
