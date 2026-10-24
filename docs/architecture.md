# 移行アーキテクチャ

## 1. 方針と文書の優先順位

Rust/Axumはデータ前処理・ロード・外部サービス呼出し・候補生成・事故集計・選定を担当する。出発日時からの時間帯・曜日区分の判定（祝日表）と、地域の天候の取得（気象提供元）もbackendが担う。React/TypeScript/Vite/MapLibre GL JSは入力、出発日時と条件の自動/手動の管理、地図、比較、GPS許可・ナビの進捗/接近/逸脱判定・模擬走行、PWAを担当する。両者はHTTP/JSONだけで連携し、ソース・ビルド・依存関係を分離する。

[api/openapi.yaml](api/openapi.yaml) を外部契約の正本、[api/behavior.md](api/behavior.md) を計算と互換性の正本、[frontend-spec.md](frontend-spec.md) を操作の正本にする。本書は構成と運用の新設計であり、現行Pythonの実装済み機能を示すものではない。配備先・アクセス制御・外部提供元・共有レート制御の決定と根拠は [backend-spec.md](backend-spec.md) と [cloud-selection.md](cloud-selection.md) にあり、本書はそれに合わせて記述する。

検討した境界は、旧Pythonの配列JSONをそのまま維持する案と、MapLibre用のGeoJSONへ表示データを正規化する案。前者は旧UIを接続しやすいがフロント側に変換規則が残る。今回は新Reactとの並列実装を優先し、**集計値を維持して、事故点と高リスク区間をFeatureCollectionにする契約**を採用する。この選択は本仕様の設計判断であり、旧UIの完全互換を主張しない。

## 2. ディレクトリ案と依存境界

以下は将来の構成案。本作業はdocsの仕様書と契約fixtureだけを編集する。既存のapp/はデモ、backend/・frontend/は誤って作成された実装であり、仕様の根拠にせず変更しない。

```text
app/                         デモ。仕様の根拠にせず変更しない
accident/                    原本CSV/PDF/XLSX。読み取り専用
backend/
  Cargo.toml / Cargo.lock
  src/
    main.rs                  起動・設定・HTTP組立て
    api/                     Axum handlers、入出力DTO、エラー変換
    domain/                  座標・条件・集計・候補選定、時間帯/曜日区分の判定
    data/                    CSV変換、列配列、空間索引、metadata、祝日表の読込
    providers/               routing/geocoding/weatherの抽象と実装
    bin/                     前処理コマンド
  tests/
    fixtures/                Pythonと共通の入力・外部応答・期待結果
    contract/                OpenAPI/DTO/HTTP整合試験
  data/
    holidays/syukujitsu.csv  内閣府「国民の祝日」CSV。前処理とconditionsで共用
    releases/<release-id>/   accidents.csv.gz、metadata.json、syukujitsu.csv（前処理に使った版）
    current                  公開中releaseへの参照（Web配信禁止）
frontend/
  package.json / lockfile
  src/
    api/generated/           OpenAPI由来型。手修正禁止
    api/                     HTTPラッパー
    components/              スタイルを持たない最小構成
    state/                   入力・出発日時・条件の自動/手動・結果・世代管理
    navigation/              実機/模擬の位置源、時計、共通判定、音声/Wake Lock
    map/                     MapLibre source/layerとスタイル接続
    styles/                  後で確定するデザイントークン
  public/                    manifest/SWの生成元等
  tests/                     契約fixtureによる機能試験
  dist/                      独立ビルドした静的配信物
  vite.config.ts
docs/
  api/openapi.yaml
  api/behavior.md
  frontend-spec.md
  architecture.md
```

backendはfrontendのソースを読まず、ビルド済みdistを設定されたパスから配信するだけ。frontendはbackendのRust型やデータCSVを直接importしない。API層はドメイン型とDTOを分離し、GeoJSON包装やHTTPの都合を空間集計へ持ち込まない。候補とナビ区間の数値・順序・結合を決めるロジックはbackend一か所にする。選定後だけnavigationを組み立て、表示用の上位100区間と同じsid別集計を使う。フロントは送られた累積距離軸で幾何学的な進捗と通知時点を判定し、事故リスクを再計算しない。

時間帯・曜日区分の判定関数もbackendのdomainに一つだけ置き、前処理コマンドと `/api/conditions` の両方から呼ぶ。frontendは時刻境界や祝日を計算せず、ブラウザのタイムゾーンにも依存しない。気象提供元への通信はbackendだけが行い、ブラウザから気象APIを直接呼ばない。

## 3. 型生成と契約変更

OpenAPI 3.1から `openapi-typescript` でTypeScript型を生成する。同ツールは3.1とローカルYAMLをサポートし、型はランタイム検証器ではない（[公式説明](https://openapi-ts.dev/introduction)）。依存版は実装時に検証してlockfile固定し、常にlatestを取得するCIにしない。

将来の生成コマンド（今回実行・実装しない）：

```text
cd frontend
npx openapi-typescript ../docs/api/openapi.yaml -o src/api/generated/schema.d.ts
```

frontendはpaths/components由来型を使用する。`Position` のprefixItemsが `[number,number]` に生成されるかを採用版で確認し、座標順を意味付きアダプタで保つ。型生成時にスキーマ機能が欠落する場合は検証できる版へ合わせるか契約を修正し、手書き型で黙って補わない。

Rust側はserde DTOを実装し、OpenAPIのoptional/required/null/enumに合わせる。とくにリスク削減率と `/api/conditions` のweather/weather_sourceのnullはOption、period/weatherの日本語と整数は入力専用の互換デシリアライズ、day_typeは日本語/all/nullだけを受理、boolは数値として拒否する。省略値は入力正規化で明示的に補い、JSON Schemaのdefaultを書いただけで自動適用されると考えない。periodと時間帯の優先、明示nullと省略の区別を失わない。12.0の整数値受理は移行追加の表記ルールとしてbehaviorに従う。

契約の整合は次の順で保証する。

1. 変更はまずOpenAPIと関連仕様に記載し、同じ変更単位でDTO・生成TS・fixtureを更新する。サーバから逆生成したOpenAPIを別の正本にしない。
2. CIでOpenAPI 3.1文法・全参照を検査し、各共通fixtureをJSON Schema Draft 2020-12で検証する。HTTP status/code、ID参照・個数一致・非緩和（relaxed_conditions=[]）・weather_matchとsafest_idのnull条件、選定候補だけnavigation非null・累積配列長/末尾・区間順序/総数等の相関制約は別の契約試験で検査する。
3. RustでfixtureをDTOへdeserialize/serializeして再検証する。レスポンスの必須値欠落、null/省略の違いを検出する。Axum extractorの既定エラーもJSON形式へ変換されることを確認する。
4. frontendで型を再生成して差分ゼロを確認し、型チェックとfixture利用の操作試験を実施する。生成型だけでは実応答を保証しないので、API境界ではJSON形状とID参照の検査を行う。
5. 固定外部応答でRustの決定性とfixture一致の試験を通し、その後同一契約のReact/Axumを結合する。Pythonとの数値同等性は要求しない。

必須フィールド削除・型変更・意味変更は契約版更新が必要。追加フィールドも本OpenAPIのadditionalProperties制約に従い、仕様更新→生成・検査→協調リリースとする。時間帯の5区分化（periodの整数3と「夜」の意味の変更）、day_typeの追加、effective_conditionsへの必須項目追加、天候の重み付け化とweather_matchの追加、スコア方式・半径の変更、safest_idのnull許容は意味変更なので、未リリースの統合契約をapi_version 2.0として揃える。frontendは2.0以外のapi_versionを受けたら、古いUIとAPIの混在として扱う。GPSナビ用の必須nullable navigationとその全対象区間も未リリースの2.0に含める。既存UIが2.0とだけ検査して通過しても、navigation欠落・相関不正なら応答を拒否する。新旧DTO/生成TS/fixtureを協調更新し、既に配信済みになった後の同様な契約変更は版更新を必要とする。表示用high_risk_sectionsの上位100件の意味は変えない。判断の比較・根拠は [navigation-delegated-decisions.md](navigation-delegated-decisions.md) を参照。

## 4. 開発・配信

開発時はbackendを127.0.0.1:8000、Viteを127.0.0.1:5173で起動する。Viteの `server.proxy` は `/api` をbackendへ転送し、パスを削除しない。つまり `/api/route` は同じ `/api/route` へ届く。API_BASE相当は常に相対パスで、フロントにOSRM/Geoapify/Open-Meteo URLを埋め込まない。[Vite公式proxy仕様](https://vite.dev/config/server-options.html#server-proxy) を参照。

strictPortを有効にして勝手なポート移動を避ける。localhost利用だけならCORS不要。別オリジン運用は初期版の対象外で、必要時に許可originを限定して追加設計する。LAN端末でGPS/PWAを検証する場合は信頼されたHTTPSを使う。

本番は **Google Cloud Run 東京（`asia-northeast1`）** とし、Cloud RunのTLS終端→Axum（`0.0.0.0:$PORT`）で、APIとビルド済みdistを1つのコンテナから同一オリジンで配信する。初期1 vCPU / 2 GiB・同時受付4・最大1台、通常は最小0台、実演中は最小1台（[cloud-selection.md](cloud-selection.md)）。実演はCloud RunのURL（HTTPS）へブラウザから直接アクセスし、IAPで許可したGoogleアカウント（開発者・審査員）だけに限定する。`allUsers` を付けない。アプリ独自のログインやブラウザへのトークン埋込みは作らない。IAPのログインはページ遷移で行われ、セッション切れの `/api/*` はログインへのリダイレクトになり得る。Cloud Run・IAPがアプリ到達前に返す認証リダイレクト・拒否・429等は非JSONになるので、フロントは非JSONの失敗として扱い、再読込を案内する。PWAのmanifestはIAPのcookieが必要なので `<link rel="manifest" crossorigin="use-credentials">` で読み込む。`/api/*` を先に解決し、未知APIへHTMLを返さない。static root外、dotfiles、ディレクトリ一覧、ソース/原本/データへのアクセスを禁止する。

HTML/manifest/sw.jsはno-cache（再検証可能）、内容ハッシュ付きassetsは長期immutable、APIはすべてno-store。画面は `/` のみ、未知パスは404。MapLibre style/tilesの配信元と帰属は別途設定する。APIクライアントは45秒で打切り、サーバのroute/geocodeは受付から応答JSON準備完了まで最大35秒の要求全体期限を設ける（キュー待ち・集計・シリアライズを含み、クライアントへの転送時間は含まない）。全体期限切れは処理段階によらず503 service_busyで終了し、既存候補があっても200を返さない。この制御は追加候補のAPIError捕捉より外側に置く。全体期限内の個別上流通信の8秒期限切れは502 external_api_errorで、追加候補だけの失敗ならbehavior通り警告にして集計を続けられる。待ち行列満杯は503 service_busy。未完了の処理をキャンセルし、CPU集計も期限を確認して停止できるようにし、無制限に積み上げない。

`/api/conditions` はクライアント10秒、サーバ全体8秒、気象提供元への1通信5秒とする。サーバ全体の期限はクライアントより短くし、期限内に天候が得られなければ天候だけunavailableとして200を返す。気象提供元の失敗・待ち行列満杯はエラー応答にせず、時間帯・曜日区分は常に返す。

## 5. データ配置と更新

原本はaccident/を読み取り専用で扱い、前処理はビルド前に開発環境で行って結果をbackend/data配下へ書く。旧16列の `app/data/accidents.csv.gz` は流用しない。backendの前処理コマンドがbehaviorの変換を担当する。読み取り用成果物だけをコンテナイメージへ同梱し、原本・前処理の作業領域は実行環境へ持ち込まない。

更新手順：

1. 原本7年度とコード表を揃え、出所・ハッシュ・source_yearを監査する。未知年度の追加はコード表差分と除外方針を確認してから仕様の年度集合を更新する。現行原本を作業中に書き換えない。祝日表は内閣府の最新CSVを取得し、出所・取得日・SHA-256を記録する。前版との差分（追加年・変更日）を確認する。
2. backend/dataの非公開stagingディレクトリで前処理。入力件数=保持+除外、除外理由、modes=0、コード表照合、17列形式、非空、period 0〜4・day_type 0〜1、祝日表の収録範囲外の行数を検査する。期待値は確定仕様から作ったfixtureで検査し、Pythonの出力との一致は合格条件にしない。
3. gzipのSHA-256をdataset_idとし、祝日表のSHA-256と組み合わせたrelease_id（behavior 3章）とともに、処理版と合わせてmetadataへ記録する。前処理日時/圧縮メタデータ差でIDが変わり得るため、意味的同等性は復号後のデータでも確認する。
4. 検証済みのgzip・metadata・前処理に使った祝日表を同じreleases/<release-id>に配置し、不変・読み取り専用にする。途中ファイルをcurrentへ置かない。`/api/conditions` もこのreleaseの祝日表を読むので、事故記録の曜日区分と自動判定の祝日表は常に一致する。
5. 検証済みreleaseを同梱したイメージで新しいCloud Runリビジョンを作り、起動時に全件ロードと、実ファイルから再計算したdataset_id/release_id・schema・件数の検証を終えてからstartup probeを成功させる（起動4分以内）。成功したリビジョンへトラフィックを切り替える。初期版ではホットリロードを実装しない。プロセスは起動時にreleaseの実パスを1回解決して同じスナップショットから2ファイルを読む。
6. `/api/health` のrelease_id/dataset_id/accidents/data_yearsと固定検索fixtureを確認。失敗時は旧リビジョンへ戻す。途中でmetadataだけ新しくなる状態を公開しない。

一つの検索要求中は一つのデータスナップショットを固定する。公開事故データの保存と、利用者の現在地・検索履歴の無保存は別の対象である。

祝日表は毎年2月ごろ内閣府が翌年分を公開する。収録範囲外の日付は土日と年末年始だけの判定になり、holiday_table_covered=falseとして利用者へ警告される。年1回、祝日表の更新とデータの再リリースを運用予定に入れる。祝日表だけの更新でも前処理をやり直し、新しいrelease_idとして公開する（事故CSVが変わらなければdataset_idは同じでよい）。

## 6. 環境変数の移行契約

以下は新設する設定名。現行はCLIの--host/--port/--data以外の設定はなく、上流URL/UA等はハードコードされている。単位を名前に含める。起動時に型・範囲・必須値を検査して誤設定なら停止する。

| 変数 | 既定/必須 | 意味 |
|---|---|---|
| BACKEND_BIND | 127.0.0.1:8000 | API待受け（開発）。Cloud Runでは `0.0.0.0:$PORT`（ローカル既定8080） |
| ACCIDENT_DATA_DIR | backend/data/current（repo root基準） | gzip/metadataを含むディレクトリ。起動時resolve |
| FRONTEND_DIST_DIR | frontend/dist（repo root基準） | 静的root。API専用開発起動では未生成でも可 |
| ROUTING_PROVIDER | osrm | 初期対応アダプタ識別子。未対応値は起動拒否 |
| OSRM_WALK_BASE_URL | 必須 | `.../routed-foot` 相当。ここへ `/route/v1/walking/...` を付加 |
| OSRM_BIKE_BASE_URL | 必須 | `.../routed-bike` 相当。cyclingを使用 |
| OSRM_CAR_BASE_URL | 必須 | `.../routed-car` 相当。drivingを使用 |
| GEOCODING_PROVIDER | geoapify | Geoapify Forward Geocodingアダプタ。未対応値は起動拒否 |
| GEOAPIFY_BASE_URL | 必須 | `/v1/geocode/search` を付加するベースURL |
| GEOAPIFY_API_KEY_FILE | 必須 | Geoapify APIキーのファイルパス。Secret Manager参照等でサーバだけへ渡す。VITE_*に置かない |
| GEOCODING_DAILY_LIMIT | 2000 | アプリ側の1日あたり住所検索上限（公式Free枠3,000 credits/日より低く置く） |
| EXTERNAL_USER_AGENT | 必須 | アプリ識別と運用連絡先を含むUA。秘密値を含めない |
| ROUTING_ATTRIBUTION | 必須 | ページの経路帰属表示に渡すビルド/配備設定と一致させる |
| GEOCODING_ATTRIBUTION | 必須 | geocode応答のattribution（`{text,url}` の配列をJSONで指定） |
| UPSTREAM_TIMEOUT_MS | 8000 | 1通信の期限。結果同等性試験では既定固定 |
| UPSTREAM_MAX_BYTES | 12000000 | 上流応答最大バイト数。既定固定 |
| ROUTING_MIN_INTERVAL_MS | 1050 | 同一提供元の通信開始最短間隔。公共OSRMでは短縮不可 |
| GEOCODING_MIN_INTERVAL_MS | 1050 | 全インスタンス共通の開始最短間隔。同時通信は1 |
| UPSTREAM_QUEUE_CAPACITY | 16 | 提供元別の待ち通信上限。満杯はservice_busy |
| REQUEST_TIMEOUT_MS | 35000 | queue待ち含むAPI要求全体の上限 |
| UPSTREAM_GATE | firestore | 全インスタンス共通の上流ゲート。本番はfirestore必須、localはローカル開発専用 |
| WEATHER_PROVIDER | open-meteo | 初期対応の気象アダプタ識別子。未対応値は起動拒否 |
| OPEN_METEO_BASE_URL | 必須 | `/v1/forecast` を付加するベースURL。無料版と有償版で接続先が異なる |
| OPEN_METEO_API_KEY_FILE | 任意 | 有償プランのAPIキーを置いたファイルのパス。サーバ専用のsecret。VITE_*に置かない |
| WEATHER_TIMEOUT_MS | 5000 | 気象提供元への1通信の期限 |
| WEATHER_FORECAST_DAYS | 7 | 予報を取得する日数（1〜16）。これより先の日時はout_of_range |
| WEATHER_CACHE_TTL_SEC | 600 | 丸めた座標ごとの気象応答のメモリキャッシュ期間 |
| WEATHER_MIN_INTERVAL_MS | 200 | 気象提供元への全インスタンス共通の通信開始最短間隔。Open-Meteoの分/時/日/月上限も上流ゲートへ設定 |
| CONDITIONS_TIMEOUT_MS | 8000 | `/api/conditions` の要求全体の上限。クライアントの10秒より短くする |
| RUST_LOG | info | URL/query/body/位置を含まないログレベル |
| DEV_API_PROXY_TARGET | http://127.0.0.1:8000 | Vite開発プロキシ専用。ブラウザbundleへ公開しない |
| VITE_MAP_STYLE_URL | 必須 | 選択したMapLibre styleの公開URL。秘密を含めない |
| VITE_ROUTING_ATTRIBUTION | 必須 | ページで表示する経路帰属。配備時にbackend設定と同じ提供元にする |
| VITE_MAP_ATTRIBUTION | 必須 | 地図提供元の帰属。style内のsource attributionと整合させる |

パスは本番では絶対パスを指定し、cwd依存を避ける。APIキーはサーバ専用のsecret参照で渡し、VITE_*へ置かない。VITE_*は公開値として扱う。mode/period/day_type/weather、時間帯の境界、天候コードの変換表、半径、重み、候補数、30km/80km等の互換性定数は初期版では環境変数化しない。祝日表はACCIDENT_DATA_DIRのreleaseから読み、別の環境変数を設けない。気象・祝日の帰属表示は提供元が固定なので、frontendのDataAttributionに直接書く。

## 7. 外部サービスと商用運用

2026-10-24に以下の公式方針を参照した。OSSライセンスと公開サーバの利用条件は別であり、「OSRM/NominatimがOSSだから公共APIも無制限に商用利用できる」と解釈しない。法務・契約の最終判断と配備時の再確認は**要確認**。

- **Geoapify Forward Geocoding（住所検索に採用）**：Free枠は3,000 credits/日・最大5要求/秒、単純な住所検索は通常1 credit/要求。上限は自動停止や無料の保証ではないため、実使用量を確認し、有料プランへ自動変更しない。API要求の技術情報を保持する旨を公表している。[料金](https://www.geoapify.com/pricing/)、[利用条件](https://www.geoapify.com/terms-and-conditions/)、[保持方針](https://www.geoapify.com/privacy-policy/)
- **Nominatim公共API（不採用）**：全利用者合計で最大1request/秒、キャッシュ利用、帰属に加え、個人情報・機密を送らない制限がある。住所入力を扱う今回の構成には採用しない。外部運用ログの許容だけでこの利用方針が解除されるとは扱わない。[公式利用方針](https://operations.osmfoundation.org/policies/nominatim/)
- **現行のFOSSGIS OSRM公共サーバ**：最大1request/秒、識別UA/適切なReferer、帰属と地図修正リンク、heavy usage/scraping禁止。経路要求をサーバログへ保存する旨が記載されている。[提供元の説明・利用制限](https://routing.openstreetmap.de/about.html)。この条件は `router.project-osrm.org` 等の別サーバに一般化しない。リンク先の[ドイツ語利用規約全文](https://www.fossgis.de/arbeitsgruppen/osm-server/nutzungsbedingungen/)は取得拒否により今回未確認。商用SLA/許容負荷と全文は配備前に確認し、本番の既定基盤にしない。
- **Open-Meteo**：無料APIは非商用利用に限られ、APIキーは不要。1日・1時間・1分あたりの呼出し上限があり、データはCC BY 4.0で帰属表示が必要。商用利用は有償プラン（APIキー付きの別エンドポイント）が必要。上限値と規約は[公式利用規約](https://open-meteo.com/en/terms)と[料金](https://open-meteo.com/en/pricing)で配備前に再確認する（**要確認**）。気象モデルの推定値であり、事故記録の天候（現場判定）とは尺度が異なる。
- **内閣府「国民の祝日」CSV**：[国民の祝日について](https://www8.cao.go.jp/chosei/shukujitsu/gaiyou.html)で公開されているCSVを取得し、リポジトリ外の配布物として同梱する。実行時には取得しない。振替休日と国民の休日はどちらも名称「休日」と記載されるため、区別はbehaviorの規則で行う。
- MapLibreは描画ライブラリであり、タイルサービスの利用権を提供しない。背景地図、スタイル、フォント等の商用利用・帰属・オフライン保存条件は採用提供元ごとに確認する。公共タイルの無断一括ダウンロードをPWA要件に含めない。

**採用方針**：本番の上流URLに公共サーバを既定値として入れない。経路提供元と自己ホストの要否は経路決定側の結論を接続する。住所・経路のキャッシュは標準off。外部の経路・住所提供元が送信座標・住所を運用ログへ保存することは利用者が許容した（[spec-decisions.md](spec-decisions.md)）。提供元の無保存契約は必須にしないが、利用規約・送信可能な情報・帰属・制限は満たす。

weatherアダプタは丸めた緯度経度を受け、現況のWMO天気コードと時刻、毎時予報の時刻とコードの列を返す。WMOコードから晴/曇/雨/霧/雪への変換はアダプタの外（domain）に置き、提供元を替えても変換表を共通にする。提供元が別のコード体系を返す場合は、アダプタ内でWMOコードへ変換する。

routingアダプタはorigin/dest/mode/optional viaを受け、順序付きduration秒/distance m/LineStringを返す。候補生成・重複排除・事故集計はその外側に置く。geocodingアダプタはqを受け、lat/lon/labelとattributionを返す。認証、URL組立て、provider独自エラーはアダプタ内で共通APIErrorへ変換する。日本語HTTPエラーに上流URLを埋め込まない。

接続先設定の変更で自己ホストや同互換サービスへ替えられるようにする。異なるプロトコルの提供元は新アダプタを実装するが、フロント契約は維持する。徒歩/自転車プロファイルを提供できないサービスへ替えたとき、drivingへ黙って代替しない。提供元が変わると候補geometry/時間が変わるため、受け入れは固定上流応答を使い、確定仕様から作ったfixtureとの一致で判定する。Pythonとの数値互換は保証しない。

提供元別に全利用者共通の直列化・通信開始間隔・有限待ち行列を設ける。Cloud Runは最大1台でも再配備・保守で複数インスタンスが一時的に併存するため、**全インスタンス共通の上流ゲートをFirestore Standardのトランザクションで管理する**。保存するのは提供元/アカウントの識別子、呼出し開始時刻・期限、同時通信のリース/世代番号と集計件数だけで、住所・座標・経路・利用者ID・リクエストIDを保存しない。通信をトランザクション内で実行せず、共有状態が読めない/競合が解消しない場合は呼び出さない（route/geocodeは追加候補の取得中でも要求全体を503 service_busy、conditionsは天候unavailable）。クライアントからFirestoreへの直接アクセスは許可しない。OSRM追加候補の要求順は既存契約と同じで、最大3呼出しを並列化しない。公共OSRMは全利用者合計1req/秒、ROUTING_MIN_INTERVAL_MS=1050を維持する。ナビデータは既存route応答の選定1〜2本に同梱し、ナビ開始/進行/模擬には上流呼出しを追加しない。ナビ中の明示再検索だけが通常検索として同じゲートを通る。steps=falseを維持し、専用追跡・区間再取得APIやサーバ上の経路保存は設けない。

## 8. プライバシー

利用者の位置（GPS/手入力/地図指定）、住所検索語、出発地/目的地、経路、検索履歴を永続保存しない。アカウント、位置DB、追跡イベント、リクエスト本文の計測基盤送信は初期版に設けない。公開事故原本と前処理結果は保持してよい。

ブラウザはページのメモリだけ、サーバは要求処理中の一時保持だけを標準とする。ナビ用区間・実GPS/模擬位置・accuracy・進捗・通知済みIDも同じ扱いで、ログ・分析イベント・履歴配列に蓄積しない。判定に必要な直近値/タイマーだけを保持し、終了/unmountでナビ状態を破棄する。localStorage/IndexedDB/SW/API cache/URL query・hashへルート状態・出発日時・条件取得結果を保存しない。GPS取得・ナビ開始・各測位・模擬更新だけではサーバ送信しない。ナビ中に条件の定期再取得をしない。サーバへ座標を送るのは、両地点の確定後の条件取得（2地点の中点を小数2桁、約1kmに丸めた1点）と、明示的な経路検索（ナビの再検索ボタンを含む）だけである。再検索時の現在日時・条件の再取得/手動保持はfrontend-spec 7.6に従う。

例外として、気象応答のキャッシュは丸めた座標（約1km格子）をキーにしてプロセスメモリに最大10分保持する。利用者・要求との対応付けは持たず、永続化しない。再起動で消える。気象提供元へは丸めた座標だけを送り、元の出発地・目的地は送らない。ブラウザ自身やOSの許可履歴までアプリが消去できるという保証はしない。

アクセスログはルート名（パステンプレート）、status、処理時間、匿名のエラーコードなど位置を含まない項目に限定する。Cloud Runが自動生成するリクエストログはクエリ文字列を含むため、実位置・住所を送る前に `_Default` sinkへ当該サービスのリクエストログの除外フィルタを設定し、他sink・組織転送も確認する（[cloud-selection.md](cloud-selection.md)）。`/api/geocode?q=...` と `/api/conditions?...&lat=..&lon=..` のraw URI、OSRMとOpen-Meteoの座標入りURL、body、Refererの機微値、エラー内のURLを、Axum・HTTPクライアント・リバースプロキシ・APMすべてで記録しない。メトリクスlabelにも含めない。ページのReferrer-Policyはno-referrerを基本とし、提供元識別にはUAを使用する。

hidden/pagehideでwatch・模擬・音声を停止し、復帰は明示操作とする。バックグラウンド追跡を行わず、SWはナビ中（一時停止を含む）に強制更新しない。通信なしのメモリ内ナビはオフラインでも継続できるが再訪では復元しない。IAPの定期確認はせず、明示通信で認証失敗を検出したら停止する（frontend-spec 7.8）。

外部ルーティングには座標、住所検索（Geoapify）にはq、気象提供元には丸めた座標、地図提供元には表示範囲が伝わり得る。外部提供元の運用ログは利用者が許容済みである。アプリ内で位置・検索履歴を永続保存しない方針とは別に扱い、提供元の一時処理や運用ログまで消せるとは説明しない。

## 9. 並列実装と結合の完了条件

backend担当はbehavior 11章の試験・追加境界試験・OpenAPI応答検証を行う。frontend担当は同じOpenAPIから生成した型と共通fixtureで実装し、UI側に独自の事故リスク計算を作らない。先に通常・同一・全候補0件（safest_id=null）・天候一致0件・天候all（weather_match=null）・間引き・HTTP失敗（非JSONのプラットフォーム失敗を含む）のfixtureを共有して、その後は互いの実装待ちなしに進める。`/api/conditions` も、ok（現況/予報）・no_location・past・out_of_range・unavailable・祝日表範囲外のfixtureを先に共有する。

結合では座標順/単位、選定ID、GeoJSONの直接描画、事故点のID重複排除、区間の元頂点対応、null削減率、エラー/ローディング、healthデータ世代、API no-storeを確認する。条件については、conditionsの結果がそのままrouteのperiod/day_type/weatherへ渡ること、手動値が上書きされないこと、送信座標が丸めた中点だけであることを確認する。実上流の疎通・実道路妥当性、実際の気象応答、スマートフォンGPS、HTTPS、PWA更新/オフライン、提供元条件、Cloud Run IAP・共有上流ゲート・ログ除外の実効性は別の受け入れ項目であり、単体テスト成功で代用しない。

ナビはbehavior 11章の人工fixtureとfrontend-spec 11章の模擬走行で結合検証する。2選定/同一ID/null・上位100件超と全区間・交差/折返し・低精度・通知抑止・逸脱/明示再検索・503・世代競合を確認する。最大navigation 5,000,000バイトと概ね11〜12MBの全応答ケースで、Cloud Run初期資源/35秒期限・ブラウザ解析/描画時間を測る。実機での通知・GPS・音声・Wake Lock・IAP・非表示・SW更新は別の受け入れとし、模擬成功で代用しない。

最終的に**要確認**なのは商用提供元/タイル契約、Open-Meteoの商用プラン、天候変換表、σ・天候倍率の妥当性（[route-scoring-decision.md](route-scoring-decision.md) 6章）、対応端末、IAPの組織外アカウント、ナビの精度/距離/継続時間の暫定値とサイズ/性能、Web Speech APIのja-JPとiOS操作起点、Wake Lock、道路交通法と2024年11月からの自転車ながらスマホ罰則を踏まえた注意文の最終適否。今回ネットワークで現行の外部仕様・法令は確認していない。ナビの機能範囲は確定済みで、これらは実装/配備前の検証残件とする。走行中に復旧操作を要求せず、設定・再検索・再ログインは安全な場所で停止後に行う。
