# Issue #26 実装計画書：新方式に合わせた表示と契約の更新

- 対象：[Issue #26](https://github.com/jphacks/tkb_2612/issues/26)「フロント：自前経路探索（新方式）に合わせた表示と契約の更新」
- 基準コミット：`origin/main` の `1c80c17`（M0〜M5、#27、#29 を含む）。Issue に書かれた行番号は `0ef4caa` のものなので、本書では `1c80c17` の行番号を使う
- 実装の体制：指示役（Claude）がタスクの指示を書き、DeepSeek V4.1 Flash（`dsh-flash`）が作業役として実装する。差分の確認、検証、コミットは指示役が行う
- 状態：項目の決定と文面は利用者の確認で確定した（2026-11-09）。未確定は1章末尾の2点

## 0. 調査で分かったこと

Issue の記述に加えて、計画に影響する事実が5つある。

1. **結果画面に説明がほとんど出ていない。** `reason`、`warnings`、`risk_note`、`risk_reduction_percent` を描画するコードが `frontend/src` にない（`demo.ts` の生成と `validate.ts` の検査だけ）。`frontend-spec.md:161` の「reason、全warnings、risk_noteを省略しない」を満たしていない。「事故0は安全の証明ではない」という警告も画面に出ない。項目1は「置き場所を足す」では足りず、説明の表示を新しく作る。
2. **`safest_id=null` は例外的な場面ではない。** 全国・関東の1,800組で、徒歩56%、自転車41%、自動車4%（M5.md 8.3節）。徒歩の半数以上で安全ルートが出ず、その理由も画面に出ない。項目5は1と合わせて考える。
3. **項目2には実装上の罠がある。** `evaluate/accidents.rs:39` の `Location::Sid` は「歩道の親の車道への割り当て」と「帰属不明」の両方を表す。`nearest` の表も両方を混ぜている（同 `:300`〜`:364`）。このまま `Sid` を帰属不明と扱うと、歩道沿いの事故を誤って帰属不明と表示する。
4. **応答例は旧方式のエンジンで検証されている。** `backend/tests/engine_fixtures.rs` は、`openapi.yaml` の全ての応答例を旧方式のエンジンが再現することを確かめる。したがって、応答例の事故点の種類はすべて「帰属不明」にし、新方式でしか起きない応答（事故0件で安全ルートあり）を応答例に足さない。
5. **M5.md 11章の Issue #26 向け文案は未投稿。** Issue にコメントは0件。項目7・8の判断材料がここにある。

## 1. 項目ごとの決定（完了の条件「変える／変えないを記録」）

| # | 項目 | 決定 | 根拠・内容 |
|---|---|---|---|
| 1 | 安全ルートの説明 | **変える** | 件数は残し、安全ルートに推定リスクの減少率を添える。`weighted_risk` の絶対値は出さない（予測が実績の約0.4〜0.85倍で、件数と誤解されるため。M5.md 11章）。説明の欄を新設する |
| 2 | 帰属不明の事故の区別 | **変える** | `AccidentFeature.properties.attribution` を必須で追加する。値は `edge`・`intersection`・`unattributed`。地図は「道路に割り当てた事故」と「帰属不明」の2種類で描き分け、3種類の区別はポップアップに出す |
| 3 | `radius_m`・`distance_weight_sigma_m` | **変える** | `distance_weight_sigma_m` を契約から外す。`radius_m` を `unattributed_radius_m` に改名する（`const: 72` のまま）。旧方式の定数 `DISTANCE_WEIGHT_SIGMA_M` は `scoring.rs` が使うので残す |
| 4 | 説明の文言 | **変える** | 仕様の「事故考慮経路（取得候補内）」と、新方式の `reason` の「取得した{N}候補」を直す（文面は3章） |
| 5 | `safest_id` が null になる条件 | **変える**（出す） | 新方式では全候補0件でも安全ルートを選ぶ。モデルの値は手段・時間帯・曜日・天候ごとに計算されるので（behavior.md 7章）、件数が0でも比較できる。「該当事故0件」の警告は残す。旧方式は今のまま null を返す。効果は未評価なので（下記）、**他と切り離したタスク（B）にする** |
| 7 | 高リスク区間のしきい値 | **変えない** | M5 で3件に決定。`minimum: 3` と各所の検査はそのまま |
| 8 | 経路の長さと頂点数の上限 | **変えない** | `route_too_long` は1,800組中1組、80km超の候補0本、間引き0本、頂点数の最大は2,968（M5.md 8.3節）。入力の直線距離30km以下の範囲で上限に当たることはほとんどない。長距離の利用を想定するときに見直す |

項目6（バランス案のラベル）は、利用者の判断で不要とし、Issue #26 から削除した。別 Issue も作らない。項目7・8の番号は、M5.md などから参照されているので詰めない。

### 項目5の注意点

全候補0件の場面での効果は未評価である。M5 の削減率は条件の指定なしで測ったものなので、件数が0の場面で遠回りが事故を減らすかは確かめていない。また、裏道への誘導（M5.md 0.3節）と平均+8〜9分の遠回りを勧めることになる。件数0件を表示し、選ぶかどうかは利用者に任せる。

### 未確定の点（着手は妨げない。PR の前に確認する）

- 契約の変更について、②・③の担当者と合意する方法（PR のレビューでよいか）。`backend/src/api/` は③の範囲である。
- 本番に配備済みかどうか。`api_version` は `2.0` のままにする（説明に「未リリースの統合契約版」とあるため）。配備済みなら、古い画面を開いたままの利用者には `additionalProperties: false` により一時的に `invalid_response` が出る。バックエンドとフロントは同じイメージで同時に更新されるので、再読み込みで解消する。

## 2. 契約の変更

`docs/api/openapi.yaml` を正本とする。

```yaml
# AccidentFeature.properties（required に attribution を追加）
attribution:
  type: string
  enum: [edge, intersection, unattributed]
  description: >-
    事故を経路に結び付けた方法。edge は道路の区間（歩道の親の車道を含む）、
    intersection は交差点、unattributed はどの道路の事故か判断できず、
    経路から unattributed_radius_m 以内にあるもの。旧方式は常に unattributed。

# RouteResponse
unattributed_radius_m: {type: number, const: 72, description: 帰属不明の事故を数える経路からの範囲、メートル。旧方式では事故を数える半径}
# distance_weight_sigma_m は properties・required・応答例から削除
```

項目5の相関制約の変更：

| 場所 | 今 | 変更後 |
|---|---|---|
| `frontend/src/api/validate.ts:96` | `allEmpty !== (safest_id === null)` なら拒否 | `safest_id === null && !allEmpty` なら拒否 |
| `backend/src/api/route_response.rs:191` | `safest.is_some() != any_accident` なら違反 | `safest.is_none() && any_accident` なら違反 |
| `backend/src/domain/routing/evaluate/mod.rs:290` | `any_accident.then(...)` | 常に選ぶ（旧方式の `domain/engine.rs` は変えない） |
| `backend/src/domain/routing/evaluate/mod.rs:421` | 両方向の検査 | `(false, true)` だけ残す |
| `backend/src/testing/fixtures.rs` の `check_outcome_invariants` | 両方向 | 同じく一方向（`backend/tests/connection.rs:161` の期待も合わせて確認する） |

`extra_minutes`・`risk_reduction_percent` の検査は変えない。`safest` があれば `extra_minutes` は必ずある。`risk_reduction_percent` は基準リスクが0なら null のままである。

## 3. 確定する文面

Flash には文面を考えさせず、次をそのまま使わせる。

| 場所 | 場面 | 文面 |
|---|---|---|
| 新方式の `reason` | 通常（`evaluate/mod.rs:320`） | 道路網を探索して得た{N}候補のうち、最短時間経路から+{d}分以内で、推定リスクが最小の経路を選びました。追加{x}分、リスク{y}%減です。 |
| 新方式の `reason` | 全候補0件で2経路が異なる（項目5） | 全候補で該当する過去の事故はありません。道路の種類ごとの平均から推定したリスクが最小の経路を選びました。追加{x}分、リスク{y}%減です。 |
| カード | 2経路が異なる安全ルート | 推定リスク {p}%減 |
| カード | 同上で `risk_reduction_percent` が null | 推定リスクの減少率は算出できません |
| 説明の欄 | `safest.accident_count > fastest.accident_count` | 安全ルートは、過去事故の件数が最短ルートより多くなっています。件数は道路の種類による偶然のばらつきを含むため、推定リスクでは低い経路を選んでいます。 |
| 説明の欄 | `safest_id` が null（旧方式だけ） | 全候補で該当する事故がないため、安全ルートは選んでいません。 |
| ポップアップ（項目名「結び付け」） | `edge` | 道路の区間 |
| 同上 | `intersection` | 交差点 |
| 同上 | `unattributed` | 道路を特定できず（経路から{r}m以内） |
| 凡例 | 割り当てあり／帰属不明 | 経路上の事故／経路の近くの事故 |
| `DataAttribution.tsx` | 冒頭の文 | 「事故件数と重み」を「事故件数と推定リスク」に替える |

「同じ経路」「全候補0件」の警告と、`risk_note` の文面は変えない。

## 4. 画面の設計

- **カード**（`RouteCallouts.tsx`）：`.map-card-accidents` の下に、3章のカードの1行を足す。安全ルートで2経路が異なるときだけ出す。`978dcfe` で距離と追加時間の行を意図して消しているので、それらは戻さない。追加時間は `reason` で伝える。`cardHeight`（112/116/128）を1行分増やす。
- **説明の欄**（新規 `RouteResultNotes.tsx`）：`App.tsx` の `.map-stage` の中に、`RouteMap` と並べて置く。地図が使えないときも見えるようにするためである。ナビ中は出さない。
  - 常に表示：`warnings` の全件、件数の逆転の注記
  - `<details>`「この結果について」の中：`reason`、`risk_note`、候補数、帰属不明の範囲、`aggregation_ms`（「集計時間」と書き、通信時間と混同させない）、対象データ年
  - 480px未満では幅いっぱいに広げ、凡例とカードに重ならないようにする。重なりは人の確認で見る（タスクF）
- **地図**（`map/layers.ts:100` の `route-accidents-point`）：`["get","attribution"]` で描き分ける。帰属不明は半径5、灰色の枠（色は既存のトークンに合わせる）。色だけに頼らないように、大きさも変える。地図プレビュー（`RouteMap.tsx:651` の SVG）も同じ規則にする。

## 5. 進め方

### 準備（指示役）

1. `origin/main` から統合ブランチ `feat/issue-26-route-display` と worktree `data/work/wt/i26` を作る。**今の作業ツリー（`feat/routing-m0`、未コミットの変更あり）には触れない。** タスクごとに子ブランチと worktree（`data/work/wt/i26-a` など）を切る。
2. `data/work/M5/tasks/run-flash.sh` と `launch.sh` を `data/work/i26/tasks/` に写し、パスを直す（DeepSeek の一時的な障害で同じセッションを再開する仕組み）。
3. worktree で `npm ci` と基本の検証（6章）を実行し、**変更前の結果を記録する**。今の失敗を Flash のせいにしないためである。

### タスクの順序

```text
A 契約の変更（表示は変えない）
├─ B 項目5と reason の文面（backend ＋ validate.ts）
└─ C カードと説明の欄（項目1・4）
     └─ D 地図の描き分け（項目2の表示）
E 仕様の更新と記録（指示役）→ F 実データと人の確認 → PR
```

BとCは並行できる。Dは `RouteMap.tsx` が衝突するのでCの後にする。各タスクの後に指示役がレビューし、1タスク1コミットで統合する。

### 全タスクの共通の規則（Flash への指示に必ず入れる）

`data/work/M5/tasks/common.md` を基にし、次のように変える。

- 作業は与えられた worktree の中だけで行う。`git commit`、`push`、`stash` をせず、`.git` と git の設定に触れない。
- **生成物を手で編集しない。** `frontend/src/api/generated/` は `npm run generate:api`、`backend/tests/fixtures/route-responses.json` は `python3 -B backend/scripts/extract_route_examples.py` で作り直す。
- **テストを消さない、弱めない。** 期待値を変えてよいのは、タスクに挙げたものと、必須項目の追加・削除に機械的に伴うものだけである。それ以外のテストが落ちたら、直さずに報告する。
- 文面は3章のものをそのまま使う。
- 依存を足さない（`package.json`、`Cargo.toml`）。Rust は1.88で使える API に限る。旧方式のエンジンの動作を変えない。
- `backend/tests/boundaries.rs` の依存の制約を守る（`domain` は `api`・`data`・`testing` に依存しない）。
- 判断に迷ったら推奨に従い、最後の報告の「推測で決めた点」に書く。報告には、変更ファイル、検証コマンドの実際の出力、できなかった点、未解決の点を書く。

### タスクA：契約の変更（項目2・3の契約と backend）

**目的**：2章の `attribution` の追加、`distance_weight_sigma_m` の削除、`radius_m` の改名を、CI が通る状態で全体に通す。画面の見た目は変えない。

**変更する所**

1. `docs/api/openapi.yaml`：スキーマと6つの応答例。事故点は270件ほどあるので、スクリプトで変換する。`day_type` は `effective_conditions` にも現れるので、事故点の `properties` の中だけに `attribution: unattributed` を足す。
2. 生成物：`route-responses.json`、`frontend/src/api/generated/*`。
3. `backend/src/contract/engine.rs`：`pub enum Attribution { Edge, Intersection, Unattributed }` と文字列表現を足し、`AccidentPoint` に `attribution` を足す。`AccidentPoint` は `Copy` のままにする。
4. `backend/src/domain/scoring.rs:132` の `accident_point`：`Unattributed` を入れる（旧方式はこれで終わり）。
5. `backend/src/domain/routing/evaluate/accidents.rs`：
   - `Location::Sid` を `SidewalkParent { sid }` と `Unassigned { sid }` に分ける。`nearest` の表の値でも、どちらかを区別する。距離とリスク区間の計算（同 `:639`）の動作は変えない。
   - `summarize` で `Edge`・`SidewalkParent` → `Edge`、`Cluster` → `Intersection`、`Unassigned` → `Unattributed` を入れる。`summarize` は今 `ids` しか受け取らないので、`locations` も渡す。呼び出し元に合わせてシグネチャを変える。
6. `backend/src/api/route_response.rs`：DTO の `AccidentPropertiesDto` に足し、`radius_m` を改名し、`distance_weight_sigma_m` を外す。
7. `backend/src/testing/fixtures.rs:268`：`attribution` を読み、`Unattributed` であることを含めて比較できるようにする。
8. `frontend/src/api/demo.ts:387-388`：改名と削除。デモの事故点には3種類を混ぜる。`frontend/src/test` などの型エラーを機械的に直す。

**足すテスト**（`backend/tests/routing_engine.rs`）：

- 辺・クラスタ・歩道の親・帰属不明の事故を持つ経路で、順に `Edge`・`Intersection`・`Edge`・`Unattributed` になること
- **歩道の親の事故が `Unattributed` にならないこと**（0章の3の回帰防止）

**受け入れ**：6章の全項目。`engine_fixtures` は変更なしで通ること。

### タスクB：項目5と `reason` の文面

**目的**：2章の相関制約の変更と3章の `reason` を入れる。

**変更する所**：2章の表の5か所。`evaluate/mod.rs` の `None` の枝は残らない。全候補0件で2経路が異なるときの文面を分ける。同じ経路なら `SAME_ROUTE_REASON` のまま。`warnings` の全候補0件の文は残す。`frontend/src/api/demo.ts` の `reason` は旧方式の模擬なので変えない。

**期待値を変えてよいテスト**：

- `backend/tests/routing_engine.rs:935` `all_zero_accidents_selects_no_safest_route`：`safest` が選ばれることへ改め、関数名も変える。警告が残ることを確かめる
- `backend/tests/connection.rs:161`：片方向になる検査の期待

**足すテスト**：

- `validate.test.ts`：全候補0件で `safest_id` がある応答を受け付ける。件数があって `safest_id` が null の応答を拒否する（既存）
- `backend/tests/api_route_dto.rs`：`render_route_response` の同じ2通り
- `routing_engine.rs`：全候補0件で2経路が異なるときの `reason`、`extra_minutes`、`navigation` が付くこと
- 旧方式の全候補0件が今も null であること（`backend/tests/selection.rs:199` が変更なしで通ること）

**応答例を足さない**（0章の4）。

### タスクC：カードと説明の欄（項目1・4）

**目的**：4章のカードと `RouteResultNotes` を作る。

**変更する所**：`RouteCallouts.tsx`、`map.css`、新規 `RouteResultNotes.tsx` とそのテスト、`App.tsx`、`DataAttribution.tsx`。

**足すテスト**（`RouteResultNotes.test.tsx`、`App.test.tsx`）：

- `warnings` の全件が開かなくても見えること
- 2経路が異なるときだけカードに減少率が出ること。null を「0%」にせず3章の文面になること
- 件数の逆転の注記が `safest > fastest` のときだけ出ること（同数では出ない）
- `weighted_risk` の値が画面に出ないこと
- ナビ中は欄が出ないこと、地図が使えないときも出ること
- `reason` などを HTML として解釈しないこと（文字列として描画する）

**注意**：カードの高さの変更で、`RouteMap.test.tsx` や `App` の配置の試験が落ちたら報告する。文字列の照合で済む範囲を除いて直さない。

### タスクD：地図の描き分け（項目2の表示）

**目的**：4章の地図、ポップアップ、凡例を作る。

**変更する所**：`map/layers.ts`、`RouteMap.tsx`（`AccidentPopup` に行を足す。プレビューの SVG、凡例）、`map.css`。ラベルの対応は `components/` に小さな関数として置く。

**足すテスト**：

- `RouteMap.test.tsx`：3種類のポップアップの文面、`{r}` に `unattributed_radius_m` の値が入ること、凡例
- `layers` の試験：`route-accidents-point` の描画式が `attribution` を参照すること。既存の試験の形式に合わせる
- 帰属不明の事故が非表示の切り替え（`showAccidents`）でも一緒に消えること

### タスクE：仕様の更新と記録（指示役が書く）

文面の判断を含むので、Flash に任せない。

- `docs/api/behavior.md`：冒頭の「契約は変えない」（`:18`）を今回の変更に合わせる。7章の事故点に `attribution`、8章の `safest_id`（`:359` は旧方式の規則として残し、`:382` を変更）、`:375`・`:392` の固定値、`reason` の文面、末尾の「利用者の確認を経て確定する」（`:613`）。
- `docs/frontend-spec.md`：`:151`（項目4）、`:152`、`:159` の `weighted_risk` の表示、`:160`、`:161`、`:163`（項目3）、`:173-174`、`:207` の事故レイヤー、`:390` の試験の一覧。
- `openapi.yaml` の `description`：`:49`、`safest_id`、`reason`。`description` の変更でも `generate:api` を実行する。
- `docs/routing-reports/` は当時の記録なので直さない。`docs/routing-plan.md` の「フロントを変えない」も同様である。
- Issue #26 へのコメントは利用者の承認を得てから行う。コメントの内容は、M5.md 11章の文案と、本書1章の決定表。

### タスクF：実データと人の確認（指示役＋利用者）

**実データ**：本番用の release `data/work/wt/m5/backend/data/releases/aeb32e87dd4c74d419182432a1a7e6a605dbe5f71307145fc3b913b75148d753` を使う。

1. `backend/tests/routing_real_release.rs` の `#[ignore]` の試験を実行する。
2. `data/work/M5/m58/engine-kanto4.jsonl` などの OD から、徒歩・自転車・自動車の各100組ほどをサーバへ問い合わせ、応答を確かめる。
   - 全応答が `validate.ts` と同じ規則を通ること（`check_outcome` の違反が0件）
   - `safest_id` が null の応答が0件
   - `attribution` の内訳。`unattributed` の事故点が経路から72m以内にあること（歩道の親の区別はタスクAの試験で確かめる）
   - 処理時間が M5 と変わらないこと（99%点8.2秒）

**人の確認**（M4.md 7章の手順で、`dev_stack`、backend、`VITE_API_MODE=live` のフロントを起動する。release の指定は上記に替える）：

| 見ること | 例 |
|---|---|
| 件数の逆転の注記と、その説明で意味が通じるか | 横浜市の自動車（14.9%で起きる） |
| 全候補0件の安全ルートの見え方（項目5） | 千葉市の徒歩、夜・雨 |
| 帰属不明の事故の見分けやすさ、ポップアップ | 東京23区の徒歩 |
| 幅360pxでカード・説明の欄・凡例が重ならないこと | スマートフォン幅 |
| 再検索とナビの開始が今までどおり動くこと | 全手段 |

## 6. 検証コマンド（CI と同じ。各タスクで全項目）

repo root から実行する。

```sh
cargo fmt --manifest-path backend/Cargo.toml --check
cargo clippy --manifest-path backend/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path backend/Cargo.toml --locked
python3 -B backend/scripts/extract_route_examples.py --check
(cd frontend && npm run generate:api && git diff --exit-code -- src/api/generated)
(cd frontend && npm run typecheck && npm test && npm run build)
```

`generate:api` の差分の検査は、生成物をコミットした後でなければ意味がない。Flash はコミットできないので、作業中は `git status` で生成物の変更を確かめ、指示役がコミットの後でもう一度実行する。CI の Node は22、手元は24なので、最後に PR の CI で確かめる。

## 7. 指示役のレビューの観点

Flash の報告をそのまま信じず、差分とコマンドの出力を自分で確かめる。

- `.git` が書き換えられていないこと（`.git` の指す先、`.git-local` がないこと）
- テストの削除、`#[ignore]`、`it.skip`、期待値の変更がタスクの許す範囲か（`git diff` を `tests/` と `*.test.*` で見る）
- 生成物が、生成コマンドの出力と一致すること（もう一度実行して差分がないこと）
- 項目2：`Sid` の分割で、歩道の親と帰属不明を取り違えていないこと
- `evaluate` のホットパスに、経路を毎回走査するような処理が入っていないこと（M4 で実際に起きた）
- `frontend/src` の表示で、`weighted_risk` の絶対値や「事故の予想件数」のような表現が出ていないこと
- 旧方式のファイル（`domain/{engine,scoring,candidates,corridor,navigation}.rs`）の差分が、`attribution` を1つ足すだけであること
- `reason`、`warnings`、`risk_note` を HTML として描画していないこと

## 8. リスク

| リスク | 対応 |
|---|---|
| 項目5で、件数0の場面で効果の分からない遠回りを勧める | 件数0件の表示と警告を残す。タスクBを1コミットにして戻せるようにする。結果を人の確認で見る |
| 説明の欄が狭い画面で地図を覆う | 既定は閉じた状態にする。常に見せるのは `warnings` と逆転の注記だけ。タスクFで360pxを確かめる |
| 他の担当者が同じファイルを変える（`RouteMap.tsx`、`App.tsx` は #27・#29 で大きく変わった） | PR の前に `origin/main` を取り込む。先行 PR があれば worktree を作り直す |
| 契約の変更が古い画面を拒否させる | 1章のとおり。同じイメージで同時に更新し、再読み込みで解消する |

## 9. 完了の条件（Issue #26 より）

- [ ] 項目1〜5、7、8の決定が Issue #26 に記録されている（本書1章を投稿）
- [ ] 変える項目（1〜5）について、`openapi.yaml`、生成物、フロント、③の検査、`frontend-spec.md`、`behavior.md` が揃って更新されている
- [ ] PR の CI がすべて通っている
- [ ] タスクFの人の確認が合格している
