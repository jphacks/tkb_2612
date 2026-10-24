# backend：内部インターフェースの引き継ぎ

事故データ・計算担当（②）とAPI・外部通信・基盤担当（③）が、相手の完成を待たずに並行して実装するための内部契約です（Issue #4）。外部のHTTP/JSONは [openapi.yaml](../docs/api/openapi.yaml)、計算は [behavior.md](../docs/api/behavior.md)、構成は [architecture.md](../docs/architecture.md) に従います。ここでは公開APIと計算仕様を変更していません。

現時点で入っているのは契約の型、スタブ、fixtureの読込、検証用のテストと、②の本実装（前処理、release、祝日表の読込と判定、`load_calendar`・`load_engine`、計算エンジン。下記「事故データ・計算（②）」）、③の本実装（Axumハンドラ、外部提供元のアダプタ、上流ゲート、設定と起動、CI、配備定義。下記「API・外部通信・基盤（③）」。Issue #9）です。

## 型の所在

| 内容 | 場所 | 実装 / 利用 |
|---|---|---|
| 座標 `LatLon`・`LonLat`、直線距離 | `src/contract/geo.rs` | 共有 |
| `Mode`・`Period`・`Weather`・`DayType`、コードと日本語ラベル、`Period::from_hour`、`ConditionFilter` | `src/contract/conditions.rs` | 共有 |
| 時間帯・曜日判定の入口 `Calendar`、`TokyoMinute`、`DayClassification` | `src/contract/calendar.rs` | 実装は②、利用は②（前処理）と③（conditions） |
| 期限・キャンセル `CancelToken`・`Interrupt`・`CancelGuard` | `src/contract/cancel.rs` | ③が作り、②とアダプタが確認する |
| 経路取得 `Router`・`RoutingRequest`・`RoutedPath`・`ProviderRoute`・`MalformedRoute`・`RoutingError` | `src/contract/routing.rs` | 実装は③、呼び出すのは② |
| 計算の入口 `RouteEngine`、入力 `RouteQuery`、結果 `RouteOutcome`、`EngineError`、`DatasetInfo` | `src/contract/engine.rs` | 実装は②、呼び出すのは③ |
| releaseの読込 `data::load_calendar`・`data::load_engine` | `src/data/mod.rs` | 実装は②（現在は常に失敗する仮実装）、呼び出すのは③ |
| スタブとfixtureの読込 | `src/testing/` | 共有（テスト専用） |

## 担当・依存方向・編集責任

```text
api ──┬──> contract <──┬── domain
      │       ^        └── data
      └──> data::load_calendar / load_engine
providers ──> contract
```

| パス | 担当 | 規則 |
|---|---|---|
| `src/contract/` | ②③の共同所有 | 変更するPRは両担当が承認する。外部契約に関わる変更は、先にOpenAPIとbehavior.mdを更新する |
| `src/domain/`、`src/data/`、前処理の `src/bin/` | ② | `api`・`providers`・`testing`・tokio/axum/reqwest/hyperに依存しない |
| `src/api/`、`src/providers/`、`src/main.rs`・設定 | ③ | `api` は `domain` に依存せず、`data` は上の2関数と `LoadError` だけを使う。`providers` は `contract` だけに依存する |
| `src/testing/` | 共同 | 本番の処理から使わない。スタブの追加は各担当が自由に行ってよい |
| `tests/fixtures/route-responses.json` | 生成物 | 手で編集せず、スクリプトで再生成する（下記） |

依存方向は `tests/boundaries.rs` で簡易的に検査します。この検査は `name::` の形のパスだけを見ています。他モジュールは `use crate::{api, ..}` のようにまとめて書かず、`crate::name::item` の形で参照してください。

## 座標順と単位

- 座標は名前付きフィールドで持ちます。入力と住所候補は `LatLon`、経路geometryと事故点は `LonLat` です。GeoJSONの `[lon, lat]` 配列への変換は、③のDTOとアダプタだけが行います。
- 単位はフィールド名の接尾辞で示します。`duration_s` は秒、`distance_m`・`length_m`・`start_m` はm、`max_detour_min`・`extra_minutes` は分、`aggregation_ms` はミリ秒です。
- `RouteOutcome` の候補は添字で参照します（`fastest`・`safest`）。添字iのIDは `route-(i+1)` です。`route-N`、`route-N:segment-S`、`route-N:nav-S-E` の文字列、`api_version`、`exact_condition_count`、`points_shown`、`effective_conditions` の表記（全時間帯・全曜日・全天候）は、③がDTOへの変換時に組み立てます。
- 数値の丸め、件数、順序、選定、`reason`・`warnings`・`risk_note` の文言は②が決めます。③はこれらを再計算しません。

## 責任の分担

| 処理 | ② | ③ |
|---|---|---|
| route入力 | — | JSON・Content-Type・型・列挙・互換入力（整数コード、null、`時間帯`、`hour`）の解釈。`RouteQuery::new` で座標・直線距離・許容時間を検証する（`validate_endpoints` を先に呼ぶと、behavior.md 4章の検証順を守れる） |
| 経路取得 | 追加候補の要否、左右の経由点、呼出し順（初回→左→右）、値と頂点数の検証、80km判定、重複排除、並べ替え、警告 | URLの組立て、識別UA、上流ゲート、8秒の通信期限、サイズ上限、JSON構造の解釈、提供元のエラーの変換。先頭6本までを提供元の順で1本ずつ解釈して返す |
| 計算 | 事故集計、選定、ナビ区間、説明文、計算中の中断 | `spawn_blocking` での実行、CPU同時数の上限、navigationのサイズ（5,000,000バイト）とJSONの安全整数範囲の検査 |
| conditions | `Calendar` の実装（祝日表の読込、判定） | datetimeの検証とAsia/Tokyoへの換算、`TokyoMinute` の作成、天候の取得 |
| health | `DatasetInfo` の提供 | 応答の組立て、未ready時の503 |

## 呼出し例

起動時の処理です。③は `data/current` を一度だけresolveし、同じreleaseを両方に渡します。

```rust
let release_dir = std::fs::canonicalize(data_dir.join("current"))?;
let calendar: Arc<dyn Calendar> = data::load_calendar(&release_dir)?; // 先に読む（conditionsは事故データの準備を待たない）
let engine: Arc<dyn RouteEngine> = data::load_engine(&release_dir)?;
```

routeハンドラの処理です（`tests/connection.rs` の `run_route` が動く見本です）。

```rust
let cancel = CancelToken::with_timeout(Duration::from_millis(35_000)); // 受付から35秒
let guard = cancel.cancel_on_drop(); // futureが破棄されたら、集計も取り消す
let deadline = cancel.deadline().unwrap();
let result = tokio::time::timeout_at(deadline.into(), async {
    let candidates = engine.collect_candidates(router.as_ref(), &query, &cancel).await?;
    let (engine, cancel) = (engine.clone(), cancel.clone());
    tokio::task::spawn_blocking(move || engine.evaluate(&query, candidates, &cancel))
        .await
        .map_err(|e| EngineError::ContractViolation(e.to_string()))?
}).await.unwrap_or(Err(EngineError::Interrupted(Interrupt::DeadlineExceeded)));
// JSONの準備・圧縮の後、応答を返す直前に呼ぶ。期限後に完了した結果を200にしない
let result = cancel.settle(result);
if result.is_ok() { guard.disarm(); }
```

`timeout_at` は処理側を先にpollするので、完了と期限切れが同時に観測されると `Ok` を返します。そのため最後に `settle` で確認します。

conditionsでは `calendar.classify(TokyoMinute::from_local(jst_naive))` を呼びます。結果の `period` と `day.reason` / `day.day_type()` / `day.holiday_table_covered` をそのまま返します。

## 期限・キャンセル

- ③が要求ごとに `CancelToken` を作ります。route/geocodeは受付から35秒で、待ち行列・集計・JSONの準備・圧縮を含みます。
- **非同期の待ち（上流）**：③は期限付きで待ちます。アダプタは、送信の前に `cancel.check()` を確認します。待つ時間の上限は `min(8秒, cancel.remaining())` です。要求全体の期限で打ち切った場合は、`ExternalApi` ではなく `Interrupted(DeadlineExceeded)` を返します。
- **CPU処理（集計）**：futureを破棄しても止まりません。②は `evaluate` の中で、少なくとも約10msごとに `cancel.check()?` を呼びます（例：ループ1024回ごと、候補ごと）。③は `CancelGuard` を使い、クライアントの切断や期限切れで必ず取り消します。
- **結果の採用**：応答を返す直前に `cancel.settle(result)` を呼びます。期限切れ・取消し済みなら、計算が成功していても中断として扱います。
- **不正な経路**：アダプタは、形を解釈できない経路（頂点が数値2個の `[lon, lat]` でない等）を呼出し全体の失敗にせず、その位置に `Err(MalformedRoute)` を置きます。②はそこで検査を止め、初回の呼出しなら要求全体を `ExternalApi` に、追加の呼出しならそれより前の正常な経路を残して警告にします（behavior.md 5章）。
- **追加候補の失敗**：`RoutingError::aborts_request()` が真の失敗（`Busy`・`Interrupted`）は、追加候補の取得中でも警告にせず、要求全体を失敗にします。

## エラーとHTTP応答

HTTP応答への変換は③が行います。messageは日本語で、位置や入力値を含めません。

| 型 | HTTP |
|---|---|
| `QueryError::*`（`message()` を使う） | 400 invalid_request |
| `EngineError::RouteTooLong` | 422 route_too_long |
| `EngineError::NoRoute` | 502 no_route |
| `EngineError::ExternalApi` | 502 external_api_error |
| `EngineError::Busy` | 503 service_busy |
| `EngineError::Interrupted(DeadlineExceeded)` | 503 service_busy（候補を取得済みでも200にしない） |
| `EngineError::Interrupted(Cancelled)` | 応答しない（接続は切断済み） |
| `EngineError::ContractViolation`、集計タスクのpanic | 500 internal_error |
| `data::LoadError` | 起動を止める |

conditionsでは、天候の取得が失敗・期限切れ・混雑になっても `weather_status=unavailable` の200を返します（behavior.md 9章）。

## スタブとfixture

| 用途 | 使うもの |
|---|---|
| ②がOSRMなしで候補生成を検証する | `testing::ScriptedRouter`（呼出し順の応答と、受けた要求の記録。想定外の呼出しはpanic）。全経路が正常な応答は `testing::valid_routes(&routes)` で作る |
| ②の計算結果の期待値 | `testing::fixtures::route_example(name)` が入力（事故・要求・固定経路）を、`expected_route_outcome(name)` が期待値を返す。`aggregation_ms` は比較から除く |
| ③が計算本体なしでハンドラを検証する | `testing::StubEngine`（`Fixed` は期待値を返し、`SpinUntilInterrupted` は中断されるまで集計を続ける）、`testing::FixedCalendar` |
| ③が期限を検証する | `testing::StallingRouter`（期限か取消しまで応答しない） |
| ③のDTO変換の期待値 | `testing::fixtures::route_response_json(name)`（openapi.yamlの応答例そのもの） |
| 両方 | `testing::fixtures::check_outcome_invariants`（件数・null・navigationの相関制約） |

fixtureの例は `singleRoute`・`noAccidents`・`twoSelected` です。入力は [route-example-inputs.json](../docs/api/fixtures/route-example-inputs.json)、期待値はopenapi.yamlの応答例から生成した `tests/fixtures/route-responses.json` です。ナビ区間だけの期待値は [navigation-cases.json](../docs/api/fixtures/navigation-cases.json) にあり、②が使います。期待値の `dataset_id` の0埋めと `aggregation_ms=0` は人工の値です。

## Cargo設定とlockfile

- `Cargo.toml` と `Cargo.lock` の統合は③が担当します。`Cargo.lock` はコミットします。
- 依存を追加するPRには、`Cargo.toml` の変更と、`cargo build` で更新した `Cargo.lock` の両方を含めます。必要のない `cargo update` はしません。
- `Cargo.lock` が競合した場合は、mainの `Cargo.lock` を採用し、自分の `Cargo.toml` の変更を残して `cargo build` で再生成します。手で編集しません。
- `rust-version` は1.88です（edition 2024の `if let` チェーンを使うため）。

## 検証コマンド

repo rootから実行します。前提はRust 1.88以上です。fixtureの検査にはPython 3.9以上とPyYAMLを使います。

```sh
cargo fmt --manifest-path backend/Cargo.toml --check
cargo clippy --manifest-path backend/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path backend/Cargo.toml
python3 -B backend/scripts/extract_route_examples.py --check  # openapi.yamlの応答例とfixtureの一致
python3 -B backend/scripts/extract_route_examples.py          # openapi.yamlの変更後に再生成
```

`cargo test` が検査するのは次の内容です。

- `tests/contract_types.rs`：コードとラベル、時間帯の境界、直線距離と許容時間の境界、期限とキャンセルの規則（`settle` を含む）
- `tests/connection.rs`：fixtureの整合と相関制約（崩した結果を拒否すること）、スタブを経由した接続、不正な経路、上流で待っている間の期限切れ、集計中の期限切れ、完了と期限切れの同時観測、要求の破棄による取消し
- `tests/boundaries.rs`：依存方向

## API・外部通信・基盤（③）

Issue #9 の実装です。設定は環境変数（architecture.md 6章）だけから読み、表にない変数は作りません。

| パス | 内容 |
|---|---|
| `src/main.rs`、`src/startup.rs` | 設定の検査→ログ→releaseの解決（`ACCIDENT_DATA_DIR` を1回だけ `canonicalize`）→祝日表→待受け→事故データの読込（readyまで `/api/health`・`/api/route` は503、`/api/conditions` は使える）。読込の失敗・panicで待受けをやめて非0で終了する。SIGTERM/Ctrl-Cで受付をやめ、処理中の接続を最大10秒待つ |
| `src/config.rs` | 6章の変数の既定値と検査。誤りは変数名だけを出し、値・秘密・URLを出さない。`BACKEND_BIND` が無く `PORT` があれば `0.0.0.0:$PORT` |
| `src/logging.rs` | ルートのテンプレート・status・処理時間・匿名のエラーコードだけを出す。依存ライブラリのログは出さない |
| `src/api/` | `/api/route`・`/api/geocode`・`/api/conditions`・`/api/health`・`/api/status` と静的配信、エラー変換、gzip、転送期限、気象キャッシュ |
| `src/providers/` | OSRM・Geoapify・Open-Meteoのアダプタ、上流HTTPクライアント（プロキシ・リダイレクトなし、期限とサイズ上限）、上流ゲート（`local`・`firestore`） |
| `examples/dev_stack.rs` | C4用の偽上流（下記）。本番の処理から使わない |

### テスト

すべてループバックの偽上流だけを使い、外部へ通信しません。

| ファイル | 内容 |
|---|---|
| `tests/api_contract.rs` | C1（全例のroute応答＝`route_response_json`、gzip有無）、C2の生成と比較 |
| `tests/api_route*.rs`、`tests/api_http.rs` | route入力の検証順、DTO変換、期限・取消し・エラー変換・同時数・gzip・転送期限 |
| `tests/api_geocode.rs`、`tests/api_conditions.rs`、`tests/api_static.rs`、`tests/api_smoke.rs` | 各API・静的配信の規則 |
| `tests/providers_*.rs` | アダプタのURL・解釈・失敗の変換・期限・サイズ上限・鍵を出さないこと |
| `tests/gate_local.rs` | 上流ゲート（間隔・同時数・待ち行列・日次上限・リース・競合の再試行）。`local` と偽Firestoreの両方で同じ検査 |
| `tests/config.rs`、`tests/startup.rs`、`tests/logging.rs` | 設定の検査、起動の配線、ログに検索語・座標・鍵・上流URLが出ないこと |

### C2：`tests/fixtures/api-responses.json`

実ハンドラから生成した、route以外の応答とエラー応答の一式です（①がフロントの検証関数で確かめる）。手で編集しません。

```json
{
  "description": "…",
  "fixed_now": "2026-10-24T09:00:00+09:00",
  "responses": {
    "conditions.currentOk": {
      "validator": "validateConditions",
      "request": {"method": "GET", "target": "/api/conditions?…"},
      "status": 200,
      "headers": {"cache-control": "no-store", "content-type": "application/json", "x-content-type-options": "nosniff"},
      "body": {"api_version": "2.0", "…": "…"}
    }
  }
}
```

- キーは `<種類>.<場合>`（`error.<code>.<API>`、`conditions.*`、`health.*`、`geocode.*`）。`request.body` はPOSTのときだけあり、要求本文のJSONです。`headers` は `content-length` を除く応答ヘッダー（小文字名）です。
- 9種のエラーcode、天候の5つのstatus、current/forecast、`holiday_table_covered=false`、0件/複数件のgeocodeを含みます（`tests/api_contract.rs` が網羅を検査する）。
- 応答を変えたら再生成し、差分を確認してコミットします。

```sh
UPDATE_API_FIXTURES=1 cargo test --manifest-path backend/Cargo.toml --test api_contract
```

### C4のローカル起動

偽上流（偽OSRM・偽Geoapify・偽Open-Meteo）をループバックの空きポートで起動し、それを指す環境変数を出力します。外部のネットワークには通信しません。

1. ②が提供する人工releaseのディレクトリを用意する。
2. 端末A：`cargo run --manifest-path backend/Cargo.toml --example dev_stack -- --example twoSelected`。`export NAME='value'` の行（同じ内容の `env.sh` のパスも表示）が出る。Ctrl-Cで停止。`--example` で偽OSRMが返す例を選ぶ（fixtureの例は出発地・目的地が同じなので、座標では区別できない）。
3. 端末B：出力された環境変数を設定し（`source <表示されたenv.sh>`）、`ACCIDENT_DATA_DIR` に人工releaseを指定して `cargo run --manifest-path backend/Cargo.toml` で `127.0.0.1:8000` に起動する。
4. 端末C：`(cd frontend && VITE_API_MODE=live npm run dev)`。`VITE_API_MODE=live` が無いとデモ応答になる。`/api` は `http://127.0.0.1:8000` へ転送される。
5. ブラウザで `http://127.0.0.1:5173` を開き、実際に検索する。

偽上流の応答：

- 偽OSRMは、`--example` で選んだ例とmodeが一致すればその例の固定経路を返す。選んでいないかmodeが違えば、出発地・目的地・modeがfixture（`docs/api/fixtures/route-example-inputs.json`）の例と一致する最初の例、それも無ければ同じmodeの最初の例の経路を返す。
- 偽Geoapifyは、どの検索語にもfixtureの出発地・目的地を候補として返す。`該当なし` を含む検索語は0件。
- 偽Open-Meteoは、今日0時から `forecast_days` 日分の毎時の天気（日ごとに晴・雨・雪）を返し、現在は晴。

人工releaseが無い間の予行には簡易モードを使えます。`StubEngine`（`Fixed`）と `FixedCalendar`（平日）でAPIサーバも起動し、どの入力にも選んだ例の結果を返します。**実エンジンではないので、C4の合格には使えません。**

```sh
cargo run --manifest-path backend/Cargo.toml --example dev_stack -- --simple            # 例 twoSelected
cargo run --manifest-path backend/Cargo.toml --example dev_stack -- --simple noAccidents
# 8000番が使用中なら BACKEND_BIND=127.0.0.1:18000 を付けて起動する
```

### CI・配備

- `.github/workflows/ci.yml`：PRとmainへのpushで、基本4点（`cargo test` はstableと1.88）とC0-a〜C0-d（frontendの `npm ci`、`generate:api` 後の差分なし、`typecheck`、`test`、`build`）を実行する。秘密・実通信・GCP認証を使わない。
- ルートの `Dockerfile`・`.dockerignore` と [deploy/README.md](../deploy/README.md)：Cloud Run＋IAPの配備定義と手順書。**配備は未実施**。

## 範囲外

PR #5（Issue #4）には含めなかったものです（③の部分はIssue #9で実装済み）：Axumハンドラ、DTO、OSRM・Geoapify・Open-Meteoのアダプタ、上流ゲート、集計、ナビ区間、前処理、祝日表の読込と判定、クラウドへの配備。WMOコードから天候への変換表（architecture.mdでdomainに置くとされているもの）の担当は、本Issueでは決めていません。

## 事故データ・計算（②）

Issue #8 の実装です。計算は [behavior.md](../docs/api/behavior.md) 2〜8章に従い、公開API・計算仕様・`src/contract/` は変更していません。

### 依存方向

`data` は `domain` に依存してよい（エンジンと `Calendar` の実装は `domain` にあり、`data` がそれを組み立てる）。`domain` は `data` に依存しない。どちらも `api`・`providers`・`testing`・tokio/axum/reqwest/hyper を参照しない。

| モジュール | 内容 |
|---|---|
| `domain::calendar` | `HolidayCalendar`（`contract::Calendar` の唯一の実装）。前処理と `/api/conditions` が同じ型を使う |
| `domain::candidates` | 候補の取得（初回→side=-1→+1）・検証・80km判定・5桁fingerprintの重複排除・duration安定ソート |
| `domain::corridor` | 局所投影・75m分割・72m回廊（0.001度グリッド。結果は索引の走査順に依存しない） |
| `domain::scoring`・`navigation`・`engine` | 集合B・スコア・事故点・高リスク区間・ナビ区間・選定・文言（`AccidentEngine`） |
| `domain::rounding` | `py_round`（Pythonの `round` と同じ二進値のhalf-even）。丸めはすべてこれを使う |
| `data::encoding`・`holidays` | 文字コード判定（`utf-8-sig`→cp932）、祝日CSVの読込 |
| `data::preprocess` | 原本CSVの行・ファイル処理、コード表の照合 |
| `data::release`・`load` | releaseの書き出し、起動時ロードと検証 |

### release形式

`ACCIDENT_DATA_DIR`（既定 `backend/data/current`）はこのディレクトリそのものを指します（`current` をさらに付けません）。

| ファイル | 内容 |
|---|---|
| `accidents.csv.gz` | UTF-8・gzip（レベル3）の17列CSV（behavior.md 2章の列順）。緯度経度は小数7桁。行番号0始まりが事故ID |
| `syukujitsu.csv` | 前処理に使った内閣府の祝日表（原本のバイト列のまま） |
| `metadata.json` | 下表 |

`metadata.json` のキー（③のコードは読まない。`/api/health` は `RouteEngine::dataset()` の `DatasetInfo` から作る）：

| キー | 内容 | ロード時の検証 |
|---|---|---|
| `schema_version` | 2 | 2であること |
| `dataset_id` | `accidents.csv.gz` 全バイトのSHA-256（小文字hex） | 実ファイルから再計算して一致 |
| `release_id` | `dataset_id` と祝日表のSHA-256を `\n` で連結した文字列のSHA-256 | 実ファイルから再計算して一致 |
| `rows` | 保持行数（modes=0を含む） | ロード行数と一致 |
| `holiday_table` | `file`・`sha256`・`first_date`・`last_date`（表中の最小・最大日付）・`rows`・`coverage_start`・`coverage_end`（収録範囲） | `sha256` が実ファイルと一致（`load_calendar` も照合） |
| `holiday_uncovered_rows` | 祝日表の収録範囲外の日付で、土日と年末年始だけで判定した行数 | — |
| `processor` | `name`（`prepare_release`）・`version`（crateの版） | — |
| `generated_at` | 前処理の日時（タイムゾーン付きRFC 3339） | — |
| `columns`・`gzip_bytes`・`data_years` | 17列の名前、gzipのバイト数、source_yearの昇順 | — |
| `files` | 年度ごとの `source_year`・`file`・`sha256`（原本）・`encoding`・`input_rows`・`retained_rows`・`excluded_rows`・`excluded_by_reason`・`no_target_mode` | — |
| `totals` | 全年度の `input_rows`・`retained_rows`・`excluded_rows` | — |
| `codebooks_xlsx`・`codebooks_pdf` | 照合したコード表の `source_year`・`file`・`sha256` | — |
| `elapsed_seconds` | 前処理の秒数 | — |

ロードでは、あわせてヘッダの17列の完全一致、先頭10列の型と範囲（緯度経度の範囲、modes 0〜7、period 0〜4、weather 1〜5、day_type 0〜1、人数は各0〜999で合計1以上、実在する8桁の日付で1900 ≦ 発生年 ≦ source_year かつ発生年は9999以下）、空でないことを検証します。失敗は `LoadError` で、メッセージに位置や原本行を含めません。

除外理由のキー：`missing_field`・`invalid_coordinates`・`invalid_datetime`・`invalid_casualties`・`unknown_party_code`・`unknown_weather_code`・`unknown_injury_code`・`unknown_daynight_code`・`invalid_accident_type`（この順で最初の理由に1回だけ計上）。

### 前処理コマンド

バイナリ名は `prepare_release` です（`default-run` とDockerfileの `--bin` は③が決めます）。

```sh
cargo run --release --manifest-path backend/Cargo.toml --bin prepare_release -- \
    --accident-dir accident --holidays accident/syukujitsu.csv --output backend/data
```

- 原本（`honhyo_2019.csv`〜`honhyo_2025.csv`、`codebook_2019〜2021.pdf`、`codebook_2022〜2025.xlsx`）は読むだけです。コード表は常に実ファイルで照合します（省略するオプションはありません）。
- `<output>/staging/` で3ファイルを作ってから `<output>/releases/<release_id>/` へ移し、読み取り専用にします。同じ `release_id` が既にあれば上書きせず失敗します。`current` は変更しません。
- 成果物（`backend/data/releases/`・`staging/`・`current`）は置き場所とgit管理が決まるまでコミットしません（`backend/data/.gitignore`）。
- 標準出力に件数・ID・年度別の除外理由をJSONで出します。

### テスト用のヘルパー

| 用途 | 使うもの |
|---|---|
| 人工releaseを一時ディレクトリに作る（C3） | `testing::TestRelease::for_example(name)`。`path()` を `data::load_calendar`・`data::load_engine` に渡す。破棄時に消える。`ids` は実際のハッシュ（fixtureの0埋めとは異なる） |
| 例ごとの固定経路 | `testing::example_router(&example)`。初回は固定経路、初回が3本未満なら追加の2回にも同じ経路を返す |
| 人工releaseを永続ディレクトリに書く（C4） | `cargo run --manifest-path backend/Cargo.toml --example make_test_release -- <出力先> <例の名前>` |

人工releaseの事故は、その例の `accident_ids`（0からの連番であることを検査）だけです。`DatasetInfo.accidents` はその件数（`singleRoute`=1、`noAccidents`・`twoSelected`=3）、`data_years` は `[2025]` です。同梱の祝日表（`testing::release::TEST_HOLIDAY_CSV`）は2025年の祝日・休日で、収録範囲は2025-01-01〜2025-12-31です（範囲内の例：2025-11-24は振替休日、範囲外の例：2026-01-05は `holiday_table_covered=false`）。

### 追加したテスト

`calendar`（時間帯・曜日区分・祝日表）、`preprocess_rows`・`preprocess_files`・`preprocess_release`（前処理・コード表）、`release_load`（起動時検証）、`test_release`（人工release）、`candidates`・`corridor`・`scoring`・`navigation`（`navigation-cases.json` の全ケース）・`selection`、`engine_fixtures`（人工release＋実エンジン＋固定経路がopenapi.yamlの全応答例と一致。比較規則は共通前提のC3と同じ）、`engine_cancel`（中断）。実コード表・実祝日表の試験はリポジトリの `accident/` を使い、なければ失敗します（省略しません）。実releaseの確認は `real_release` で、既定では `ignored` と数えます。`ACCIDENT_REAL_RELEASE_DIR` を指定し、`-- --ignored` を付けて実行します。

## 範囲外

③の担当で、ここに含めないものです：Axumハンドラ、DTO、OSRM・Geoapify・Open-Meteoのアダプタ、上流ゲート、クラウドへの配備。WMOコードから天候への変換表は③の担当です（#7〜#9 共通前提2章）。
