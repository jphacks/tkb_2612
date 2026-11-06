# 自前経路探索の作業書

[routing-plan.md](routing-plan.md)（以下「計画書」）を実装するための作業手順。指示役（Opus）がこの作業書を読み、マイルストーン（M0〜M5）ごとに作業役（Sonnet）へ作業を割り振って実行する。この作業書と、ここから参照する文書だけで作業を進められるように書いている。

利用者は、マイルストーンを1つずつ指示役に依頼する。依頼の例：「`docs/routing-workplan.md` のM1を実行して」。

## 0. 前提

### 0.1 作業の起点は `origin/main`

この作業書は、2026-11-03時点の `origin/main`（`0ef4caa`）を前提に書いている。`origin/main` には、次のものがすでに実装されている（`backend/README.md`）。

- ③：Axumのハンドラ、DTO、OSRM・Geoapify・Open-Meteoのアダプタ、上流ゲート、設定と起動、CI、配備定義（`Dockerfile`、`deploy/`）。
- ②：前処理 `prepare_release`、release形式、祝日表の読込と判定（`load_calendar`）、起動時ロード（`load_engine`）、現行方式（OSRM候補＋72m・σ=24mの集計）のエンジン `AccidentEngine`。
- 祝日表：`accident/syukujitsu.csv`。

指示役は、作業を始める前に次を確かめる。満たしていなければ、作業せずに利用者に伝えて止まる。

```sh
git fetch origin
git rev-list --count HEAD..origin/main   # 0であること（起点がorigin/mainより古くない）
test -f accident/syukujitsu.csv && test -f accident/honhyo_2025.csv   # 事故原本と祝日表がaccident/にある
test -f data/osm_japan/japan-261101.osm.pbf   # 全国OSM
git status --porcelain                   # 未コミットの変更があれば、内容を利用者に見せて確認する
```

`origin/main` のテストは、事故原本とコード表と祝日表が repo root の `accident/` にあることを前提にしている。この作業書のパスも `accident/` で書く。

### 0.2 文書の優先順位

内容が食い違うときは、上にあるものを優先する。

1. 計画書の12.1節（利用者の決定事項）
2. M0で更新した `docs/api/behavior.md`・`docs/api/openapi.yaml`・`backend/src/contract/`（M0の完了後）
3. この作業書
4. `backend/README.md`（担当・依存方向・release形式・テストの規則）
5. 計画書のその他の節
6. `plan1.md`・`plan2.md`（背景資料。仕様の根拠にしない）

`app/` はPythonのデモで、仕様の根拠にしない。

## 1. 共通の規則

### 1.1 変更してはいけないもの

- `frontend/` の配下。フロントの変更は Issue #26 にまとめてあり、バックエンドの作業が終わった後に行う。例外は `frontend/src/api/generated/` だけで、`openapi.yaml` の `description` を変えたときに `npm run generate:api` で作り直す（CIが「`generate:api` の後に差分がないこと」を検査するため）。作り直した差分は、`description` の文字列だけであること。
- `docs/api/openapi.yaml` の型・`const`・`required`・`enum`・`minItems`/`maxItems`。変えてよいのは `description` と応答例（`examples`）の文言だけ。
- `accident/` の原本、`data/osm_japan/`・`data/n13_2024_geojson/`・`data/road_census_2021/` の取得済みファイル。
- ③の担当範囲：`backend/src/api/`・`backend/src/providers/`・`backend/src/main.rs`・`backend/src/startup.rs`・`backend/src/config.rs`・`Dockerfile`・`deploy/`・`.github/`。M0.1の推奨どおりに進めれば、ここを変える必要はない。変える必要が出たら、2.4節のとおり止まる。
- 現行方式のエンジン（`domain::candidates`・`corridor`・`scoring`・`engine` の `AccidentEngine`）の動作。M5で比較に使うので、動作を変えずに残す。

### 1.2 してはいけない操作

- Cloud Runの設定変更、配備、課金に関わる操作。
- `git push`、PRの作成。各マイルストーンは、専用のブランチでコミットまで行う。pushとPRは、人の確認の後に利用者が指示する。
- 公共のOSRMサーバー（router.project-osrm.org）への問い合わせ。OSRMは、M1の手順で手元のdockerに立てる。

### 1.3 外部から取得してよいもの

| 対象 | 取得元 | 使うマイルストーン |
|---|---|---|
| 関東のOSM抽出 `kanto-latest.osm.pbf` | Geofabrik | M1・M5（OSRM用） |
| OSRMのdockerイメージ `ghcr.io/project-osrm/osrm-backend` | GitHub Container Registry | M1・M5 |
| Rustのcrate、Pythonのパッケージ | crates.io、PyPI | 全体 |
| 確認用地図の背景タイル | OpenStreetMapの標準タイル（人の確認で数百枚程度を表示するだけ） | M1・M2b |

これ以外の外部データ（人口メッシュなど）が必要になったら、取得せずに代替案で進め、報告に書く。

### 1.4 ブランチとコミット

- ブランチ名は `feat/routing-m0` 〜 `feat/routing-m5`。M(n+1) のブランチは M(n) のブランチから切る。並行作業（3章）では、タスクごとに `feat/routing-m1-graph` のような子ブランチを切り、指示役がマイルストーンのブランチへ統合する。
- コミットメッセージは既存の形式（`feat: 〜`、`test: 〜`、`docs: 〜`、日本語）に合わせる。
- `Cargo.toml` と `Cargo.lock` の扱いは `backend/README.md` の「Cargo設定とlockfile」に従う。`rust-version` は1.88。

### 1.5 backendの依存方向

`backend/README.md` の「担当・依存方向・編集責任」と「事故データ・計算（②）」の「依存方向」に従う。今回の実装はすべて②の範囲である。

- 探索・リスク・グラフの本体は `backend/src/domain/` に置く。
- releaseの書き出しと読込は `backend/src/data/` に置く。`data` は `domain` に依存してよいが、逆は不可。
- 前処理と評価のコマンドは `backend/src/bin/` に置く。
- `domain`・`data`・`src/bin/` の②のコマンドは、`api`・`providers`・`testing`・tokio・axum・reqwest・hyperに依存しない。HTTPが必要な処理（M5で現行方式の経路を取る処理）は、Pythonのスクリプトで書く。
- 他モジュールは `crate::name::item` の形で参照する（`tests/boundaries.rs` の検査のため）。

### 1.6 生成物の置き場所

| 置き場所 | 内容 | Git |
|---|---|---|
| `data/work/<マイルストーン>/` | 中間生成物、ログ、確認用のHTMLとGeoJSON、Pythonの仮想環境 | 管理しない（M0でルートの `.gitignore` に `/data/work/` を足す） |
| `backend/data/releases/<release-id>/` | 配信物一式（`backend/README.md` のrelease形式に、グラフとリスクのファイルを加えたもの） | 管理しない（`backend/data/.gitignore`） |
| `docs/routing-reports/M<n>.md` | 各マイルストーンの完了報告 | 管理する |
| `docs/routing-data-format.md` | M0で作るデータ形式の仕様 | 管理する |

### 1.7 検証コマンド

CI（`.github/workflows/ci.yml`）と同じ内容を、repo rootから実行する。

```sh
cargo fmt --manifest-path backend/Cargo.toml --check
cargo clippy --manifest-path backend/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path backend/Cargo.toml --locked
cargo +1.88 test --manifest-path backend/Cargo.toml --locked
python3 -B backend/scripts/extract_route_examples.py --check
(cd frontend && npm ci && npm run generate:api && git diff --exit-code -- src/api/generated \
   && npm run typecheck && npm test && npm run build)
git diff --stat origin/main -- frontend/ ':!frontend/src/api/generated'   # 出力が空であること
```

全国データを使う重いテストは `#[ignore]` を付け、`cargo test --release -- --ignored` で別に実行する。

### 1.8 Pythonを使う場合

numpyなどは入っていない。`data/work/venv` に仮想環境を作り、そこに入れる。重い集計はRustの `src/bin/` で書く。PythonはHTTPでの取得と図の作成（matplotlib）に限る。

## 2. 指示役の進め方

### 2.1 役割

| 役割 | モデル | 仕事 |
|---|---|---|
| 指示役 | Opus | 作業書を読み、タスクを作業役に渡す。受け入れ条件を自分で確かめる。統合し、完了報告を書く。人の確認で止まる |
| 作業役 | Sonnet | 1つのタスクを実装し、テストを通して報告する |
| 難所の担当 | Opus | M0の設計、M3aの探索（楕円・LARAC・RCSP）の正しさの確認。作業役に任せず、指示役が自分で行うか、Opusのサブエージェントに任せる |

`~/.claude/rules/pstack-models.md` に設定がある場合は、その設定に従う。

### 2.2 作業役への指示の型

作業役は、この会話の文脈を持たない状態で始まる。指示には、毎回次の項目をすべて含める。

```text
## 目的
（このタスクで何を作るか。1〜3文）

## 先に読む文書
（計画書・behavior.md・routing-data-format.md・backend/README.md の該当節を、節番号まで指定する）

## 編集してよいパス
（ファイルかディレクトリを列挙する。これ以外は編集しない）

## 守るインターフェース
（M0で固定した型・関数のシグネチャ、データ形式の節。変更が必要なら実装せずに報告する）

## 受け入れ条件
（実行するコマンドと、期待する結果）

## 報告の形式
- 変更したファイルの一覧
- 受け入れ条件のコマンドの実際の出力（要約しない。長ければ末尾50行）
- 仕様どおりにできなかった点、推測で決めた点
- 未解決の問題
```

### 2.3 作業役の成果の確認

- 作業役の報告をそのまま信じない。指示役が、受け入れ条件のコマンドを自分で実行して確かめる。
- 差分を読み、「編集してよいパス」の外が変わっていないか、テストを弱めたり消したりしていないかを確かめる。既存のテスト（現行方式と③のテスト）は、1件も消さず、期待値も変えない。
- 不合格なら、失敗の出力を添えて、同じ作業役に直させる。2回直しても通らなければ、指示役が自分で原因を調べる。

### 2.4 判断に迷ったとき

- 0.2節の優先順位で決まるものは、それに従う。
- 決まらないものは、この作業書の「推奨」に従って進め、完了報告の「推測で決めた点」に書く。
- 次の場合は、作業を止めて利用者に聞く。
  - 計画書12.1節の決定事項に反する変更が必要になったとき。
  - 1.1節・1.2節の禁止事項に触れる必要が出たとき。
  - M0で固定したインターフェースや、契約（`backend/src/contract/`）を変える必要が出たとき。

### 2.5 完了報告

各マイルストーンの最後に、`docs/routing-reports/M<n>.md` を書いてコミットする。含める内容は次のとおり。

- 実施したタスクと、その結果
- 受け入れ条件ごとの合否と、根拠（コマンドの出力、数値）
- 推測で決めた点、計画書から外れた点とその理由
- 人の確認の手順（何を、どのファイルで、どう見ればよいか）
- 次のマイルストーンへの申し送り

報告を書いたら、人の確認があるマイルストーンでは、利用者に確認を依頼して止まる。

## 3. マイルストーンの全体像

```text
M0 仕様とインターフェースの固定 ──人の確認──┐
                                            ├─> M1 グラフ・最短時間経路 ──人の確認──┐
                                            ├─> M2a 事故の属性の前処理             ├─> M2b 事故の割り当て ──人の確認──┐
                                            └─> M3a 探索（合成グラフ）              │                                  │
                                                                                    └──────────────> M3b リスクと探索の結合 <┘
                                                                                                        │
                                                                                          M4 エンジンの組み込み ──人の確認──┐
                                                                                                                            │
                                                                                          M5 評価と係数の決定 ──人の確認──> 配備（利用者）
```

- M0の完了後、M1・M2a・M3aは並行して進めてよい。
- M2bは、M1のグラフとM2aの属性が必要。
- M3bは、M2bとM3aの両方が必要。
- 利用者は、M1・M2a・M3aをまとめて1回で依頼してよい。その場合、指示役は3つを並行で進め、M1の人の確認で止まる。

## 4. マイルストーン

### M0 仕様とインターフェースの固定

**担当**：指示役（Opus）が自分で行う。文書の下書きを作業役に任せてもよいが、設計の判断は指示役が行う。

**目的**：以後の作業役が推測せずに実装できるように、仕様書・契約・データ形式・受け入れテストを先に決める。

#### M0.1 エンジンの差し替え方

現行の `RouteEngine` は、OSRMから候補を取る `collect_candidates`（非同期）と、集計する `evaluate`（CPU処理、③が `spawn_blocking` で呼ぶ）の2段階になっている。③のrouteハンドラ（`backend/src/api/route.rs`）は、この順で呼ぶ。

推奨する差し替え方（契約のシグネチャと③のコードを変えない）：

- 新しいエンジン（以下 `RoutingEngine`）も `RouteEngine` を実装する。
  - `collect_candidates`：ルーターを呼ばず、空の `CandidateSet` をすぐに返す。
  - `evaluate`：受け取った `CandidateSet` を使わず、探索から集計までを行う。`spawn_blocking` の中で動くので、CPU処理を置いてよい。
- `data::load_engine` は、releaseの中身でエンジンを選ぶ。
  - `metadata.json` の `schema_version` が3（グラフとリスクのファイルを含む）なら、`RoutingEngine` を返す。
  - 2（現行の3ファイル）なら、今までどおり `AccidentEngine` を返す。
  - これで、現行方式のテスト（③のテストを含む）は、人工のrelease（schema 2）のまま通り続ける。M5の比較でも、現行方式をそのまま動かせる。
- `backend/src/contract/engine.rs` は、ドキュメントコメントだけを直す（`CandidateSet` が空になり得ること、`collect_candidates` がルーターを呼ばないエンジンがあること）。シグネチャは変えない。
- `EngineError` の意味（`RoutingEngine` の場合）：
  - `NoRoute`：出発地か目的地を吸着できない、または経路がない。
  - `RouteTooLong`：最短時間経路の距離が80km超（現行の規則を引き継ぐ）。最短時間経路以外の候補で80kmを超えるものは、候補から外す。
  - `ExternalApi`：返さない。
- ③は、応答を返す前に `api::route_response` で結果を検査し、違反があれば500にする。`RoutingEngine` の結果は、次の範囲に必ず収める。
  - 経路の頂点数が2〜15,000（`ROUTE_VERTICES_RANGE`）。OSMの形状点をそのまま出して超える場合は、表示用の形状だけを間引く（集計は間引く前の形状で行う）。
  - 距離が0より大きく80km以下（`ROUTE_MAX_DISTANCE_M`）。
  - `check_detour` と `check_outcome` のその他の検査（丸め、件数、ナビ用データの大きさなど）。
- 起動時の設定（OSRMのURLなど）は③の範囲なので変えない。`RoutingEngine` の検索では、OSRMを呼ばない。

ドキュメントコメントだけでも `contract` は②と③の共同所有なので、③の担当者の確認が必要になる（人の確認で扱う）。

#### M0.2 `docs/api/behavior.md` の書き換え

現行方式の章は消さずに「付録：旧方式（比較用、schema 2のrelease）」へ移し、新方式の章を書く。

| 章 | 書く内容 |
|---|---|
| 1 | 適用範囲。経路を自前で探索すること、根拠が計画書であること、旧方式を付録に残す理由 |
| 2 | CSV前処理。`accidents.csv.gz` の17列と、その中身は変えない（`dataset_id` が旧方式と同じになる）。割り当てに使う属性（路線コード、道路形状、車道幅員、歩車道区分、地形）は、`accident_attrs.bin` に事故IDで対応付けて持つ |
| 3 | 起動時ロード。schema 3のreleaseに入るファイル、`release_id` の計算（グラフ・リスクなどのファイルのSHA-256も含める）、起動4分以内、RSSの上限 |
| 5 | 「外部経路取得と候補選定前処理」を「経路探索」に置き換える。計画書6章の手順、候補の上限6本、重複除去、所要時間の昇順、IDの振り方 |
| 6 | 「72m回廊」を「事故の集合B」に置き換える。計画書11.1節の定義（割り当て重み0.1以上の事故＋72m以内の帰属不明の事故） |
| 7 | 件数・人数は集合Bの観測値。`weighted_risk` はモデルの値（自動車は生活道路の係数を含む）。事故点の抽出規則は現行どおり。高リスク区間はM0.3の規則。ナビ用の区間の構造は現行どおり（`domain::navigation` を再利用する） |
| 8 | 事故考慮経路の決め方（計画書6章）。`safest` が `None` になるのは全候補の `accident_count` が0のときだけ。`reason` の文面は現行の型を使う。探索の打ち切りでは警告を出さない |
| 11 | 受け入れテストの対応表に、M0.6の内容を足す。旧方式のテストは付録の対象として残す |
| 12 | 要確認事項。計画書12.2節の、実測と実験で決める項目 |

`docs/route-scoring-decision.md` の冒頭に、新方式に置き換わったことと、計画書へのリンクを書き足す。本文は残す。

#### M0.3 高リスク区間を、75mの小線分に対応付ける規則

契約の高リスク区間は、経路を75mで分けた小線分（sid）単位である（`HighRiskSegment`、`domain::corridor` の75m分割）。計画書11.2節の案Aを、sid単位に移す規則を決める。

契約には、表示用の区間とナビ用の区間の `severity_weight` を3以上にする制約がある（`openapi.yaml` の `minimum: 3`、③の `api::route_response`、`testing::fixtures`、フロントの `navigation/preflight.ts`）。計画書の「100mの窓」をそのまま使うと、窓に重なるsidの中に重みが3未満のものが出て、この制約を破る。そこで、件数を数える単位を75mのsidにする。

推奨する規則：

1. 集合Bの各事故を、経路上の1点に置く。
   - 経路の辺に割り当てた事故：割り当てた辺上の位置。同じ経路の複数の辺に割り当てがあれば、重みが最大のもの。
   - 交差点クラスタに割り当てた事故：クラスタの中心を経路に投影した点（同じクラスタの事故が、同じsidに入るようにする）。
   - 帰属不明の事故：経路上の最寄りの点。
2. 各sidについて、そこに置いた事故の件数と、`10×死者+負傷者` の合計（`severity_weight`）を数える。
3. 件数が3件以上のsidを、高リスクのsidとする。事故1件の深刻度は1以上なので、`severity_weight` は必ず3以上になる。
4. `high_risk_total` は、高リスクのsidの数とする。表示用の100件の並べ方（重みの降順、同値はsidの昇順）と、ナビ用の区間の結合は現行どおり（`domain::navigation` を使う）。
5. しきい値の3件は定数にし、M5で3件か2件かを決める。2件にする場合は、契約の制約を満たすため「2件以上かつ `severity_weight` が3以上」とする（負傷者1人の事故2件だけのsidは出ない）。純粋に件数2件で出すには契約とフロントの変更が要るので、Issue #26 の7に記録してある。

`openapi.yaml` の `high_risk_total` などの `description` も、この規則に合わせて直す。

#### M0.4 `docs/api/openapi.yaml` の文言の更新

- 1.1節の範囲（`description` と応答例の文言）だけを変える。
- `risk_note` の応答例は変えない（応答例は、旧方式の人工releaseで③のテストが一致を検査しているため）。新方式の `risk_note` の文面は `behavior.md` に書く。
- 変更後に、`python3 -B backend/scripts/extract_route_examples.py` と `(cd frontend && npm run generate:api)` を実行し、1.7節の検証コマンドを通す。

#### M0.5 データ形式とモジュール構成

`docs/routing-data-format.md` を作り、次の内容を決める。

- schema 3のreleaseのファイル一覧と、それぞれの形式。先頭に、マジックナンバー・形式の版・件数・SHA-256を持つヘッダーを置く。リトルエンディアン。
  - 現行の3ファイル（`accidents.csv.gz`・`syukujitsu.csv`・`metadata.json`）はそのまま含める。`metadata.json` に、追加ファイルの一覧とSHA-256を足す。
  - `graph.bin`：計画書3.5節の表のとおり（隣接リストの開始位置、隣接リスト、ノード座標、ノード属性、辺属性、形状点）。
  - `restrictions.bin`：via-node型とvia-way型の右左折禁止。
  - `topology.bin`：上下線の双子、交差点クラスタ、歩道と親の車道の対応。
  - `odindex.bin`：吸着用の格子。
  - `accident_attrs.bin`：事故IDごとの属性。
  - `assignment.bin`：事故ごとの割り当て（辺か交差点クラスタ、辺上の位置、重み）と、帰属不明の事故の一覧と理由。
  - `risk.bin`：種別ごとの係数表（R、W、S、ρ、a_k、α_j、β、c）と、事故のある辺・交差点の疎な表。
  - `alt.bin`：ランドマークと距離表。
- ID：ノード番号・辺番号はu32。辺番号は、グラフのファイルの中での順番。
- 手段ごとの通行規則の表（計画書3.2節）と、その版番号。
- 道路種別 k の区分（highway × 車線・幅員帯 × 人口集中地区か）の定義。

あわせて、型と関数のシグネチャだけを持つモジュールの骨組みを作る。中身は `todo!()` でよいが、1.7節のコマンドが通る状態にする。作業役は、これを埋める形で並行して実装する。既存のモジュールは動かさない。

```text
backend/src/domain/routing/      新方式（既存の candidates・corridor・scoring・engine は旧方式として残す）
  graph/      グラフの型、読み書き、手段別の通行判定、所要時間
  topology/   双子、交差点クラスタ、歩道の対応
  snap/       出発地・目的地の吸着
  accident/   事故の属性、割り当て
  risk/       リスクの推定（前処理）と、リクエストごとのリスクの計算
  search/     最短時間経路、楕円、LARAC、RCSP、ALT、候補の整理
  legality/   経路の合法性の検査器（M1。評価でも使う）
  engine.rs   RoutingEngine（M4）。既存の domain::navigation と domain::rounding を使う
backend/src/data/
  release.rs・load.rs に、schema 3の書き出しと読込を足す
backend/src/bin/
  routing_prep.rs   新方式の前処理（P1〜P9）のサブコマンド。既存の prepare_release は変えない
  routing_eval.rs   評価と計測（M1・M3b・M5）
```

#### M0.6 受け入れテストの定義

各マイルストーンの受け入れテストを、`behavior.md` の11章に書く。少なくとも次を含める。

| テスト | 内容 |
|---|---|
| 分割の不変性 | 辺を2つに分けても、経路のリスクの合計が変わらない（計画書5.1節） |
| 割り当ての重み | 1件の事故の重みの合計が1以下。高速・自専道のコードの事故は、motorwayかmotorroadにしか載らない |
| RCSPの正しさ | ノード12個以下のランダムなグラフ600通りで、時間予算なしのRCSPの結果が、全単純経路の列挙と一致する |
| LARACの性質 | 返す解が上限を満たす。下界が最適値以下 |
| 楕円の安全性 | ノード数千のグラフで、楕円で切った部分グラフ上の最適解と、全体グラフ上の最適解が一致する |
| 上限の保証 | 返す候補はすべて、所要時間がB以下 |
| 右左折禁止 | 禁止された右左折を含む経路を返さない（via-way型を含む） |
| 中断 | 探索中に `CancelToken` を取り消すと、約10ms以内に `Interrupted` で返る |
| 人工release | 小さな合成グラフで作ったschema 3の人工releaseを `load_engine` で読み、`evaluate` の結果で `check_outcome_invariants` が通る（`testing::TestRelease` に、schema 3の版を足す） |
| 旧方式の維持 | 既存のテストがすべて、期待値を変えずに通る |

#### M0の受け入れ条件

- 1.7節の検証コマンドがすべて通る。
- `docs/routing-data-format.md` があり、M0.5の項目がすべて決まっている。
- 骨組みがビルドでき、`tests/boundaries.rs` が通る。

#### M0の人の確認

- 利用者：`behavior.md` の差分（特に5〜8章）と、M0.3の規則を読む。
- ③の担当者：`backend/src/contract/engine.rs` のドキュメントコメントの変更と、M0.1の差し替え方（`load_engine` がschemaでエンジンを選ぶこと）を確認する。

---

### M1 グラフと最短時間経路

**目的**：全国の共通グラフを作り、合法な最短時間経路を返せるようにする。

#### タスク

| タスク | 内容 | 計画書 |
|---|---|---|
| M1.1 | P1 `osm-graph`：`osmpbf` でPBFを複数パス読み、手段フラグ、交差点の判定、辺の分割、CSR、形状点を作って `graph.bin` に書く | 3.1〜3.3 |
| M1.2 | P2 `restrictions`：右左折禁止のrelation（via-node型・via-way型、`except`、手段別）を `restrictions.bin` に書く | 3.4 |
| M1.3 | P3 `topology`：小さな連結成分、上下線の双子、交差点クラスタ、歩道と親の車道の対応を `topology.bin` に書く | 3.3、4.1、4.4 |
| M1.4 | P7 `od-index` と吸着：辺の途中に仮想ノードを置く吸着 | 3.3 |
| M1.5 | 所要時間のモデル（手段別、信号ペナルティ、角度による右左折コスト）と、双方向A*の最短時間経路。部分グラフ上で「直前の辺」を状態に持つ探索（M3でも使う） | 3.4、3.5 |
| M1.6 | 合法性の検査器（`domain::routing::legality`）：経路の各辺が手段で通れるか、方向、右左折禁止、通行できない道路を検査する | 3.4 |
| M1.7 | 関東のOSRMを立て、所要時間を照合する。自動車の既定速度を回帰で合わせる | 3.5 |
| M1.8 | 全国のODでの動作確認：手段ごとに300組 | 9.4 |

M1.1〜M1.4は順番に依存する。M1.6はM1.1の後に並行で進めてよい。

#### OSRMの立て方（M1.7、M5でも使う）

```sh
mkdir -p data/work/osrm && cd data/work/osrm
curl -LO https://download.geofabrik.de/asia/japan/kanto-latest.osm.pbf
for p in car bicycle foot; do
  mkdir -p $p && cp kanto-latest.osm.pbf $p/
  docker run --rm -v "$PWD/$p:/data" ghcr.io/project-osrm/osrm-backend osrm-extract -p /opt/$p.lua /data/kanto-latest.osm.pbf
  docker run --rm -v "$PWD/$p:/data" ghcr.io/project-osrm/osrm-backend osrm-partition /data/kanto-latest.osrm
  docker run --rm -v "$PWD/$p:/data" ghcr.io/project-osrm/osrm-backend osrm-customize /data/kanto-latest.osrm
done
# 起動（手段ごとにポートを分ける）
docker run -d --name osrm-car  -p 5001:5000 -v "$PWD/car:/data"     ghcr.io/project-osrm/osrm-backend osrm-routed --algorithm mld /data/kanto-latest.osrm
docker run -d --name osrm-bike -p 5002:5000 -v "$PWD/bicycle:/data" ghcr.io/project-osrm/osrm-backend osrm-routed --algorithm mld /data/kanto-latest.osrm
docker run -d --name osrm-foot -p 5003:5000 -v "$PWD/foot:/data"    ghcr.io/project-osrm/osrm-backend osrm-routed --algorithm mld /data/kanto-latest.osrm
```

Geofabrikの関東抽出は、全国PBF（2026-11-01版）と日付がずれることがある。照合の報告にその旨を書く。

#### 受け入れ条件

| 項目 | 基準 |
|---|---|
| グラフの規模 | 共通グラフのノード数・辺数が、`plan2.md` の実測（1,476万ノード、2,003万辺）から±5%以内。外れたら、規則の違いで説明できること |
| 常駐メモリ | グラフ一式を読み込んだときのRSSが1.5GB以下 |
| 合法性 | M1.8の全国のODで、検査器の違反が0件 |
| 経路が見つかる割合 | M1.8で、吸着できたODのうち98%以上で経路が見つかる（目安）。見つからない例は、理由（離島、連結成分など）を分類して報告する |
| OSRMとの照合 | 関東の自動車300組で、所要時間の比（自前÷OSRM）の中央値が0.85〜1.15（目安）。徒歩・自転車は速度の仮定の差を報告するだけ |
| 応答時間 | M1.8の最短時間経路の95%点を記録する（基準は設けない） |
| テスト | 1.7節のコマンドと、M0.6のうちM1に関係するもの（右左折禁止）が通る |

#### 人の確認

指示役は、`data/work/M1/review/index.html` を作る。

- 手段ごとに10本、合計30本の経路を、OSMの背景地図の上に表示する。都市部・郊外・地方を混ぜる。
- 経路ごとに、所要時間・距離・OSRMの経路（関東のみ）を並べて表示する。
- 利用者は、ブラウザでこのファイルを開き、逆走、通れない道、不自然な遠回りがないかを見る。

---

### M2a 事故の属性の前処理

**目的**：事故原本から、割り当てに使う属性を、既存の事故IDに対応付けて取り出す。

既存の前処理（`prepare_release`、`data::preprocess`）が、17列の `accidents.csv.gz`、祝日表による曜日区分、5区分の時間帯をすでに作っている。M2aは、これに属性の取り出しを足すだけにする。

#### タスク

| タスク | 内容 |
|---|---|
| M2a.1 | 既存の行処理と同じ保持判定で、同じ行から属性（路線コード、道路形状、車道幅員、歩車道区分、地形）を読む。既存の行処理と同じく、列は列名（全空白を除いたもの）で引く。既存の必須列（`Column::ALL`）は変えない（変えると、旧方式の前処理の失敗条件が変わるため）。コードの意味は、年ごとのコード表（`accident/codebook_*`）で確かめる。手元に `app/codebook_notes.md` があれば参考にしてよい |
| M2a.2 | `routing_prep` のサブコマンド `accident-attrs`：保持した行と同じ順番（＝事故ID）で `accident_attrs.bin` を書く。`accidents.csv.gz` は、既存の `prepare_release` と同じバイト列になること |
| M2a.3 | 監査：属性の分布（路線コードの区分、道路形状、車道幅員）と、欠損・未知コードの件数を記録する。2024年の分布を、`plan2.md` の実測（計画書4.2節の表）と照合する |

#### 受け入れ条件

- 既存の前処理のテスト（`preprocess_rows`・`preprocess_files`・`preprocess_release`）が、期待値を変えずに通る。
- 同じ原本から作った `accidents.csv.gz` のSHA-256（`dataset_id`）が、既存の `prepare_release` の出力と一致する。
- 2024年の路線コードの区分と道路形状の分布が、計画書4.2節の表から±1ポイント以内。
- 年によって列の位置やコードが違う箇所を、どう扱ったかを報告に書く。

人の確認はない（M2bでまとめて行う）。

---

### M2b 事故の割り当て

**前提**：M1とM2aが完了していること。

**目的**：事故を、道路の辺と交差点クラスタに割り当てる。

#### タスク

| タスク | 内容 | 計画書 |
|---|---|---|
| M2b.1 | 候補の抽出：35m以内の車道の辺、30m以内の交差点クラスタ。高速・自専道のコードの一致を必須条件にする | 4.3 |
| M2b.2 | 尤度表の推定：候補が1本しかない事故から、路線区分×OSM種別、幅員の混同表を作る | 4.3 |
| M2b.3 | P5 `accident-snap`：スコアを計算して正規化し、0.1以上の候補（最大3本）に配分する。双子の辺にも同じ値を写す。帰属不明の事故は理由別に記録する。`assignment.bin` に書く | 4.3、4.4 |
| M2b.4 | 集計：道路種別・道路形状ごとの、割り当てた割合、帰属不明の割合と理由、最大の重みの分布 | 4.6 |
| M2b.5 | 人手判定の準備：道路種別・道路形状で層別に200件を抽出し、確認用のページを作る | 4.6 |

σ0の初期値は3.5mとする。M5で調整する。

#### 受け入れ条件

- M0.6の「割り当ての重み」のテストが、全件で通る。
- 帰属不明の割合を、道路種別・道路形状ごとに報告する（基準は設けない）。
- 国道の事故のうち、番号が一致する `ref` を持つ辺に最大の重みが載った割合を報告する。

#### 人の確認

指示役は、`data/work/M2/review/index.html` を作る。

- 200件の事故を1件ずつ表示する。表示するのは、事故の位置、候補の辺と交差点（重みを色と数値で示す）、事故の属性（路線コード、道路形状、車道幅員）。背景はOSMの地図。
- 各事故に「正しい／誤り／判断できない」のボタンを付ける。結果はブラウザ内に保存し、CSVとしてダウンロードできるようにする（外部へは送らない）。
- 利用者は、判定したCSVを `data/work/M2/review/judgments.csv` に置く。指示役は、正解率を、最大の重みが0.8以上の群と、それ以外の群に分けて集計し、報告に追記する。
- 最大の重みが0.8以上の群の正解率が85%を下回る場合（目安）、指示役は誤りの傾向を分析し、利用者に作り直すかどうかを聞く。

---

### M3a 探索（合成グラフ）

**前提**：M0が完了していること。M1・M2と並行して進めてよい。

**目的**：探索の部品を、合成グラフの上で正しく作る。

#### タスク

| タスク | 内容 | 計画書 |
|---|---|---|
| M3a.1 | 楕円の切り出し：始点からの前向き、終点からの後ろ向きの、上限付き探索。局所番号の部分グラフ（CSR）を作る。作業領域は部分グラフの大きさで確保する | 6.2、7.2 |
| M3a.2 | LARAC：λの更新、下界、凸包上の途中解の記録 | 6.2 |
| M3a.3 | 時間予算付きRCSP：多ラベル、優越判定、暫定解と下界による枝刈り、メモリ上限、約1024回の拡張ごとの `cancel.check()` | 6.2 |
| M3a.4 | ALT：ランドマークの選び方（16個、最遠点法）、時間の下界 | 7.1 |
| M3a.5 | 候補の整理：凸包上の解、ペナルティ法、共有する物理区間の長さによる重複除去、上限6本、所要時間の昇順 | 6.3 |
| M3a.6 | 探索範囲が大きすぎる場合の処理 | 6.4 |
| M3a.7 | M0.6の探索に関するテスト（RCSPの正しさ、LARACの性質、楕円の安全性、上限の保証、中断） | — |

時間とリスクは、固定小数点の整数で持つ（時間は秒×10、リスクは計算に十分な桁数の整数）。

#### 受け入れ条件

- M0.6の探索に関するテストがすべて通る。
- **難所の確認**：指示役（Opus）が、M3a.1〜M3a.3のコードを自分で読み、次の点を確かめて、報告に書く。
  - 楕円の枝刈りが、実行可能な経路を落とさないこと。
  - RCSPの優越判定で、進入した辺や右左折禁止の状態が異なるラベルどうしを比べていないこと。
  - 中断と時間予算で打ち切ったときに、その時点の最良解を返し、最適と記録しないこと。

人の確認はない。

---

### M3b リスクの推定と探索の結合

**前提**：M2bとM3aが完了していること。

#### タスク

| タスク | 内容 | 計画書 |
|---|---|---|
| M3b.1 | P6 `risk-fit`：道路種別ごとの R・S・ρ_k の推定。a_k・α_j・βは、2019〜2022年で学習し、2023年の辺・交差点ごとの件数に対するポアソン逸脱度が最小になるようにグリッド探索で決める。生活道路の係数 c は1.0で仮置きする | 5.1〜5.5 |
| M3b.2 | リクエストごとのリスクの計算（`RiskView`）：条件のセル、天候、交差点クラスタに入る辺へのリスクの加算。探索中に触れた辺だけ計算してキャッシュする | 5.1、5.4、4.4 |
| M3b.3 | P8 `alt`：全国グラフでのランドマークと距離表 | 7.1 |
| M3b.4 | 全国グラフでの結合：吸着 → 最短時間経路 → 楕円 → LARAC → RCSP → 候補の整理 | 6.2 |
| M3b.5 | 計測：関東の4地域（計画書9.3節）と全国のODで、手段ごとに応答時間の中央値と95%点、楕円のノード数、RCSPを完了した割合、6.4節の処理に入った割合を記録する | 7.1 |

#### 受け入れ条件

- M0.6の「分割の不変性」のテストが通る。
- 全国のOD（手段ごとに300組）で、合法性の違反が0件、すべての候補が上限を満たす。
- 1件の検索の処理時間（サーバの全体期限35秒の内側）の99%点が、10秒以下（目安）。超える場合は、RCSPの時間予算とノード数の上限を下げて、再計測する。
- M3b.5の計測結果を報告に書く。時間予算とノード数の上限の初期値は、この結果から指示役が決める（計画書12.2節の3。M5で見直す）。

人の確認はない。

---

### M4 エンジンの組み込み

**目的**：`RoutingEngine` と、schema 3のreleaseの書き出し・読込を実装し、実際のサーバとフロントで動かす。

#### タスク

| タスク | 内容 |
|---|---|
| M4.1 | P9 `package`：`routing_prep` で、schema 3のreleaseを `backend/data/releases/<release-id>/` に作る。既存の `prepare_release` と同じく、`staging/` で作ってから移し、読み取り専用にし、`current` は変えない。`release_id` の計算は、M0で更新した `behavior.md` 3章に従う |
| M4.2 | `data::load_engine`：schemaでエンジンを選び、schema 3ではハッシュ・形式の版・件数を検証して `RoutingEngine` を作る。起動4分以内、RSSは3.2GB以下（4GiBから余裕を残す） |
| M4.3 | `RoutingEngine::evaluate`：探索の結果から、`EvaluatedRoute` と `RouteOutcome` の全項目を `behavior.md` 6〜8章のとおりに作る（集合B、件数、天候一致、事故点の抽出、`weighted_risk`、`risk_per_km`、高リスクのsid、ナビ用データ、追加時間、`reason`、`risk_note`、`safest` の規則、`RouteTooLong`）。ナビ用データと丸めは、既存の `domain::navigation`・`domain::rounding` を使う |
| M4.4 | 人工release：`testing::TestRelease` に、小さな合成グラフで作るschema 3の版を足す。M0.6の「人工release」のテストを通す |
| M4.5 | 実データでの契約の検査：`backend/tests/` に、`#[ignore]` 付きの結合テストを作る（既存の `real_release.rs` と同じく、releaseのディレクトリを環境変数で渡す）。関東の4地域から、手段・時間帯・曜日区分・天候・遠回り上限を変えた300件の問い合わせで、`check_outcome_invariants` と、③の `api::route_response::check_detour`・`render_route_response` が全件通ることを確かめる |
| M4.6 | 実際のサーバとフロントでの確認：`backend/README.md` の「C4のローカル起動」の手順で、偽上流（`dev_stack`）の環境変数を使い、`ACCIDENT_DATA_DIR` にM4.1のreleaseを指定して起動する。新方式の検索ではOSRMを呼ばないので、偽OSRMは使われない。`curl` で `/api/health` と `/api/route` を確かめ、フロント（`VITE_API_MODE=live`）からも検索する |
| M4.7 | イメージの確認：`docker build --build-arg ACCIDENT_RELEASE=backend/data/releases/<release-id> .` が通り、コンテナが起動して `/api/health` が `ok` を返すことを確かめる。配備はしない。イメージの大きさを報告に書く |

#### 受け入れ条件

- 1.7節の検証コマンドがすべて通る。
- M4.5の300件で、`check_outcome_invariants`・`check_detour`・`render_route_response` が全件通る。
- 起動時間とRSSが、M4.2の基準を満たす（M4.6とM4.7の両方で測る）。
- M4.6で、`/api/route` が200を返し、フロントの応答検証を通る。

#### 人の確認

利用者が、M4.6の状態の画面で、手段・条件を変えて数回検索する。2経路の比較、事故点、高リスク区間、ナビの開始が表示されることを確かめる。偽Geoapifyは住所検索にfixtureの地点しか返さないので、地点は地図で指定する。

---

### M5 評価と係数の決定

**目的**：実験で決める項目（計画書12.2節）を決め、現行方式との比較を報告し、本番用のreleaseを作る。

#### タスク

| タスク | 内容 | 計画書 |
|---|---|---|
| M5.1 | 評価地域の境界：OSMの行政界relation（東京23区、千葉市、さいたま市、横浜市）を全国PBFから取り出す | 9.3 |
| M5.2 | ODの抽出：地域内の道路ノード（motorwayを除く）から一様に抽出し、手段別の距離帯でふるい分ける。1地域・1手段あたり約100組。人口のデータは手元にないので、計画書の「人口密度で重み付け」の代わりに一様抽出とし、報告に書く | 9.3 |
| M5.3 | 現行方式の経路：既存の `prepare_release` で作ったschema 2のreleaseと、M1の関東のOSRM（`OSRM_*_BASE_URL` を `http://127.0.0.1:5001` などにする。ほかの上流は `dev_stack` の偽上流の環境変数を使う）でサーバを起動し、Pythonのスクリプトから `/api/route` を呼んで、各ODの全候補と選定結果を保存する。現行方式を作り直さない | 9.3 |
| M5.4 | 物差しの実装（`routing_eval`）：Y1（経路から15m以内の将来事故）、Y2、Y3（M2bの割り当てで通過した辺・交差点に載る将来事故）。過去の実験のスクリプトは残っていないので、Y2は「距離だけで最寄りの辺に割り当てた将来事故」で代用する | 9.3 |
| M5.5 | 係数の調整（2019〜2022年で学習、2023年で検証）：σ0、生活道路の係数 c（計画書5.5節の折れ目の規則）、高リスク区間のしきい値（典型的な経路で何区間出るかを数え、3件か2件か決める） | 5.5、11.2 |
| M5.6 | 探索の時間予算の決定：全国のOD（手段ごとに300組）で、RCSPの時間予算、LARACの反復回数、楕円のノード数上限を変えて測り、処理時間の99%点が10秒以下（目安）に収まる範囲で、RCSPを完了する割合が最大になる組み合わせを選ぶ | 7.1 |
| M5.7 | 比較の報告（2019〜2023年で学習、2024〜2025年で評価）：計画書9.3節の指標、比較対象、要素を外した比較、較正。信頼区間は、OD単位と地域単位のクラスターブートストラップで求める。東京23区は、過去の実験による偏りを注記する。現行方式の経路はM5.3のもの（2019〜2025年の事故で選んだもの）なので、評価期間の事故を選定に使っている点も注記する。可能なら、2019〜2023年の事故だけで作ったschema 2のreleaseで取り直す | 9.2、9.3 |
| M5.8 | 全国での最終確認：合法性、応答時間、メモリ（M1.8と同じ手順）。あわせて、`route_too_long` を返した割合、80km超で外した候補の数、表示用の形状を間引いた割合を記録する | 9.4 |
| M5.9 | 本番用のrelease：決めた係数で、2019〜2025年の全期間で学習し直し、M4.1の手順でreleaseを作る。M4の受け入れ条件を確かめる | 5.1 |
| M5.10 | 文書の更新：計画書12.2節に決めた値を書き込む。図（削減率と追加時間の曲線、c の折れ目、較正）を `docs/routing-reports/M5/` に置く。Issue #26 に追記する文案（しきい値の決定、M5.8の上限に関する数値、安全ルートの件数が最短ルートより多かった割合）を報告に書く。Issueへの書き込みは利用者が行う | — |

採用基準は設けない（計画書9.1節）。結果が現行方式より悪くても、そのまま報告する。

#### 受け入れ条件

- 計画書12.2節の3項目が、すべて値として決まっている。
- M5.7の比較の報告が、指標・比較対象・要素を外した比較・較正のすべてについて揃っている。
- M5.8で、合法性の違反が0件。
- M5.9のreleaseで、M4の受け入れ条件が満たされる。

#### 人の確認

利用者が `docs/routing-reports/M5.md` を読む。報告の冒頭には、次を1ページにまとめて書く。

- 新方式と現行方式の、削減率・追加時間・勝率（手段ごと）
- 決めた係数（c、しきい値、時間予算）と、その根拠の図
- 評価で分かった弱点（生活道路や裏道への誘導の有無、地域による差）

この確認が終わったら、利用者が配備を行う（Cloud Runのメモリを4GiBにし、M5.9のreleaseを `ACCIDENT_RELEASE` に指定したイメージを `deploy/README.md` の手順で配備する）。

## 5. 全体の完了の定義

- M0〜M5の受け入れ条件が、すべて満たされている。
- `docs/routing-reports/M0.md` 〜 `M5.md` が揃っている。
- 本番用のschema 3のreleaseが `backend/data/releases/` にあり、`frontend/` に生成物以外の差分がない。
- 既存のテスト（旧方式と③のテスト）が、すべて期待値を変えずに通る。
- 残った課題が、M5の報告にまとまっている。
