# backend：内部インターフェースの引き継ぎ

事故データ・計算担当（②）とAPI・外部通信・基盤担当（③）が、相手の完成を待たずに並行して実装するための内部契約です（Issue #4）。外部のHTTP/JSONは [openapi.yaml](../docs/api/openapi.yaml)、計算は [behavior.md](../docs/api/behavior.md)、構成は [architecture.md](../docs/architecture.md) に従います。ここでは公開APIと計算仕様を変更していません。

現時点で入っているのは契約の型、スタブ、fixtureの読込、検証用のテストです。各機能の本実装（Axumハンドラ、OSRMアダプタ、集計、前処理、祝日表の読込）はまだありません。

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

## 範囲外

本PRには含めないものです：Axumハンドラ、DTO、OSRM・Geoapify・Open-Meteoのアダプタ、上流ゲート、集計、ナビ区間、前処理、祝日表の読込と判定、クラウドへの配備。WMOコードから天候への変換表（architecture.mdでdomainに置くとされているもの）の担当は、本Issueでは決めていません。
