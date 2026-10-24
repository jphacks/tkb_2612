//! S11：静的配信。一時ディレクトリに作ったdistの模擬で、配信規則どおりに応答することを確かめる。

mod common;

use std::path::Path;

use axum::body::Body;
use axum::http::{Method, StatusCode};

use common::{TestApp, TestAppBuilder};

struct Dist {
    _temp: tempfile::TempDir,
    app: TestApp,
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// `root/dist` を配信rootにし、root外に秘密ファイルを置く。
fn dist() -> Dist {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let dist = root.join("dist");
    write(&dist.join("index.html"), "<!doctype html><title>t</title>");
    write(&dist.join("assets/app.abc123.js"), "console.log(1)");
    write(&dist.join("assets/style.abc123.css"), "body{}");
    write(&dist.join("assets/data.bin"), "bin");
    write(&dist.join("manifest.webmanifest"), "{\"name\":\"t\"}");
    write(&dist.join("sw.js"), "self.addEventListener('fetch',()=>{})");
    write(&dist.join(".env"), "SECRET=1");
    write(&dist.join("assets/.hidden.js"), "hidden");
    write(&dist.join("src/main.ts"), "source");
    write(&dist.join("assets/nested/inner.js"), "nested");
    std::fs::create_dir_all(dist.join("assets/dir.js")).unwrap();
    write(&root.join("secret.txt"), "TOP-SECRET");
    write(&root.join("assets/outside.js"), "TOP-SECRET");
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("secret.txt"), dist.join("assets/link.js")).unwrap();
    let mut builder = TestAppBuilder::default();
    builder.settings.frontend_dist_dir = dist;
    Dist {
        _temp: temp,
        app: builder.build(),
    }
}

#[tokio::test]
async fn the_four_static_routes_have_their_headers() {
    let d = dist();
    let cases = [
        (
            "/",
            "text/html; charset=utf-8",
            "no-cache",
            "<!doctype html><title>t</title>",
        ),
        (
            "/assets/app.abc123.js",
            "text/javascript",
            "public, max-age=31536000, immutable",
            "console.log(1)",
        ),
        (
            "/assets/style.abc123.css",
            "text/css",
            "public, max-age=31536000, immutable",
            "body{}",
        ),
        (
            "/assets/data.bin",
            "application/octet-stream",
            "public, max-age=31536000, immutable",
            "bin",
        ),
        (
            "/manifest.webmanifest",
            "application/manifest+json",
            "no-cache",
            "{\"name\":\"t\"}",
        ),
        (
            "/sw.js",
            "text/javascript",
            "no-cache",
            "self.addEventListener('fetch',()=>{})",
        ),
    ];
    for (target, content_type, cache, body) in cases {
        let response = d.app.get(target).await;
        assert_eq!(response.status, StatusCode::OK, "{target}");
        assert_eq!(
            response.header("content-type"),
            Some(content_type),
            "{target}"
        );
        assert_eq!(response.header("cache-control"), Some(cache), "{target}");
        assert_eq!(response.header("x-content-type-options"), Some("nosniff"));
        assert_eq!(response.body, body.as_bytes(), "{target}");
        let expected_sw = (target == "/sw.js").then_some("/");
        assert_eq!(response.header("service-worker-allowed"), expected_sw);
    }
}

#[tokio::test]
async fn traversal_dotfiles_sources_and_unknown_paths_are_404() {
    let d = dist();
    for target in [
        "/../secret.txt",
        "/%2e%2e/secret.txt",
        "/%2E%2E/",
        "/assets/../secret.txt",
        "/assets/%2e%2e%2fsecret.txt",
        "/assets/..%2fsecret.txt",
        "/assets/%2e%2e/outside.js",
        "/assets/app%2eabc123%2ejs",
        "/assets/",
        "/assets",
        "/assets/.hidden.js",
        "/assets/nested/inner.js",
        "/assets/nested",
        "/assets/dir.js",
        "/assets/link.js",
        "/assets/missing.abc.js",
        "/.env",
        "/src/main.ts",
        "/index.html",
        "/secret.txt",
        "/some/spa/route",
        "/favicon.ico",
        "/sw.js/",
    ] {
        let response = d.app.get(target).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{target}");
        assert_eq!(
            response.header("content-type"),
            Some("text/plain; charset=utf-8"),
            "{target}"
        );
        assert!(
            !String::from_utf8_lossy(&response.body).contains("TOP-SECRET"),
            "{target}"
        );
        assert!(
            !response.body.starts_with(b"<!doctype"),
            "SPAフォールバックしない: {target}"
        );
    }
}

#[tokio::test]
async fn api_paths_are_json_404_not_html() {
    let d = dist();
    for target in [
        "/api",
        "/api/",
        "/api/xxx",
        "/api/route/extra",
        "/api/health/",
    ] {
        let response = d.app.get(target).await;
        assert_eq!(response.status, StatusCode::NOT_FOUND, "{target}");
        common::assert_api_headers(&response);
        assert_eq!(response.error_code(), "not_found");
    }
}

#[tokio::test]
async fn other_methods_are_405_and_head_is_like_get() {
    let d = dist();
    for method in [Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
        for target in [
            "/",
            "/assets/app.abc123.js",
            "/manifest.webmanifest",
            "/sw.js",
        ] {
            let response = d
                .app
                .send(common::request(method.clone(), target, Body::empty()))
                .await;
            assert_eq!(
                response.status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {target}"
            );
            assert_eq!(response.header("allow"), Some("GET"));
        }
    }
    let head = d
        .app
        .send(common::request(Method::HEAD, "/", Body::empty()))
        .await;
    assert_eq!(head.status, StatusCode::OK);
    assert_eq!(
        head.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert!(head.body.is_empty());
}

/// `dist/assets` 自体がdist外へのリンクなら、その先のファイルを配信しない。
#[cfg(unix)]
#[tokio::test]
async fn assets_directory_linked_outside_dist_is_not_served() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let dist = root.join("dist");
    write(&dist.join("index.html"), "<!doctype html>");
    write(&root.join("release/metadata.json"), "TOP-SECRET");
    std::os::unix::fs::symlink(root.join("release"), dist.join("assets")).unwrap();
    let mut builder = TestAppBuilder::default();
    builder.settings.frontend_dist_dir = dist;
    let app = builder.build();
    let response = app.get("/assets/metadata.json").await;
    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert!(!String::from_utf8_lossy(&response.body).contains("TOP-SECRET"));
    assert_eq!(app.get("/").await.status, StatusCode::OK);
}

/// `FRONTEND_DIST_DIR` が未生成でもAPIは使える（API専用の開発起動）。
#[tokio::test]
async fn missing_dist_still_serves_the_api() {
    let app = TestAppBuilder::default().build();
    assert_eq!(app.get("/").await.status, StatusCode::NOT_FOUND);
    assert_eq!(app.get("/sw.js").await.status, StatusCode::NOT_FOUND);
    assert_eq!(app.get("/api/health").await.status, StatusCode::OK);
}
