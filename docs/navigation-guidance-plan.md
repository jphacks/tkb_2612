# ナビの進行方向案内（Issue #22 の残り）：実装計画書

| 項目 | 内容 |
|---|---|
| 作成日 | 2026-11-09 |
| 対象 | Issue #22「Navi機能の改善」のうち、PR #29 で残ったバックエンドとAPI |
| 基準 | `origin/main`（`1c80c17`）。本書の行番号はこのコミットのもの |
| ブランチ | `feat/issue-22-guidance`（worktree `/home/hayato/tkb_2612-i22`） |
| 体制 | Claude（Opus）が指示・検証・判断を担当し、DeepSeek V4.1 Flash（`dsh-flash`）が実装を担当する |
| 状態 | 承認済み（2026-11-09、利用者の回答「新方式だけでOKです、T0から進めてください」） |

---

## 1. 要約

PR #29 で、案内バーの画面と進捗に応じた更新はできている。ただし、右左折の案内地点はデモの模式図から作っており、実APIには案内地点がない。そのため、実経路では常に「経路に沿って進む」と表示される。

本計画では次の3つを行う。

1. 選定経路の `navigation` に、案内地点の列 `guidance` を追加する（API 2.0 の契約変更）。
2. 新方式エンジン（`RoutingEngine`）が、道路グラフから右左折を判定して `guidance` を作る。OSRMは呼ばず、releaseも作り直さない。
3. フロントはデモ専用の分岐をやめ、実機GPSでも模擬走行でもAPIの `guidance` を使う。

### 現状と残作業

| Issue #22 の完了条件 | 現状 | 残作業 |
|---|---|---|
| 進行方向と案内地点までの距離が表示される | デモだけ表示される | 実APIの案内地点を追加する（T1〜T5） |
| ナビ案内と危険区間の警告が適切に表示される | PR #29 の上部バーで実装済み | 実データでの確認（T7） |
| 移動に応じて案内内容が更新される | `nextGuidance` で実装済み。デモだけで動く | 実データへの接続（T3） |

---

## 2. 決定事項

| # | 決定 | 理由 |
|---|---|---|
| D1 | **実際の案内地点を作るのは新方式（schema 3）だけとする。旧方式（schema 2）は `arrive` 1件だけを返し、`steps=false` を維持する。** | 旧方式は新方式へ置き換える予定で（Issue #26）、本番用の schema 3 release（`aeb32e87…`）もある。`steps=false` は5つの文書の6か所で固定している |
| D2 | 右左折の判定はサーバで行う。形状の角度だけでは判定せず、グラフの分岐を使う | 道路のカーブを「右折」と誤案内しないため。PR #29 の「実APIでは曲がり角を推測しない」という方針とも一致する |
| D3 | release の形式は変えない | 既存の `graph.bin`（辺・種別・手段別の通行可否）と `topology.bin`（交差点クラスタ）で判定できる。全国の前処理をやり直さずに済む |
| D4 | `api_version` は `2.0` のままとし、`guidance` を必須項目にする | 2.0 は未リリースの統合契約で、フロントとバックエンドは同じイメージで配備する |
| D5 | 道路名は案内に含めない | release に道路名がない（`graph_attrs.bin` は前処理専用）。Issue の「○○m先を右折」は道路名なしで満たせる |
| D6 | 音声案内は今回は追加しない | Issue の条件に含まれない。音声は危険区間の通知専用という現在の整理を保つ |

**D1 の影響：** 旧方式の release で動く環境（README の Docker 手順、現在の `acciless-demo`）では、案内は「経路に沿って進む」のままになる。#22 の機能が見えるようになるのは schema 3 の release で配備してからである。旧方式にも実案内が必要な場合は、OSRM の `steps=true` への変更（13章）を追加する。

この決定は、`docs/spec-decisions.md:119-126` の「段階Cは採用しない」「曲がり角ごとの案内は行わない」を一部変更する。T6 で決定の記録を追加する。自動再検索とOSRMの `steps` 取得を行わない方針は変えない。

---

## 3. API契約

`NavigationData` に `guidance` を追加する（`docs/api/openapi.yaml:2331`）。

```yaml
guidance:
  type: array
  minItems: 1
  maxItems: 14999
  items: {$ref: '#/components/schemas/GuidanceStep'}

GuidanceStep:
  type: object
  required: [maneuver, at_m]
  additionalProperties: false
  properties:
    maneuver:
      type: string
      enum: [left, right, slight_left, slight_right, uturn, arrive]
    at_m:
      type: number
      minimum: 0
      maximum: 110000
      description: 経路始点から案内地点までの距離、m。vertex_distances_mと同じ軸で、丸めない。
```

不変条件はバックエンドの2か所（`domain/routing/evaluate/mod.rs` の `check_navigation`、`api/route_response.rs` の `check_navigation`）とフロント（`api/validate.ts` の `validateNavigation`）で同じ内容を検査する。

- 1件以上ある。
- 最後の要素だけが `arrive` で、その `at_m` は `length_m` と完全に一致する。
- `at_m` は有限で、0以上 `length_m` 以下、非減少である。

### サイズの上限

`id` を持たせないのは、サイズを抑えるためである。IDはフロントの通過履歴にしか使わないので、フロントが `route-N:guide-K` を作る。

1件の最大は60バイト（`{"maneuver":"slight_right","at_m":<最大24文字>},`）で、1経路の上限は14,999件とする。15,000頂点の制限は間引いた後の `display` にかかるもので、案内は間引く前の経路から作るため、頂点数からは件数を導けない。根拠は次のとおりである。

- 分岐で始まる連の先頭どうしは、経路に沿って15mより離れる（手順2。15m以内なら同じ連になる）。候補の距離の上限は80kmで、局所平面の保守的な全長110,000mでも `110000/15 ≈ 7,334` 件である。
- 分岐なしで連を始める引き返しと環状交差点の入口は、この間隔を保証しない。これらは経路にまれにしか現れないので、上の見積もりの約2倍を上限とした。厳密な保証ではない。
- 上限を超えた場合は `render_route_response` が検査で拒否する。案内を切り捨てた応答は返さない。実データの最大は1kmあたり12.41件で、80kmでも1,000件程度である。

追加量は `2*14999*60 = 1,799,880` バイトである。

- `navigation` の上限は `4,967,344 + 1,799,880 = 6,767,224` より、`5,000,000` から **`6,800,000`** に上げる。変更箇所は `api/route_response.rs:27`、`frontend/src/api/validate.ts:225`、`openapi.yaml:2364`、`docs/api/behavior.md` のサイズの節。
- 応答全体の理論上限の小計は `10,847,344` から `12,647,224` になる。これは全頂点が案内地点で、かつ区間が最多になる、実際には起きない組み合わせの値である。負荷試験の基準12MBと180秒の見積りは変えない。T7 の実測で、実際の案内データの大きさを記録する。

---

## 4. 案内地点の判定規則

新規モジュール `backend/src/domain/routing/evaluate/guidance.rs` に実装する。入力は `graph`、`topology`、`path`、`shape`、`mode`、`vertex_distances_m`、`cancel` とする。

角度は時計回りを正（右折が正）とし、`graph::time::turn_angle_deg` と符号を揃える。定数はファイル先頭に名前付きで置く。本章は T7 の実データでの調整を反映した最終の規則である（経緯は `docs/routing-reports/guidance.md`）。

| 定数 | 初期値 | 意味 |
|---|---|---|
| `BEARING_BASELINE_M` | 20 | 連の前後の方位を測る長さ |
| `RUN_GAP_M` | 15 | 同じ連にまとめる、分岐ノード間の距離 |
| `RUN_MAX_M` | 60 | 連の長さの上限（環状交差点を除く） |
| `FORK_MAX_DEG` | 45 | 分岐の相手とみなす出口の角度 |
| `STRAIGHT_MAX_DEG` | 10 | これより小さい連の角度はほぼ直進とみなす（T7 で追加） |
| `STRAIGHT_FORK_MAX_DEG` | 20 | ほぼ直進のときに分岐の相手とみなす出口の角度（T7 で追加） |
| `TURN_MIN_DEG` | 45 | 右折・左折にする下限 |
| `SLIGHT_MIN_DEG` | 30 | 斜めにする下限 |
| `OBVIOUS_MAX_DEG` | 60 | 道なりとして省略できる上限 |
| `UTURN_MIN_DEG` | 150 | Uターンにする下限 |
| `END_SUPPRESS_M` | 10 | 到着の直前の案内を省く距離 |
| `START_SUPPRESS_M` | 10 | 連の先頭がこれより始点に近ければ案内しない距離（T7 で追加） |

### 手順

**手順1：他の出口を求める。** 中間ノード `v = dedge_to(dedges[i])`（`i = 0..n-2`）ごとに、`others(i)` を求める。`graph.out_dedges(v)` のうち、次をすべて満たす有向辺である。

- `graph.passable(d, mode)` が真。
- `dedges[i+1]` でも `dedges[i].reversed()` でもない。
- 行き先のノードが `node_blocked(.., mode)` でない。

右左折禁止の規制は考慮しない。曲がれなくても、道路は目に見えて分岐しているためである。同じ理由で、逆向きにしか通れない道路（`passable(d.reversed(), mode)` だけが真。行き先のノードの条件は同じ）を `one_ways(i)` として別に求める。手順2・手順4の「`others` が空」は `others` と `one_ways` の両方が空という意味で、道なりの判定も両方を見る。分岐の判定だけは、進めない道路を選択肢にしないように `others` だけを見る。

**手順2：連にまとめる。** 中間ノードを順に見て、次のどれかに当たれば直前の連に加える。

- (a) 直前のノードが連の末尾で、同じ交差点クラスタである（`topology.cluster_of` が両方 `Some` で一致）。
- (b) `others` が空でなく、直前の分岐ノードから経路に沿って `RUN_GAP_M` 以内である。
- (c) `dedges[i]` に `edge_flags::ROUNDABOUT` がある。

(a)〜(c) に当たらないノードは、`others` が空でなければ新しい連を始める。`others` が空なら無視し、連も切らない。ただし、環状交差点の入口（`dedges[i+1]` に `ROUNDABOUT` があり `dedges[i]` にないノード）は、`others` が空でも新しい連を始める。こうすると、環の曲線ではなく環に入る前の道路で方位を測れる。同じ辺を引き返すノード（`dedges[i+1] == dedges[i].reversed()`）も、行き止まりで案内を落とさないように、`others` が空でも新しい連を始める。環状交差点を含む連には、環を出た後のノードを (a)(b) に当たっても加えない（案内地点を出口に保ち、後続の曲がりと合成しないため）。(c) 以外で連の長さが `RUN_MAX_M` を超えるときは、新しい連を始める。

**手順3：角度を測る。** 連の先頭ノードの手前 `BEARING_BASELINE_M` の方位から、末尾ノードの先 `BEARING_BASELINE_M` の方位までの変化を、連の角度 `a` とする。`shape.full` と `shape.projection` で測る。経路の端までの長さが足りなければ、ある分だけで測る。環状交差点の連とその隣の連は、前後どちらの方位も相手の連の境界を越えて測らない（環の曲線や、環のすぐ前後の曲がりが合成されて向きを誤るため）。

**手順4：分類する。** 上から順に評価し、最初に当たったものを採用する。

| 条件 | 結果 |
|---|---|
| 連の中に `dedges[i+1] == dedges[i].reversed()` がある | `uturn` |
| 連の全ノードの `others` が空 | 案内なし（ただのカーブ） |
| `|a| ≥ UTURN_MIN_DEG` | `uturn` |
| `|a| < OBVIOUS_MAX_DEG`、`|a| ≥ SLIGHT_MIN_DEG`、かつ「道なり」 | 案内なし |
| `|a| ≥ TURN_MIN_DEG` | `right`（`a > 0`）／`left` |
| `|a| ≥ SLIGHT_MIN_DEG` | `slight_right`（`a > 0`）／`slight_left` |
| `|a| < SLIGHT_MIN_DEG` で「分岐」に当たる | 下記の向きの `slight_*` |
| それ以外 | 案内なし（直進） |

- **道なり：** 連の入る辺と出る辺の道路区分が同じで、`others` と `one_ways` のすべてが出る辺より下位の道路である。区分は `risk::road_group(class)` で比べ、`None` は最下位とする。
- **分岐：** `others` のうち、`turn_angle_deg(入る辺, other)` の絶対値が上限未満で、道路区分が出る辺と同じか上位のものがある。上限は、`|a| < STRAIGHT_MAX_DEG` なら `STRAIGHT_FORK_MAX_DEG`、それ以外は `FORK_MAX_DEG` とする。出る辺が `_link` でないときは、`_link` の出口を数えない（本線を走る間はランプの出口を案内しない）。
- **分岐の向き：** `|a| ≥ STRAIGHT_MAX_DEG` なら `a` の符号で決める。ほぼ直進なら、その出口の角度が経路の局所角より大きければ、経路は左側の枝なので `slight_left`、小さければ `slight_right` とする。

**手順5：距離を決める。** 案内地点は、連の中で局所角 `|turn_angle_deg(dedges[i], dedges[i+1])|` が最大のノードとする（同値なら先のもの）。環状交差点の連は末尾のノードとする。`shape.internal_node_vertex(i)` で `full` の頂点番号を取り、下記の方法で `at_m` に変換する。

**手順6：仕上げる。** `at_m ≥ length_m - END_SUPPRESS_M` の案内と、連の先頭が始点から `START_SUPPRESS_M` 未満の案内を除き、最後に `arrive`（`at_m = length_m`）を加える。始点の近くは手前の方位を測れず、向きを誤るためである。

### 距離軸への変換

`vertex_distances_m` は、応答の `display` の頂点に対応する。15,000頂点を超えて間引いた経路では、`full` と `display` の頂点番号が一致しない。そこで次のように変換する。

- `douglas_peucker` が残した頂点の番号を返すように変え、`RouteShape` に `display_source: Option<Vec<usize>>`（間引いていなければ `None`）を追加する。
- 間引いていなければ、`vertex_distances_m[j]` をそのまま使う。
- 間引いていれば、`j` を含む `display` の区間を二分探索で求め、`full` の累積距離の比率で区間内を線形補間する。

---

## 5. Flash への指示の進め方

| 項目 | 方法 |
|---|---|
| 実行 | `cd /home/hayato/tkb_2612-i22 && dsh-flash - <<'TASK' ... TASK`。Bash の timeout は 600000 |
| 仕様の渡し方 | Flash は会話を見ないので、本書を worktree に置き、節番号で参照させる。指示文は8章のものを使う |
| コミット | Flash は `git commit`・`git push` をしない。Claude が検証後にタスクごとにコミットする |
| Flash の検証範囲 | 全件の `cargo test --no-fail-fast` まで実行させる（全件で約23秒。T1 で対象を絞って見落としがあったため変更した）。Claude も同じコマンドを実行する |
| 検証 | Flash の報告は下書きとして扱う。Claude が差分を読み、合格条件のコマンドを自分で実行する |
| やり直し | 不合格なら、失敗の出力と具体的な指摘を添えて再指示する。2回続けて不合格なら Claude が直接直す |

### 差分を読むときに必ず確かめること

- テストの削除、`#[ignore]` や `it.skip` の追加、期待値を実装に合わせただけの変更がない。
- 生成物を手で編集していない（`backend/tests/fixtures/route-responses.json`、`frontend/src/api/generated/`）。
- 指定外のファイルを変更していない。無関係な整形やリファクタリングがない。
- 旧方式の探索結果、経路の選定、リスク値に影響する変更がない。

---

## 6. タスク

依存関係：`T0 → T1 → T2 → T3`、`T1 → T4 → T5`、`T3・T5 → T6 → T7 → T8`。worktree が1つなので、順番に実行する。

各タスクの完了後も、バックエンドの実データでの動作は T5 まで変わらない。T1〜T3 の時点では、実APIの案内は `arrive` だけで、画面は今と同じ「経路に沿って進む」のままである。

### T0：準備（Claude）

- worktree を作る：`git worktree add ../tkb_2612-i22 -b feat/issue-22-guidance origin/main`。`feat/routing-m0` の未コミットの変更には触れない。
- 本書を worktree の `docs/` へ移す。
- `dsh-flash "OKとだけ返してください"` で資格情報を確認する。`MISSING_CREDENTIAL` なら利用者に伝えて止まる。
- `cargo test --no-run` と `npm ci` を先に実行し、Flash の待ち時間を減らす。

### T1：バックエンドの契約（Flash）

変更するファイル：

- `docs/api/openapi.yaml`：3章のスキーマ。`navigation` を持つ応答例すべてに `guidance: [{maneuver: arrive, at_m: <その例のlength_m>}]` を追加する。
- `docs/api/behavior.md`：7章に `guidance` の説明を追加し、サイズの節を更新する。
- `backend/src/contract/engine.rs`：`Maneuver`、`GuidanceStep` を追加し、`NavigationData.guidance` を追加する。
- `backend/src/domain/navigation.rs`：`navigation_data_checked` が `arrive` 1件を入れる。
- `backend/src/api/route_response.rs`：DTO、不変条件の検査、上限 `6_800_000`、`NAV_MAX_GUIDANCE_STEPS = 14_999`。
- `backend/src/domain/routing/evaluate/mod.rs`：`check_navigation` に不変条件を追加する。
- `backend/src/testing/fixtures.rs`、`backend/tests/api_route.rs:425`、`backend/tests/api_route_dto.rs`：追加項目への対応。100行目付近の上限のテストは、名前とコメントを新しい上限に合わせる。

追加するテスト（`backend/tests/api_route_dto.rs`）：`arrive` が末尾にない、`at_m` が逆行する、`length_m` を超える、0件、の4つがそれぞれ拒否される。

合格条件：

```sh
python3 -B backend/scripts/extract_route_examples.py        # 再生成
python3 -B backend/scripts/extract_route_examples.py --check
cargo fmt --manifest-path backend/Cargo.toml --check
cargo clippy --manifest-path backend/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path backend/Cargo.toml --locked      # Claude が全件実行
```

### T2：フロントの契約（Flash）

- `npm run generate:api` で生成型を更新する。
- `frontend/src/api/validate.ts`：不変条件の検査と上限 `6_800_000`。
- `frontend/src/api/demo.ts`：`frontend/src/navigation/demoGuidance.ts` の角度判定を移し、デモ応答の `navigation.guidance` を作る。`demoGuidance.ts` を削除する。
- `frontend/src/navigation/guidance.ts`：`Maneuver` を生成型から取り、`guidanceStepsFromRoute(route)` を追加する。`navigation` が `null` なら `[]`、それ以外は `id: ${route.id}:guide-${i}`、`atM: at_m` に変換する。
- `frontend/src/navigation/test-utils.ts` と、型エラーになるテスト：追加項目への対応。
- `frontend/src/api/validate.test.ts`：T1 と同じ4つの拒否の確認を追加する。

この時点では `App.tsx` の表示条件を変えず、`createDemoGuidance` の呼び出しを `guidanceStepsFromRoute` へ置き換えるだけにする。`isDemo` の条件は残す。

合格条件（`frontend/` で実行）：

```sh
npm run generate:api && git diff --exit-code -- src/api/generated   # コミット後に差分なし
npm run typecheck
npm test
npm run build
```

### T3：フロントの表示（Flash）

- `frontend/src/App.tsx:156-160`：`isDemo` と `simulation` の条件を外し、すべてのナビで `guidanceStepsFromRoute(navigation.route)` を使う。`:161` の「推測しない」のコメントを現状に合わせる。
- 通過履歴のリセット（`frontend/src/App.tsx:166-168`）は変えない。計画時は「再検索で同じ `route-1` が選ばれると履歴が残る」と考えたが誤りだった。再検索は `takeForReSearch` から `end()` を通り、`route` がいったん `null` になるので、履歴は今のコードで消える（T3 で Flash が指摘し、Claude がコードで確認した）。実際の再検索の流れを通るテストだけを回帰防止として追加する。
- `frontend/src/components/NavigationGuidanceBar.tsx:15-20`：`slight_left`「斜め左」、`slight_right`「斜め右」、`uturn`「Uターン」を追加し、`straight` を削除する。アイコンは `lucide-react@0.468` にあるものから選ぶ。
- `frontend/src/App.navigation-guidance.test.tsx:21-27`：モックの対象を `guidanceStepsFromRoute` に変える。
- `frontend/README.md`：「実APIでは右左折を表示しない」という記述を更新する。

追加するテスト：

- 実機GPSのナビ（`kind` が `simulation` でない）でも、APIの `guidance` から距離と方向が表示される。
- 同じ `route-1` で再検索した後、前の経路で通過した案内が新しい経路で再び表示される。
- `slight_left`、`slight_right`、`uturn` の表示。
- 案内が `arrive` だけの経路では「経路に沿って進む」と表示され、右左折は出ない。

合格条件：T2 と同じ。

### T4：案内地点の判定（Flash）

- `backend/src/domain/routing/evaluate/geometry.rs`：`display_source` を追加する（4章）。表示座標の結果は変えない。
- `backend/src/domain/routing/evaluate/guidance.rs`（新規）：4章を実装する。`evaluate/mod.rs` に `pub mod guidance;` を追加する。この時点ではまだ `evaluate` から呼ばない。
- `backend/tests/routing_guidance.rs`（新規）：`graph::synthetic::GraphBuilder` と `EdgeSpec` で次の場面を作る。

| # | 場面 | 期待 |
|---|---|---|
| 1 | 十字路で東から南へ | `right`、`at_m` はノードの距離に一致 |
| 2 | 十字路で東から北へ | `left` |
| 3 | 十字路で直進 | `arrive` のみ |
| 4 | 分岐のない90°の折れ曲がり | `arrive` のみ |
| 5 | 同じ区分の道路が50°曲がり、下位の脇道がある | `arrive` のみ |
| 6 | 同じ交差点で脇道が上位の道路で、50°曲がる | `right` |
| 7 | Y字路で右の枝へ（±20°） | `slight_right` |
| 8 | 車が通れない歩道だけが分岐する90°の折れ曲がり | car は `arrive` のみ、walk は `right` |
| 9 | 同じクラスタの2ノード（12m）を通って右折する | `right` が1件、局所角が大きい方のノード |
| 10 | 中央分離帯のある道路で右・右と折り返す | `uturn` が1件 |
| 11 | 歩行者が横断歩道で右、15m以内に左（差し引き0） | `arrive` のみ |
| 12 | 入る辺の最後の3mだけ形状が斜めで、全体は直進 | `arrive` のみ |
| 13 | 同じ辺を引き返す | `uturn` |
| 14 | 環状交差点に入り2つ目の出口で右へ出る | 出口のノードに `right` が1件 |
| 15 | 有向辺1本だけの経路 | `arrive` のみ、`at_m == length_m` |
| 16 | 到着の8m手前で右折 | `arrive` のみ |
| 17 | 取り消し済みの `CancelToken` | `Interrupted` を返す |

`display_source` の補間のテスト：15,001頂点の折れ線で、変換後の距離が単調で、残した頂点では `vertex_distances_m` と完全に一致し、間の頂点では `full` 軸の位置との差が許容誤差以内である。

合格条件：全テスト成功、fmt・clippy を通過。Claude は、1〜14の場面の座標と角度の符号が意図どおりかをテストコードで確認する。

### T5：エンジンへの組み込み（Flash）

- `backend/src/domain/routing/evaluate/mod.rs`：
  - 候補ごとのループ（`:186` の `build_shape` の後）で `guidance::build_guidance` を呼び、`Candidate` に持たせる。`shape` はそのループを抜けると使えないため、全候補で計算する（最大6本で、計算量は辺数に比例する）。
  - `:347` で、`navigation_data_checked` の結果の `guidance` を置き換える。選定候補以外には出さない。
- `backend/tests/routing_engine.rs`：合成グラフの十字路を通る `evaluate` の結果に、`right` と `arrive` が入ることを確かめる。
- `backend/tests/routing_real_release.rs`：`run_query` の検査に `guidance` の不変条件を追加する。案内の総数とバイト数を `eprintln!` で出す。

合格条件：全テスト成功、fmt・clippy を通過。Claude が実 release で次を実行する。

```sh
ROUTING_REAL_RELEASE_DIR=/home/hayato/tkb_2612/data/work/wt/m5/backend/data/releases/aeb32e87dd4c74d419182432a1a7e6a605dbe5f71307145fc3b913b75148d753 \
  cargo test --release --manifest-path backend/Cargo.toml --test routing_real_release -- --ignored --nocapture
```

300件すべてが成功し、処理時間の最大が M5 の記録（徒歩0.33秒、自転車2.51秒、自動車5.27秒）から大きく増えていないこと。

### T6：文書（Claude）

- `docs/spec-decisions.md`：段階Cの一部変更の決定を追加する（D1〜D6）。
- `docs/frontend-spec.md`：ナビの章に案内バーの仕様を追加する。
- `docs/api/behavior.md` の T1 の記述を読み直し、4章の規則の要点を追加する。
- `docs/navigation-delegated-decisions.md`：Flash に実装させた経緯を残す。過去の決定の記録（24行・70行の5,000,000バイト）は書き換えない。
- サイズの上限の更新：`docs/architecture.md:200`、`docs/backend-spec.md:54`、`backend/README.md:52` の周辺。
- `docs/architecture.md:178`、`docs/backend-spec.md:56`：`steps=false` の維持は変えない。案内は新方式のグラフから作ることを追記する。

### T7：実データでの調整（Claude）

Flash には任せない。実装者とは別に Claude が検査を書いて判定する。

1. schema 3 の release でサーバを起動し（メモリ約2.7GB）、`routing_real_release.rs` の300件を `/api/route` へ送る。
2. スクラッチパッドの Python で、応答だけを使って次を集計する。案内地点の判定コードは流用しない。

| 指標 | 目安 |
|---|---|
| 手段別の1kmあたりの案内数（中央値、95%点） | 自動車で極端に多くない。徒歩・自転車は記録して判断する |
| `left`・`right` と、形状の前後30mの方位変化の符号の一致率 | 98%以上。不一致はすべて個別に見る |
| 取りこぼし候補：形状が60°以上曲がるのに30m以内に案内がない地点 | 手段ごとに30件を抽出して目視で確認する |
| 案内の間隔が30m未満の組の数 | 記録して判断する |
| `guidance` の最大バイト数 | 記録する |

3. 手段ごとに5経路、合計15経路を地図に重ねて表示し、案内を目視で確認する。自動車は50か所を抽出し、誤案内10%以下、取りこぼし5%以下を目安とする。
4. 目安を満たさなければ4章の定数を調整し、必要なら手段ごとに値を分ける。調整で4章の規則を変えた場合は、本書、T4 のテスト、T6 で書いた `docs/api/behavior.md` の要点も更新する。徒歩の案内が多すぎて定数で解決できない場合は、利用者に判断を仰ぐ。
5. 結果を `docs/routing-reports/guidance.md` に記録する。

### T8：画面の確認と仕上げ（Claude）

- 実 release のサーバに `VITE_ENABLE_SIMULATION=true` のフロントをつなぎ、Chromium の 390px と 1440px で模擬走行する。
  - 距離が減り、案内地点を通ると次の案内に変わる。
  - 危険区間の注意文と進行方向の案内が同時に読める。
  - 逸脱・一時停止・位置確認中には古い右左折を表示しない。
  - 再検索の後、案内が新しい経路のものになる。
- CI と同じ全コマンドを実行する。
- PR の本文に、実機GPSでの確認が済んでいないことを明記する（Claude は実機で確認できない）。PR の作成とプッシュは、利用者に確認してから行う。

---

## 7. 完了条件との対応

| Issue #22 の完了条件 | 確認方法 |
|---|---|
| 進行方向と案内地点までの距離が表示される | T3 のテスト、T7 の目視、T8 の模擬走行 |
| ナビ案内と危険区間の警告が適切に表示される | PR #29 の既存テスト、T8 の模擬走行 |
| 移動に応じて案内内容が更新される | PR #29 の `guidance.test.ts`、T3 の再検索のテスト、T8 の模擬走行 |
| 実機GPSでの動作 | 利用者の確認が必要 |

---

## 8. Flash への指示文

### 共通の前置き（全タスクの先頭に付ける）

```text
あなたは /home/hayato/tkb_2612-i22 で作業します。Rust（backend/）と TypeScript（frontend/）のリポジトリです。
仕様は docs/navigation-guidance-plan.md です。作業の前に、指定した節を読んでください。

守ること：
- 指定されたファイルと、コンパイルに必要な最小限の箇所だけを変更する。無関係な整形やリファクタリングをしない。
- git commit、git push、git checkout、git stash をしない。
- テストを削除しない。#[ignore]、it.skip を追加しない。期待値を実装に合わせて書き換えない。
  既存のテストが仕様と矛盾すると思ったら、変更せずに報告する。
- 生成物を手で編集しない。backend/tests/fixtures/route-responses.json は
  python3 -B backend/scripts/extract_route_examples.py で、frontend/src/api/generated/ は
  npm run generate:api で作る。
- バックエンドを変更したら cargo test --manifest-path backend/Cargo.toml --locked --no-fail-fast を全件実行する。
- 仕様が足りない、または矛盾している場合は、推測で埋めずに報告する。

最後に、次を報告する：変更したファイル、実行したコマンドとその結果、未解決の点。
実行していないことを「確認した」と書かない。
```

### T1

```text
計画書の3章と6章のT1を実装してください。旧方式・新方式とも、この時点では guidance は arrive 1件だけです。
先に backend/src/api/route_response.rs の check_navigation と NavigationDto、
backend/src/domain/navigation.rs の navigation_data_checked を読んでください。
openapi.yaml の応答例の at_m には、同じ例の length_m と同じ数値を書いてください。

実行するコマンド：
python3 -B backend/scripts/extract_route_examples.py
python3 -B backend/scripts/extract_route_examples.py --check
cargo fmt --manifest-path backend/Cargo.toml --check
cargo clippy --manifest-path backend/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path backend/Cargo.toml --locked --test api_route_dto --test api_route --test navigation --test engine_fixtures --test api_contract
```

### T2

```text
計画書の3章と6章のT2を実装してください。frontend/ の作業です。App.tsx の表示条件（isDemo）は変えず、
呼び出す関数の置き換えだけをしてください。T1 でバックエンドと openapi.yaml は変更済みです。
デモの案内は demoGuidance.ts と同じ結果になるように移してください。
guidance.test.ts のデモ案内のテストは、デモ応答の navigation.guidance を確かめる形に書き換えてください。
期待する案内の内容は変えないでください。

実行するコマンド（frontend/ で）：
npm run generate:api
npm run typecheck
npm test
npm run build
```

### T3

```text
計画書の6章のT3を実装してください。frontend/ の作業です。
PR #29 の案内バーの配置、CSS、注意文の表示は変えないでください。
追加するテストは frontend/src/App.navigation-guidance.test.tsx の既存の書き方に合わせてください。

実行するコマンド（frontend/ で）：
npm run typecheck
npm test
npm run build
```

### T4

```text
計画書の4章と6章のT4を実装してください。backend/ の作業です。
先に次を読んでください：
- backend/src/domain/routing/graph/time.rs の turn_angle_deg とそのテスト（角度の符号）
- backend/src/domain/routing/graph/synthetic.rs の GraphBuilder、EdgeSpec
- backend/src/domain/routing/evaluate/geometry.rs の全体
- backend/src/domain/routing/topology/mod.rs の cluster_of
- backend/tests/routing_engine.rs の合成グラフと Topology の作り方

4章の規則と定数の値をそのまま実装してください。T4 の17個の場面のテストが通らないときに、
定数や規則を変えないでください。規則どおりに実装しても場面の期待と合わない場合は、その場面を報告してください。
テストの座標は、角度が境界の値にならないように選んでください。
evaluate からはまだ呼ばないでください。未使用の警告が出る場合は pub にして解決します。allow は使わないでください。

実行するコマンド：
cargo fmt --manifest-path backend/Cargo.toml --check
cargo clippy --manifest-path backend/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path backend/Cargo.toml --locked --test routing_guidance
cargo test --manifest-path backend/Cargo.toml --locked --lib evaluate::geometry
```

### T5

```text
計画書の6章のT5を実装してください。backend/ の作業です。
候補の探索、選定、リスクの値、表示用の区間を変えないでください。変更は guidance の受け渡しだけです。
実 release を使うテストは #[ignore] のままにし、実行しないでください。

実行するコマンド：
cargo fmt --manifest-path backend/Cargo.toml --check
cargo clippy --manifest-path backend/Cargo.toml --all-targets --locked -- -D warnings
cargo test --manifest-path backend/Cargo.toml --locked --test routing_engine --test routing_guidance
```

---

## 9. リスク

| リスク | 対策 |
|---|---|
| 徒歩・自転車で歩道の分岐が多く、案内が多すぎる | T4 の場面8・11で基本を押さえ、T7 で実測して手段別に調整する。解決しなければ利用者に判断を仰ぐ |
| 道路区分だけでは「道なり」を正しく判定できない（道路名がない） | T7 で目視する。誤案内は安全側（余分な案内）に倒れる |
| 判定の誤りで、誤った方向を案内する | 誤案内の率を T7 で確認する。案内は参考情報であり、経路線も地図に表示している |
| Flash がテストや定数を合格するように変える | 指示文で禁止し、差分を Claude が読む。T7 の検査は Flash に見せずに Claude が作る |
| `navigation` の上限を上げたことの影響 | 理論上限は実際には起きない組み合わせの値である。実測値を T7 で記録する |

---

## 10. 今回やらないこと

- 音声での進行方向の案内（D6）。
- 旧方式の実案内（D1、13章）。
- 道路名の表示（D5）。
- 自動再検索。逸脱時は現在の再検索ボタンのまま。
- Issue #26 の表示と契約の変更（本書の `guidance` 以外）。
- PR #29 で把握済みの2点（8倍速での切り替えの遅れ、スマホで地図障害時にバナーが隠れる）。
- schema 3 の release での本番配備。`docs/routing-reports/M5.md` の12章の手順による。

---

## 11. 作業量の目安

| タスク | 担当 | 規模 |
|---|---|---|
| T0 準備 | Claude | 小 |
| T1 バックエンドの契約 | Flash | 中（応答例の更新が多い） |
| T2 フロントの契約 | Flash | 中 |
| T3 フロントの表示 | Flash | 小 |
| T4 判定 | Flash | 大（本計画の中心） |
| T5 組み込み | Flash | 小 |
| T6 文書 | Claude | 小 |
| T7 実データでの調整 | Claude | 大（ここで定数が決まる） |
| T8 画面の確認 | Claude | 中 |

---

## 12. 参照

| 内容 | 場所 |
|---|---|
| 現在の案内の計算 | `frontend/src/navigation/guidance.ts` |
| 案内バー | `frontend/src/components/NavigationGuidanceBar.tsx` |
| 応答の形状と `ranges` | `backend/src/domain/routing/evaluate/geometry.rs` |
| 新方式の評価 | `backend/src/domain/routing/evaluate/mod.rs` |
| 右左折の角度 | `backend/src/domain/routing/graph/time.rs:354` |
| 交差点クラスタの定義 | `docs/routing-data-format.md` 10章 |
| 本番用 release の場所 | `docs/routing-reports/M5.md` 12章 |

---

## 13. 旧方式にも実案内を出す場合（D1 を変える場合のみ）

本計画には含めない。必要になった場合の追加作業は次のとおり。

- `providers/osrm.rs`：`steps=true` に変え、`legs[].steps[].maneuver` の `type`・`modifier`・`location` を読む。`depart`・`arrive`・`new name`・`notification` は案内にしない。
- `contract/routing.rs`：`RoutedPath` に案内を追加する。構造体リテラルが10ファイルの約12か所にあり、すべて更新が必要になる。
- `domain/engine.rs`：`location` を `vertex_distances_m` の軸へ対応させる。OSRM の座標と `location` の一致を、実際の応答で確かめてから実装する。
- `steps=false` を定めた文書（`docs/api/behavior.md:562,627`、`docs/api/openapi.yaml:52`、`docs/architecture.md:178`、`docs/backend-spec.md:56`、`docs/navigation-delegated-decisions.md:25`）を変更する。
- 上流の応答が大きくなるので、`UPSTREAM_MAX_BYTES` の範囲に収まることを実測で確かめる。
