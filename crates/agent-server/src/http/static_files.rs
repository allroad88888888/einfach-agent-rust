//! SPA 静态托管：issue 036 的 `ServerConfig::with_static_dir`——把
//! `packages/web` 的构建产物从同一个端口发出去。这个文件只做一件事：把「给
//! 一个目录」翻成一个 axum fallback service，未命中的路径（SPA 客户端路由）
//! 落回 `index.html`，真实存在的文件（`/assets/*`）原样吃流式响应体。
//!
//! # 为什么选 tower-http `ServeDir`/`ServeFile` 而不是手写
//!
//! 手写要重新实现的东西 tower-http 已经做对且做全：`Range`/条件请求
//! （`If-None-Match`）、正确的 `Content-Type` 猜测、目录穿越防护（`..`
//! 归一化）、大文件流式发送不整个读进内存。这个仓库对依赖最小化的一贯取向
//! （`agent-server/Cargo.toml` 里 `axum` 只开直接用到的 feature）在这里不
//! 适用——手写会重新发明一个更小、但更容易在这些边缘条件上出 bug 的静态文件
//! 服务器，那不是「更少依赖」，是「更多自己维护的攻击面」。`tower-http` 只开
//! `fs` feature：只要 `ServeDir`/`ServeFile` 两个类型，不要
//! `cors`/`trace`/`compression` 这个仓库用不上的东西。
//!
//! # SPA 兜底怎么跟 API 路由共存
//!
//! [`crate::http::AgentServer::new`] 把这个当 axum `Router::fallback_service`
//! 挂上——**只有请求命中不了任何显式路由（`/sessions...` 那六个端点 + 会话
//! 创建/查询）时才会落到这里**，API 路由优先是 axum 路由匹配本身的语义，不
//! 需要这个模块另外判断。真实存在的静态文件正常返回；不存在的路径（前端
//! 客户端路由）落回 `index.html` 交给前端路由处理，不是 404——这是 SPA 的
//! 标准形状（`not_found_service` 正是这个意思：目录里找不到这个路径对应的
//! 文件时，改发这个 service 的响应，而不是 404）。

use std::path::Path;

use tower_http::services::{ServeDir, ServeFile};

/// 给一个 dist 目录，造一个可以直接喂给 `Router::fallback_service` 的 service。
///
/// 用 `ServeDir::fallback`，不是看着更顺手的 `ServeDir::not_found_service`——
/// 后者内部拿 `SetStatus` 把响应状态码强制改写成 404（tower-http 自己文档写的
/// 「常见于单页应用」指的是 GitHub Pages 那种 `404.html` 技巧：浏览器直接
/// 导航照样渲染，但状态码是 404）。这个仓库要的是真正的 SPA 兜底语义——客户端
/// 路由（比如以后加的 `/session/:id` 前端页面）是一个正常存在的页面，不是
/// 「资源没找到」，`fetch`/`fetch().ok` 之类的调用方也不该把它误判成失败。
/// `fallback` 不碰状态码，`index.html` 本身存在，`ServeFile` 自然应答 200。
pub(in crate::http) fn spa_fallback(dir: &Path) -> ServeDir<ServeFile> {
    let index = ServeFile::new(dir.join("index.html"));
    ServeDir::new(dir).fallback(index)
}
