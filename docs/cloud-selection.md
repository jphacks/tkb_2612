# ハッカソン用クラウドの選定

2026-10-24選定。**アプリAPIの配備先は Google Cloud Run、東京（`asia-northeast1`）を採用する。** Rust/Axumとビルド済みReact画面を1つのコンテナから配信する構成を初期案とする。東京に置け、未使用時に0台へ縮退できるため、限定的な短期利用に合う。[リージョン](https://docs.cloud.google.com/run/docs/locations)、[課金](https://cloud.google.com/run/pricing)

予算・既存アカウントは不明であり、低コストの短期利用を設計上の仮定にした技術判断。アカウント作成・契約・配備・課金設定は未実施。経路決定アルゴリズムは検証中であり、この選定は経路エンジンの自己ホストまで保証しない。

## 候補の比較

| 候補 | 費用と起動の特徴 | 今回の判断 |
|---|---|---|
| **Cloud Run** | リクエスト課金で起動・処理時間等に課金。最小0台では初回起動待ちがある。実演中は最小1台にして待機課金を許容する。 | 採用。東京対応、読み取り専用コンテナに適する。GCPの請求・IAM・ログ設定には初期作業が必要。[料金](https://cloud.google.com/run/pricing)、[最小台数](https://docs.cloud.google.com/run/docs/configuring/min-instances) |
| Render | `1c-2g`（1 CPU / 2 GB）は$25/月相当。無料512 MBは15分無通信で停止し、復帰に約1分かかる。 | 配備操作は簡単だが、アジア候補はシンガポール。未計測のメモリ要件を無料枠へ合わせず、有料常駐より短期従量課金を選ぶ。[料金](https://render.com/pricing)、[無料枠](https://render.com/docs/free)、[地域](https://render.com/docs/regions) |
| Railway Hobby | 月最低$5（$5分の利用を含む）。CPU・RAM等の利用が超えれば追加課金。 | 配備操作は簡単。アジア候補はシンガポール。最低料金があり、RAMを保持し続ける時間も費用に効く。[プラン](https://docs.railway.com/pricing/plans)、[単価](https://railway.com/pricing)、[地域](https://docs.railway.com/deployments/regions) |

日本からの実際の応答時間は未計測。いずれの候補も、データの配置・起動時ロード・外部API制限・プラットフォームのログ設定は別途必要になる。

## 初期構成と終了時の扱い

- Cloud Runサービス1つ、Rustプロセス1つ、**1 vCPU / 2 GiBを測定用の初期値**にする。同時受付数は暫定4、最大インスタンス数は1。CPU/RAM・受付数は完成実装の計測後に見直す。
- 通常は最小0台。実演前に最小1台へ変更し、データ読込完了と実検索を確認する。最小1でも再起動は起こり得るため、初回起動や応答時間を保証しない。[最小台数](https://docs.cloud.google.com/run/docs/configuring/min-instances)
- 事故原本約420 MiBはRAM使用量の実測ではない。前処理を配備前に済ませ、必要な読み取り用成果物だけをイメージへ同梱する。原本・前処理の作業領域を実行環境に持ち込まず、成果物の版とハッシュを記録する。起動時に索引を読み込み、準備完了をstartup probeで確認する。Cloud Runは起動4分以内に待受・正常化する必要があり、起動後にファイルへ書くデータはメモリを消費して再起動で消える。[実行契約](https://docs.cloud.google.com/run/docs/container-contract)
- 事故・位置・検索履歴のDB、書き込み用永続ディスク、独自ドメインは置かない。共有レート制御にはFirestoreの小さな運用状態だけを使い、提供元識別子・時刻・件数・制御用世代番号以外を保存しない。事故データは引き続き読み取り専用ファイルとする。詳細は[バックエンド仕様](backend-spec.md)に従う。
- **実演はCloud RunのURL（`https://…run.app`）へブラウザから直接アクセスして行う（利用者決定、2026-10-24）。アクセスはIdentity-Aware Proxy（IAP）で許可したGoogleアカウントに限定する。** Cloud RunサービスでIAPを有効にし、開発者・審査員のアカウントへ`roles/iap.httpsResourceAccessor`を与え、IAPのサービスエージェントへ`roles/run.invoker`を与える。未認証公開や`allUsers`付与は行わない。ingressはall。HTTPSで配信されるため、スマートフォンでもGPS・PWAを利用できる。組織外のGoogleアカウント（審査員の個人アカウント等）を許可する場合にOAuthクライアントの追加設定が必要かは配備前に確認する。[Cloud RunのIAP](https://cloud.google.com/run/docs/securing/identity-aware-proxy-cloud-run)
- 実演後に継続利用するなら最小0台へ戻す。利用終了ならサービスを削除し、イメージ・ビルド成果物・残るログ等の保持設定と請求を確認する。最小0台だけでは再アクセスによる起動や保存領域の料金は止まらない。

## 外部API制限とログ

**最大1台は、外部APIの共有レート制御の代わりにならない。** 通常の単一プロセス内では提供元ごとの同時数・呼出し間隔・待ち行列を制御するが、再配備・保守等では複数インスタンスが一時的に併存する。複数プロセスで共有する上流上限がある場合は共有ゲートを設けるか、その構成を許す提供元を選んでから配備する。最大台数を増やす場合も同じ。単一プロセス内のロックだけで全利用者共通の上限を保証しない。[スケーリング上の例外](https://docs.cloud.google.com/run/docs/about-instance-autoscaling#exceeding_maximum_instances)

共有ゲートはFirestoreのトランザクションで管理する。読書き・保存・通信に別途料金が発生し得るため、下記のCloud Run計算例には含めない。アプリ全体の見積もり時にはこの利用量も含める。[Firestoreの料金](https://cloud.google.com/firestore/pricing)

利用者は経路・住所提供元での座標/住所の運用ログを許容した。提供元の無保存契約を必須にはしないが、利用規約・送信可能な情報・帰属・制限は別に満たす。自アプリの位置/履歴DBを新設する許可へ広げない。住所はGeoapify、気象はOpen-Meteoを使い、経路提供元はアルゴリズム側の結論を接続する。詳細は[バックエンド仕様](backend-spec.md)。

アプリ内で位置を永続保存しない方針と、クラウドのログは別に確認する。Cloud Runはリクエストログを自動生成し、HTTPログのURLにはクエリ文字列も含まれる。GET `/api/conditions` の丸めた地域座標や、住所を含むGET URLも対象になる。[自動ログ](https://docs.cloud.google.com/run/docs/logging)、[HTTPログの形式](https://docs.cloud.google.com/logging/docs/reference/v2/rest/v2/LogEntry#HttpRequest)

配備時は、実位置・住所を送る前にCloud Loggingの`_Default` sinkへ次の**除外フィルタ**を追加する。`PROJECT_ID`と`tkb-api`は実際のプロジェクトID・サービス名へ置き換える。このサービスの自動リクエストログを全件、当該sinkの保存対象から外す。

```text
resource.type="cloud_run_revision"
resource.labels.service_name="tkb-api"
logName="projects/PROJECT_ID/logs/run.googleapis.com%2Frequests"
```

他のsink・組織側の転送先があれば同じログが保存されないことも確認する。アプリの標準出力にはURLクエリ・住所・座標・リクエスト本文・上流URLを出さず、ルート名・HTTP状態・処理時間・匿名のエラーコードだけを記録する。除外は生成後のルーティングに作用し、提供元の一時処理までなくす保証ではない。ダミー入力で保存先を確認してから実利用する。[ログの保存・転送と除外](https://docs.cloud.google.com/logging/docs/routing/overview)

## 費用の前提と検証残件

東京はTier 1。リクエスト課金の通常USD単価は、稼働CPUが$0.000024/vCPU秒、RAMが$0.0000025/GiB秒、リクエストが$0.40/100万件。最小台数の待機CPUは$0.0000025/vCPU秒、待機RAMは同じ単価。1 vCPU / 2 GiBで「8時間すべて稼働」と仮定するとCPU/RAM約$0.84、「8時間すべて待機」なら約$0.22。これは無料枠適用前の計算例であり、リクエスト・起動/終了・通信・保存・ビルド・ログ・税等を含む総額見積もりではない。[現行料金](https://cloud.google.com/run/pricing)

無料枠は請求アカウント内で共有され、適用可否・消費状況は未確認。無料や費用上限を保証しない。実測後に利用時間・回数・データ量から見積もり、配備時にプロジェクト単位の予算通知を設定する。通知だけの予算は支出を停止しない。[予算通知](https://docs.cloud.google.com/billing/docs/how-to/budgets)

配備前の検証残件：IAPでの許可/拒否と組織外アカウントの扱い、前処理成果物のサイズ、起動時/検索中のピークRAM、CPU時間、起動・検索待ち時間、同時利用、イメージサイズ、外部提供元の上限と共有制御、アクセス制限、自動ログ除外の実効性。経路エンジンの自己ホストが必要と確定した場合は、道路データ・RAM・常時稼働の要件を別途測定し、その部分のホスティングを再選定する。
