//! 找到 `packages/web` 的构建产物目录——这个文件只干这一件事，不掺起服务器的
//! 逻辑。issue 036「web dist 经 tauri 资源打包或构建脚本拷贝」：这里选**资源
//! 打包**——`tauri.conf.json` 的 `bundle.resources` 把 `packages/web/dist`
//! 整个目录拷进安装包的资源目录（映射到 `web-dist/`），不是构建脚本另外拷贝
//! 一份到别处再嵌入二进制。理由：`with_static_dir` 要的是运行时能读的一个
//! 真实目录（`tower_http::ServeDir` 直接对着它做文件 IO），Tauri 自己的
//! `frontendDist` 内嵌资源协议（`tauri://`）是给 webview 直接加载用的，两条路
//! 都要同一份 dist 内容、但落地形态不同——`bundle.resources` 是唯一一条能在
//! 打包后还留下「一个目录、一堆文件」这种形状的路。
//!
//! # 前端一套不变的证据链
//!
//! `frontendDist`/`beforeBuildCommand` 与 `bundle.resources` 都指向同一个源
//! （`../../../packages/web/dist`，`pnpm --filter web build` 的产物）——打包时
//! 没有第二次构建、没有另外的拷贝脚本，装进 app 里的资源就是仓库里
//! `packages/web/dist` 那份文件的原样拷贝（`tauri build` 内部机制），跟
//! `examples/serve.rs`/`agent-server-bin` 用 `with_static_dir` 发的是逐字节
//! 相同的目录内容。

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

/// 打包资源里那份的相对位置——跟 `tauri.conf.json` 的
/// `bundle.resources."../../../packages/web/dist"` 映射目标必须一致。
const BUNDLED_RESOURCE_SUBDIR: &str = "web-dist";

#[derive(Debug)]
pub struct DistNotFound {
    pub tried: Vec<PathBuf>,
}

impl std::fmt::Display for DistNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "找不到 packages/web 的构建产物（没有 index.html），试过：")?;
        for p in &self.tried {
            writeln!(f, "  {}", p.display())?;
        }
        write!(f, "先 `pnpm --filter web build`")
    }
}

/// 找 `packages/web` 的构建产物目录：打包安装后用 `bundle.resources` 拷进去的
/// 那份；`cargo run`/`tauri dev` 这种没打包的开发态，退回仓库源码树里的
/// `packages/web/dist`（`CARGO_MANIFEST_DIR` 是这个 crate 自己的目录，往上三层
/// 是仓根——跟 `tauri.conf.json` 里 `frontendDist`/`bundle.resources` 用的同一个
/// 相对路径 `../../../packages/web/dist`）。两个候选都没有 `index.html` 就是
/// 真的没构建过，报错列出试过的路径，不猜、不 panic。
pub fn resolve(app: &AppHandle) -> Result<PathBuf, DistNotFound> {
    let mut tried = Vec::new();

    if let Ok(resource_dir) = app.path().resource_dir() {
        let bundled = resource_dir.join(BUNDLED_RESOURCE_SUBDIR);
        if bundled.join("index.html").is_file() {
            return Ok(bundled);
        }
        tried.push(bundled);
    }

    let dev_dist = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../packages/web/dist");
    if dev_dist.join("index.html").is_file() {
        return Ok(dev_dist);
    }
    tried.push(dev_dist);

    Err(DistNotFound { tried })
}
