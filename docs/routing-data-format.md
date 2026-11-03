# 自前経路探索のデータ形式（schema 3）

[routing-workplan.md](routing-workplan.md) のM0.5で決めた、schema 3のreleaseのファイル形式・ID・規則と、骨組みのモジュール構成。計算の意味（集合B、件数、選定、高リスク区間）は [api/behavior.md](api/behavior.md) が正本で、ここではファイルの中身と、それを読む型だけを決める。根拠は [routing-plan.md](routing-plan.md)（以下「計画書」）。

形式を変える必要が出たら、作業役は実装せずに指示役へ報告する（作業書2.4節）。指示役が変える場合は、この文書の該当節と形式の版（`format_version`）を同時に上げる。

## 1. 用語と表記

| 用語 | 意味 |
|---|---|
| N・E | 共通グラフのノード数・辺数 |
| A | 事故数（`accidents.csv.gz` のデータ行数、`metadata.json` の `rows`） |
| C | 交差点クラスタの数 |
| 辺（edge） | 交差点とwayの端点で区切った、無向の道路区間。辺番号 `EdgeId` は `graph.bin` の `EDGE` 節での順番 |
| 有向辺（dedge） | 辺を向き付きで見たもの。`DirEdge = 2×辺番号 + 向き`（向き0：辺の始点→終点、1：終点→始点） |
| 始点・終点 | 辺の `tail`・`head`。OSMのwayのノード順の向き |
| 手段 | walk・bike・car。ビットは `Mode::mask` と同じ（walk=1、bike=2、car=4） |
| セル | 条件の単位。時間帯5区分×曜日区分2区分。`cell = period_code×2 + day_type_code`（0〜9） |

- 数値はすべてリトルエンディアン。構造体は記載の順にパディングなしで並べる（`elem_size` に書いた大きさ）。
- 座標は `i32` の1e-7度単位（`lat_e7`・`lon_e7`）。十進度への変換は `x as f64 / 1e7`。
- 長さは `u32` のデシメートル（`_dm`、0.1m単位）。辺の長さは、形状を含む折れ線の各線分の `contract::geo::straight_distance_m` の和を10倍して四捨五入した値（最小1）。経路の `distance_m` は通った長さの和（m）で、`geometry` の頂点から計算した長さとは丸めと間引きの分だけ違ってよい。
- 時間は探索中は `u32`/`u64` のデシ秒（`_ds`、0.1秒単位）。表示の `duration_s` は `_ds / 10`。
- リスクは計算と表示には `f64` を使い、探索の比較だけに固定小数点 `u64`（`_fx`、`RISK_SCALE = 1e6` 倍して四捨五入）を使う。`weighted_risk` などの応答値は、確定した経路について `f64` で計算し直す（固定小数点の和を表示に使わない）。

## 2. releaseのファイル一覧

`backend/data/releases/<release_id>/` に次の11ファイルを置く。

| ファイル | 作る段階 | 起動時 | 内容 |
|---|---|---|---|
| `accidents.csv.gz` | 既存 `prepare_release` | 読む | 17列の事故CSV。**既存の `prepare_release` の出力と同じバイト列**（`dataset_id` を旧方式と揃える） |
| `syukujitsu.csv` | 既存 | 読む | 祝日表（原本のバイト列） |
| `metadata.json` | P9 `package` | 読む | 4章 |
| `graph.bin` | P1 `osm-graph` | 常駐 | 6章 |
| `restrictions.bin` | P2 `restrictions` | 常駐 | 9章 |
| `topology.bin` | P3 `topology` | 常駐 | 10章 |
| `odindex.bin` | P7 `od-index` | 常駐 | 11章 |
| `accident_attrs.bin` | P4 `accident-attrs` | ハッシュの照合だけ（常駐させない） | 12章 |
| `assignment.bin` | P5 `accident-snap` | 常駐（形を変えて持つ） | 13章 |
| `risk.bin` | P6 `risk-fit` | 常駐 | 14章 |
| `alt.bin` | P8 `alt` | 常駐 | 15章 |

P9 `package` は、既存の `prepare_release` が作ったschema 2のrelease（`--base-release`）から `accidents.csv.gz` と `syukujitsu.csv` をバイト列のままコピーし、8つの `.bin` を加える。事故CSVを作り直さないので、`dataset_id` は旧方式と必ず一致する。P4・P5・P6も、同じschema 2のreleaseの `accidents.csv.gz` を入力にする（事故IDの行番号が一致することを、`DEPS` 節のハッシュで保証する）。

## 3. バイナリファイルの共通形式

8つの `.bin` は、同じ容器形式を使う。実装は `data::binfile`（M0で実装済み）。

### 3.1 ヘッダー（64バイト）

| オフセット | 型 | 内容 |
|---:|---|---|
| 0 | `[u8; 8]` | マジックナンバー（下表） |
| 8 | `u32` | 形式の版 `format_version`（このファイル種別の版。初版は1） |
| 12 | `u32` | 節の数 `section_count` |
| 16 | `u32` | ヘッダー長 `header_len` = 64 + 32×`section_count` |
| 20 | `u32` | 予約（0） |
| 24 | `[u8; 32]` | `payload_sha256`：`header_len` からファイル末尾までのバイト列のSHA-256 |
| 56 | `[u8; 8]` | 予約（0） |

| ファイル | マジックナンバー |
|---|---|
| `graph.bin` | `TKRGRAPH` |
| `restrictions.bin` | `TKRRESTR` |
| `topology.bin` | `TKRTOPOL` |
| `odindex.bin` | `TKRODIDX` |
| `accident_attrs.bin` | `TKRACATR` |
| `assignment.bin` | `TKRASSGN` |
| `risk.bin` | `TKRRISK_` |
| `alt.bin` | `TKRALT__` |

### 3.2 節表（32バイト×`section_count`、オフセット64から）

| オフセット | 型 | 内容 |
|---:|---|---|
| 0 | `[u8; 8]` | 節の名前（ASCII。8文字に満たない分は `\0` で埋める） |
| 8 | `u64` | ファイル先頭からの開始位置。8の倍数 |
| 16 | `u64` | 要素数 `count` |
| 24 | `u32` | 要素の大きさ `elem_size`（バイト）。JSONの節は1 |
| 28 | `u32` | 予約（0） |

- 節は節表の順に並び、重ならない。節の間の詰め物は0。節の長さは `count × elem_size`。
- 同じ名前の節は1つのファイルに1つだけ。読込側は、必須の節がない、`elem_size` が仕様と違う、`count` が他の節や `PARAMS` と矛盾する、のいずれかで失敗する。
- 節の名前が `PARAMS`・`DEPS` などのJSONの節は、UTF-8のJSONオブジェクト（`serde_json` で読める）を入れる。

### 3.3 共通の節

| 節 | 内容 |
|---|---|
| `DEPS` | JSONオブジェクト。このファイルを作るときに入力にしたreleaseのファイル名と、そのファイル全体のSHA-256（小文字hex）。例：`{"graph.bin": "…", "accidents.csv.gz": "…"}`。入力がなければ `{}` |
| `PARAMS` | JSONオブジェクト。作成時のパラメータ・規則の版・件数・監査値。中身は各章 |

各ファイルの `DEPS` に必ず含めるもの：

| ファイル | `DEPS` のキー |
|---|---|
| `graph.bin` | なし |
| `restrictions.bin`・`topology.bin`・`odindex.bin`・`alt.bin` | `graph.bin` |
| `accident_attrs.bin` | `accidents.csv.gz` |
| `assignment.bin` | `graph.bin`・`topology.bin`・`accident_attrs.bin`・`accidents.csv.gz` |
| `risk.bin` | `graph.bin`・`topology.bin`・`accident_attrs.bin`・`assignment.bin`・`accidents.csv.gz` |

起動時と `package` は、`DEPS` のすべてのハッシュが、同じreleaseにある実ファイルのハッシュと一致することを確かめる。これで「異なる版の辺IDと事故の表を混ぜない」（計画書8章）を保証する。

## 4. `metadata.json`（schema 3）と識別子

### 4.1 キー

schema 2のキー（`backend/README.md` の「release形式」）はすべて残し、値も同じ意味にする。ただし次を変える・足す。

| キー | 内容 |
|---|---|
| `schema_version` | 3 |
| `processor` | `{"name": "routing_prep", "version": <crateの版>}` |
| `base_release` | コピー元のschema 2のreleaseの `release_id`・`processor`・`generated_at` |
| `routing.files` | 8つの `.bin` の配列。各要素は `file`・`sha256`（ファイル全体）・`bytes`・`magic`・`format_version`・`sections`（節名→`count`） |
| `routing.rules_version` | 手段別の通行規則の版（7章、初版 `jp-rules-1`） |
| `routing.inputs` | 入力の版とチェックサム：OSM PBF（`file`・`sha256`・PBFヘッダーの `osmosis_replication_timestamp`）、事故原本とコード表（`base_release` の `files`・`codebooks_*` を参照）、祝日表 |
| `routing.graph` | `nodes`・`edges`・`shape_points`（`graph.bin` の `PARAMS` の写し） |
| `routing.risk` | `train_source_years`（学習に使ったsource_yearの昇順）・`c`・しきい値などの写し（`risk.bin` の `PARAMS` の主要部分） |

`dataset_id`・`rows`・`holiday_table`・`data_years`・`files`・`totals`・`codebooks_xlsx`・`codebooks_pdf`・`holiday_uncovered_rows`・`columns`・`gzip_bytes` は、`base_release` の値をそのまま写す。

### 4.2 `release_id`

schema 3の `release_id` は、次の行を `\n` で連結した（末尾に改行を付けない）UTF-8文字列のSHA-256（小文字hex）とする。

```text
<dataset_id>
<祝日表のSHA-256>
graph.bin <SHA-256>
restrictions.bin <SHA-256>
topology.bin <SHA-256>
odindex.bin <SHA-256>
accident_attrs.bin <SHA-256>
assignment.bin <SHA-256>
risk.bin <SHA-256>
alt.bin <SHA-256>
```

先頭2行はschema 2の式と同じ。ファイル名とハッシュの間は半角空白1つ。行の順番は上の固定順（`data::release::ROUTING_FILES`）。実装は `data::release::routing_release_id`（M0で実装済み）。

### 4.3 起動時の検証（`data::load_engine`）

1. `metadata.json` の `schema_version` を読む。2なら従来どおり `AccidentEngine`、3なら `RoutingEngine`、それ以外は失敗（メッセージに `schema_version` を含める）。
2. schema 3では、schema 2の検証（祝日表と `dataset_id`・事故CSVの全件検証・`rows`）に加えて、次を確かめる。いずれかが一致しなければ `LoadError` で停止する。
   - 8つの `.bin` がすべてあり、ファイル全体のSHA-256が `routing.files[].sha256` と一致する。
   - 4.2節で再計算した `release_id` が `metadata.json` の値と一致する。
   - 各ファイルのマジックナンバー・`format_version`（読込側が対応する版と一致）・節の `count` が `routing.files[]` と一致する。
   - 各ファイルの `DEPS` が、実ファイルのハッシュと一致する。
   - 各章の「読込時の検査」。
3. `payload_sha256` は、起動時には検査しない（ファイル全体のハッシュの一致で足りる）。前処理の各段階と `package` が、入力ファイルについて検査する。
4. 起動時間は4分以内、読込後のRSSは3.2GB以下（作業書M4.2）。最初は、mmapを使わず `Vec` に読み込む（計画書7.2節）。

## 5. ID・座標・方向

- ノード番号・辺番号は `u32`。`u32::MAX` は「なし」を表す（`NONE`）。有向辺は `DirEdge(u32)`。辺数は2^31未満とする（P1で検査）。
- ノード番号は、P1が決める順番（OSMのノードIDの昇順）。辺番号は、P1が決める順番（wayのIDの昇順、way内は先頭から）。どちらも同じ入力からは同じになること（並列処理でも決定的にする）。
- 座標の範囲は、緯度20〜46度・経度122〜154度（契約の日本矩形）。範囲外のノードを含むwayは、範囲外の部分を捨てる。

## 6. `graph.bin`

### 6.1 節

| 節 | 要素 | 件数 | 内容 |
|---|---|---|---|
| `DEPS` | JSON | — | `{}` |
| `PARAMS` | JSON | — | `rules_version`、`nodes`、`edges`、`shape_points`、`interpolated_points`（6.4節で足した点の数）、OSM PBFのファイル名・SHA-256・日時、除外したwayの件数（理由別） |
| `TIMEMODL` | JSON | — | 所要時間のモデル（8章） |
| `NODEOFS` | `u32` | N+1 | 隣接リストの開始位置（CSR）。`NODEOFS[0]=0`、`NODEOFS[N]=2E`、非減少 |
| `ADJ` | `u32`（`DirEdge`） | 2E | ノードvから出る有向辺。`ADJ[NODEOFS[v]..NODEOFS[v+1]]`。通行の可否によらず、vに接する辺の両方向の有向辺のうち、vを出発点とするものをすべて入れる。各ノードの中は昇順。始点と終点が同じ辺（ループ）は2回現れる |
| `NODELL` | `{lat_e7: i32, lon_e7: i32}` | N | ノード座標 |
| `NODEFLG` | `u8` | N | ノードのフラグ（6.3節） |
| `EDGE` | 16バイト（6.2節） | E | 辺の属性 |
| `SHPOFS` | `u32` | E+1 | 辺の中間形状点の開始位置。辺eの中間点は `SHAPE[SHPOFS[e]..SHPOFS[e+1]]`。両端のノードは含まない |
| `SHAPE` | `{dlat_e7: i16, dlon_e7: i16}` | 形状点数 | 中間形状点。直前の点（辺の最初の中間点では始点ノード）からの差分 |

有向辺 `d` の出発ノードは、`d` が向き0なら辺の `tail`、向き1なら `head`。vに入る有向辺は、`ADJ` のvの範囲にある各 `d` について `d ^ 1`（逆向き）である。後ろ向きの探索も、この1つの隣接リストで行う。

### 6.2 `EDGE` のレコード（16バイト）

| オフセット | 型 | 内容 |
|---:|---|---|
| 0 | `u32` | `tail`（始点ノード） |
| 4 | `u32` | `head`（終点ノード） |
| 8 | `u32` | `length_dm`（1節の規則、1以上） |
| 12 | `u8` | `class`（道路の種類、6.5節） |
| 13 | `u8` | `access`（手段×方向の通行可否、下表） |
| 14 | `u8` | `flags`（下表） |
| 15 | `u8` | `maxspeed_kmh`（`maxspeed` タグの値、km/h。0は不明・解釈不能。255は255以上） |

`access` のビット（7章の規則で決める。`access=0` の辺はグラフに入れない）：

| ビット | 値 | 意味 |
|---|---|---|
| 0 | 0x01 | car：始点→終点 |
| 1 | 0x02 | car：終点→始点 |
| 2 | 0x04 | bike：始点→終点 |
| 3 | 0x08 | bike：終点→始点 |
| 4 | 0x10 | walk：始点→終点 |
| 5 | 0x20 | walk：終点→始点 |
| 6〜7 | — | 0 |

`flags` のビット：

| ビット | 値 | 意味 |
|---|---|---|
| 0 | 0x01 | `motorroad=yes` |
| 1 | 0x02 | 歩道（`footway=sidewalk`、または `sidewalk` 系の歩道を独立したwayで描いたもの） |
| 2 | 0x04 | 横断歩道（`footway=crossing`、`highway=footway` かつ `crossing=*`） |
| 3 | 0x08 | トンネル（`tunnel=yes/building_passage` 等） |
| 4 | 0x10 | 橋（`bridge=yes/viaduct` 等） |
| 5 | 0x20 | 環状交差点（`junction=roundabout/circular`） |
| 6 | 0x40 | 有料（`toll=yes`） |
| 7 | 0x80 | 0（予約） |

### 6.3 `NODEFLG` のビット

| ビット | 値 | 意味 |
|---|---|---|
| 0 | 0x01 | 信号（`highway=traffic_signals`、`crossing=traffic_signals` のノード） |
| 1 | 0x02 | 横断歩道のノード（`highway=crossing`） |
| 2 | 0x04 | 一時停止（`highway=stop`） |
| 3 | 0x08 | 踏切（`railway=level_crossing`・`railway=crossing`） |
| 4 | 0x10 | carはこのノードを通過できない（7.3節の障害物） |
| 5 | 0x20 | bikeはこのノードを通過できない |
| 6 | 0x40 | walkはこのノードを通過できない |
| 7 | 0x80 | 0（予約） |

「通過できない」ノードは、その手段の経路の途中に現れてはいけない（出発・到着の辺の端点としては使える）。

### 6.4 形状点の差分の規則

隣り合う2点の差分が `i16` の範囲（±32767、約±360m）を超える場合、P1は、その2点を結ぶ線分を等分する点を足し、すべての差分を範囲内に収める。足した点の座標は1e-7度に四捨五入する（線分からのずれは1cm未満）。`PARAMS.interpolated_points` に足した点の数を記録する。辺の長さは、足す前の折れ線から計算する。

### 6.5 道路の種類 `class`

| 値 | `highway` | 値 | `highway` |
|---:|---|---:|---|
| 0 | motorway | 11 | residential |
| 1 | motorway_link | 12 | living_street |
| 2 | trunk | 13 | service |
| 3 | trunk_link | 14 | road |
| 4 | primary | 15 | track |
| 5 | primary_link | 16 | path |
| 6 | secondary | 17 | cycleway |
| 7 | secondary_link | 18 | footway |
| 8 | tertiary | 19 | pedestrian |
| 9 | tertiary_link | 20 | steps |
| 10 | unclassified | 21 | bridleway |

これ以外の `highway`（`construction`・`proposed`・`abandoned`・`platform`・`corridor`・`elevator`・`bus_guideway`・`raceway` など）と、`area=yes` の閉じたwayは、グラフに入れない。

### 6.6 読込時の検査

`NODEOFS` が非減少で末尾が2E、`ADJ` の各要素の辺番号がE未満でその出発ノードが範囲のノードと一致、`EDGE` の `tail`・`head` がN未満、`length_dm ≥ 1`、`access ≠ 0`、`class ≤ 21`、`SHPOFS` が非減少で末尾が形状点数、`NODELL` が日本矩形の内側、`TIMEMODL` が8章の形で読めること。

## 7. 手段別の通行規則（`jp-rules-1`）

P1が、wayのタグから `class`・`access`・`flags`・`maxspeed_kmh` を決める規則。実装は `domain::routing::graph::rules::classify_way`（純粋関数。タグの組を入力にとる）。規則を変えたら版を上げ、`PARAMS.rules_version` と `metadata.json` に記録する。タグがないことを「禁止なし」や「安全」と同じに扱わない（計画書3.2節）。

### 7.1 種類ごとの既定

| 種類 | car | bike | walk |
|---|---|---|---|
| motorway・motorway_link | 可 | 不可 | 不可 |
| trunk〜tertiary（link含む）・unclassified・residential・living_street・service・road | 可 | 可 | 可 |
| track | 不可 | 可 | 可 |
| path | 不可 | 可 | 可 |
| cycleway | 不可 | 可 | 可 |
| footway・pedestrian | 不可 | `bicycle=yes/designated/permissive` のときだけ可 | 可 |
| steps | 不可 | 不可 | 可 |
| bridleway | 不可 | 不可 | 不可（`foot=yes` 等で可） |

### 7.2 タグによる上書き（上から順に適用し、後のものが優先）

1. `access=no/private/agricultural/forestry/delivery` → 全手段を不可。`access=yes/permissive/destination/customers` は既定のまま。
2. `vehicle=no/private` → car・bikeを不可。`vehicle=yes/permissive/destination` → car・bikeを7.1の既定で可にしない（既定が不可のものは不可のまま）。
3. `motor_vehicle=no/private`・`motorcar=no/private` → carを不可。`motor_vehicle=yes/destination`・`motorcar=yes/destination` → carを可（motorway・track等の既定不可も可にする）。
4. `bicycle=no/use_sidepath/dismount` → bikeを不可。`bicycle=yes/designated/permissive/destination` → bikeを可。
5. `foot=no/use_sidepath` → walkを不可。`foot=yes/designated/permissive/destination` → walkを可。
6. `motorroad=yes` → bike・walkを不可（4・5の可より優先）。motorwayのbike・walkは、4・5のタグがあっても不可（道路交通法）。
7. 一方通行（carとbike）：`oneway=yes/true/1` は始点→終点のみ、`oneway=-1/reverse` は終点→始点のみ、`oneway=no/false/0` は両方向。`oneway` がなければ、motorway・motorway_link・`junction=roundabout/circular` は始点→終点のみ、それ以外は両方向。`oneway=reversible/alternating` はcarを不可にする。
8. bikeの一方通行：carの一方通行に従う。ただし `oneway:bicycle=no`、`cycleway=opposite*`、`cycleway:left/right=opposite*` があれば両方向、`oneway:bicycle=yes/-1` があればそれに従う。
9. walkは一方通行に従わない。`oneway:foot=yes/-1` があるときだけ従う。

### 7.3 ノードの障害物

`barrier=bollard/block/jersey_barrier/bus_trap/chain/lift_gate/gate/swing_gate/kissing_gate/stile/turnstile/cycle_barrier/fence/wall` のノードを、次の規則で手段ごとに通過不可にする（`NODEFLG` のビット4〜6）。そのノードの `access`・`motor_vehicle`・`bicycle`・`foot` のタグがあれば、7.2節と同じ規則で上書きする。

| barrier | car | bike | walk |
|---|---|---|---|
| bollard・block・jersey_barrier・bus_trap・cycle_barrier | 不可 | 可 | 可 |
| chain・lift_gate・gate・swing_gate | 不可（`access=yes/permissive` 等で可） | 可 | 可 |
| kissing_gate・stile・turnstile | 不可 | 不可 | 可 |
| fence・wall | 不可 | 不可 | 不可 |

### 7.4 `maxspeed`

`maxspeed` が整数（`40`、`40 km/h` など）ならその値、`mph` 付きなら1.609倍して四捨五入、`JP:urban` などの記号や解釈できない値は0（不明）。`maxspeed:forward/backward` は使わない（初版）。

## 8. 所要時間のモデル（`graph.bin` の `TIMEMODL`）

所要時間の係数は、規則のコードではなくデータとして `graph.bin` に持つ。M1.7で自動車の既定速度を回帰で決め直したら、P1をやり直して `graph.bin` を作り直す（後続のファイルもすべて作り直す）。`alt.bin` の距離表もこのモデルから作るので、版の不一致は起きない。

`TIMEMODL` はJSONで、キーは次のとおり（実装は `domain::routing::graph::time::TimeModel`、`serde` で読む）。未知のキーは読込時に失敗させる。

| キー | 型 | 意味 |
|---|---|---|
| `version` | 文字列 | モデルの版（例 `jp-time-1`） |
| `speed_kmh` | `{"walk": [f64; 22], "bike": [f64; 22], "car": [f64; 22]}` | 種類（6.5節の値を添字）ごとの既定速度。通行不可の種類は0 |
| `car_maxspeed_factor` | f64 | carで `maxspeed_kmh` があるときの速度 = `min(maxspeed_kmh × factor, speed_kmh.car[class] × car_maxspeed_cap_ratio)` |
| `car_maxspeed_cap_ratio` | f64 | 上の上限の倍率 |
| `signal_penalty_s` | `{"walk", "bike", "car": f64}` | 信号のノードを通過するときの加算（秒） |
| `stop_penalty_s` | 同上 | 一時停止のノード |
| `level_crossing_penalty_s` | 同上 | 踏切のノード |
| `steps_factor_walk` | f64 | walkで階段の辺の時間に掛ける倍率（1以上） |
| `turn` | `{"walk", "bike", "car": {"right_s", "left_s", "u_turn_s", "straight_max_deg", "u_turn_min_deg"}}` | 角度による右左折コスト（秒）。進行方向の変化角が `straight_max_deg` 以下なら直進（0秒）、`u_turn_min_deg` 以上ならUターン、それ以外は右（時計回り）か左。左側通行なので `right_s ≥ left_s` |

所要時間の定義（`domain::routing::graph::time`）：

- 辺の時間 `edge_time_ds(dedge, mode)` = `length_dm / 10 ÷ (速度km/h ÷ 3.6)` 秒を10倍して切り上げた整数。walkの階段は `steps_factor_walk` 倍。通れない有向辺は `None`。
- ノードの加算 `node_penalty_ds(node, mode)`：経路の途中のノード（出発・到着の辺の端点は除く）を通過するときに、信号・一時停止・踏切の加算の最大値を足す。
- 右左折コスト `turn_cost_ds(in_dedge, out_dedge, mode)`：入る有向辺の最後の線分と出る有向辺の最初の線分の方位の差から決める。
- 経路の時間 = 辺の時間（出発・到着の辺は通った割合を掛けて切り上げ）＋ノードの加算＋右左折コストの和。
- ノードの加算と右左折コストは0以上なので、これらを除いた「辺の時間だけの最短時間」は、本当の最短時間の下界になる。楕円の切り出し（ノード単位の探索）とALTの距離表は、この下界を使う（計画書3.4節）。

初版の値（M1.7で見直す）：walk 4.8km/h（80m/分）、bike 15km/h、carはmotorway 80・trunk 50・primary 40・secondary 35・tertiary 30・unclassified/residential 25・living_street 10・service 15（km/h）。

## 9. `restrictions.bin`

右左折禁止を、「続けて通ってはいけない有向辺の列」に統一して持つ。

| 節 | 要素 | 件数 | 内容 |
|---|---|---|---|
| `DEPS` | JSON | — | `graph.bin` |
| `PARAMS` | JSON | — | relationの総数、種類別（no_*・only_*、via-node・via-way）の件数、解決できなかった件数（理由別）、`only_*` から作った禁止の数 |
| `RESTR` | 8バイト | R | `{seq_start: u32, seq_len: u16, modes: u8, kind: u8}`。禁止する有向辺の列は `SEQ[seq_start..seq_start+seq_len]`（`seq_len ≥ 2`）。`modes` は対象の手段のビット（walk=1、bike=2、car=4）。`kind` は元のrelation（0：via-node、1：via-way） |
| `SEQ` | `u32`（`DirEdge`） | — | 列の本体 |

- 列 `(d0, d1, …, dk)` は、隣り合う有向辺がつながっている（`d_i` の到着ノード = `d_{i+1}` の出発ノード）。経路の有向辺の並びが、この列を連続した部分列として含んではいけない（その手段のとき）。
- via-nodeは長さ2、via-wayは長さ3以上（via のwayが複数の辺に分かれていれば、その分長くなる）。
- `only_*` は、P2が同じ意味の `no_*` の集まりに変換する。via-nodeでは、from の有向辺からvia ノードを出る他のすべての有向辺（通行の可否によらない）を禁止する。via-wayでは、via の列の最後のノードで、to 以外に出るすべての有向辺を禁止する。
- 対象の手段：`restriction=*` はcarとbike（`except` に `bicycle` があればcarだけ。`except` に `motorcar`・`motor_vehicle` があればbikeだけ）。`restriction:motorcar`・`restriction:motor_vehicle` はcar、`restriction:bicycle` はbike。walkには適用しない。`restriction:conditional` は初版では無視して件数だけ記録する。
- `RESTR` は列の辞書順（`SEQ` の値の並び）で並べ、同じ列・同じ手段は1つにまとめる。
- 読込時に、手段ごとにAho–Corasickの照合器（`TurnAutomaton`）を作る。探索の状態は「直前の有向辺＋照合器の状態」で、照合器が禁止の列の末尾に達する遷移は通れない。照合器の状態数は、列の接頭辞の数＋1以下。

読込時の検査：列の有向辺がすべて2E未満、隣り合う有向辺がつながっていること、`modes ≠ 0`。

## 10. `topology.bin`

| 節 | 要素 | 件数 | 内容 |
|---|---|---|---|
| `DEPS` | JSON | — | `graph.bin` |
| `PARAMS` | JSON | — | `rules_version`（`jp-topology-2`）、小さな連結成分のしきい値（初期値は手段ごとに200ノード）、交差点クラスタをつなぐ辺の長さの上限（初期値20m）、双子の判定の値（`config.twin_*`・`config.twin_rule`）、件数と監査値（双子は、辺ごとの双子の数の分布を含む） |
| `SNAPCAR`・`SNAPBIK`・`SNAPFOT` | `u64`（ビット列） | ⌈E/64⌉ | 手段ごとに、出発地・目的地の吸着先にしてよい辺。辺eはワード `e/64` のビット `e%64`。その手段で通れる辺のうち、両端が、その手段の有向グラフでしきい値以上の大きさの強連結成分に入るもの |
| `NODECLS` | `u32` | N | ノードが属する交差点クラスタ番号。属さなければ `u32::MAX` |
| `CLUSTER` | 20バイト | C | `{lat_e7: i32, lon_e7: i32, node_start: u32, node_count: u16, top_class: u8, branches: u8, flags: u8, pad: [u8; 3]}`。座標はクラスタの中心（属するノードの座標の平均）。`top_class` はクラスタに接する車道の最上位の `class`（6.5節の値が最小のもの）。`branches` はクラスタに外から接する車道の辺の数（255で頭打ち）。`flags` のビット0は信号あり、ビット1は環状交差点 |
| `CLSNODE` | `u32` | — | クラスタに属するノード。クラスタiのノードは `CLSNODE[node_start..node_start+node_count]`（昇順） |
| `TWIN` | `{a: u32, b: u32}` | — | 上下線の双子の辺の組。各組を `(a,b)` と `(b,a)` の両方の向きで入れ、`(a, b)` の辞書順（`a` の昇順、同じ `a` の中は `b` の昇順）に並べる（二分探索で引く）。**1つの辺に双子は複数ありうる**（同じ `a` に複数の `b`）。同じ組の重複と `a = b` はない |
| `SWPARENT` | `{sidewalk: u32, parent: u32}` | — | 歩道の辺と、その親の車道の辺。`sidewalk` の昇順 |

- 交差点クラスタ：carで通れる辺の次数が3以上のノードを交差点ノードとし、長さが上限以下のcarの辺でつながる交差点ノードを1つのクラスタにする。交差点ノードから上限以内にある横断歩道のノード（`NODEFLG` のビット1、または横断歩道の辺の端点で車道上にあるもの）も、そのクラスタに入れる。
- クラスタに「外から入る有向辺」は、出発ノードがクラスタの外（または `u32::MAX`）で、到着ノードがクラスタの中にある有向辺。交差点のリスクは、この有向辺を通るときに1回だけ加える（計画書4.4節）。
- 双子：平行（方位の差が小さい）で逆向きの一方通行の車道の組で、同じ `ref` か `name` を持ち、間隔が一定以下のもの。多対多で、条件を満たす逆向きの辺はすべて双子になる（片方の車線が長い1本で、反対側が短い複数の辺に分かれているときは、短い辺の全部が長い辺と組になる）。照会は「辺 → 双子の辺の列」で、利用側（事故の写し `MIRROR`、候補の重複判定、幅員の判定）は、位置に対応する双子を選ぶ。判定は規則の版 `PARAMS.rules_version`（`jp-topology-2`）と `PARAMS.config` に記録する。
  - 判定（`jp-topology-2`）：どちらもcarが一方通行で、同じ `ref`（または同じ `name`。`ref` が両方にあって違うものは除く）を持つ2本の辺 `a`・`b` について、**並走の長さ**を求める。短い方の辺（長さが同じなら番号の小さい辺）の進行方向に沿って5mおきに点を取り、長い方の折れ線までの距離が60m以内で、その点での長い方の進行方向が逆向き（角度の差が150°以上＝許容30°）で、最寄りの点が長い方の端の外側にはみ出さない（長い方の延長線上にあるだけの点でない）ものの割合に、短い方の長さを掛けた値が並走の長さ。並走の長さが「短い方の長さの6割」と30mの小さい方以上なら双子にする。交差点をはさんで向かい合う辺（端でつながるだけで並走しない）は双子にならない。
  - 前の版（`jp-topology-1`）は、点を20mおきに取り、長い方の端の外側の60m以内も並走に数え、お互いが最良の相手である1対1の組だけを採用していた。本線の一方通行の辺の33.5%（長さの22.4%）で、分割の違う側の辺が余って双子がなかった。

読込時の検査：ビット列の長さ、`NODECLS` の値がC未満か `u32::MAX`、`CLSNODE` とクラスタの対応が `NODECLS` と一致、`TWIN`・`SWPARENT` の辺番号がE未満で並び順どおり（`TWIN` は `(a,b)` の辞書順の狭義昇順で、`(a,b)` があれば `(b,a)` もある）。

## 11. `odindex.bin`

出発地・目的地を辺に吸着するための格子。

| 節 | 要素 | 件数 | 内容 |
|---|---|---|---|
| `DEPS` | JSON | — | `graph.bin` |
| `PARAMS` | JSON | — | `lat0_e7`・`lon0_e7`（格子の原点、日本矩形の南西端 20°・122°）、`cell_lat_e7`・`cell_lon_e7`（初期値はどちらも25,000＝0.0025度）、`rows`・`cols` |
| `CELLKEY` | `u32` | K | 空でないセルの番号 `row×cols + col` の昇順。`row = floor((lat_e7 − lat0_e7) / cell_lat_e7)`、`col` も同様 |
| `CELLOFS` | `u32` | K+1 | 各セルの辺の開始位置 |
| `CELLEDG` | `u32` | — | セルを通る辺の番号（セル内は昇順）。辺の折れ線のいずれかの線分が通るセルすべてに登録する（外接矩形ではなく線分で判定） |

吸着（`domain::routing::snap`）：

- 対象は、その手段で少なくとも一方向に通れて、`SNAP*` のビットが立っている辺。
- 点から辺の折れ線までの距離（`contract::geo` と同じ局所平面近似）が最小の辺の、最も近い点に吸着する。同じ距離なら辺番号が小さいもの。
- 吸着の距離の上限は `SNAP_MAX_DISTANCE_M = 300`（初期値。M1で見直す）。上限内に辺がなければ `NoRoute`。
- 吸着した点は、辺番号と始点からの距離（`offset_dm`）で表す仮想ノードとして探索に入れる。

## 12. `accident_attrs.bin`

| 節 | 要素 | 件数 | 内容 |
|---|---|---|---|
| `DEPS` | JSON | — | `accidents.csv.gz` |
| `PARAMS` | JSON | — | 年ごとの列とコードの扱い、欠損・未知コードの件数（属性別・年別） |
| `ATTR` | 8バイト | A | 事故ID（行番号）の順。下表 |

| オフセット | 型 | 内容 | 欠損・未知 |
|---:|---|---|---|
| 0 | `u16` | 路線コードの上4桁（0〜9999） | `0xFFFF` |
| 2 | `u8` | 路線コードの下1桁（0：現道・包括路線、1〜9：バイパス） | `0xFF` |
| 3 | `u8` | `route_class`（下表）。上4桁から決める | `0xFF` |
| 4 | `u8` | 道路形状のコード（2024年のコード表の値：0、1、7、11〜14、21〜23、31、37） | `0xFF` |
| 5 | `u8` | 車道幅員のコード（0、1〜6、11、14、15、17、18、19） | `0xFF` |
| 6 | `u8` | 歩車道区分のコード（1〜4） | `0xFF` |
| 7 | `u8` | 地形のコード（1：市街地－人口集中、2：市街地－その他、3：非市街地） | `0xFF` |

`route_class`：

| 値 | 上4桁 | 区分 |
|---:|---|---|
| 0 | 0001〜0999 | 一般国道（上4桁が国道番号） |
| 1 | 1000〜1499 | 主要地方道－都道府県道 |
| 2 | 1500〜1999 | 主要地方道－市道 |
| 3 | 2000〜2999 | 一般都道府県道 |
| 4 | 3000〜3999 | 一般市町村道 |
| 5 | 4000〜4999 | 高速自動車国道 |
| 6 | 5000〜5499 | 自動車専用道－指定 |
| 7 | 5500〜5999 | 自動車専用道－その他 |
| 8 | 6000〜6999 | 道路運送法上の道路 |
| 9 | 7000〜7999 | 農（免）道 |
| 10 | 8000〜8499 | 林道 |
| 11 | 8500〜8999 | 港湾道 |
| 12 | 9000〜9499 | 私道 |
| 13 | 9500 | その他 |
| 14 | 9900 | 一般の交通の用に供するその他の道路 |

`route_class` が5〜7の事故を「高速・自専道のコードの事故」とする。年によってコードの体系が違う場合は、M2aで2024年のコードに写し、写せないものを未知にして `PARAMS` に件数を記録する。

## 13. `assignment.bin`

事故ごとの割り当て（計画書4.3節）。

| 節 | 要素 | 件数 | 内容 |
|---|---|---|---|
| `DEPS` | JSON | — | 3.3節の表 |
| `PARAMS` | JSON | — | `sigma0_m`（初期値3.5）、候補の距離の上限（辺35m、クラスタ30m）、重みの下限0.1、候補の上限3、尤度表、帰属不明の理由別件数 |
| `ACCOFS` | `u32` | A+1 | 事故iの割り当ては `TARGET[ACCOFS[i]..ACCOFS[i+1]]` |
| `TARGET` | 16バイト | — | `{target: u32, pos_dm: u32, weight: f32, kind: u8, flags: u8, reserved: u16}` |
| `UNASSGN` | 8バイト | U | `{accident: u32, reason: u8, pad: [u8; 3]}`。事故IDの昇順 |

- `kind`：0は辺（`target` は辺番号、`pos_dm` は始点からの位置）、1は交差点クラスタ（`target` はクラスタ番号、`pos_dm` は `u32::MAX`）。
- `flags` のビット0（`MIRROR`）：双子の辺への写し。写しは元の候補と同じ `weight` を持ち、`pos_dm` は双子の辺に事故の位置を投影した位置。
- 1件の事故について、`MIRROR` でない候補は最大3個、各 `weight ≥ 0.1`、合計は1以下（浮動小数の誤差1e-6まで）。候補は `(kind, target, pos_dm)` の昇順。
- 割り当てのある事故は `UNASSGN` に入らず、割り当てのない事故は必ず `UNASSGN` に1回だけ入る（両者は排他で、合わせて全事故）。
- `reason`：1＝候補なし（35m以内に車道の辺がなく、交差点の事故で30m以内にクラスタもない）、2＝高速・自専道のコードの不一致（候補はあるが、条件で全部外れた）、3＝重みの不足（正規化後、0.1以上の候補がない）、4＝一般交通の場所（道路形状0）、255＝その他。

起動時の持ち方：読込時に、辺ごと（辺番号→事故・位置・重み、位置の昇順）と、クラスタごと（クラスタ番号→事故・重み）の索引に組み替え、事故ごとの形は捨てる。帰属不明の事故には、72m判定のための格子索引を別に作る。

## 14. `risk.bin`

計画書5章のモデルの係数と、辺・交差点の種別。事故の観測件数 `n_e` は持たない。リクエストごとに、`assignment.bin` の索引と事故の列（手段・時間帯・曜日区分・天候・人数・source_year）から、探索で触れた辺だけ計算する（`RiskView`、作業書M3b.2）。

### 14.1 節

| 節 | 要素 | 件数 | 内容 |
|---|---|---|---|
| `DEPS` | JSON | — | 3.3節の表 |
| `PARAMS` | JSON | — | 14.4節 |
| `EDGEK` | `u8` | E | 辺の道路種別k（14.2節）。`0xFF` はリスクを持たない辺（リスク0）、`0xFE` は歩道で、親の車道（`SWPARENT`）のリスク密度を引き継ぐ |
| `CLUSJ` | `u8` | C | 交差点クラスタの種別j（14.3節） |
| `KTAB` | 48バイト | 3×256×10 | 辺の係数。添字は `(m×256 + k)×10 + cell`（mは0：walk、1：bike、2：car） |
| `JTAB` | 48バイト | 3×256×10 | 交差点の係数。添字は `(m×256 + j)×10 + cell` |

`KTAB`・`JTAB` のレコード（48バイト）：`{r: f64, s: f64, a: f64, rho: [f32; 5], pad: f32}`。

| フィールド | `KTAB` の意味 | `JTAB` の意味 |
|---|---|---|
| `r` | 基準の事故密度 R[m][k][C]：その手段が関与した、そのセルの事故の、1mあたり・1年あたりの期待件数 | 1クラスタあたり・1年あたりの期待件数 |
| `s` | 平均の深刻度 S[m][k][C]（`10×死者+負傷者` の、1件あたりの平均） | 同じ |
| `a` | 縮小の母数 a_k（1mあたり）。同じ (m,k) の10セルで同じ値 | α_j。同じ (m,j) の10セルで同じ値 |
| `rho[w-1]` | 天候 w（1〜5）の事故の割合 ρ_k(C,w) | 同じ |

使わない種別の行は、すべて0。データの少ない種別は、P6がより粗い種別の値を写して埋める（どの種別をどこへまとめたかを `PARAMS.pooling` に記録する）。

### 14.2 道路種別 k（`u8`）

`k = group×8 + band×2 + urban`（group 0〜15、band 0〜3、urban 0〜1）。

| group | `class` | group | `class` |
|---:|---|---:|---|
| 0 | motorway・motorway_link | 8 | service |
| 1 | trunk・trunk_link | 9 | road |
| 2 | primary・primary_link | 10 | track |
| 3 | secondary・secondary_link | 11 | path・bridleway |
| 4 | tertiary・tertiary_link | 12 | cycleway |
| 5 | unclassified | 13 | footway（歩道・横断歩道以外） |
| 6 | residential | 14 | pedestrian |
| 7 | living_street | 15 | steps |

band（車線・幅員帯。境界は事故原本の車道幅員の区分に合わせる）：

| band | 条件 |
|---:|---|
| 0 | 不明（`lanes`・`width` がない） |
| 1 | 狭い：`lanes=1`、または `width < 5.5m` |
| 2 | 中：`lanes` が2〜3、または `5.5m ≤ width < 13m` |
| 3 | 広い：`lanes ≥ 4`、または `width ≥ 13m` |

双子の辺は、2本の `lanes` の和で判定する（車道幅員は上下線の合計なので）。`lanes` と `width` が両方あって食い違うときは `lanes` を使う。

urban（人口集中地区か）：人口集中地区の境界データは手元になく、作業書1.3節により取得しない。代わりに、事故原本の地形コードから推定する。0.005度（約500m）の格子の各セルについて、そのセルと周囲8セルにある学習期間の事故のうち地形コードが1（市街地－人口集中）のものの割合が0.5以上で、事故が10件以上あればurban=1、それ以外は0。辺の中点が入るセルで決める。この代替は計画書から外れるので、M3bとM5の報告に書く。

### 14.3 交差点の種別 j（`u8`）

`j = top_group×8 + branch_band×2 + signal`。`top_group` はクラスタの `top_class` を14.2節のgroupにしたもの（0〜9）、`branch_band` は `branches` が3以下で0、4で1、5以上で2、`signal` は信号の有無。

### 14.4 `PARAMS`

| キー | 内容 |
|---|---|
| `train_source_years` | 学習に使ったsource_yearの昇順（係数の調整用は2019〜2022、本番は2019〜2025） |
| `years` | T（学習年数） |
| `c` | 自動車の生活道路の係数（計画書5.5節）。初期値1.0 |
| `c_groups` | cを掛けるgroup（residential・unclassified・living_street・service＝`[5, 6, 7, 8]`） |
| `beta` | 天候の縮小の母数β（手段ごと `{"walk", "bike", "car"}`） |
| `weather_multiplier` | 天候の倍率M（`{"rain": 2, "fog": 4, "snow": 4}`。現行の値） |
| `urban` | 14.2節の格子の大きさ・割合・件数のしきい値 |
| `pooling`・`fit` | 種別のまとめ方、推定の監査値（逸脱度、グリッド探索の結果） |

### 14.5 リクエストごとのリスク（`RiskView`）

手段m、セルの集合 Cset（指定がallなら該当する全セル）、天候wについて：

```text
辺e（k = EDGEK[e]、ℓ = length_dm/10、T = years）：
  R    = Σ_{C∈Cset} r[m][k][C]
  μ_e  = T·ℓ·R
  W    = T·R / (T·R + a[m][k])            // R=0 なら W=0
  n_e  = Σ 割り当て重み a_i               // 辺eへの割り当て（MIRROR含む）のうち、事故が手段mに関与し、
                                          //   セルがCsetに入り、source_yearが学習年に入るもの
  ŷ_e  = (1 − W)·μ_e + W·n_e
  S    = Σ_C s[C]·r[C] / Σ_C r[C]          // 期待件数で重み付け
  天候の項 = 1                            // 天候allまたは晴・曇
           = 1 + (M_w − 1)·ρ_e            // 雨・霧・雪：ρ_e = (n_e^w + β·ρ_k)/(n_e + β)、
                                          //   ρ_k = Σ_C r[C]·rho[C][w] / Σ_C r[C]
  risk_e = c_k · S · ŷ_e · 天候の項        // c_k は car かつ group∈c_groups のとき c、それ以外1

交差点クラスタv（j = CLUSJ[v]）：
  μ_v = T·Σ_C r_v[C]、W_v = μ_v / (μ_v + α_j)、ŷ_v = (1 − W_v)·μ_v + W_v·n_v
  risk_v = S_v · ŷ_v · 天候の項（c は掛けない）
```

- 出発・到着の辺を途中まで通るとき、その部分のリスクは `(1 − W)·μ_e·(通った長さ/ℓ) + W·(通った部分に位置がある事故の重みの和)` に深刻度と天候の項を掛けたもの。辺を2つに分けても合計が変わらない（計画書5.1節、M0.6の「分割の不変性」）。
- 歩道（`EDGEK=0xFE`）は、親の車道のリスクを親の長さで割った密度に、歩道の長さを掛ける。
- 交差点のリスクは、10章の「外から入る有向辺」を通るときに加える。

## 15. `alt.bin`

ALTの下界（計画書7.1節）。

| 節 | 要素 | 件数 | 内容 |
|---|---|---|---|
| `DEPS` | JSON | — | `graph.bin` |
| `PARAMS` | JSON | — | `unit_ds`（距離表の単位、初期値40＝4秒）、手段ごとの `landmarks`（個数L）と `directions`（`["from"]` または `["from", "to"]`）、選び方（最遠点法の始点など） |
| `LM_<M>` | `u32` | L | 手段Mのランドマークのノード番号。`<M>` は `CAR`・`BIK`・`FOT` |
| `FR_<M>` | `u16` | N×L | ランドマークからの最短時間 d(L→v)。ノード順に並べ、ノードvの値は `[v×L .. v×L+L]` |
| `TO_<M>` | `u16` | N×L | ノードからランドマークへの最短時間 d(v→L)。`directions` に `to` があるときだけ |

- 距離は、8章の「辺の時間だけ」の最短時間（ノードの加算と右左折コストを除く、その手段の通行可否と方向に従う）をデシ秒で求め、`q = floor(d / unit_ds)` を入れる。65535（`0xFFFF`）は「到達できない、または表せない」で、そのランドマークは下界に使わない。
- 下界：`d(v,t) ≥ unit_ds × (q_from[L][t] − q_from[L][v] − 1)`、`d(v,t) ≥ unit_ds × (q_to[L][v] − q_to[L][t] − 1)`。負なら0。ランドマークのうち最大のものを使う。
- 初版は、carだけに表を持つ（L=16、`from` だけ）。walk・bikeは速度がほぼ一定なので、直線距離÷その手段の最高速度の下界を使う。表を持つ手段と個数はM3bの計測で見直す。

## 16. 中間生成物（releaseに入れない）

`data/work/<マイルストーン>/` に置き、Gitで管理しない。

| ファイル | 内容 |
|---|---|
| `graph_attrs.bin`（マジックナンバー `TKRGATTR`） | 前処理だけで使う辺の属性。節 `WAYID`（`i64`×E、OSMのway ID）、`REF`（国道番号など `ref` の文字列表への添字 `u32`×E）、`NAME`（`name` の文字列表への添字 `u32`×E）、`STRTAB`（`\0` 区切りの文字列表）、`LANES`（`u8`×E、0は不明）、`WIDTHDM`（`u16`×E、デシメートル、0は不明）、`LAYER`（`i8`×E） |
| 各段階のログ・監査のJSON | 件数、除外の理由、計測値 |

## 17. 常駐メモリの見積もり【推定】

全国（N=1,476万、E=2,003万、形状点5,800万）での見積もり。M1・M4で実測する。

| 内容 | 大きさ |
|---|---:|
| `graph.bin`（`NODEOFS` 59MB、`ADJ` 160MB、`NODELL` 118MB、`NODEFLG` 15MB、`EDGE` 320MB、`SHPOFS` 80MB、`SHAPE` 232MB） | 約985MB |
| `topology.bin`（`NODECLS` 59MB、クラスタ・双子・歩道・吸着のビット列） | 約150MB |
| `odindex.bin` | 約120MB |
| `restrictions.bin` と照合器 | 数MB |
| 割り当ての索引（辺ごと・クラスタごと） | 約70〜100MB |
| `risk.bin` | 約22MB |
| `alt.bin`（car、L=16、`from` だけ） | 約472MB |
| 事故の列と格子（既存の `AccidentSnapshot`） | 約100MB |
| **合計** | **約1.9〜2.0GB** |

M1の受け入れ条件（グラフ一式のRSSが1.5GB以下）は、`graph.bin`・`topology.bin`・`odindex.bin`・`restrictions.bin` の合計で測る。探索の作業領域は部分グラフの局所番号で持ち（計画書7.2節）、同時に走る探索は③のCPU同時数の上限で抑える。全体のRSSの上限は3.2GB（作業書M4.2）。

## 18. モジュール構成と骨組み

M0で、型と関数のシグネチャだけを持つ骨組みを作った（中身は `todo!()`）。作業役は、これを埋める形で実装する。シグネチャを変える必要が出たら、実装せずに報告する。

```text
backend/src/domain/routing/
  mod.rs          RoutingData（常駐データ一式）、定数
  graph/
    mod.rs        NodeId・EdgeId・DirEdge・Coord・EdgeRecord・HighwayClass・ビット定数・Graph
    rules.rs      手段別の通行規則（7章）classify_way・classify_barrier・RULES_VERSION
    time.rs       TimeModel（8章）と edge_time_ds・node_penalty_ds・turn_cost_ds
    restrictions.rs  Restrictions（9章）・TurnAutomaton
  topology/mod.rs Topology（10章）
  snap/mod.rs     OdIndex（11章）・SnapPoint・snap
  accident/mod.rs AccidentAttr・RouteClass（12章）、Assignment・AssignTarget・UnassignedReason（13章）、
                  EdgeAccidentIndex（起動時の索引）
  risk/mod.rs     RiskModel・CoefRecord・RiskParams（14章）、RiskView、道路種別k・交差点種別jの計算
  search/
    mod.rs        Path・SearchStatus・SearchParams・ArcCost・SearchContext
    fastest.rs    最短時間経路（ノード単位の双方向A*、直前の辺を状態に持つ探索）
    ellipse.rs    Subgraph・ellipse_subgraph（楕円の切り出し）
    larac.rs      larac
    rcsp.rs       bounded_rcsp
    alt.rs        Landmarks（15章）
    candidates.rs 候補の整理（重複除去・上限6本・並べ替え）
    reduced.rs    探索範囲が大きすぎる場合（計画書6.4節）
  legality/mod.rs check_path・Violation
  engine.rs       RoutingEngine（RouteEngine の実装。collect_candidates は空の CandidateSet を返す）
backend/src/data/
  binfile.rs      3章の容器形式の読み書き（M0で実装済み）
  routing_files.rs  8ファイルと Graph などの型の相互変換（encode_* / decode_*）
  release.rs      ROUTING_FILES・SCHEMA_VERSION_ROUTING・routing_release_id（M0で実装済み）
  load.rs         schema 3の読込（load_routing_engine）
  mod.rs          load_engine が schema_version でエンジンを選ぶ（M0で実装済み）
backend/src/bin/
  routing_prep.rs   P1〜P9のサブコマンド（osm-graph、restrictions、topology、accident-attrs、
                    accident-snap、risk-fit、od-index、alt、package）
  routing_eval.rs   評価と計測のサブコマンド
```

依存方向は既存の規則（`backend/README.md`）に従う。`domain::routing` は `contract` と `domain` の既存モジュール（`navigation`・`rounding`・`snapshot`・`corridor` の75m分割）だけを使い、`data` に依存しない。ファイルの読み書きは `data` に置く。
