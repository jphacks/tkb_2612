# API・経路決定以外の処理・運用の仕様

2026-10-24。独立して確定できるバックエンドの契約をまとめる。**「利用者合意」は機能の意味と利用方針、「委任判断」はGPT-6.1 Solが具体化した技術上の選択**を表す。仕様整理のみで、実装・契約・配備・課金操作は行っていない。

根拠は[最新の決定記録](spec-decisions.md)、当初依頼、利用者が確定扱いにした[PR #1のhead `ac64e525a758f47bdb61399827bd4619f79e968c`](https://github.com/jphacks/tkb_2612/tree/ac64e525a758f47bdb61399827bd4619f79e968c/docs)（base `3a2fd9b59a477323f6553c6709fd9a3dea5a3816`）、[委任判断の理由](backend-delegated-decisions.md)。不一致は最新の利用者合意を優先する。古いローカルOpenAPI・behavior・architectureやデモの実装だけを採用済みの根拠にしない。

## 1. 利用者合意と対象範囲

- Rust/Axumをクラウドで動かし、React/TypeScript/Vite/MapLibre GL JSの画面と連携する。警察庁の事故原本を使い、徒歩・自転車・自動車の経路を比較する。一般ユーザー向けには公開しない。審査員各自端末での操作は初期の必須条件にしない。
- 時間帯・曜日区分・移動手段は事故の対象条件として維持する。**天候は除外条件にせず、一致する事故を評価で重く扱う。** 全天候と指定天候一致の件数/死傷者数を分け、0件でも条件を自動解除しない。死傷者は該当事故全体の人数。
- 外部の経路/住所提供元による座標/住所の運用ログは許容する。提供元まで含めた完全な無保存を要件にしない。利用規約の免除や、自アプリでの位置/検索履歴の永続保存機能の追加には拡張しない。
- 経路探索の式・係数・候補生成・周辺事故の空間判定・選定・区間抽出と、フロントエンド実装は別検証の対象。本書はそれらの旧案を無条件には確定しない。

## 2. APIの共通契約【委任判断】

| API | 契約 |
|---|---|
| POST `/api/route` | 条件を検証し、別検証で確定する経路処理へ渡す。候補geometry、集計値、比較対象ID、追加時間、説明、warningsを返す。 |
| GET `/api/geocode?q=...` | 明示操作による住所/地点検索。trim後Unicode2〜200文字、qは1個。提供元順で最大5件の`{lat,lon,label}`とattributionを返す。0件は200/空配列。 |
| GET `/api/conditions` | 出発日時と任意の地域座標から時間帯・曜日区分・天候候補を返す。事故データを参照せず、気象失敗でも時間帯・曜日区分を200で返す。 |
| GET `/api/health` | ロード済みデータの準備状態、`api_version / dataset_id / accidents / data_years / load_seconds`を返す。grid_cellsはグリッド索引を採用した場合だけ追加し、このフィールドから索引方式を強制しない。上流へ疎通しない。`/api/status`は同じ応答の非推奨エイリアス。 |

統合契約版は`api_version: "2.0"`。新フロントは日本語条件値と`all`を送る。routeの必須入力は`origin/dest={lat,lon}`。`mode`省略はwalk、`period/day_type/weather`省略はall、`max_detour_min`省略は10分。PR #1のperiod/weather整数・null、`hour`・`時間帯`の互換入力は入口で正規化し、優先順も同PRに従う。day_typeは整数を受理せず、routeへ出発日時を追加しない。

入力はUTF-8のJSONオブジェクト、`application/json`（charset付き可）。有限数値だけを受理し、数値文字列/boolを数値へ変換しない。整数値の12.0表記は受理する。JSON重複キーと既知GETキー重複は400、未知キーは無視。本文は1〜16,384バイト。不正入力は外部呼出し前に400へ統一する。

GeoJSONはWGS84の`[lon,lat]`、入力/住所候補は名前付きlat/lon。緯度20〜46・経度122〜154、標高なし。所要時間は秒、経路距離はm、追加時間/許容時間は分。PR #1の直線距離20m〜30km、許容分0〜30、候補80km/15,000頂点は初期の保護値として使う。距離判定とアルゴリズム側の接続が確定した時点で共同検証し、値が利用者指定だったとは扱わない。

## 3. 事故集計の意味と返却【利用者合意を委任判断で表現】

各候補について、移動手段・時間帯・曜日区分と、別検証で確定する経路周辺条件を満たす事故の集合をBとする。天候ではBを減らさない。

| 候補内のフィールド | 意味 |
|---|---|
| `accident_count` | Bの事故件数。表示点の間引き前。 |
| `fatal_accidents / injury_accidents` | Bのうち死者1人以上の事故数/それ以外の人身事故数。合計はaccident_count。 |
| `deaths / injuries` | Bの事故全体の死者/負傷者合計。死亡事故内の負傷者も含む。 |
| `weather_match` | 天候指定時は上記5フィールドを持つ一致事故だけの集計オブジェクト。該当なしはすべて0。天候all時はnull。常にキーを返す。 |
| `accident_points / points_shown` | Bから抽出したPoint FeatureCollectionと表示件数。集計値を変更しない。 |
| `exact_condition_count` | 非推奨の互換値。天候指定時はweather_match.accident_count、all時はaccident_count。 |

件数/人数は非負整数、内部加算はu64。JSONではJavaScriptの安全な整数範囲を検証する。`effective_conditions`は正規化した指定条件を表し、`relaxed_conditions`は常に`[]`。天候一致0件や全候補0件を理由に他条件を変えない。事故0件を安全の証明として扱わない。

事故点は最大250点。死亡事故を優先し、各群の元順から等間隔に抽出する。GeoJSONのID・発生日・source_year・時間帯・天候・day_type・事故全体の人数を返し、`points_shown == features.length`を満たす。事故点がない場合は空FeatureCollection。事故IDは`(dataset_id, feature.id)`で識別する。

**選定スコア、削減率、高リスク区間の式/型/閾値はBやweather_matchの生集計と別契約**。別検証の結論は[route-scoring-decision.md](route-scoring-decision.md)で暫定決定し、`api/behavior.md` 6〜8章へ統合した（Bの経路周辺条件は72m以内）。フロントで集計値から独自にスコアを再計算しない。経路ID/区間IDは当該検索応答内だけで有効とする。

## 4. 時間帯・曜日・気象【PR #1合意と委任判断】

| 時間帯 | Asia/Tokyoの時刻 |
|---|---|
| 早朝 / 昼 / 夕 / 夜 / 深夜 | 4:00〜8:59 / 9:00〜15:59 / 16:00〜18:59 / 19:00〜23:59 / 0:00〜3:59 |

休日は土日・国民の祝日・振替休日・国民の休日。深夜もその暦日で判定する。前処理とconditionsで同じ関数と同梱の内閣府祝日CSVを使う。年末年始/お盆/企業休業日の追加特例はなく、元日や土日を平日へ変えない。祝日表のハッシュ・収録年を記録し、範囲外は土日だけで判定して`holiday_table_covered=false`を返す。

conditionsの`datetime`は必須1個、`YYYY-MM-DDThh:mm[:ss]`にZまたは±hh:mmを付ける。Asia/Tokyoへ変換して秒以下を切り捨て、2000-01-01T00:00〜2100-12-31T23:59の分単位を扱う。lat/lonは両方指定または両方省略。両地点の中点をフロントで小数2桁へ丸め、サーバも再丸めして気象提供元へ送る。routeにはこの日時を送らず、確定した条件値を送る。

Open-Meteo Forecast APIへ`current=weather_code / hourly=weather_code / timezone=Asia/Tokyo / forecast_days=7`を指定する。現在±15分は現況、15分超先は対象時刻を時単位に切り捨てた予報、15分超過去はpast。座標なしはno_location、予報範囲外はout_of_range、通信/形式/未知コード/混雑はunavailable。成功だけokとし、失敗時はweather/weather_sourceをnullにする。応答の時刻/UTCオフセットと配列長を検証する。

WMO変換はPR #1の対応表を自動初期値用の近似として採用する（0/1/2→晴、3→曇、45/48→霧、雨・霧雨・雷雨→雨、雪系→雪、未知→unavailable）。現場の事故天候判定と同じ精度は保証しない。手動値を自動取得で上書きせず、出発日時変更時は自動へ戻す。現在モードは最後の取得から15分以上で経路検索前に再取得する。[気象コード・API](https://open-meteo.com/en/docs)

## 5. 原本・前処理・ロード【利用者合意を委任判断で具体化】

- `honhyo_2019.csv`〜`honhyo_2025.csv`の7年を年昇順/原本行順で扱い、原本を変更しない。UTF-8 BOM/UTF-8/cp932とCSVの引用/改行を扱う。空白正規化した列名で参照し、必須列欠損/重複は全体失敗。
- 座標/日時の欠損・不正はその行を除外して続行する。人数/必須分類の型不正・年度コード表外も補完せず理由別に除外する。読込不能、CSV構文、途中復号、必須ファイル欠損、コード表不一致、全行除外は全体失敗。一部年度だけで成功扱いにしない。理由の優先順は読込・必須列→座標→日時→人数→分類値。
- DMSから十進度へ変換し、保存時だけ緯経度を小数7桁にする。source_yearと実発生年を区別する。当事者A/Bのいずれかが対象手段なら該当とし、二輪/原付/特定小型原付43を自転車へ混ぜない。年度別コード表で対応を照合し、対象手段なしのmodes=0は保持して別計数する。道路スナップや真の事故IDを推測した重複行削除は行わない。
- 成果物はPR #1の17列UTF-8 gzip CSV（weather直後にday_type）と`schema_version=2`のmetadata。同梱祝日表のハッシュ/収録年、原本ハッシュ、処理版、年度別入力/保持/除外理由を記録する。入力=保持+除外を検証し、複数不正は最初の理由に1回だけ計上する。原本行そのものはWebへ返さない。
- gzipのSHA-256をdataset_id、出力行0始まりを事故IDとする。gzipとmetadataは同じ不変releaseへ置き、起動時にhash/schema/件数/値を検証して全件ロードする。旧16列成果物は流用しない。失敗は起動停止。新イメージのロード確認後に切替え、失敗時は旧イメージへ戻す。ホットリロードは初期対象外。

## 6. 外部提供元・通信制御【委任判断】

**住所検索はGeoapify Forward Geocodingを採用する。** 明示検索をサーバから`https://api.geoapify.com/v1/geocode/search`へ送り、`text=q / lang=ja / filter=countrycode:jp / limit=5 / format=json`を指定する。apiKeyはサーバのsecretに置き、外部JSONのlat/lon/formattedを既存のlat/lon/labelへ変換する。Geoapify/OpenStreetMapの帰属とリンクを返し、画面で表示できるようにする。[住所API](https://apidocs.geoapify.com/docs/geocoding/forward-geocoding/)

初期はGeoapify Freeの枠内での限定利用を設計上の前提にする。公式枠は3,000 credits/日・最大5要求/秒で、単純な住所検索は通常1 credit/要求。アプリの初期制御は1.05秒以上の間隔・1同時通信・2,000要求/日とし、同じアカウントの他利用も含めて配備前に残枠を確認する。上限は自動停止や無料の保証ではなく、規約上超過料金等があり得るため、実使用量を確認し、有料プランへ自動変更しない。[料金](https://www.geoapify.com/pricing/)、[利用条件](https://www.geoapify.com/terms-and-conditions/)

公開Nominatimは初期接続先にしない。全利用者合計1要求/秒、キャッシュ、帰属等に加え、個人情報・機密を送らない制限があるため、住所入力を扱う今回の初期構成には採用しない。外部運用ログ許容だけでこの利用方針が解除されるとは扱わない。[公開Nominatimの利用方針](https://operations.osmfoundation.org/policies/nominatim/)

気象は合意済みのOpen-Meteoを使う。非商用のハッカソン評価/試作として無料エンドポイントを初期選択し、商用へ変える場合は該当プランへ切り替える。気象キャッシュは丸め座標単位、プロセスメモリのみ、成功応答TTL600秒・最大1024地点。住所/経路キャッシュは標準off。Open-Meteoの分/時/日/月の各上限を共有ゲートへ設定し、提供元の最新枠以内に抑える。[気象の利用枠](https://open-meteo.com/en/pricing)

**全インスタンス共通の上流ゲートはFirestore Standardのトランザクションで管理する。** 保存するのは提供元/アカウントの識別子、呼出し開始時刻・期限、同時通信のリース/世代番号と集計件数だけ。住所・座標・経路・利用者ID・リクエストIDを保存しない。全呼出しが同じゲートを通り、開始間隔・同時数・提供元の集計期間別上限を満たしてから送信する。通信をトランザクション内で実行せず、共有状態が読めない/競合が解消しない場合は呼び出さず失敗させる。送信前にリースの期限/世代を確認し、失効した処理は送信せず、古い処理の終了で新しいリースを解放しない。再配備・再起動時も制御が継続することを結合試験する。[原子的更新](https://docs.cloud.google.com/firestore/native/docs/manage-data/transactions)

Firestoreは事故DBではなく少数の運用レコードだけに用い、期間終了時の件数は上書き、終了後は削除する。クライアントからの直接アクセスを許可しない。要求ごとの読書き・競合時の再実行も課金見積もりに含め、無料枠に収まると断定しない。[Firestore料金](https://cloud.google.com/firestore/pricing)

経路提供元・自己ホスト要否はアルゴリズム側の結論を接続する。ルーティングはorigin/dest/mode等を受け、候補geometry・秒・mを返すアダプタ境界に置く。提供元URL・識別UA/連絡先・帰属・制限値はサーバ設定にし、無設定で公共サーバへ流さない。徒歩/自転車を自動車経路へ代替しない。

## 7. 期限・HTTP・静的配信【委任判断】

| 対象 | 初期制御 |
|---|---|
| route / geocode | 受付から応答JSON準備まで35秒（待ち・集計を含む）。1上流通信8秒、上流応答12,000,000バイト、提供元別のローカル待ち行列16。自動再試行なし。全体期限後に部分成功200を返さない。 |
| conditions | 全体8秒・気象通信5秒・上流応答1,000,000バイト。気象失敗/混雑/期限は天候unavailableの200。クライアント期限10秒。 |
| キャンセル | 期限/切断で未着手の上流呼出しを除去し、開始済み通信とCPU集計も停止させる。返答後に無制限に処理を残さない。 |

エラー本文は`{"error":{"code":"…","message":"…"}}`、日本語の説明に住所/座標/秘密/スタックトレースを含めない。400 invalid_request、404 not_found、405 method_not_allowed（Allow必須）、422 route_too_long、502 no_route/external_api_error、503 data_unavailable/service_busy、500 internal_error。上流429は502。健康確認/routeでHTTP稼働中に未readyなら503、conditionsは事故データ未準備を理由に503へしない。Cloud Runでアプリ到達前に起こるIAM拒否/429等は別形式になり得る。

API成功/失敗は`Cache-Control: no-store`・nosniff。APIを先に解決し、未知APIへHTMLを返さない。静的rootはビルド済みdistだけ。`/ /assets/* /manifest.webmanifest /sw.js`を配信し、未知パスは404、一覧/原本/データ/ソース/dotfileを公開しない。HTML/manifest/SWは再検証、ハッシュassetsはimmutable。API/位置/経路/外部応答をPWAの永続キャッシュへ入れない。具体的な画面操作と描画は別検証に従う。

## 8. 配備・アクセス・ログ【委任判断】

Cloud Run東京のリクエスト課金、初期1 vCPU/2 GiB・同時受付4・最大1台、通常最小0台/実演中1台を使う。事故原本約420 MiBはRAM実測ではない。前処理をビルド前に行い、読み取り専用成果物とビルド済み画面を同梱する。実測でCPU/RAM/同時数と起動時間を見直す。詳細な比較・料金・ログ除外フィルタは[cloud-selection.md](cloud-selection.md)。

**初期実演はCloud Run IAMで開発者だけに許可する。** ingressは開発者端末から届くall、認証は必須、`roles/run.invoker`を対象Googleアカウントだけへ与え、allUsersを付けない。ブラウザへのトークン埋込みや独自ログインは作らず、開発者端末で次のローカルプロキシを使う。localhostにだけ公開し、審査員にはその画面を見せる。各自端末への一般配布は初期対象外。[認証付きブラウザ実演](https://docs.cloud.google.com/run/docs/authenticating/developers)

```text
gcloud run services proxy tkb-api --project=PROJECT_ID --region=asia-northeast1 --port=8080
```

開発時はAxum127.0.0.1:8000、Vite127.0.0.1:5173・strictPort・/apiプロキシ。本番Axumは0.0.0.0:$PORT、パスは絶対パス、同一オリジンとCloud Run TLS終端を使う。APIキーはSecret Manager参照等でサーバだけへ渡し、VITE変数へ入れない。必須設定の不足/型不正は起動失敗。事故/位置/検索履歴のDBは作らない。

外部提供元の運用ログ許容は反映済み。GeoapifyもAPI要求の技術情報を保持する旨を公表しており、位置/住所を送る提供元として明示する。アプリ側ログはルート名・status・時間・匿名エラーだけに限定し、Cloud Runの自動URLログは実入力前に除外設定する。他sink/組織転送・HTTPクライアントのエラーURLも確認する。これで提供元の一時処理や運用ログまで消せるとは説明しない。[Geoapifyの保持方針](https://www.geoapify.com/privacy-policy/)

実演前はデータreadyと固定検索、IAM拒否/許可、各上流、共有制御、ログ除外を確認する。終了後に継続するなら最小0へ戻し、終了ならサービス/不要イメージ/運用レコード・秘密の参照を整理して請求を確認する。最小0や予算通知だけを費用上限と扱わない。契約/鍵発行/配備作業は今回行わない。

## 9. PR #1・旧仕様との差分、別検証との接続

下表の反映はPR #1のブランチ上で実施済み（2026-10-24）。経路提供元・必要資源は引き続き経路側の結論を待つ。

| 統合先 | 必要な反映 |
|---|---|
| `api/openapi.yaml` | PR #1のAPI2.0/conditions/day_type/5時間帯を取り込み、全天候集計とweather_match/null、非緩和、住所提供元・帰属、HTTP説明・例を更新する。選定スコアは別検証の定義で確定する。 |
| `api/behavior.md` | 天候hard filterと条件の自動緩和を除き、事故全体の人数、前処理失敗/監査、0件、共有上流制御へ整合する。旧デモ完全互換・旧テスト件数を必須条件にしない。 |
| `architecture.md` | Cloud Run IAM実演・Geoapify・Firestore運用状態・外部運用ログ許容へ更新する。提供元の無保存契約が必須という旧案を除く。アプリ内保存とプラットフォームログは別に扱う。 |
| 契約検証 | schema/DTO/生成TS/fixtureを同時更新し、全天候/一致0/all/null、非緩和、時間/祝日、前処理失敗、期限キャンセル、ID参照、ログとIAMを確認する。実上流・性能は別の受け入れとして記録する。 |

**経路側への依存**は、経路提供元・必要資源・候補取得/生成/選定・空間条件・天候一致を含む評価式・高リスク区間・0件時の選定と説明。**フロント側への依存**は、更新APIの型/fixture、全天候と一致集計の区別、all時null、非緩和、帰属リンク、非JSONのプラットフォーム失敗、IAM実演経路での動作。旧仕様の式やUIを依存先の結論として捏造しない。

独立して確定できる範囲は本書で完了とする。原本/コード表の実検証、完成実装のサイズ・RAM/CPU・起動/応答時間・同時利用、提供元の鍵/枠/日本語住所品質、共有ゲート、実端末、IAMとログの実効性は**未実施の配備前検証**として残す。
