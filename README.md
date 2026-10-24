# サンプル（プロダクト名）

## フロントエンドの起動

AcciLessNaviのフロント実装は [`frontend/`](frontend/README.md) にあります。`cd frontend` → `npm ci` → `npm run dev` でデモを起動できます。画面・実API接続・検証手順は同ディレクトリのREADMEを参照してください。

## バックエンドの起動と配備

APIサーバ（Rust/Axum）は [`backend/`](backend/README.md) にあります。設定は環境変数（[architecture.md](docs/architecture.md) 6章）で渡します。偽の上流を使ったローカル起動（`cargo run --manifest-path backend/Cargo.toml --example dev_stack`）と検証コマンドは backend/README.md の「API・外部通信・基盤（③）」、Cloud Run＋IAPへの配備手順（未実施）は [`deploy/README.md`](deploy/README.md) を参照してください。

[![IMAGE ALT TEXT HERE](https://jphacks.com/wp-content/uploads/2026/07/IMG_5316.png)](https://www.youtube.com/watch?v=piaNsc6ilBI)

## 製品概要
### 背景(製品開発のきっかけ、課題等）
### 製品説明（具体的な製品の説明）
### 特長
#### 1. 特長1
#### 2. 特長2
#### 3. 特長3

### 解決出来ること
### 今後の展望
### 注力したこと（こだわり等）
* 
* 

## 開発技術
### 活用した技術
#### API・データ
* 
* 

#### フレームワーク・ライブラリ・モジュール
* 
* 

#### デバイス
* 
* 

### 独自技術
#### ハッカソンで開発した独自機能・技術
* 独自で開発したものの内容をこちらに記載してください
* 特に力を入れた部分をファイルリンク、またはcommit_idを記載してください。
