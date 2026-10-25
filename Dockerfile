# syntax=docker/dockerfile:1
# Cloud Run用のイメージ（担当：③）。APIサーバ・ビルド済みフロント・検証済みreleaseを1つのイメージに同梱する。
# 手順と未決事項は deploy/README.md。原本 accident/ と前処理の作業領域は含めない（.dockerignore）。
#
#   docker build \
#     --build-arg ACCIDENT_RELEASE=backend/data/releases/<release-id> \
#     --build-arg VITE_MAP_STYLE_URL=… \
#     --build-arg VITE_ROUTING_ATTRIBUTION=… \
#     --build-arg VITE_MAP_ATTRIBUTION=… \
#     -t tkb-api .

# ---- frontend（①のビルドコマンド） ----
FROM node:22-bookworm-slim AS frontend
WORKDIR /src/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ ./
# frontendのテストが型検査（tsc -b）の対象で、../../../docs/api/fixtures/ のJSONをimportする。配置はリポジトリと同じ相対位置にする。
COPY docs/api/fixtures/ /src/docs/api/fixtures/
# frontendの既定はdemo。配備ではlive modeでビルドする。VITE_* はブラウザへ公開される値で、秘密を入れない。
ARG VITE_ENABLE_SIMULATION
ARG VITE_MAP_STYLE_URL
ARG VITE_ROUTING_ATTRIBUTION
ARG VITE_MAP_ATTRIBUTION
ENV VITE_API_MODE=live \
    VITE_ENABLE_SIMULATION=${VITE_ENABLE_SIMULATION} \
    VITE_MAP_STYLE_URL=${VITE_MAP_STYLE_URL} \
    VITE_ROUTING_ATTRIBUTION=${VITE_ROUTING_ATTRIBUTION} \
    VITE_MAP_ATTRIBUTION=${VITE_MAP_ATTRIBUTION}
RUN test -n "$VITE_MAP_STYLE_URL" && test -n "$VITE_ROUTING_ATTRIBUTION" && test -n "$VITE_MAP_ATTRIBUTION" \
    || { echo "VITE_MAP_STYLE_URL・VITE_ROUTING_ATTRIBUTION・VITE_MAP_ATTRIBUTION は必須です" >&2; exit 1; }
RUN npm run build

# ---- backend（③のAPIサーバ） ----
FROM rust:1.88-bookworm AS backend
WORKDIR /src/backend
COPY backend/ ./
RUN cargo build --release --locked --bin accident-route-backend

# ---- release（②の検証済み成果物。置き場所は未決のためビルド引数で渡す） ----
FROM busybox:1.37 AS release
ARG ACCIDENT_RELEASE
COPY ${ACCIDENT_RELEASE:-backend/data/releases/__missing__} /release/
RUN test -n "$ACCIDENT_RELEASE" \
    && test -f /release/accidents.csv.gz && test -f /release/metadata.json && test -f /release/syukujitsu.csv \
    || { echo "ACCIDENT_RELEASE に検証済みrelease（accidents.csv.gz・metadata.json・syukujitsu.csv）を指定してください" >&2; exit 1; } \
    && chmod -R a-w,a+rX /release

# ---- 実行 ----
FROM gcr.io/distroless/cc-debian12:nonroot
WORKDIR /app
COPY --from=backend /src/backend/target/release/accident-route-backend /app/accident-route-backend
COPY --from=frontend /src/frontend/dist /app/frontend/dist
COPY --from=release /release /app/data/release
# パスは絶対パス。待受けはCloud Runの $PORT（BACKEND_BIND を設定しないと 0.0.0.0:$PORT）。
# 上流URL・帰属・鍵ファイル等の必須設定は配備時に渡す（deploy/README.md）。
ENV ACCIDENT_DATA_DIR=/app/data/release \
    FRONTEND_DIST_DIR=/app/frontend/dist \
    PORT=8080
USER nonroot:nonroot
EXPOSE 8080
ENTRYPOINT ["/app/accident-route-backend"]
