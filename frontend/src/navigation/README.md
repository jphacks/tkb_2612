# ナビ共通コア（Issue #7 手順6・7・8）

UI、通信、事故集計、位置履歴の保存を持たないTypeScriptモジュール。既存の `../lib/types.ts` の `NavigationData` / `RouteCandidate` を使用する。接近通知、逸脱の閾値と10秒・3測位の状態、音声は後続モジュールで管理する。

2026-10-25の追加レビューに対し、単一の折返し判定から**各有向辺の複数仮説と累積整合の比較**へ変更した。M1〜M11の必須448試行を維持し、さらに揺らぎON・seed 1〜512・倍率1で7入力、計3,584試行を検査する。10m手前反転も全件で誤差条件と非到着を満たし、識別の限界の範囲は広げていない。既決の2〜5m手前反転6入力は従来どおり専用検査とする。

## 時刻・距離・結果の契約

- 位置の `timestamp` と `Clock.now()` はミリ秒。同じ時間軸を使う。実機は端末のepoch時計、模擬は0からの仮想時計。タイマー駆動の `realNow` は実時間の単調時計で、倍率を掛けない。
- 進捗 `p` はAPIの局所平面累積距離、m。R=6,371,008.8m、`navigation.projection_lat_deg`、geometry先頭原点を使用。OSRMのdistanceは残量の割合換算と基準速度にだけ使用する。
- `MatchResult.status` は `waiting`（測位待ち）、`matching-hold`（初回/再確認/曖昧/経路外で照合保留）、`tracking`（進行中）、`poor-accuracy`（精度不足/無効/古い/欠測）、`arrived`、`paused`、`stopped`。
- `p` は今回確定した進捗、未確定ならnull。`updated` は新規確定の有無。到着時も測定したpを返し、Lへ置換しない。残量の表示を0にするのは到着表示側の責務。
- `lastConfirmedP` / `lastConfirmedTimestamp` は更新停止表示用の旧確定値と時点。`lastAccepted` と `accuracy` / `timestamp` は最後に受理した測位を保持する。`accepted` は**今回**の受理位置であり、無効・欠測時はnull。
- `distanceToRouteM` は今回受理した位置と**全経路**の最短距離。枝の識別に失敗していても得られる。精度不足で受理された測位にも距離を返すが、逸脱の確定には使わない。無効/欠測/停止時はnull。
- 後続判定には、新規受理 `accepted !== null`、accuracy<=30、状態、`updated` を使う。接近は `tracking && updated && p !== null` のみ。曖昧さのある良好測位でも、逸脱担当は全経路距離から独立して判定する。逸脱疑いの最初の位置からコアは照合を保留し、復帰は再度3測位・5秒を要するため、コア自体に逸脱10秒の状態を重ねて持つ必要はない。
- 無効入力、良好測位の5秒超の間隔、曖昧さは確認・到着の連続判定をリセットする。`poll()` をタイマーから呼び、測位なし5秒超を反映する。`poll()` は新しい進捗を確定しない。

## 選定した照合方式と理由

射影に加え、位置・速度・共分散・累積スコアを持つ枝候補を並行して更新する。回帰の残差幅に頂点が含まれることを通過の根拠にする旧方式を廃止した。候補間の曖昧さは辺変更で解除しない。

1. 全辺へ局所平面で射影し、最短横距離は枝の選択から独立して求める。同方向の完全な共線辺だけを照合内で統合し、APIの累積距離軸は維持する。正長辺だけを候補とする。
2. accuracy内の候補が隣接し方向幅90度以内、または同じ方向幅で進捗の幅が2×accuracy以内なら、一つの局所進捗として扱う。全候補がこの条件を満たす場合だけ今回の最短射影へ縮約する。交差などの離れた進捗への切替では確認をリセットする。短辺列・単純直線では、この幾何的な一意性から通常走行・後戻り・停止へ直接追従する。
3. 重複区間の全経路探索では、近傍の各辺に前進/逆行の2仮説を生成する。**新規開始の最初の探索だけ**、始点観測後の2×accuracyを超える移動で始点側の枝を選ぶ。途中からの逆行、長い中断後、明示再開にはこの始点仮説を適用しない。静止したM6/M7では枝を選ばない。
4. 仮説ごとに進捗p、符号付き速度v、位置/速度の共分散を予測・更新する。測位分散Rは `max(0.01, accuracy²/4)`、加速度の標準偏差は1m/s²。各仮説は予測進捗の前後2×accuracy内にある経路辺へだけ移る。距離列の二分探索でこの範囲を探す。今回の位置がaccuracy内にある辺を使い、全辺がaccuracy外の場合は最短距離までの辺を使う。経路全体への許容横距離 `max(20m,accuracy)` は別に検査する。
5. 通常継続と同じ辺での実際の反転を毎回比較する。反転時点を前回測位に固定せず、測位間に到達できる `p±abs(v)×dt` の範囲から反転仮説を作る。これにより、測位間の折返しや反転直前の減速を一方の枝へ即決しない。逆行生成は5秒でaccuracyを超える移動に相当する速度 `abs(v)>=accuracy/5` のときに行う。
6. 速度フィルタが誤った枝の位置差を吸収しないよう、頂点から2×accuracyより離れ、有意な移動がある区間の進捗回帰も保持する。この回帰は**同じ方向の経路上の辺変更をまたいで継続**し、逆行仮説では新たに開始する。十分統計は中心化した6数値で、重み合計32を上限として31/32で忘却する。6標本以上で予測残差を評価する。分散は `(残差平方和+R)/(n−1)` とし、accuracy由来の1標本分を加えて、少数標本の小さい残差を過信しない。観測を頂点通過と判定するboolean値は持たない。
7. 累積スコアへ、今回の位置・移動方向・速度の連続性との不整合を加える。位置の予測残差をe、予測測位分散をS、辺への距離をd、回帰の予測残差をr、その分散をQとすると、増分は `e²/(2S) + d²/(2R) + log(S/R)/2 + max(0,r²/Q−1)/2`。回帰評価前と経路始終点付近では最後の項を使わない。逆行には、方向変更と自由な反転時点の追加自由度として費用9（3σ相当のスコア4.5を2組）を加える。これはモデル比較の実装値であり、実GPSの誤確率を保証する確率ではない。
8. 同じ有向辺へ合流した候補は最良の1個へ縮約する。最小スコアを毎回差し引き、差24超を除外する。**進捗差が2×accuracyを超え、スコア差が4.5未満の競合が残る間は保留**する。各辺の変更時にも全競合を保持する。単に向きが変わったことを理由に逆行へ固定せず、以後の観測で通常の通過候補も再評価する。非隣接・非平行の交差候補が残る場合も保留し全枝から確認し直す。
9. 折返しの手前2×accuracy以内では保留する。競合が解消してから3測位・5秒を改めて確認し、今回の選択辺への**実測の射影p**を公開する。方向の変更・折返し辺への切替・不連続な進捗変更でも確認をリセットする。到着はこの確認時間を算入せず、従来の90%・20m・終点30m・3測位/5秒の条件で別に確認する。
10. 良好測位または照合に使える位置の30秒超の中断と明示再開では全枝を再探索する。5秒超〜30秒の欠測では枝だけを保持し、速度・回帰・連続確認を解除して外挿しない。無効入力と欠測pollは連続判定を解除する。座標ジャンプの幅は従来どおり70m/s×timestamp間隔＋前後accuracy。時計はすべて注入された位置源の時計を使う。

全経路で毎回一番近い辺だけを使う方式は重複を識別できず、単調増加の進捗は実際の後戻りに追従できない。一度の回帰判定で枝を捨てる方式は、測位ノイズで誤選択した後に復帰できない。今回の方式では通過と反転の候補を比較し続け、同じ辺に移ったという理由で競合候補や過去の不整合を消さない。回帰残差は継続モデルの比較に使い、「頂点が残差幅内だから通過した」という判定には使わない。

閾値は距離・accuracy・時刻・モデルの自由度に基づく共通値で、seed・模擬倍率・duration・正解進捗を参照しない。距離境界には計算誤差を吸収する1e-6mの許容差を設ける。人工ケースの合格は、任意の道路形状や実GPSでの正解枝の保証とは分けて扱う。

## 保持状態と上限

Vは元頂点数（API上限15,000）、Eは統合後の正長辺数（E<=V−1）、Hは現在の仮説数。走行時間Tに比例する配列はない。

| 状態 | 上限 |
|---|---|
| 投影頂点、API距離列、検算距離列 | 各O(V) |
| 仮説：辺index・方向・p・v・共分散3個・スコア | 有向辺ごとに1個、H<=2E |
| 各仮説の回帰：重み、平均時刻/進捗、中心化平方和/積和 | 各6数値、重み合計32以下。測位列なし |
| 選択枝、始点観測/初回探索フラグ・開始p、前回時刻、受理/良好/使用可能/探索開始時刻 | 固定個数 |
| 前回良好測位、最終受理測位、今回結果と最後の確定p/時刻 | 各1個（参照共有あり） |
| 確認・到着の開始時刻とカウンタ | 各1組、回数は3で飽和 |
| simulatorの乱数・時計・倍率・停止期限・進捗tick・測位時刻オフセットなど | 固定個数 |

永続状態はO(V+H)。更新時の射影列・候補MapもO(V)が上限で、走行履歴を追加しない。計算は射影O(V)、交差比較は近傍数Kに対してO(K²)、各仮説の遷移探索は二分探索と予測進捗の範囲内の辺数に依存する。全体の最悪計算量はO(V²)で、Tには依存しない。実機の最大経路での処理時間は人工テストとは別の評価対象。

停止時に測位・仮説・回帰・旧確定・タイマーを破棄する。経路は比較結果として保持できる。通知済み/通過済み集合は後続側の所有で、本コアがリセットしない。模擬の停止期限は再探索で延長しない。

## 公開API

型のプロパティ詳細は各宣言を参照。以下はすべてnamed export。

```ts
// geometry.ts
interface Coordinate { lon: number; lat: number }
interface PlanePoint { x: number; y: number }
// RouteGeometry、RouteProjectionもexport。RouteProjectionはp/distanceM/point/segmentIndex。
clamp(value: number, min: number, max: number): number
createRouteGeometry(coordinates: readonly (readonly number[])[], navigation: NavigationData): RouteGeometry
projectPoint(g: RouteGeometry, p: Coordinate): PlanePoint
unprojectPoint(g: RouteGeometry, p: PlanePoint): Coordinate
projectOnSegment(g: RouteGeometry, p: PlanePoint, i: number): RouteProjection
routeProjections(g: RouteGeometry, p: Coordinate): RouteProjection[]
nearestRoutePoint(g: RouteGeometry, p: Coordinate): RouteProjection
pointAtProgress(g: RouteGeometry, p: number): RouteProjection

// preflight.ts
// NavigationSelection、NavigationStartInput、NavigationStartResultをexport。
checkNavigationStart(input: NavigationStartInput): NavigationStartResult
// input: route/selection/fastestId/safestId/responseGeneration/currentGeneration/mapAvailable
// result: {ok:true,route,selection} | {ok:false,reason}

// remaining.ts
// Remaining: distanceM/durationS/displayDistanceM/displayMinutes
calculateRemaining(route: { distance: number; duration: number; navigation: { length_m: number } }, p: number | null): Remaining | null

// clock.ts / source.ts
interface Clock { now(): number }
class VirtualClock { now(): number; set(time: number): void }
class RealClock {
  constructor(wall?: () => number, monotonic?: () => number)
  now(): number
  monotonicNow(): number
}
interface PositionFix { lon: number; lat: number; accuracy: number; timestamp: number }
type PositionConsumer = (position: PositionFix) => void
interface PositionSource {
  readonly clock: Clock
  start(realNow: number, consumer: PositionConsumer): void
  pause(): void
  resume(realNow: number): void
  stop(): void
}

// simulator.ts
// SimulationMultiplier=1|2|4|8、SimulationStopReason、SimulatorOptionsをexport。
// options: seed? / jitter? / multiplier? / inject?: (fix, virtualSeconds) => fix | null
lcgNext(seed: number): number
class NavigationSimulator implements PositionSource {
  constructor(route: RouteCandidate, options?: SimulatorOptions)
  readonly clock: VirtualClock
  readonly baseSpeedMps: number
  readonly progressSpeedMps: number
  state: "idle" | "running" | "paused" | "stopped" | "arrived"
  stopReason: SimulationStopReason | null
  start(realNow: number, consumer: PositionConsumer): void
  pump(realNow: number): void
  measureStoppedPosition(): PositionFix | null
  pause(): void
  resume(realNow: number): void
  stop(): void
  setMultiplier(value: SimulationMultiplier): void
  setJitter(value: boolean): void
  tryDeviation(): void
  report(status: MatchStatus, reason?: SimulationStopReason): void
}

// measureStoppedPositionはpaused/stoppedでのみ呼べる（他の状態では例外）。
// tickの進捗を保ち、現在の仮想時刻を1秒進めて未補正測位を1回返す。consumerには配信しない。
// emitと同じLCG2回（揺らぎOFFでも消費）・左120mオフセット・accuracy=10m・injectを使う。
// injectのvirtualSecondsは新しい仮想時刻/1000。nullなら欠測でも時計・乱数は保持する。
// 追加の時刻オフセットは以後のpumpにも反映し、stopで0に戻す。
// 呼ばない場合はオフセット0で従来の測位列を維持する。

// matcher.ts
// MatchStatus、MatchResultをexport。
class NavigationMatcher {
  constructor(geometry: RouteCandidate["geometry"], navigation: NavigationData, clock: Clock)
  update(fix: PositionFix): MatchResult
  poll(): MatchResult
  pause(): MatchResult
  resume(): MatchResult
  stop(): void
}
```

## 統合時の呼出し順

開始前に既存API検証を通した候補へ `checkNavigationStart` を適用する。空sectionsは有効、地図不可/長さ0/旧世代は開始不可。同一IDでも選定種別を結果で保持する。距離列と75m分割したsid参照を検算し、リスクの再集計はしない。

```ts
const source = new NavigationSimulator(route, { seed: 1 });
const matcher = new NavigationMatcher(route.geometry, route.navigation!, source.clock);
source.start(realClock.monotonicNow(), fix => {
  const result = matcher.update(fix);
  // ここで別担当の接近・逸脱・画面表示を更新する。
  source.report(result.status);
});
// UI側のタイマーからsource.pump(realClock.monotonicNow())を呼ぶ。
```

`pump` は仮想1秒ごとに時計を合わせて同期配信する。1回の実時間空白が5秒を超えれば追いつかせず一時停止し、呼出し側もmatcher.pauseと音声/ライフサイクル停止を行う。タイマーの作成/破棄とvisibilityの監視はこの純粋コアの外側。倍率変更直前に古い倍率でpumpを呼び、その後setMultiplierを使う。

明示一時停止/hidden時はsource.pauseとmatcher.pause、明示再開時はsource.resume(realNow)とmatcher.resumeを両方呼ぶ。再開時に測位は生成せず、次の仮想tickから全経路の再確認が始まる。通知済み集合は外側で保持する。自動再開しない。終了/unmount時は両方のstopを呼ぶ。

`report` は各測位の共通判定後に呼び、到着なら停止、連続照合保留60秒/終点後15秒の早い方で理由付き停止する。欠測中もpump自体が終点後期限を検査する。逸脱担当が確定した理由は第二引数へ渡せる。明示再開では期限をリセットし、基準位置が既に終点なら再開から15秒を確保する。正解の基準pや既知の終点時刻はmatcherへ渡さない。

残量更新は確定したときだけ `calculateRemaining` を呼び、保留/精度不足/逸脱時は最後の戻り値を時点付きで保持する。引数に時計や倍率を取らないので停止中に減少しない。

## 検査範囲と結果（2026-10-25）

| 検査 | 範囲・結果 |
|---|---|
| M1〜M11必須 | 14入力（M6/M7の50/51秒、M11のpause/hiddenを区別）×揺らぎOFF/ON×seed 1/2/3/42×倍率1/2/4/8＝448試行、全成功。倍率間の出力一致・通信0も維持 |
| 追加の頑健さ | 揺らぎON×seed 1〜512×倍率1×M1/M2/M3/M4/M5/通常往復2回/10m手前反転＝3,584試行。各入力512/512（100%）。全確定の誤差<=20m、通常走行4入力は期限内到着、反転3入力は非到着 |
| 追加レビューの再現入力 | 10m手前反転seed4、通常往復2回seed3、共有fixture M1 seed8/M2 seed5を倍率1/2/4/8で実行＝16試行、全成功 |
| 従来の反転・再確認・交差 | 必須反転34件、既決の限界記録6件、重複区間のpause/欠測32件、交差17件を維持 |
| 境界・保持量 | 精度/鮮度/時刻/確認/到着/中断/ジャンプ、短辺・曲線・減速反転、10,000秒でも保持量不変を維持。正の極小accuracyでも共分散を有限に保つ検査を追加 |

seed 1〜256での検証後、未使用だったseed 257〜512の各256試行もすべて成功し、同じ必須検査として固定した。

追加3,584試行の確定誤差の最大値は7.991m未満。10m手前反転は7.991m未満、通常往復2回は7.988m未満。accuracy=10m、許容誤差は20m。正解進捗は試験ハーネスだけが持ち、判定器には渡さない。

### レビュー反例の結果

- 反例1（10m手前反転seed4）：t=38で真のp=200mに対し **p=198.280704m**、以後も誤差条件内。0〜70秒で非到着。全512seedで誤確定0件、成功率100%。限界扱いへの変更はしていない。
- 反例2（往復2回seed3、L=900m）：t=42で真のp=756mに対し **p=759.380156m**、t=56に到着。辺変更後も競合スコアを維持する。全512seedで誤確定0件・期限内到着、成功率100%。
- 反例3（共有fixture）：M1 seed8はt=45以降確定を維持し **t=53到着**、M2 seed5はt=47以降確定を維持し **t=55到着**。4倍率すべて同じ結果。通常走行を保留停止で代用しない。

### 期限の余裕

すべて仮想秒、時刻範囲は両端を含む。M1〜M5は必須組合せと追加512seedを合わせた最悪値、M6〜M11は必須組合せの最悪値。「継続確定」は最後の保留から復帰し、以後到着/観測終了まで新規確定を維持した時刻。

| 入力 | 最遅の初回/再確定など | 最遅到着/停止 | 上限までの余裕 |
|---|---|---|---|
| M1 OFF | 初回8、継続確定45（上限51） | 到着53（上限56） | 確定6秒、到着3秒 |
| M1 ON | 初回9、継続確定57（上限59） | 到着63（上限64） | 確定2秒、到着1秒 |
| M2 OFF | 初回8、継続確定46（上限52） | 到着54（上限57） | 確定6秒、到着3秒 |
| M2 ON | 初回9、継続確定59（上限59） | 到着65（上限65） | **確定/到着とも0秒** |
| M3 | 初回5（上限10）、以後保留なし | OFF到着53 / ON到着54（上限56/64） | 初回5秒、到着3/10秒 |
| M4 | 初回5、反転後の継続確定は最大50（上限59） | 0〜70秒で非到着 | 再確定9秒 |
| M5 | 初回5、反転後p<240mの確定は最大27（上限35） | 0〜60秒で非到着、保留なし | 後戻り追従8秒 |
| M6（50/51秒） | 全期間保留 | 照合保留で60秒停止 | 規定時刻どおり |
| M7（50/51秒） | 全期間保留、再開後も非到着 | 照合保留で65/66秒停止、再開から15秒で再停止 | 規定時刻どおり |
| M8 | 全期間精度不足 | 精度不足で65秒停止 | 規定時刻どおり |
| M9 | 精度回復後50に確定（上限55）、以後保留なし | 到着56（上限64） | 確定5秒、到着8秒 |
| M10 | 初回5、欠測後105に再確定（上限110） | 到着205（上限214） | 再確定5秒、到着9秒 |
| M11 pause/hidden | 初回5、再開後26に再確定（上限31） | OFF到着53 / ON到着54 | 再確定5秒、到着3/10秒 |
| 通常往復2回 ON | 初回8、継続確定58 | 到着64（上限64） | **到着0秒** |

M1 OFFは旧t=56からt=53になった。一方、追加512seedでのM2 ONと通常往復2回ONは上限ちょうどであり、余裕があるとは扱わない。

### コマンド

frontendディレクトリ、node_modules導入済み、ネットワークなしで実行。経過時間は `/usr/bin/time` で測定。

| コマンド | 結果 | 経過時間 |
|---|---|---|
| `npm run typecheck` | 成功、終了コード0 | 2.06秒 |
| `npm test -- --reporter=dot` | **16ファイル・4,309件成功、失敗0・skip0** | 9.76秒（Vitest表示9.49秒） |
| `npm run build` | 成功、終了コード0、1,792モジュール | 3.58秒（Vite表示1.78秒） |

時刻・誤差の集計は `NAV_MATCH_REPORT=1 npm test -- --reporter=dot src/navigation/matcher-cases.test.ts src/navigation/matcher-regressions.test.ts` で再生成できる。`test-utils.ts` の集計器は試験専用で、位置列や正解進捗を製品コードへ渡さない。

`MatchResult` を含む公開APIは変更していない。後続は従来どおり状態・今回のp・全経路への最短距離・受理測位を使える。編集は本ディレクトリのmatcher・テスト・test-utils・READMEだけで、geometry/UI/App/docs/backendは変更していない。コミットは作成していない。

## 既決の識別の限界（利用者決定 2026-10-25）

経路−100→200→0→100m、duration=60秒、accuracy=10m、1秒間隔の0〜70秒。頂点dメートル手前で反転し、真位置は `t<=(300−d)/10` で `x=−100+10t`、それ以後は `x=max(−100,500−2d−10t)`。揺らぎは半径最大8mの円内一様ノイズ。

既決の例外は **d=2m×seed 1/2/3/42、d=5m×seed 1/2の揺らぎONの6入力だけ**。頂点との差が揺らぎ半径に比べて小さく、正常往復と似た測位列になるため、利用者が識別の限界として認めた範囲を維持する。全6件で全71測位の非到着、状態、updated、pの有限性・範囲、旧確定値と時点の整合を検査し、skip/todoにはしない。

今回の実装の観測結果（誤差値をテストの期待値にはしない）：

| d | seed | 全確定の最大誤差 | 非到着・出力契約 |
|---|---|---|---|
| 2m | 1 | 289.069m | 成功 |
| 2m | 2 | 340.538m未満 | 成功 |
| 2m | 3 | 7.750m未満 | 成功 |
| 2m | 42 | 7.597m未満 | 成功 |
| 5m | 1 | 301.070m未満 | 成功 |
| 5m | 2 | 6.974m未満 | 成功 |

このうち**2m seed1/2と5m seed1の3入力には誤差20m超の確定が残る**。改善した残り3入力も既決の専用検査のままとし、例外を追加/削除しない。d=2/5mの揺らぎOFF、d=5m ONのseed3/42、d=10/15/20mの従来のseed全件は引き続き誤差条件を必須とする。10m ONはさらにseed1〜512を全件必須としている。

後続の通知/逸脱/音声との結合、DOMライフサイクル、実API接続、実機の測位特性や最大経路の性能は本コアの人工入力試験とは別の評価対象である。

再検索は `NavigationSession.stopForReSearch()` で模擬位置源を一時停止し、タイマー・判定器・音声を終了する。返す測位関数だけを再試行snapshotに保持し、毎試行で1回呼ぶ。成功・入力編集・unmountでsnapshotを破棄する。成功後は従来どおり新経路・同seedから初期化し、旧位置源の進捗・時刻・乱数・逸脱状態は引き継がない。
