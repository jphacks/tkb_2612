# Rust移植の挙動仕様

## 1. 適用範囲と根拠

2026-10-24のPythonプロトタイプを基準とする。数値・候補順・集計対象の互換性の正本は本書、HTTPとJSONの正本は [openapi.yaml](openapi.yaml)。フロントの責務は [frontend-spec.md](../frontend-spec.md)、配置・運用は [architecture.md](../architecture.md) に定義する。本作業は文書のみで、`app/` と `accident/` は変更しない。

根拠は [server.py](../../app/server.py) の `route_request / collect_candidates / AccidentIndex / Geocoder / Handler`、[common.py](../../app/common.py)、[prepare_data.py](../../app/prepare_data.py)、[test_server.py](../../app/test_server.py)、[test_prepare_data.py](../../app/test_prepare_data.py)、[README.md](../../app/README.md)、[VALIDATION.md](../../app/VALIDATION.md)、[codebook_notes.md](../../app/codebook_notes.md)、[app.js](../../app/static/app.js)。説明文より実コードの条件分岐を優先した。

本文の「現行」は確認したコードの事実、「移行契約」は今回決める新しい境界を指す。「要確認」は原本や運用方針の追加確認が必要な事項であり、並列実装の間は本書の明示した暫定契約に従う。Pythonの偶発的な例外や外部サービスの将来の応答まで再現する契約ではない。

### 互換性と変更点

| 項目           | 現行                                                               | 移行契約                                                                                                                      |
| -------------- | ------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------- |
| 経路・住所検索 | POST `/api/route`、GET `/api/geocode`                              | パス・主要フィールド名・集計値を維持                                                                                          |
| 稼働情報       | GET `/api/status`                                                  | `/api/health` を正規化。`/api/status` は非推奨の同一応答エイリアスとして維持                                                  |
| 時間帯         | 早朝/昼/夕/夜（19〜3時）の4区分。日本語、整数0〜3、all、nullを受理 | **早朝/昼/夕/夜（19〜23時）/深夜（0〜3時）の5区分へ変更**。日本語、整数0〜4、all、nullを受理。新フロントは日本語とallのみ送信 |
| 曜日区分       | なし                                                               | **移行追加**。事故記録とrouteリクエストに平日/休日を追加（2.・4.・7.）                                                        |
| 天候           | 日本語、整数コード、all、nullを受理                                | すべて受理。新フロントは日本語とallのみ送信                                                                                   |
| 条件の自動判定 | なし                                                               | **移行追加**。GET `/api/conditions` で出発日時と地域から時間帯・曜日区分・天候を返す（9.）                                    |
| 事故点         | lat/lon等を持つ配列                                                | `accident_points` をPointのFeatureCollectionに変更。propertiesにday_typeを追加                                                |
| 高リスク区間   | geometry/weighted_riskの配列                                       | `high_risk_sections` をLineStringのFeatureCollectionに変更し、線分参照を追加                                                  |
| 経路geometry   | GeoJSON LineString                                                 | 維持。順序は必ず `[lon, lat]`                                                                                                 |
| バージョン     | なし                                                               | health/route/conditionsに `api_version: "2.0"`、health/routeに `dataset_id` を追加                                            |
| HTTP異常処理   | 一部はhttp.server標準応答                                          | APIの未定義パス・不正メソッド・内部失敗もJSONエラーに統一                                                                     |

この契約に旧 `app/static/app.js` をそのまま接続することはできない。移行フロントは生成型を使う。旧Pythonとの比較はGeoJSONの包装を外して行う。GPS継続追跡・ナビゲーション判定自体は今回の実装対象外で、区間参照だけ準備する。

時間帯の再定義と曜日区分の追加により、Pythonプロトタイプとの完全な数値互換は次の範囲に限定する。period=早朝/昼/夕/all かつ day_type=all のリクエスト、および前処理CSVのperiod/day_type以外の列。旧「夜」（19〜3時）は新「夜」と「深夜」の和に相当するが、単一の新しい値では表せない。旧整数コード3の意味も変わる（旧：19〜3時、新：19〜23時）。互換外の部分は本書の新しいtable test・fixtureで検証する（11.）。

## 2. CSV前処理

### ファイル・文字コード・監査

- `honhyo_2019.csv` から `honhyo_2025.csv` までの7ファイルが必須。順に2019→2025、各CSVの入力行順で出力する。未知年度の自動追加はしない。
- コード表検証を先に行う。2019〜2021は `codebook_YEAR.pdf` の存在だけを自動確認。内容はcodebook_notesの手動照合が根拠。2022〜2025は `codebook_YEAR.xlsx` の内容を毎回照合する。
- 先頭5行をバイト列として完全な行単位で読み、`utf-8-sig` で復号可能ならそれを採用。失敗した場合に `cp932` で復号できることを検査して採用。全年度cp932だったという事実と、UTF-8/BOMも受理する規則を混同しない。先頭5行後に発生する復号エラーはファイル処理失敗であり、行の除外扱いではない。
- CSVパーサで引用・改行を解釈する。ヘッダは `"".join(value.split())` 相当で全空白を除去する。位置固定にしない。正規化後の同名列は後勝ち（現行実装に合わせる）。値は前後の空白のみ除去。
- 必須列は地点緯度（北緯）、地点経度（東経）、天候、昼夜、事故類型、死者数、負傷者数、発生日時の年/月/日/時/分、当事者種別と人身損傷程度の当事者A/B。必須列欠損は処理全体を失敗させる。
- `convert_record` の `ValueError / IndexError` 相当は行を除外し、input/excluded/retainedと理由別件数を記録する。その他のI/Oやコード表エラーは停止。推測による補完、日時の繰り上げ、未知コードの既存分類への置換はしない。
- 出力はUTF-8 gzip CSV（現行圧縮レベル3）。列順は次の17列と完全一致させる。現行Pythonの16列に、移行追加の `day_type` をweatherの直後へ挿入した形である。

`lat,lon,modes,period,weather,day_type,deaths,injuries,source_year,date,hour,minute,daynight,accident_type,injury_a,injury_b,severity`

- 緯度経度は小数点以下7桁の文字列。`date` は8桁YYYYMMDD、`severity` は死亡/負傷。メタデータは `schema_version=2`、files（各年度の文字コード・件数・除外理由・no_target_mode）、codebooks_xlsx、rows、elapsed_seconds、gzip_bytes、generated_at（タイムゾーン付き日時）を保持する。移行追加として、holiday_table（祝日CSVのSHA-256、収録範囲の最初と最後の日付、行数）と、holiday_uncovered_rows（祝日表の収録範囲外の日付で土日だけで判定した行数）を記録する。
- 現行はgzip一時ファイルをrenameし、その後metadataを書き込むため、2ファイル一括の原子更新ではない。移行時はディレクトリ単位のスナップショット切替を採用する（architecture参照）。gzipバイト列自体の一致は受け入れ条件にしない。

### DMS変換

ASCII数字のみの文字列を整数Nとして解釈し、符号、小数点、全角数字、空文字を拒否する。固定桁数検査はない。

- `D = N // 10,000,000`
- `R = N % 10,000,000`
- `M = R // 100,000`
- `S_ms = R % 100,000`
- 十進度 `D + M/60 + S_ms/3,600,000`

`M < 60`、`S_ms < 60,000`、緯度の `0 < D < 90`、経度の `0 < D < 180` を満たすこと。その後、緯度20〜46、経度122〜154の両端を含む範囲に限定する。座標系はWGS84、北緯・東経。道路・陸地へのスナップや測地系変換はない。

例：430607590→43.1021083333…、1412109599→141.3526663889…。ミリ秒角を保持して計算し、CSV書き出し時だけ7桁に丸める。

### 日時・コード・重大度

- 年月日時分は整数化し、実在する日時であることを検査する。時0〜23、分0〜59。年は `1900 <= 発生年 <= source_year`。ファイル年より前の事故を保持する。タイムゾーン変換は行わない（日本の原データの時刻として扱う）。
- 時間帯は発生時から計算し、元の昼夜コードとは別である。分は区分に使わない（8:59は早朝、9:00は昼）。移行契約で夜を2つに分け、5区分にする。

| 内部コード | API日本語 | 発生時（整数） | 現行Python                |
| ---------: | --------- | -------------- | ------------------------- |
|          0 | 早朝      | 4〜8           | 同じ                      |
|          1 | 昼        | 9〜15          | 同じ                      |
|          2 | 夕        | 16〜18         | 同じ                      |
|          3 | 夜        | 19〜23         | 現行は19〜23および0〜3    |
|          4 | 深夜      | 0〜3           | 現行はコード3（夜）に含む |

- 曜日区分（移行追加）は発生年月日の暦日から計算する。`0=平日, 1=休日`。発生時刻は使わない。深夜0〜3時の事故もその日付の曜日に属する（前日扱いにしない）。判定規則は次節。

- 天候は `1=晴, 2=曇, 3=雨, 4=霧, 5=雪` のみ。4と5を逆にしない。
- 死者・負傷者は各0〜999、合計1以上。死者1以上なら事故全体の `severity=死亡`、それ以外は負傷。死亡の原データ定義は24時間以内（codebook_notesによる）。A/Bの損傷程度は `0=対象外, 1=死亡, 2=負傷, 4=損傷なし` のみ。人数との整合検証は現行にない。
- 昼夜は11/12/13/21/22/23のみ。事故類型は整数変換のみで許容集合の検査なし。両者とA/B損傷程度は出力に保持するが、検索・重み計算に使わない。
- 当事者種別の基本許容集合は `{0,1,2,3,4,5,7,11,12,13,14,17,21,22,23,24,31,32,33,34,35,36,41,42,51,52,59,61,71,72,75,76}`。2024/2025だけ43も許容する（発生年ではなくsource_yearで判定）。
- XLSX照合では当事者の集合全体を検査し、車種の乗用車/貨物車/特殊車と61/51/52の説明先頭を検査。天候1〜5と損傷0/1/2/4の説明先頭を検査する。天候・損傷の余分なコードを `verify_codebooks` 自体は拒否しないが、実データ行では上記集合外を拒否する。実データテストは天候集合の完全一致も確認している。
- XLSXはworkbook.xmlとrelationshipsからシートを引く。天候/当事者種別/人身損傷程度の3シートのB列が数字の行をコード、C列を説明とする。sharedStringsの単純テキスト・rich textを扱う。Rustで一般的なXLSXライブラリを使っても、照合内容は同じにする。

### 移動手段

A/B双方の種別からbitwise ORで分類する。徒歩=bit1（値1、61）、自転車=bit2（値2、51/52）、自動車=bit4（値4、1/2/3/4/5/7/11/12/13/14/17/21/22/23/24）。例：61×3→5、51×52→2、3×52→6。

31〜36の二輪、41/42、59、71/72、75/76、0は分類対象外。2024追加の43（特定小型原付）も自転車に含めない。36の一般原付への改称、61の説明追記は分類を変えない。`modes=0` の行も保持し、no_target_modeに計上するが検索には出ない。死傷者数は該当手段の当事者に限定せず、事故全体の人数を使用する。

### 曜日区分と祝日表（移行追加）

祝日表は内閣府「国民の祝日」のCSV（`syukujitsu.csv`、1列目が日付、2列目が名称）を使う。backendの配布物に同梱し、前処理と `/api/conditions` で**同じファイル**を読む。実行時にネットワークから取得しない。

- 文字コードはCSV前処理と同じ判定（`utf-8-sig`、失敗時cp932）。ヘッダ1行を読み飛ばす。日付は `YYYY/M/D`（ゼロ埋めの有無を問わない）。実在しない日付・重複日付・名称の空欄があれば、読込を失敗させる。
- 収録範囲は表中の最小日付の年の1月1日から最大日付の年の12月31日までとする。範囲外の日付は土日だけで判定し、`holiday_table_covered=false` とする（前処理ではholiday_uncovered_rowsに計上する。行は除外しない）。
- 休日は次のいずれかに当たる日。優先順位も次の順とし、day_type_reasonはこの順で最初に当たったものにする。
  1. 祝日表にあり、名称が「休日」以外：`祝日：<名称>`
  2. 祝日表にあり、名称が「休日」：直前の日から日曜日まで連続して祝日表にあり、その日曜日自体も祝日表の日なら `振替休日`。それ以外は `国民の休日`。内閣府CSVでは振替休日と国民の休日がどちらも「休日」と記載されるため、この規則で区別する。
  3. 土曜日：`土曜`
  4. 日曜日：`日曜`
- 上記に当たらない日は平日で、reasonは `平日`。
- 年末年始（12/29〜1/3）は祝日表にないため平日とする（**要確認**）。お盆・企業の休業日も扱わない。
- 曜日はグレゴリオ暦で計算する。祝日表の更新（翌年分の追加）はデータ更新手順の一部とする（architecture参照）。祝日表を差し替えると前処理結果のday_typeが変わり得るため、dataset_idも変わる。

## 3. 起動時ロードと識別子

gzip CSVヘッダは17列の完全一致を要求する。現行は先頭9列を数値化して索引に使い、残り7列を再検証しない。移行契約では先頭10列（day_typeまで含む）を数値化し、残り7列は再検証しない。空データは起動失敗。列ごとの型は緯度経度f64、modes/period/weather/day_type u8、deaths/injuries/year u16、date u32。period 0〜4、day_type 0〜1の範囲外は起動失敗とする。整数和はRustでは十分な幅（u64等）で行いオーバーフローさせない。

行番号0始まりを事故IDにし、`grid[(floor(lat*1000), floor(lon*1000))]` に入力順で登録する。セル幅は0.001度。yearsは読み込んだsource_year集合。実発生年とは別である。ロード時間は計測値であり結果同等性の対象外。

移行契約の `dataset_id` は配信中の `accidents.csv.gz` 全バイトのSHA-256小文字hex（64文字）。再圧縮でも変わり得る。同じ内容の事故でも別スナップショットではIDは保証しない。事故参照は `(dataset_id, feature.id)`、経路/区間参照はさらに**その検索応答**に限定する。検索ごとのroute-1等を永続IDと解釈しない。

## 4. リクエスト検証

HTTP JSONはUTF-8、有限数値だけ。`true/false` を数値として受け入れない。bodyは1〜16,384バイト、超過も現行同様400。Axumの既定413/415/422をそのまま露出させずOpenAPIの形式に変換する。移行契約ではContent-Typeはapplication/json（charset付き可）を要求し、異なる場合400。現行はContent-Type自体を検査しない。

検証順はJSONオブジェクト→origin/dest→直線距離→mode→max_detour_min→period→hour→weather→day_type。最初の不正で400を返し、外部サービスを呼ばない。未知のJSONキーは現行同様無視する。

- origin/destは必須の `{lat,lon}`。数値文字列は不可。緯度20〜46、経度122〜154（両端含む）。
- 地球半径 `R=6,371,008.8m`、`K=πR/180`（1度あたりのm）。直線距離は `K * hypot(lat1-lat2, (lon1-lon2)*cos((lat1+lat2)/2 * π/180))`。測地線距離に置換しない。`20 <= d <= 30,000m`。同一点は拒否。
- modeはwalk/bike/car、省略walk。null不可。
- max_detour_minは有限数値0〜30分、省略10。小数可、null不可。
- periodは早朝/昼/夕/夜/深夜、整数0〜4（2.の新しい内部コード）、all、null。省略all。キー `period` が存在すれば `時間帯` より優先（nullでも優先）。`時間帯` は互換エイリアス。旧Pythonクライアントが送る整数3は新しい意味（19〜23時）で解釈し、0〜3時を含めない。
- hourは省略/nullなら無視。指定時は整数0〜23、該当する時間帯全体へ変換（0〜3は深夜、19〜23は夜）。periodがall/nullならhourで具体化、具体値と矛盾すれば400。hourがあっても先に不正periodを検査する。12.1やbool不可。
- weatherは晴/曇/雨/霧/雪、整数1〜5、all、null。省略all。
- day_type（移行追加）は平日/休日、all、null。省略all。整数コードやエイリアスは受理しない。曜日区分の判定に必要な日時はrouteでは受け取らない。出発日時からの判定は `/api/conditions` の責務である。
- **移行契約の差分**：JSON Schemaのintegerは数学的整数なので12.0等も整数として扱う。Pythonは `type(x) is int` により12.0を拒否する。Rustの入力アダプタで整数値のfloat表記を受理し、canonicalフロントは常に整数リテラルを送る。これは数値・集計結果を変えない入力表記の拡張。Pythonの整数パーサの特殊な文字表記等は通常CSVの互換試験を優先し、曖昧な点は現行実装に合わせる。

APIの正規表記を日本語にする理由：現行UI/READMEが既に日本語を送っており、コード表の内部番号をフロントへ漏らす利点がない。コードとエイリアスは互換入力として残し、レスポンスは日本語に統一する。

## 5. 外部経路取得と候補選定前処理

### OSRM呼び出し

現行ベースは `https://routing.openstreetmap.de/routed-{backend}/route/v1/{profile}/{coordinates}`。walk→foot/walking、bike→bike/cycling、car→car/driving。徒歩・自転車を自動車経路で代替しない。pointsはorigin→任意via→dest、各点を `lon小数7桁,lat小数7桁` として `;` 接続。

queryは `alternatives=true&geometries=geojson&overview=full&steps=false`。User-Agentは現行 `AccidentRoutePrototype/0.1 (local research application)`、Acceptはapplication/json。移行時User-Agentは運用連絡先付きの設定値にする。

現行は通信ごと8秒のソケットタイムアウト、12,000,001バイトまで読み、12,000,000バイト超で失敗。HTTP失敗（429も含む）・接続/タイムアウト・JSON解析失敗は502 external_api_error。OSRMのcodeがOkでない、routesが空、外枠がobjectでない場合は502 no_route。再試行なし。移行では8秒を1通信全体の期限にも適用し、極端に遅い分割応答で無期限化しない（正常結果は変えない運用上の強化）。

### 候補検証と重複排除

1. 最初のrouter(origin,dest,mode)を呼び、結果が配列でなければ502。各呼出しの先頭6本だけを入力順で検査する。
2. duration（秒）、distance（m）は有限かつ正。geometryはLineStringで頂点2〜15,000個（両端含む）。各頂点は数値2個の `[lon,lat]`、日本周辺の範囲内。余分な標高次元は不可。
3. 頂点間の上記straight_distanceの和をgeometry_lengthとする。`distance > 80,000` **または** `geometry_length > 80,000` なら422 route_too_long。80,000ちょうどは可。durationの上限、distanceとgeometry_lengthの整合、重複頂点、始終点と入力地点の一致は現行に検査なし。
4. その他の不正形状/数値は502 external_api_error。検証後、duration/distance/geometryのみ採用。
5. 全頂点をそれぞれ小数5桁にPython `round` で丸めた順序付き列をfingerprintにする。同一なら後からの経路を捨て、先のduration/distanceを残す。形が同じでも頂点数や分割が違えば別候補。向きを逆にした線も別候補。重複検査の前に検証する。

### 左右経由点

最初の重複排除後が3本未満なら**必ず2回**（side=-1→+1）追加する。1回目で3本以上になっても2回目を省略しない。

`mid_lat=(o.lat+d.lat)/2`, `mid_lon=(o.lon+d.lon)/2`, `kx=K*cos(mid_lat*π/180)`、`dx=(d.lon-o.lon)*kx`, `dy=(d.lat-o.lat)*K`, `L=hypot(dx,dy)`。

`offset=min(1200,max(200,0.15*L))` m。

`via.lat=mid_lat + side*dx/L*offset/K`

`via.lon=mid_lon - side*dy/L*offset/kx`

入力距離下限によりLは0にならない。via自体に日本矩形の再検証は現行にはない。追加呼出しのAPIError（422も含む）は警告にして既存候補を残す。追加バッチ内で正常候補を追加した後に不正候補が出た場合、その正常候補も残す。最初のバッチで失敗した場合は既存候補があっても全体エラー。

全呼出し後に候補0なら502 no_route、1本なら比較不能の警告。durationの昇順に**安定ソート**して先頭6本だけ評価する。同duration時は挿入順のまま（ここではdistanceでソートしない）。最大3通信、各6本、最大18本を検証して6本に削る。候補に `route-1`〜`route-N` をこの順に付与する。

移行追加の要求全体期限は、route/geocodeの受付からJSON準備完了まで35秒（キュー待ち・集計・シリアライズを含む）。これを超えた場合は段階によらず503 service_busyで終了し、追加候補失敗の警告化より優先する。初回候補があっても全体期限後の200は不可。全体期限内の個別通信失敗だけが上記の部分成功規則の対象になる。

## 6. 50m回廊と同一事故の重複排除

候補ごとに全geometry頂点の緯度の算術平均φを使い、`kx=K*cos(φ*π/180)`, `ky=K`。座標原点は最初の頂点。長さで加重した平均や線分ごとの投影ではない。

1. 元の隣接頂点a,bの平面距離 `hypot((b.lon-a.lon)*kx,(b.lat-a.lat)*ky)` を計算。`n=max(1,ceil(length/75))` として線形補間でn分割する。分割後小線分index=sidは経路の先頭から0始まり。長さ0の線分も1本残す。
2. 各小線分のbboxを緯度方向50/ky、経度方向50/kxだけ拡張し、floor(座標\*1000)でセル範囲を求める。gy昇順→gx昇順で両端を含めて走査。線分の入力順にセルを登録し、セル内sidもその順。
3. セルは**初回登録順**、各セルの事故はCSV行順に走査する。同じセルの事故を線分ごとに繰り返し取り出さない。modesと指定maskのANDが0の事故は除外する。
4. 点の平面位置P、小線分始点A、方向Dに対し `t=clamp(dot(P-A,D)/dot(D,D),0,1)`、長さ0ならt=0。`d²=|P-A-tD|²`。
5. 初期bestは `50² + 1e-6` m²。`d² < best` のときだけ採用しbestとsidを更新する。したがって厳密には `d² < 2500.000001` まで含む。同距離なら最初に検査したsid（小さいsid）を残す。比較を `<= 50` へ単純化しない。
6. 採用された事故IDと最寄りsidを1組だけ出力する。折返し・交差や複数近接線分でも1候補1事故1回。別候補ではそれぞれ計数する。

これは局所正距円筒近似であり、球面距離・道路ネットワーク距離ではない。立体交差、反対車線、平行道路を区別しない。**元CSVの同じ内容の別行を除く処理はない**。事故IDを外部の真の事故識別子と解釈しない。

RustのHashMapの反復順に依存させない。セルの初回登録順を別配列等で保持する。この順序は表示点の間引き結果にも影響する。

## 7. 条件緩和・集計・高リスク区間

各候補のnearbyから指定period/day_type/weatherのANDを満たす件数を `exact_condition_count` として保存する。省略/all/nullはその条件を制約しない。

緩和は**天候→曜日区分→時間帯**の順に1条件ずつ行う（移行契約）。天候は区分が細かく記録も現場判定で揺れやすいため、最初に外す。曜日区分は2値で、時間帯より絞り込みが弱いため、時間帯より先に外す。時間帯は通行状況との関係が最も強いため、最後まで残す。

- 全候補のexact件数が0の場合だけ緩和する。遠回り対象外の候補もこの判定に含む。
- 次の手順で、残った条件のANDに合う事故が全候補合計で1件以上になった時点で止める。
  1. weather指定があれば解除し、relaxed_conditionsに「天候」を追加。
  2. それでも全候補0件で、day_type指定があれば解除し「曜日区分」を追加。
  3. それでも全候補0件で、period指定があれば解除し「時間帯」を追加。
- 指定のない条件は飛ばし、relaxed_conditionsに追加しない。最初から全条件なら何も追加しない。periodだけ指定なら「時間帯」だけになる場合もある。
- day_typeを指定しない場合、この手順は現行Pythonの「天候→時間帯」と同じ結果になる。
- 移動手段は解除しない。全候補を共通のeffective条件で集計する。1候補でもexact事故があれば他候補が0でも緩和しない。
- 緩和の警告文は解除した条件を順に列挙する（例：「全候補で指定条件の事故が0件のため、天候・曜日区分を解除して全候補を再集計しました（移動手段は維持）。」）。文面の完全一致は受け入れ条件にしない。

適用後の集合をMとすると、事故件数=|M|、死者合計D、負傷者合計I、死亡事故件数=`count(deaths>0)`、負傷事故件数=事故件数−死亡事故件数（死亡事故内の負傷者はIに含む）。

`weighted_risk = 10D + I`

`risk_per_km = round(weighted_risk / max(distance/1000,0.001), 3)`

距離はOSRMのdistanceを使い、geometry_lengthではない。最小分母0.001km=1m。交通量、年度数、曝露量、移動方向による補正はしない。選定にはrisk_per_kmを使わない。

### 表示事故点

Mをnearbyの順で走査して、死者ありFとそれ以外Oに分ける。`sample(items,limit)` はlimit<=0で空、件数<=limitなら元順の全件、それ以外は `items[floor(i*N/limit)] (i=0..limit-1)`。

先にsample(F,250)、残り枠にsample(O,250−採用死亡点数)を連結する。死亡事故が250超なら死亡事故自体も間引く。ランダム・座標ソートはしない。`points_shown=min(accident_count,250)`、間引きで集計値を変更しない。移行契約は順序を保って各点をFeature化し、ID、元年度、発生日、日本語天候/時間帯/曜日区分、人数を返す。曜日区分は前処理の値をそのまま返し、応答時に再判定しない。`points_shown < accident_count` でフロントが間引きを判定する。

### 高リスク区間と将来GPS用参照

Mの各事故の全重みを最寄り小線分1本に加算する。sid別重みを `(-weight,sid)` 昇順に並べ、先頭100件を取り、その中のweight>=3のみ返す（同等な先行フィルタでも順序を維持）。連続線分の結合、経路長による区間重み正規化はしない。

移行契約の `high_risk_sections` はFeatureCollection。geometryはその小線分の補間済み両端を持つLineString。propertiesはroute_id、weighted_risk、`segment_index`、`source_vertex_start_index` と `source_vertex_end_index`。後者2つは元geometryの隣接頂点番号jとj+1で、小線分はその内側の一部分である。geometryをそのまま描画し、元頂点範囲全体を強調しない。feature.idは `route-N:segment-S`。sidは元頂点番号ではない。

これで将来のGPS点と区間geometryの距離判定、元経路への対応付けが可能。ただし返却は上位100区間だけで、すべての重み3以上の区間を網羅しない。将来の全区間配信・接近警告閾値・進行方向・map matching・再検索頻度は**要確認**。v1では自動警告を実装しない。必要なら別バージョンで全区間を提供し、現在の表示用サブセットを完全な安全判定データに流用しない。

## 8. 最短時間・事故考慮経路の決定

- fastestは `(duration, distance)` の辞書順で最小。同値は候補順先頭。名称「最短」は**最短時間**を意味し、最短距離ではない。
- 候補extra_minutes=`round((duration−fastest.duration)/60,3)` 分。
- within_detourは `duration <= fastest.duration + max_detour_min*60 + 1e-6` 秒。丸めたextra_minutesで判定しない。
- safestはwithin_detour=trueの中から `(weighted_risk,duration,distance)` が最小、完全同値は候補順先頭。fastestは必ず対象になるので対象空にはならない。
- トップレベルextra_minutesはsafestの同名値。
- `risk_reduction_percent=round(100*(fastest.risk−safest.risk)/fastest.risk,1)`。基準リスク0ならnull（0%と区別）。同一経路で基準リスク正なら0%。
- reasonは候補数・許容分・重み合計最小という理由、同一経路の場合その旨、異なる場合追加分（小数1桁）と削減率を含む。基準0では算出不可を追記する。日本語文面の完全一致は移行受け入れ条件にしないが、意味と数値は一致させる。
- warningsは追加取得エラー（呼出し順）、1候補比較不能、条件緩和、緩和後も全候補事故0の順で蓄積。事故0は安全の証明ではないと表示する。
- effective_conditionsはmode・period・day_type・weatherの4項目。解除または未指定の条件はそれぞれ `全時間帯`・`全曜日`・`全天候` と表す。
- risk_noteは過去事故の参考値であり、交通量・通行回数で補正した事故確率ではないことを必ず説明する。
- radius_m=50。aggregation_msは候補取得・検証・ソート終了後から集計/選定/説明構築までのms、小数2桁。外部通信、JSON転送、描画を含まない。data_yearsはsource_year集合の昇順。

数値計算はf64、Pythonと同様の演算順を基本とする。`round(x,n)` はPythonの二進浮動小数上の丸め（half-even）を参照オラクルとし、Rustの単純なround（half-away）へ置換しない。5桁fingerprint、7桁CSV、3桁リスク/kmと追加分、1桁削減率の境界fixtureを用意する。距離判定付近の差を勝手に新しい許容差で吸収せず、現行実装に合わせる。

## 9. 条件の自動判定 `/api/conditions`（移行追加）

出発日時と地域から、routeへ送る時間帯・曜日区分・天候の候補値を返す。区分境界と祝日判定を事故記録の前処理と一致させるため、判定はサーバだけで行う。事故データを参照しないので、データ未準備でも応答できる（data_unavailableを返さない）。祝日表の読込に失敗した場合は起動を失敗させる。

### 入力検証

検証順はdatetime→lat/lon。最初の不正で400 invalid_requestを返し、外部サービスを呼ばない。未知のqueryキーは無視する。既知キーの重複は400。

- datetimeは1個必須。ISO 8601拡張形式 `YYYY-MM-DDThh:mm[:ss]` に、オフセット（`Z` または `±hh:mm`）を必ず付ける。オフセットなし・日付のみ・実在しない日時は400。Asia/Tokyo（UTC+9、夏時間なし）へ変換し、秒以下を切り捨てた分単位の日時を基準時刻にする。範囲は2000-01-01T00:00+09:00〜2100-12-31T23:59+09:00。
- lat/lonは両方指定するか、両方省略する。片方だけは400。各値はrouteと同じ有限数値・日本矩形（緯度20〜46、経度122〜154）。サーバは受け取った値を小数2桁に丸めてから、キャッシュキーと上流送信に使う。フロントが丸めた値を送る前提でも、サーバ側で再度丸める。丸め方は結果の互換性に関係しないため、Rust標準の四捨五入でよい。

### 時間帯・曜日区分

- periodは基準時刻の時（Asia/Tokyo）から2.の表で決める。allは返さない。
- day_type・day_type_reason・holiday_table_coveredは基準時刻の暦日から2.の「曜日区分と祝日表」の規則で決める。深夜0〜3時もその日付で判定する。
- 前処理と同じ判定関数・同じ祝日表を使う。別実装にしない。

### 天候

判定に使う現在時刻はサーバ時計とする。基準時刻をTとする。

| 条件（上から順に判定）                                         | weather_status | weather / weather_source                          |
| -------------------------------------------------------------- | -------------- | ------------------------------------------------- |
| lat/lonなし                                                    | no_location    | null / null                                       |
| T < 現在−15分                                                  | past           | null / null                                       |
| \|T−現在\| ≤ 15分                                              | ok（現況）     | 上流のcurrent値 / kind=current                    |
| 現在+15分 < T ≤ 現在+WEATHER_FORECAST_DAYS日                   | ok（予報）     | Tを時単位に切り捨てたhourly行の値 / kind=forecast |
| それより先、または該当するhourly行がない                       | out_of_range   | null / null                                       |
| 上流の失敗・期限超過・待ち行列満杯・形式不正・未知の気象コード | unavailable    | null / null                                       |

15分の幅は、フロントが「現在」モードで15分ごとに条件を取り直す規則に合わせた値である。past・out_of_range（日数超過）・no_locationの場合は上流を呼ばない。

上流呼出し（Open-Meteo Forecast API）は1地点につき1回で、現況と予報を同時に取得する。

`GET {OPEN_METEO_BASE_URL}/v1/forecast?latitude=<丸めた緯度>&longitude=<丸めた経度>&current=weather_code&hourly=weather_code&timezone=Asia%2FTokyo&forecast_days=<WEATHER_FORECAST_DAYS>`

有償プランを使う場合は、apikeyをサーバ専用のsecretから付加する。応答は `utc_offset_seconds=32400`、`current.time` と `current.weather_code`、同じ長さの `hourly.time` / `hourly.weather_code` 配列を要求する。満たさない場合はunavailableとする。時刻はAsia/Tokyoのローカル時刻として解釈する。weather_source.target_timeには、現況なら `current.time`、予報なら該当hourly行の時刻を、`+09:00` 付きで返す。providerは `Open-Meteo`。

WMO天気コードから天候への変換は次の表に固定する（**要確認**）。

| WMOコード              | 意味（Open-Meteo）   | 天候        |
| ---------------------- | -------------------- | ----------- |
| 0, 1, 2                | 快晴・晴れ・一部曇り | 晴          |
| 3                      | 曇り                 | 曇          |
| 45, 48                 | 霧・着氷性の霧       | 霧          |
| 51, 53, 55, 56, 57     | 霧雨・着氷性の霧雨   | 雨          |
| 61, 63, 65, 66, 67     | 雨・着氷性の雨       | 雨          |
| 80, 81, 82             | にわか雨             | 雨          |
| 95, 96, 99             | 雷雨（雹を含む）     | 雨          |
| 71, 73, 75, 77, 85, 86 | 雪・霧雪・にわか雪   | 雪          |
| その他                 | —                    | unavailable |

一部曇り（2）を晴とするのは、気象庁の「晴」が雲量8割以下を含むことに合わせた近似である。事故記録の天候は発生時の現場での判定であり、気象モデルの推定値とは同じ尺度ではない。この変換は自動の初期値を決めるためだけに使い、事故集計の条件の意味は変えない。

### 期限・キャッシュ・レート制御

- 1通信の期限はWEATHER_TIMEOUT_MS（既定5000ms）。応答の上限は1,000,000バイト。再試行はしない。
- 要求全体の期限はCONDITIONS_TIMEOUT_MS（既定8000ms、フロントの10秒より短い）。期限内に天候が得られなければ、天候だけunavailableにして200を返す。時間帯・曜日区分は常に返す。
- キャッシュは丸めた(lat, lon)をキーに、上流応答全体をプロセスメモリにだけ保持する。TTLはWEATHER_CACHE_TTL_SEC（既定600秒）、最大1024キー、満杯なら挿入順の最古を除去する。失敗応答は保存しない。永続化・プロセス間共有はしない。
- 提供元ごとに全利用者共通の通信開始間隔WEATHER_MIN_INTERVAL_MS（既定200ms）と有限待ち行列（UPSTREAM_QUEUE_CAPACITYと共通の上限値）を設ける。満杯は503ではなく、天候のunavailableとして扱う。

### HTTP

入力が正しければ常に200を返す。天候が取れないことはエラーではない。400 invalid_request・405 method_not_allowed・500 internal_errorだけを使う。Cache-Control no-store、nosniffはほかのAPIと同じ。

## 10. 住所検索、HTTPと静的配信

`q` を前後trimしUnicode文字数2〜200。現行parse_qsは複数qのうち最初の非空値を使うが、移行契約では**qは1個必須**、重複は400とする。検索はユーザーの明示操作だけで行う。

Nominatim `/search` に `q, format=jsonv2, limit=5, countrycodes=jp, accept-language=ja` を送る。現行は全検索をlockで直列化し、キャッシュmissの通信開始間隔を最低1.05秒あける。外部通信の共通制限はOSRM同様。結果は配列を要求しlat/lonをfloat化、display_nameをlabelへ変換、日本矩形外・欠落・型不正は502。現行は提供元の5件超応答を切り詰めていない。移行契約は先頭5件までに限定する。空配列は200でありエラーではない。

現行cacheはtrim後qをキーとし、成功時（空結果含む）の単調時計で記録。経過 `<86400秒` ならhit、境界はmiss。最大256キー。書込み時に件数>=256なら挿入順最古を1件除去する（LRUではない。期限切れキー更新でも先に除去）。大文字小文字等の追加正規化はない。

**プライバシーの移行差分**：ユーザーの住所・座標・経路を永続保存しない。現行24時間キャッシュは本移行で無効化し、リクエスト中だけ一時保持する。公共Nominatimを使う検証環境に限り、architectureの明示的なキャッシュ設定で現行動作を選べる。デフォルト本番はキャッシュを要求しない契約の提供元または自己ホストを使用し、公共サービス規約との矛盾を避ける。キャッシュ互換テストは現行互換設定で実施する。

移行APIのHTTP対応は次の通り。すべて `{"error":{"code":...,"message":...}}`、messageは日本語の利用者向け説明。スタックトレース、住所、座標入り上流URLは返さない。

| HTTP | code               | 条件                                                                                                      |
| ---: | ------------------ | --------------------------------------------------------------------------------------------------------- |
|  400 | invalid_request    | JSON/型/列挙/距離/本文サイズ/Content-Type/qの不正、conditionsのdatetime/lat/lonの不正                     |
|  404 | not_found          | 未定義API、存在しない静的ファイル（静的側の本文はtext/plain可）                                           |
|  405 | method_not_allowed | 定義済みAPIの不正メソッド。Allowヘッダ必須（移行で統一）                                                  |
|  422 | route_too_long     | 初回候補のdistanceまたはgeometry_lengthが80km超                                                           |
|  502 | no_route           | 初回OSRM経路なし、最終候補0                                                                               |
|  502 | external_api_error | 外部通信/応答形式失敗。上流429も502                                                                       |
|  503 | data_unavailable   | データ準備前/利用不能（移行追加）。conditionsでは返さない                                                 |
|  503 | service_busy       | 移行追加の受付上限・外部サービス待ち行列の満杯、またはroute/geocode要求全体の35秒期限超過（集計中も含む） |
|  500 | internal_error     | 予期しない内部失敗（移行追加）                                                                            |

JSON応答はContent-Type application/json、UTF-8、Cache-Control no-store、X-Content-Type-Options nosniff。API未定義パスへSPAのHTMLを返さない。healthはデータready時200で、件数・年度・グリッド数・読込秒数・dataset_idを返す。外部サービスへ疎通して判定しない。起動ロード失敗時は現行同様プロセス停止を基本とし、運用でHTTPが生きていて未readyなら503。

静的配信の新契約はGET `/`、`/assets/{asset}`、`/manifest.webmanifest`、`/sw.js`。配信rootはfrontend/distだけ。APIルートを優先し、パストラバーサル・ディレクトリ一覧・accident/app/backend/dataへのアクセスを許可しない。初期版は画面URLが `/` のみで、未知URLのSPAフォールバックは行わず404。詳細はOpenAPIとarchitectureに従う。

## 11. 受け入れテスト対応表

現在の24テストを以下の名前単位でRustへ移す。時間帯の5区分化と曜日区分の追加で期待値が変わるテストは、表の指針に従って期待値を新しい契約へ更新する。Pythonの結果をそのままオラクルにしない。ソケット不要のHTTP試験はAxumのin-process service呼出しに置き換える。外部実通信は不要。テストが保証していない境界まで「検証済み」としない。

### app/test_prepare_data.py — PreparationTests（7件）

| テスト名                                                   | 現行の保証 / Rustへの移植指針                                                                                                                      |
| ---------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| test_dms_examples_and_subsecond_precision                  | 緯経度例2件と0.001秒角の精度。元入力と期待値を維持、10桁相当の比較                                                                                 |
| test_invalid_dms                                           | 空/0/異常分秒/小数/負数/全角を拒否。同じ表形式fixture                                                                                              |
| test_modes_include_both_parties_without_motorcycle_leakage | A/BのOR、51/52、全CAR_CODES、36/43非対象。bitmaskの完全一致                                                                                        |
| test_time_boundaries                                       | 0〜23の全時刻と-1/24/99拒否。変換関数のtable test。期待値を5区分（0〜3時=深夜、19〜23時=夜）へ更新する                                             |
| test_encoding_utf8_cp932_and_bom                           | UTF-8/BOM/cp932の判定。バイトfixtureで検証                                                                                                         |
| test_real_codebooks_and_row_from_each_year                 | 実コード表の天候・43年度差、7年度の先頭行、年/緯度/車分類/負傷、時99/月00/座標0/天候9/当事者99拒否。実データがなければskipを明示し合格と混同しない |
| test_generated_schema_load_roundtrip                       | 17列gzip→索引の緯度/mask/date/year/day_type保持。発生年≠source_yearを維持                                                                          |

### app/test_server.py — SpatialAndSelectionTests（8件）

| テスト名                                                            | 現行の保証 / Rustへの移植指針                                                                                                                                      |
| ------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| test_corridor_segment_interior_endpoints_and_deduplication          | 内部・±49m・端点49mを含み51mを除く、手段不一致を除く、折返しでID一意。4件/重み4を維持                                                                              |
| test_filters_casualties_normalization_and_hotspots                  | 条件AND、2件/死亡事故1/負傷事故1/死者1/負傷者5/重み15/2kmで7.5、高リスク存在。構造変換後も数値一致                                                                 |
| test_diagonal_bbox_does_not_include_distant_point                   | bbox内でも斜線から離れた点を除外。空間距離で判定                                                                                                                   |
| test_safest_obeys_detour_and_uses_weighted_total                    | 600/900/1300秒、許容5分の境界、900秒選択、削減81.8%、0分ならfastest。返却全候補のwithin_detourも比較                                                               |
| test_fallback_is_shared_by_all_candidates_preserving_mode           | 全候補exact0→天候解除、各1件、手段維持、exact_count保持                                                                                                            |
| test_time_fallback_and_all_zero_have_no_false_percentage            | 天候→時間帯の順、手段不一致で全0、null削減率・同一経路・警告。day_type=allで現行と同じ結果になることを確認し、day_type指定時の天候→曜日区分→時間帯は追加試験で扱う |
| test_no_relaxation_if_another_candidate_has_exact_match             | 他候補にexactありなら緩和なし、fastest0ならnull                                                                                                                    |
| test_point_sampling_prioritizes_fatalities_without_affecting_totals | 400事故の集計を保持し250点、死亡10件を保持。Feature propertiesとして照合                                                                                           |

### app/test_server.py — ExternalAndValidationTests（7件）

| テスト名                                     | 現行の保証 / Rustへの移植指針                                                                                                               |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| test_left_right_vias_and_deduplication       | bikeの左右via緯度、追加で3候補、重複だけなら1候補+警告。router呼出しを記録する偽物で検証                                                    |
| test_additional_api_failure_keeps_original   | 追加2通信の失敗でも初回を保持、初回失敗は全体失敗                                                                                           |
| test_osrm_profiles_parameters_and_user_agent | 3modeのURL/profile、7桁座標、alternatives/geojson、UA。移行では設定済み識別UAとsteps/overviewも検証                                         |
| test_external_errors                         | URLError/timeout/上流429→APIError。Rustでは通信抽象の各失敗と502形式に対応                                                                  |
| test_geocode_cache_and_japanese_query        | 前後空白をtrimし同一結果を1通信、query日本語をURLエンコード。現行互換cache設定で実施し、標準無保存設定は別途試験                            |
| test_invalid_requests_never_call_router      | object以外、null座標、不正mode/hour/period/weather/detour、NaN/Inf、同一点/30km超を拒否しrouter0回。非JSONのNaN等はドメイン関数試験にも残す |
| test_bad_provider_geometry                   | NaN頂点を拒否。外部エラーへの変換も照合                                                                                                     |

### app/test_server.py — HTTPTests（2件）

| テスト名                               | 現行の保証 / Rustへの移植指針                                                                                             |
| -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| test_status_and_static_page            | `/api/status` 200/件数0fixture、`/` HTML、`/../data/metadata.json` 404。health正規パスとaliasを両方試験。api_versionは2.0 |
| test_json_errors_and_mocked_route_post | 不正JSON→400 invalid_request、モックroute成功→200 candidates、外部失敗→502と説明                                          |

### 移行時に追加する同等性・契約試験

1. 同一入力データ、同一順序の固定OSRM応答をPython/Rustへ渡す。生きたOSRMの別時点応答は比較しない。比較対象は1.の互換範囲（period=早朝/昼/夕/all、day_type=all）のリクエストに限る。CSVはperiodの旧3を新3/4の和として、day_type列を除いて比較する。ID/候補順/件数/重み/選定/緩和/間引き順を完全比較。geometryと点はGeoJSON包装を除いて比較し、計時値と日本語文面は意味で検証する。
2. 浮動小数の距離・補間座標は通常ケースで座標絶対誤差1e-10度以内を目安とするが、採用/除外・丸め済み数値・順序の差は許さない。50m+ε、75m分割、20m/30km/80km、許容時間+1e-6、リスク同点、duration同点のfixtureを追加する。
3. 15,000/15,001頂点、本文16,384/16,385バイト、provider12,000,000超、死亡250超、hotspot100超/重み2対3、重複座標と0長線分、元CSV重複行、初回/追加バッチ途中失敗を追加する。
4. 全スキーマの必須/null/列挙/座標順を検証。`points_shown == features.length`、source頂点範囲、feature.id/route_id、fastest_id/safest_idの参照整合を検証。正常・0件・緩和・同一経路・全HTTPエラーの共通fixtureでReact/Rustを結合する。
5. 前処理の全入力行数/保持数/除外理由とCSV復号後の全行を比較する。現行記録は入力2,182,298、保持2,182,127、除外171（全件DMS不正）だが入力スナップショットに依存する。曜日区分の追加で保持・除外件数は変わらない。性能記録（6モックケースで1秒未満）はSLAではない。実OSRM/Nominatim/Open-Meteo、ブラウザ描画、実GPSはVALIDATION時点で未検証で、別の結合受け入れ試験が必要。

### 時間帯・曜日区分・条件取得の追加試験

6. 時間帯：3:59/4:00、8:59/9:00、15:59/16:00、18:59/19:00、23:59/0:00の境界を、前処理と `/api/conditions` の両方で同じ期待値にする。
7. 曜日区分：平日、土曜、日曜、名称付き祝日、日曜の祝日、土曜の祝日、振替休日（2025-11-24など、および連休明けの2009-05-06型）、国民の休日（2026-09-22など）、12/31と1/2（平日）、祝日表の収録範囲外（土日だけで判定し、holiday_table_covered=false）。前処理と `/api/conditions` で同じ関数を使っていることも確認する。
8. 祝日表：日付形式の揺れ（ゼロ埋めの有無）、重複日付・不正日付・空欄名称での読込失敗、cp932とUTF-8の両方。
9. 条件緩和：day_type指定ありで天候→曜日区分→時間帯の各段で止まるケース、指定なしの条件を飛ばすケース、全解除後も0件のケース。relaxed_conditionsの順序とeffective_conditionsの `全曜日` を検証する。
10. route入力：period 0〜4/深夜、旧整数3の新しい意味、hour 0〜3→深夜とperiod=夜との矛盾400、day_typeの不正値・整数・bool拒否。
11. `/api/conditions`：datetimeの書式・オフセット欠落・範囲外・重複、lat/lonの片方指定。天候のno_location/past/±15分境界の現況/予報/out_of_range/hourly行欠落/上流失敗/期限超過/未知コード/utc_offset不一致。WMOコード表の全行。座標の2桁丸めがキャッシュキーと上流URLの両方に適用されること。キャッシュTTL境界と失敗非保存。上流は固定応答の偽物で検証し、実通信しない。

## 12. 要確認事項と暫定扱い

- リスク重み10、半径50m、区間重み3は任意のプロトタイプ定数。妥当性の科学的検証は未実施。移植では変更しない。
- 真の事故ID・原本の重複有無は不明。入力行IDと経路内重複排除だけを維持する。
- PDFの自動内容検証なし、事故類型コードと死傷者/A/Bの整合検証なし。強化する場合は除外件数が変わるため別の仕様変更とする。
- 公共ルーティングは位置を外部ログに残す旨を公表している。アプリ内の無保存と同じ保証にはできない。標準運用は自己ホスト/適切な契約の提供元を選ぶ（architecture参照）。
- 現行はOSRM通信の全利用者共通レート制御がない。移行では提供元に合わせた直列化と上限を追加する。正常な候補生成順は維持する。
- 将来のGPS接近警告、背景追跡、オフライン道路検索は未設計。本契約の表示用高リスク上位100件から安全性を推定しない。
- 年末年始（12/29〜1/3）を平日とする扱い、WMOコードから天候への変換表（とくに一部曇り=晴）、緩和順序（天候→曜日区分→時間帯）は暫定。変更する場合は事故集計の結果が変わるため、契約版の更新として扱う。
- Open-Meteoの無料APIは非商用利用に限られる。製品化時の提供元・プランは**要確認**（architecture参照）。
