# Cloud Run＋IAPへの配備手順（担当：③）

**この手順は未実施です。** GCPプロジェクト・請求・IAPの許可アカウント・Geoapifyの鍵・Open-Meteoのプラン・経路提供元・地図style・実releaseが揃い、利用者が承認するまで実行しません（Issue #9 S18・S20）。コマンドは公式資料に照らした確認をしていない下書きです。実行前に、各リンク先の現行の資料でフラグ名と必要な権限を確かめてください。

前提の仕様：[cloud-selection.md](../docs/cloud-selection.md)、[backend-spec.md](../docs/backend-spec.md) 7・8章、[architecture.md](../docs/architecture.md) 4〜6章。

以下の `PROJECT_ID`・`PROJECT_NUMBER`・`REGION=asia-northeast1`・`SERVICE=tkb-api`・`REPO`（Artifact Registry）・`RELEASE_ID` は実際の値に置き換えます。

## 1. イメージ

ルートの `Dockerfile` は、①のビルド（`npm ci && npm run build`、`VITE_API_MODE=live`）、③のサーバ（`cargo build --release --locked --bin accident-route-backend`）、②の検証済みreleaseを1つのイメージに入れます。実行は非rootの `nonroot` ユーザーで、`0.0.0.0:$PORT` で待ち受けます（`BACKEND_BIND` を設定しないとき）。`ACCIDENT_DATA_DIR=/app/data/release`、`FRONTEND_DIST_DIR=/app/frontend/dist` は絶対パスです。原本 `accident/`・前処理の作業領域・`.env` 類は `.dockerignore` で除きます。

```sh
docker build \
  --build-arg ACCIDENT_RELEASE=backend/data/releases/$RELEASE_ID \
  --build-arg VITE_MAP_STYLE_URL="$MAP_STYLE_URL" \
  --build-arg VITE_ROUTING_ATTRIBUTION="$ROUTING_ATTRIBUTION" \
  --build-arg VITE_MAP_ATTRIBUTION="$MAP_ATTRIBUTION" \
  -t $REGION-docker.pkg.dev/$PROJECT_ID/$REPO/$SERVICE:$RELEASE_ID .
docker push $REGION-docker.pkg.dev/$PROJECT_ID/$REPO/$SERVICE:$RELEASE_ID
```

- `ROUTING_ATTRIBUTION` は、イメージの `VITE_ROUTING_ATTRIBUTION` とCloud Runの `ROUTING_ATTRIBUTION` に同じ値を渡します（同じ変数から設定する）。
- `VITE_*` はブラウザへ公開される値です。秘密を入れません。
- **未決**：releaseをgitに含めるか、どこに置いてイメージへ入れるか（Issue #9 7章）。今はビルド引数 `ACCIDENT_RELEASE`（ビルドコンテキスト内のパス）で渡し、`accidents.csv.gz`・`metadata.json`・`syukujitsu.csv` が無ければビルドを止めます。中身の検証（hash・schema・件数）は起動時に `data::load_*` が行います。

ローカルでの確認（偽上流は `cargo run --example dev_stack` で起動。コンテナからホストのループバックへは届かないので `--network host` を使う）：

```sh
docker run --rm --network host \
  -e PORT=8080 -e UPSTREAM_GATE=local \
  -e OSRM_WALK_BASE_URL=… -e OSRM_BIKE_BASE_URL=… -e OSRM_CAR_BASE_URL=… \
  -e GEOAPIFY_BASE_URL=… -e GEOAPIFY_API_KEY_FILE=/secrets/geoapify/key \
  -e OPEN_METEO_BASE_URL=… -e EXTERNAL_USER_AGENT=… \
  -e ROUTING_ATTRIBUTION=… -e GEOCODING_ATTRIBUTION='[{"text":"Powered by Geoapify","url":"https://www.geoapify.com/"}]' \
  -v /path/to/dummy.key:/secrets/geoapify/key:ro \
  $SERVICE:local
curl -s http://127.0.0.1:8080/api/health
```

## 2. 秘密（Secret Manager）

鍵はSecret Managerに置き、Cloud Runのボリュームとしてサーバだけにファイルで渡します。環境変数の値やビルド引数・`VITE_*` には入れません。

```sh
printf %s "$GEOAPIFY_KEY" | gcloud secrets create geoapify-api-key --data-file=- --replication-policy=user-managed --locations=$REGION
# 有償プランのときだけ
printf %s "$OPEN_METEO_KEY" | gcloud secrets create open-meteo-api-key --data-file=- --replication-policy=user-managed --locations=$REGION
gcloud secrets add-iam-policy-binding geoapify-api-key \
  --member=serviceAccount:$RUNTIME_SA --role=roles/secretmanager.secretAccessor
```

[Secret Managerのシークレットを使う](https://cloud.google.com/run/docs/configuring/services/secrets)

## 3. 上流ゲート（Firestore）

既定の `UPSTREAM_GATE=firestore` では、全インスタンス共通の通信間隔・同時数・日次上限をFirestoreのトランザクションで管理します。プロジェクトIDはメタデータサーバから取得し、データベース `(default)` のコレクション `upstream_gates` に提供元ごとの文書を1つ置きます。保存するのは提供元識別子・時刻・件数・世代番号だけです。

```sh
gcloud firestore databases create --location=$REGION --type=firestore-native
gcloud projects add-iam-policy-binding $PROJECT_ID \
  --member=serviceAccount:$RUNTIME_SA --role=roles/datastore.user
```

- クライアント（ブラウザ・モバイルSDK）からの直接アクセスを許可しません。セキュリティルールを全拒否にします（サーバはIAMで認可されるので、ルールの影響を受けません）。

  ```text
  rules_version = '2';
  service cloud.firestore {
    match /databases/{database}/documents {
      match /{document=**} { allow read, write: if false; }
    }
  }
  ```

- **未決**：Firestoreのプロジェクトをメタデータから取る方式、コレクション名、Geoapifyの日次上限の日付境界（UTC）、Open-Meteoの分/時/日/月の上限値（実装の定数は600/5,000/10,000/300,000で要確認）。

[Firestoreのデータベース作成](https://cloud.google.com/firestore/docs/create-database-server-client-library)、[セキュリティルール](https://firebase.google.com/docs/firestore/security/get-started)

## 4. Cloud Runサービス

初期値（cloud-selection.md）：東京、1 vCPU / 2 GiB、同時受付4、最大1台、通常は最小0台、要求タイムアウトは180秒以上、ingressはall、未認証の公開をしない。startup probeは `/api/health`（全件ロードと検証が終わるまで503）で、起動4分以内に成功させます。

```sh
gcloud run deploy $SERVICE \
  --image=$REGION-docker.pkg.dev/$PROJECT_ID/$REPO/$SERVICE:$RELEASE_ID \
  --region=$REGION \
  --cpu=1 --memory=2Gi --concurrency=4 \
  --min-instances=0 --max-instances=1 \
  --timeout=180 \
  --ingress=all --no-allow-unauthenticated \
  --service-account=$RUNTIME_SA \
  --startup-probe=httpGet.path=/api/health,periodSeconds=10,timeoutSeconds=5,failureThreshold=24 \
  --set-secrets=/secrets/geoapify/key=geoapify-api-key:latest \
  --set-env-vars=GEOAPIFY_API_KEY_FILE=/secrets/geoapify/key \
  --set-env-vars=OSRM_WALK_BASE_URL=…,OSRM_BIKE_BASE_URL=…,OSRM_CAR_BASE_URL=… \
  --set-env-vars=GEOAPIFY_BASE_URL=https://api.geoapify.com,OPEN_METEO_BASE_URL=… \
  --set-env-vars=EXTERNAL_USER_AGENT=…,ROUTING_ATTRIBUTION=… \
  --set-env-vars=^@^GEOCODING_ATTRIBUTION=[{"text":"Powered by Geoapify","url":"https://www.geoapify.com/"}]
```

- 環境変数はarchitecture.md 6章の表のとおりです。必須の不足・不正は起動に失敗し、ログには変数名だけが出ます（値は出しません）。`BACKEND_BIND`・`UPSTREAM_GATE` は設定しません（`0.0.0.0:$PORT`、`firestore`）。
- `GEOCODING_ATTRIBUTION` はカンマを含むので、区切り文字を変える記法（`^@^`）か `--env-vars-file` を使います。
- 有償のOpen-Meteoでは `--set-secrets=/secrets/open-meteo/key=open-meteo-api-key:latest` と `OPEN_METEO_API_KEY_FILE=/secrets/open-meteo/key` を足します。
- releaseの差し替えは、新しいイメージで新リビジョンを作り、startup probeの成功後にトラフィックを移します。失敗したら旧リビジョンへ戻します。

[サービスの配備](https://cloud.google.com/run/docs/deploying)、[startup probe](https://cloud.google.com/run/docs/configuring/healthchecks)、[要求タイムアウト](https://cloud.google.com/run/docs/configuring/request-timeout)、[コンテナの実行契約](https://cloud.google.com/run/docs/container-contract)

## 5. IAP

```sh
gcloud run services update $SERVICE --region=$REGION --iap
# 許可する開発者・審査員のアカウントごと
gcloud iap web add-iam-policy-binding \
  --resource-type=cloud-run --service=$SERVICE --region=$REGION \
  --member=user:ACCOUNT@example.com --role=roles/iap.httpsResourceAccessor
# IAPのサービスエージェントにだけ呼出しを許可する（allUsersを付けない）
gcloud run services add-iam-policy-binding $SERVICE --region=$REGION \
  --member=serviceAccount:service-$PROJECT_NUMBER@gcp-sa-iap.iam.gserviceaccount.com \
  --role=roles/run.invoker
```

- 組織外のGoogleアカウント（審査員の個人アカウント等）を許可する場合に、OAuthクライアントの追加設定が必要かを配備前に確認します。
- 確認：許可したアカウントで表示できる。許可外のアカウントと未ログインでは拒否される。

[Cloud RunのIAP](https://cloud.google.com/run/docs/securing/identity-aware-proxy-cloud-run)

## 6. ログ

実位置・住所を送る前に、Cloud Loggingの `_Default` sinkへ除外フィルタを追加し、このサービスの自動リクエストログ（URLのクエリを含む）を保存対象から外します。

```sh
gcloud logging sinks update _Default \
  --add-exclusion=name=tkb-api-requests,filter='resource.type="cloud_run_revision" AND resource.labels.service_name="tkb-api" AND logName="projects/PROJECT_ID/logs/run.googleapis.com%2Frequests"'
```

- 他のsink・組織側の転送先があれば、同じログが保存されないことを確かめます。
- アプリの標準出力はルートのテンプレート・status・処理時間・匿名のエラーコードだけです（URLのクエリ・住所・座標・本文・上流URL・鍵を出しません。`backend/tests/logging.rs`）。
- ダミーの入力（実在しない地名と海上の座標など）で保存先を確認してから、実入力で使います。

[ログの保存・転送と除外](https://cloud.google.com/logging/docs/routing/overview)、[Cloud Runの自動ログ](https://cloud.google.com/run/docs/logging)

## 7. 予算

プロジェクト単位の予算通知を設定します。通知だけでは支出は止まりません。

```sh
gcloud billing budgets create --billing-account=$BILLING_ACCOUNT \
  --display-name=tkb-api --budget-amount=… \
  --filter-projects=projects/$PROJECT_ID \
  --threshold-rule=percent=0.5 --threshold-rule=percent=0.9 --threshold-rule=percent=1.0
```

[予算通知](https://cloud.google.com/billing/docs/how-to/budgets)

## 8. 実演の前後

- 実演前：最小1台にする（`gcloud run services update $SERVICE --region=$REGION --min-instances=1`）。`/api/health` の `release_id`・`dataset_id`・`accidents`・`data_years`、固定の検索、IAPの許可/拒否、各上流、共有ゲート、ログ除外を確認します。
- 実演後に続けて使うなら最小0台へ戻します（`--min-instances=0`）。
- 終了するなら、サービス・不要なイメージ・Firestoreの `upstream_gates`・シークレットとその参照を削除し、残るログの保持設定と請求を確かめます。最小0台や予算通知だけを費用の上限として扱いません。
