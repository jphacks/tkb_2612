# 移行アーキテクチャ

## 1. 方針と文書の優先順位

Rust/Axumはデータ前処理・ロード・外部サービス呼出し・候補生成・事故集計・選定を担当する。出発日時からの時間帯・曜日区分の判定（祝日表）と、地域の天候の取得（気象提供元）もbackendが担う。React/TypeScript/Vite/MapLibre GL JSは入力、出発日時と条件の自動/手動の管理、地図、比較、GPS許可、PWAを担当する。両者はHTTP/JSONだけで連携し、ソース・ビルド・依存関係を分離する。

[api/openapi.yaml](api/openapi.yaml) を外部契約の正本、[api/behavior.md](api/behavior.md) を計算と互換性の正本、[frontend-spec.md](frontend-spec.md) を操作の正本にする。本書は構成と運用の新設計であり、現行Pythonの実装済み機能を示すものではない。

検討した境界は、旧Pythonの配列JSONをそのまま維持する案と、MapLibre用のGeoJSONへ表示データを正規化する案。前者は旧UIを接続しやすいがフロント側に変換規則が残る。今回は新Reactとの並列実装を優先し、**集計値を維持して、事故点と高リスク区間をFeatureCollectionにする契約**を採用する。この選択は本仕様の設計判断であり、旧UIの完全互換を主張しない。

## 2. ディレクトリ案と依存境界

以下は将来の構成案で、今回作成するのはdocsの4文書だけ。

```text
app/                         現行Python参照実装。変更しない
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
    releases/<dataset-id>/   accidents.csv.gz、metadata.json、syukujitsu.csv（前処理に使った版）
    current                  公開中releaseへの参照（Web配信禁止）
frontend/
  package.json / lockfile
  src/
    api/generated/           OpenAPI由来型。手修正禁止
    api/                     HTTPラッパー
    components/              スタイルを持たない最小構成
    state/                   入力・出発日時・条件の自動/手動・結果・世代管理
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

backendはfrontendのソースを読まず、ビルド済みdistを設定されたパスから配信するだけ。frontendはbackendのRust型やデータCSVを直接importしない。API層はドメイン型とDTOを分離し、GeoJSON包装やHTTPの都合を空間集計へ持ち込まない。候補の数値・順序を決めるロジックはbackend一か所にする。

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
2. CIでOpenAPI 3.1文法・全参照を検査し、各共通fixtureをJSON Schema Draft 2020-12で検証する。HTTP status/code、ID参照・個数一致・条件緩和順等の相関制約は別の契約試験で検査する。
3. RustでfixtureをDTOへdeserialize/serializeして再検証する。レスポンスの必須値欠落、null/省略の違いを検出する。Axum extractorの既定エラーもJSON形式へ変換されることを確認する。
4. frontendで型を再生成して差分ゼロを確認し、型チェックとfixture利用の操作試験を実施する。生成型だけでは実応答を保証しないので、API境界ではJSON形状とID参照の検査を行う。
5. 固定外部応答でPython/Rustの同等性試験を通し、その後同一契約のReact/Axumを結合する。旧Pythonから新DTOへの包装差分をテスト側で正規化する。

必須フィールド削除・型変更・意味変更は契約版更新が必要。追加フィールドも本OpenAPIのadditionalProperties制約に従い、仕様更新→生成・検査→協調リリースとする。時間帯の5区分化（periodの整数3と「夜」の意味の変更）、day_typeの追加、effective_conditionsへの必須項目追加は意味変更なので、api_versionを2.0に上げた。frontendは2.0以外のapi_versionを受けたら、古いUIとAPIの混在として扱う。将来の本格的なGPS APIはv1の上位100区間の意味を変えず別契約にする。

## 4. 開発・配信

開発時はbackendを127.0.0.1:8000、Viteを127.0.0.1:5173で起動する。Viteの `server.proxy` は `/api` をbackendへ転送し、パスを削除しない。つまり `/api/route` は同じ `/api/route` へ届く。API_BASE相当は常に相対パスで、フロントにOSRM/Nominatim URLを埋め込まない。[Vite公式proxy仕様](https://vite.dev/config/server-options.html#server-proxy) を参照。

strictPortを有効にして勝手なポート移動を避ける。localhost利用だけならCORS不要。別オリジン運用は初期版の対象外で、必要時に許可originを限定して追加設計する。LAN端末でGPS/PWAを検証する場合は信頼されたHTTPSを使う。

本番はHTTPSリバースプロキシ→AxumでAPIを配信し、静的distをAxumまたは同一オリジンの配信層から提供する。どちらもOpenAPIの配信契約を満たす。`/api/*` を先に解決し、未知APIへHTMLを返さない。static root外、dotfiles、ディレクトリ一覧、ソース/原本/データへのアクセスを禁止する。

HTML/manifest/sw.jsはno-cache（再検証可能）、内容ハッシュ付きassetsは長期immutable、APIはすべてno-store。画面は `/` のみ、未知パスは404。MapLibre style/tilesの配信元と帰属は別途設定する。APIクライアントは45秒で打切り、サーバのroute/geocodeは受付から応答JSON準備完了まで最大35秒の要求全体期限を設ける（キュー待ち・集計・シリアライズを含み、クライアントへの転送時間は含まない）。全体期限切れは処理段階によらず503 service_busyで終了し、既存候補があっても200を返さない。この制御は追加候補のAPIError捕捉より外側に置く。全体期限内の個別上流通信の8秒期限切れは502 external_api_errorで、追加候補だけの失敗ならbehavior通り警告にして集計を続けられる。待ち行列満杯は503 service_busy。未完了の処理をキャンセルし、CPU集計も期限を確認して停止できるようにし、無制限に積み上げない。

`/api/conditions` はクライアント10秒、サーバ全体8秒、気象提供元への1通信5秒とする。サーバ全体の期限はクライアントより短くし、期限内に天候が得られなければ天候だけunavailableとして200を返す。気象提供元の失敗・待ち行列満杯はエラー応答にせず、時間帯・曜日区分は常に返す。

## 5. データ配置と更新

原本はaccident/を読み取り専用マウント、前処理結果はbackend/data配下へ書く。初回移行では検証済み `app/data/accidents.csv.gz` とmetadataを**コピーして**使用できる。app側への出力・再生成はしない。長期的にはbackendの前処理コマンドがbehaviorと同じ変換を担当する。

更新手順：

1. 原本7年度とコード表を揃え、出所・ハッシュ・source_yearを監査する。未知年度の追加はコード表差分と除外方針を確認してから仕様の年度集合を更新する。現行原本を作業中に書き換えない。祝日表は内閣府の最新CSVを取得し、出所・取得日・SHA-256を記録する。前版との差分（追加年・変更日）を確認する。
2. backend/dataの非公開stagingディレクトリで前処理。入力件数=保持+除外、除外理由、modes=0、コード表照合、17列形式、非空、period 0〜4・day_type 0〜1、祝日表の収録範囲外の行数を検査する。Rust移行時はbehaviorの互換範囲でPythonとCSV内容を比較する。
3. gzipのSHA-256をdataset_idとし、metadataにその値と処理版を移行追加の監査項目として記録する。前処理日時/圧縮メタデータ差でIDが変わり得るため、意味的同等性は復号後のデータでも確認する。
4. 検証済みのgzip・metadata・前処理に使った祝日表を同じreleases/<dataset-id>に配置し、不変・読み取り専用にする。途中ファイルをcurrentへ置かない。`/api/conditions` もこのreleaseの祝日表を読むので、事故記録の曜日区分と自動判定の祝日表は常に一致する。
5. current参照を原子的に切り替えてプロセスを再起動、または新インスタンスで全件ロード後にトラフィックを切替。初期版ではホットリロードを実装しない。プロセスは起動時にcurrentの実パスを1回解決して同じスナップショットから2ファイルを読む。
6. `/api/health` のdataset_id/accidents/data_yearsと固定検索fixtureを確認。失敗時は旧currentへ戻して再起動する。途中でmetadataだけ新しくなる状態を公開しない。

一つの検索要求中は一つのデータスナップショットを固定する。公開事故データの保存と、利用者の現在地・検索履歴の無保存は別の対象である。

祝日表は毎年2月ごろ内閣府が翌年分を公開する。収録範囲外の日付は土日だけの判定になり、holiday_table_covered=falseとして利用者へ警告される。年1回、祝日表の更新とデータの再リリースを運用予定に入れる。祝日表だけの更新でも前処理をやり直し、新しいdataset_idとして公開する。

## 6. 環境変数の移行契約

以下は新設する設定名。現行はCLIの--host/--port/--data以外の設定はなく、上流URL/UA等はハードコードされている。単位を名前に含める。起動時に型・範囲・必須値を検査して誤設定なら停止する。

| 変数 | 既定/必須 | 意味 |
|---|---|---|
| BACKEND_BIND | 127.0.0.1:8000 | API待受け |
| ACCIDENT_DATA_DIR | backend/data/current（repo root基準） | gzip/metadataを含むディレクトリ。起動時resolve |
| FRONTEND_DIST_DIR | frontend/dist（repo root基準） | 静的root。API専用開発起動では未生成でも可 |
| ROUTING_PROVIDER | osrm | 初期対応アダプタ識別子。未対応値は起動拒否 |
| OSRM_WALK_BASE_URL | 必須 | `.../routed-foot` 相当。ここへ `/route/v1/walking/...` を付加 |
| OSRM_BIKE_BASE_URL | 必須 | `.../routed-bike` 相当。cyclingを使用 |
| OSRM_CAR_BASE_URL | 必須 | `.../routed-car` 相当。drivingを使用 |
| GEOCODING_PROVIDER | nominatim | Nominatim互換アダプタ。未対応値は起動拒否 |
| NOMINATIM_BASE_URL | 必須 | `/search` を付加するベースURL |
| EXTERNAL_USER_AGENT | 必須 | アプリ識別と運用連絡先を含むUA。秘密値を含めない |
| ROUTING_ATTRIBUTION | 必須 | ページの経路帰属表示に渡すビルド/配備設定と一致させる |
| GEOCODING_ATTRIBUTION | 必須 | geocode応答のattribution |
| UPSTREAM_TIMEOUT_MS | 8000 | 1通信の期限。結果同等性試験では既定固定 |
| UPSTREAM_MAX_BYTES | 12000000 | 上流応答最大バイト数。既定固定 |
| ROUTING_MIN_INTERVAL_MS | 1050 | 同一提供元の通信開始最短間隔。公共OSRMでは短縮不可 |
| GEOCODING_MIN_INTERVAL_MS | 1050 | 全利用者共通の開始最短間隔。公共Nominatimでは短縮不可 |
| UPSTREAM_QUEUE_CAPACITY | 16 | 提供元別の待ち通信上限。満杯はservice_busy |
| REQUEST_TIMEOUT_MS | 35000 | queue待ち含むAPI要求全体の上限 |
| GEOCODE_CACHE_MODE | off | offまたはprototype-memory。後者は互換検証用の最大256キー/24hメモリのみ |
| WEATHER_PROVIDER | open-meteo | 初期対応の気象アダプタ識別子。未対応値は起動拒否 |
| OPEN_METEO_BASE_URL | 必須 | `/v1/forecast` を付加するベースURL。無料版と有償版で接続先が異なる |
| OPEN_METEO_API_KEY_FILE | 任意 | 有償プランのAPIキーを置いたファイルのパス。サーバ専用のsecret。VITE_*に置かない |
| WEATHER_TIMEOUT_MS | 5000 | 気象提供元への1通信の期限 |
| WEATHER_FORECAST_DAYS | 7 | 予報を取得する日数（1〜16）。これより先の日時はout_of_range |
| WEATHER_CACHE_TTL_SEC | 600 | 丸めた座標ごとの気象応答のメモリキャッシュ期間 |
| WEATHER_MIN_INTERVAL_MS | 200 | 気象提供元への全利用者共通の通信開始最短間隔 |
| CONDITIONS_TIMEOUT_MS | 8000 | `/api/conditions` の要求全体の上限。クライアントの10秒より短くする |
| RUST_LOG | info | URL/query/body/位置を含まないログレベル |
| DEV_API_PROXY_TARGET | http://127.0.0.1:8000 | Vite開発プロキシ専用。ブラウザbundleへ公開しない |
| VITE_MAP_STYLE_URL | 必須 | 選択したMapLibre styleの公開URL。秘密を含めない |
| VITE_ROUTING_ATTRIBUTION | 必須 | ページで表示する経路帰属。配備時にbackend設定と同じ提供元にする |
| VITE_MAP_ATTRIBUTION | 必須 | 地図提供元の帰属。style内のsource attributionと整合させる |

パスは本番では絶対パスを指定し、cwd依存を避ける。秘密のAPIキーが必要な提供元へ拡張するときはサーバ専用のsecret参照を導入し、VITE_*へ置かない。VITE_*は公開値として扱う。mode/period/day_type/weather、時間帯の境界、天候コードの変換表、半径、重み、候補数、30km/80km等の互換性定数は初期版では環境変数化しない。祝日表はACCIDENT_DATA_DIRのreleaseから読み、別の環境変数を設けない。気象・祝日の帰属表示は提供元が固定なので、frontendのDataAttributionに直接書く。

## 7. 外部サービスと商用運用

2026-10-24に以下の公式方針を参照した。OSSライセンスと公開サーバの利用条件は別であり、「OSRM/NominatimがOSSだから公共APIも無制限に商用利用できる」と解釈しない。法務・契約の最終判断と配備時の再確認は**要確認**。

- **Nominatim公共APIの制限**：全利用者合計で最大1request/秒、識別UA/Referer、帰属、明示検索、切替可能な接続先が必要。入力補完や系統的収集は禁止。キャッシュ利用を求め、反復要求が遮断される可能性がある。個人情報・機密を送らない。商用利用の一律禁止ではないが、容量制約・利用停止リスクがあり、商用向けの稼働保証ではない。一般的なジオコーディングサービスとしての提供や追跡用途には追加の禁止事項がある。採用前に[公式利用方針](https://operations.osmfoundation.org/policies/nominatim/)を確認する。
- **現行のFOSSGIS OSRM公共サーバ**：最大1request/秒、識別UA/適切なReferer、帰属と地図修正リンク、heavy usage/scraping禁止。経路要求をサーバログへ保存する旨が記載されている。[提供元の説明・利用制限](https://routing.openstreetmap.de/about.html)。この条件は `router.project-osrm.org` 等の別サーバに一般化しない。リンク先の[ドイツ語利用規約全文](https://www.fossgis.de/arbeitsgruppen/osm-server/nutzungsbedingungen/)は取得拒否により今回未確認。商用SLA/許容負荷と全文は配備前に確認し、本番の既定基盤にしない。
- **Open-Meteo**：無料APIは非商用利用に限られ、APIキーは不要。1日・1時間・1分あたりの呼出し上限があり、データはCC BY 4.0で帰属表示が必要。商用利用は有償プラン（APIキー付きの別エンドポイント）が必要。上限値と規約は[公式利用規約](https://open-meteo.com/en/terms)と[料金](https://open-meteo.com/en/pricing)で配備前に再確認する（**要確認**）。気象モデルの推定値であり、事故記録の天候（現場判定）とは尺度が異なる。
- **内閣府「国民の祝日」CSV**：[国民の祝日について](https://www8.cao.go.jp/chosei/shukujitsu/gaiyou.html)で公開されているCSVを取得し、リポジトリ外の配布物として同梱する。実行時には取得しない。振替休日と国民の休日はどちらも名称「休日」と記載されるため、区別はbehaviorの規則で行う。
- MapLibreは描画ライブラリであり、タイルサービスの利用権を提供しない。背景地図、スタイル、フォント等の商用利用・帰属・オフライン保存条件は採用提供元ごとに確認する。公共タイルの無断一括ダウンロードをPWA要件に含めない。

**採用方針**：本番の上流URLに公共サーバを既定値として入れない。自己ホスト、または用途・負荷・位置の取扱いを契約できる提供元のURLを必須設定する。公共エンドポイントは利用条件を確認した限定検証時にだけ明示指定する。利用者の住所を24時間保持する現行キャッシュと無保存方針が衝突するため標準はoff、公共Nominatimでの検証はprototype-memoryを明示指定し、個人住所ではなく公開地点だけを使う。本番の無保存方針を満たすか確認できない提供元は採用しない。

weatherアダプタは丸めた緯度経度を受け、現況のWMO天気コードと時刻、毎時予報の時刻とコードの列を返す。WMOコードから晴/曇/雨/霧/雪への変換はアダプタの外（domain）に置き、提供元を替えても変換表を共通にする。提供元が別のコード体系を返す場合は、アダプタ内でWMOコードへ変換する。

routingアダプタはorigin/dest/mode/optional viaを受け、順序付きduration秒/distance m/LineStringを返す。候補生成・重複排除・事故集計はその外側に置く。geocodingアダプタはqを受け、lat/lon/labelとattributionを返す。認証、URL組立て、provider独自エラーはアダプタ内で共通APIErrorへ変換する。日本語HTTPエラーに上流URLを埋め込まない。

接続先設定の変更で自己ホストや同互換サービスへ替えられるようにする。異なるプロトコルの提供元は新アダプタを実装するが、フロント契約は維持する。徒歩/自転車プロファイルを提供できないサービスへ替えたとき、drivingへ黙って代替しない。提供元が変わると候補geometry/時間が変わるため、Pythonとの数値互換は**同じ固定上流応答の場合**の保証に限定する。

提供元別に全利用者共通の直列化・通信開始間隔・有限待ち行列を設ける。複数インスタンスの場合、公共提供元へ出る単一の制御プロキシに集約するか共有レート制御を設ける。各プロセスで1秒あけるだけでは不十分。初期版は単一インスタンスとし、スケール時にこの境界を維持する。OSRM追加候補の要求順は現行と同じで、最大3呼出しを並列化しない。

## 8. プライバシー

利用者の位置（GPS/手入力/地図指定）、住所検索語、出発地/目的地、経路、検索履歴を永続保存しない。アカウント、位置DB、追跡イベント、リクエスト本文の計測基盤送信は初期版に設けない。公開事故原本と前処理結果は保持してよい。

ブラウザはページのメモリだけ、サーバは要求処理中の一時保持だけを標準とする。localStorage/IndexedDB/SW/API cache/URL query・hashへルート状態・出発日時・条件取得結果を保存しない。GPS取得だけではサーバ送信しない。サーバへ座標を送るのは、両地点の確定後の条件取得（2地点の中点を小数2桁、約1kmに丸めた1点）と、明示的な経路検索だけである。

例外として、気象応答のキャッシュは丸めた座標（約1km格子）をキーにしてプロセスメモリに最大10分保持する。利用者・要求との対応付けは持たず、永続化しない。再起動で消える。気象提供元へは丸めた座標だけを送り、元の出発地・目的地は送らない。ブラウザ自身やOSの許可履歴までアプリが消去できるという保証はしない。

アクセスログはメソッド、定義されたパステンプレート、status、処理時間など位置を含まない項目に限定する。`/api/geocode?q=...` と `/api/conditions?...&lat=..&lon=..` のraw URI、OSRMとOpen-Meteoの座標入りURL、body、Refererの機微値、エラー内のURLを、Axum・HTTPクライアント・リバースプロキシ・APMすべてで記録しない。メトリクスlabelにも含めない。ページのReferrer-Policyはno-referrerを基本とし、提供元識別にはUAを使用する。

外部ルーティングには座標、住所検索にはq、気象提供元には丸めた座標、地図提供元には表示範囲が伝わり得る。アプリ内で保存しなくても提供元のログは別である。送信先と用途を説明し、第三者を含めた無保存が必要なら自主管理の基盤か保存しない契約の提供元で満たす。公共OSRMの位置ログ保存と「位置情報を保存しない」の厳密な方針は両立しないため、この点を未解決のまま商用公開しない。

## 9. 並列実装と結合の完了条件

backend担当はbehaviorの24テスト対応・追加境界試験・OpenAPI応答検証を行う。frontend担当は同じOpenAPIから生成した型と共通fixtureで実装し、UI側に独自の事故リスク計算を作らない。先に通常・同一・緩和（曜日区分を含む）・0件・間引き・HTTP失敗のfixtureを共有して、その後は互いの実装待ちなしに進める。`/api/conditions` も、ok（現況/予報）・no_location・past・out_of_range・unavailable・祝日表範囲外のfixtureを先に共有する。

結合では座標順/単位、選定ID、GeoJSONの直接描画、事故点のID重複排除、区間の元頂点対応、null削減率、エラー/ローディング、healthデータ世代、API no-storeを確認する。条件については、conditionsの結果がそのままrouteのperiod/day_type/weatherへ渡ること、手動値が上書きされないこと、送信座標が丸めた中点だけであることを確認する。実上流の疎通・実道路妥当性、実際の気象応答、スマートフォンGPS、HTTPS、PWA更新/オフライン、提供元条件と無保存設定は別の受け入れ項目であり、単体テスト成功で代用しない。

最終的に要確認なのは商用提供元/タイル契約、Open-Meteoの商用プラン、年末年始と天候変換表の扱い、対応端末の範囲、将来のGPS警告の意味、統計的な重み妥当性。これらはv1の集計・JSON・UI操作を実装する際の未決定分岐にはしない。本書の暫定契約で実装し、公開前に運用側の確認を完了する。
