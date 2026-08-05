//! issue 036：`ServerConfig::with_static_dir` 独立集成测试——「起 server + curl
//! index.html/资源/API 共存」（issue 原文验收条目）。装一个假 dist 目录
//! （`index.html` + `assets/app.js`），断言三件事都在同一个端口上成立：
//! 真实文件原样吃、未命中的路径落回 `index.html`（SPA 客户端路由的标准形状）、
//! `/sessions...` 的 API 端点不被静态兜底吞掉——命中不了任何真实文件也照样是
//! `session_not_found`，不是 index.html 的 200。

use crate::support;
use std::io::Write;

use crate::support::http_client;

const INDEX_MARKER: &str = "STATIC-DIR-TEST-INDEX";
const APP_JS: &str = "console.log('static-dir-test');";

/// 造 `index.html` + `assets/app.js` 的临时 dist 目录，vite 构建产物的最小形状
/// （`packages/web/dist` 实际长这样：`index.html` 在根，其余进 `assets/`）。
fn fake_dist() -> std::path::PathBuf {
    let dir = support::temp_dir("static-dist");
    std::fs::write(
        dir.join("index.html"),
        format!("<!doctype html><html><body>{INDEX_MARKER}</body></html>"),
    )
    .unwrap();
    let assets = dir.join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    let mut f = std::fs::File::create(assets.join("app.js")).unwrap();
    f.write_all(APP_JS.as_bytes()).unwrap();
    dir
}

#[tokio::test(flavor = "multi_thread")]
async fn real_asset_index_and_spa_fallback_and_api_all_coexist_on_one_port() {
    let dist = fake_dist();
    let upstream = support::server::FakeServer::start(vec![]);
    let server = support::http_server::start_with(upstream.endpoint(), |c| {
        c.with_ring_capacity(5)
            .with_cancel_grace(std::time::Duration::from_millis(200))
            .with_static_dir(dist.clone())
    })
    .await;

    // 1. index.html 本身：根路径。
    let index = http_client::request(server.addr, "GET", "/", None);
    assert_eq!(index.status, 200, "{}", index.body);
    assert!(index.body.contains(INDEX_MARKER), "{}", index.body);
    assert!(
        index
            .header("content-type")
            .is_some_and(|v| v.starts_with("text/html")),
        "{:?}",
        index.headers
    );

    // 2. 真实存在的静态资源：`/assets/app.js` 原样吃，不是兜底的 index.html。
    let asset = http_client::request(server.addr, "GET", "/assets/app.js", None);
    assert_eq!(asset.status, 200, "{}", asset.body);
    assert_eq!(asset.body, APP_JS);
    assert!(
        asset
            .header("content-type")
            .is_some_and(|v| v.contains("javascript")),
        "静态资源该按扩展名猜 content-type：{:?}",
        asset.headers
    );

    // 3. 未命中任何真实文件的路径（前端客户端路由，比如以后加的
    //    `/session/abc` 前端页面）——SPA 兜底：落回 index.html，200 不是 404。
    let spa_route = http_client::request(
        server.addr,
        "GET",
        "/session/abc/does-not-exist-on-disk",
        None,
    );
    assert_eq!(spa_route.status, 200, "{}", spa_route.body);
    assert!(spa_route.body.contains(INDEX_MARKER), "{}", spa_route.body);

    // 4. API 路由优先：`/sessions` 前缀是显式路由，即便请求的 id 在磁盘上也
    //    找不到对应文件，也不会被静态兜底吞成 200 的 index.html——还是
    //    `session_not_found` 的 404。
    let unknown_session = http_client::request(server.addr, "GET", "/sessions/never-existed", None);
    assert_eq!(unknown_session.status, 404, "{}", unknown_session.body);
    assert!(
        unknown_session.body.contains("\"session_not_found\""),
        "{}",
        unknown_session.body
    );

    // 5. API 本身照常工作：同一个端口，静态托管没有把 POST /sessions 顶掉。
    let create = http_client::request(server.addr, "POST", "/sessions", Some("{}"));
    assert_eq!(create.status, 201, "{}", create.body);
    let id = support::extract_json_string_field(&create.body, "id");
    assert!(!id.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn without_static_dir_unknown_paths_are_a_plain_404_not_index_html() {
    // 没设 `with_static_dir` 时行为跟 M3 之前一字不变——没有 fallback_service，
    // 未命中路由的路径是 axum 默认的空 404，不会意外长出静态托管。
    let upstream = support::server::FakeServer::start(vec![]);
    let server = support::http_server::start(upstream.endpoint()).await;

    let resp = http_client::request(server.addr, "GET", "/", None);
    assert_eq!(resp.status, 404, "{}", resp.body);
}
