# 036 Tauri 桌面内嵌 —— M4 终点

**里程碑** M4 · **依赖** 035 · **模型** sonnet · **独立测试 agent** 否（终局主会话验收）· **状态** 完成

## 目标

ROADMAP 里程碑表 M4 原文：「Tauri 内嵌同一个库，前端代码一套不变」。

## 做什么

1. **agent-server 加静态托管选项**：`ServerConfig::with_static_dir(path)`——把
   `packages/web` 的 build 产物从同一个端口发出去（同源，零 CORS，正好是企业
   网关形态的本地缩影）。SPA 兜底路由到 index.html；SSE/API 路由优先
2. **`packages/web` 适配双宿主**：生产构建下 API 走相对路径（已是），vite dev
   proxy 不变——前端代码一套，构建产物两处用（vite dev / 静态托管）
3. **`apps/desktop`**：Tauri 2（`@tauri-apps/cli` devDep 进 pnpm workspace）。
   Rust 侧 setup：随机 loopback 端口起 `AgentServer`（with_static_dir 指向打包进
   资源的 web dist）→ 主窗口导航到 `http://127.0.0.1:PORT`。providers.toml 与
   会话目录放平台标准目录（`dirs` 或 Tauri path API），首启无配置给出可读提示页
4. 退出时优雅 close 会话（快照落盘）

## 验收

- `pnpm --filter desktop tauri build`（或 debug bundle）产出 .app；
  `tauri dev` 起窗口
- 主会话终局：起 app（或其内嵌 server 端口）→ 真实一轮对话 + undo —— 与 web
  版行为一致（同一套前端的证据：dist 哈希一致或直接复用构建产物）
- 静态托管选项独立测试（起 server + curl index.html/资源/API 共存）
- 红线 8：内嵌 server 永远 loopback（桌面场景无 AGENT_BIND 出口，写死）

## 注意

不签名不公证（发布工程不在范围）；窗口 UI 就是 packages/web，桌面独有功能
（托盘/快捷键）不做——M4 是「装得上」不是「桌面产品」。

### 实做记录

**agent-server 静态托管**（`crates/agent-server/src/http/`）：`ServerConfig::
with_static_dir(PathBuf)`（`config.rs`）+ 新文件 `static_files.rs`（38 行）。
依赖选 `tower-http`（只开 `fs` feature，只要 `ServeDir`/`ServeFile` 两个类型）
而不是手写——理由写在模块文档：`Range`/条件请求/`Content-Type`猜测/目录穿越
防护/大文件流式发送这些手写会重新踩的坑,tower-http 已经做对。**用
`ServeDir::fallback`，不是更顺手的 `not_found_service`**——后者内部拿
`SetStatus` 把响应强制改成 404（tower-http 文档说这「常见于单页应用」指的是
GitHub Pages 那种 `404.html` 技巧,状态码依然是 404）；这个仓库要真正的 SPA
兜底语义（客户端路由是一个正常存在的页面，`fetch().ok` 之类的调用不该被
误判失败），改用 `fallback` 后不碰状态码，`index.html` 本身存在自然应答 200
——实测踩过这个坑（第一版用 `not_found_service`，独立测试断言 200 时实际拿到
404，body 内容其实是对的，是状态码错，调试记录见测试文件早期版本）。挂法：
`AgentServer::new` 里 `router.fallback_service(...)`——axum 路由匹配语义保证
显式路由（`/sessions...` 六个端点）永远优先，不需要这层自己判断优先级。
独立测试 `tests/http_static_dir_serves_spa_alongside_api.rs`：真实资源
（`/assets/app.js`）原样吃、SPA 兜底路径落回 index.html（200）、`/sessions`
前缀即使命中不了真实文件也是 `session_not_found` 不会被兜底吞掉、没设
`with_static_dir` 时行为跟 M3 之前一字不变（4 个断言点 2 个测试函数）。

**`apps/desktop`**：035 落地的 `agent_server::bootstrap`/`SessionsHandle`
出现得早（并行开发期间实时观察到），按提示直接复用，没有重新发明装配线。
`src-tauri`（独立 `[workspace]`，根 `Cargo.toml` members 不动，注释写了理由）
六个源文件、单一职责各管一段：`lib.rs`（78 行，Tauri 生命周期粘合）、
`server.rs`（101 行，bootstrap → with_static_dir → 写死 loopback 绑端口 →
后台 serve）、`paths.rs`（39 行，`app.path()` 解平台标准目录）、`dist.rs`
（67 行，找 `packages/web` 构建产物：打包资源优先，开发态退回源码树）、
`first_run.rs`（69 行，缺配置提示页，`file://` 而不是复用
`with_static_dir`——理由写在模块文档：那条路需要先有一份真实
`SessionTemplate`，为了复用去伪造一份没人会用的模板不比直接写文件更省心，
这条路径也从不经过 loopback，红线 8 不适用）。红线 8：桌面场景**不**读
`AGENT_BIND`（不用 `agent_server::default_bind_addr`），直接
`SocketAddr::from((Ipv4Addr::LOCALHOST, 0))`，源码里连读那个环境变量的分支
都没给。

**dist 复用方式——「前端一套不变」的证据链**：`tauri.conf.json` 的
`bundle.resources` 把 `../../../packages/web/dist` 整个目录映射进
`Resources/web-dist/`（不是构建脚本另外拷贝一份，是 tauri 打包步骤直接对
`pnpm --filter web build` 的产物做资源打包）。实测 `tauri build --debug`
产出的 `agent-desktop.app/Contents/Resources/web-dist/` 三个文件（`index.
html`/`assets/index-*.css`/`assets/index-*.js`）跟 `packages/web/dist/` 逐个
`shasum -a 256` **完全一致**。冒烟跑通整条链路：起裸二进制（非 GUI 交互，
只看进程/日志/curl）→ 无配置时首启提示页现造并写出（不 panic，日志报
`BootstrapError` 原文，列出三个试过的路径）→ 写一份假 `providers.toml` 到
平台标准目录后 → 内嵌 `AgentServer` 正常起、`curl GET /` 拿到跟
`packages/web/dist/index.html` 同一个 JS bundle 文件名的页面、
`curl GET /assets/index-*.js` 原样吃、`curl POST /sessions` 201——静态托管
与 API 在桌面壳里同一个端口共存,跟 crate 级独立测试断言的是同一件事。

**异议 / 未做的事**：
- `agent_transport::config` 的三级查找（`$AGENT_PROVIDERS_CONFIG` →
  `./providers.toml` → `~/.config/agent/providers.toml`）是既有共享模块行为，
  桌面壳只把平台标准目录路径写进第一档环境变量，**该档文件不存在时会继续
  往下试**——从仓库根目录的终端直接跑裸二进制做手测时，第二档
  `./providers.toml` 会意外命中仓库自己的开发配置（不是桌面场景的正常
  路径：真正打包安装后由 Finder/Dock 启动，cwd 通常是 `/`，不会撞见这个仓库
  的 `providers.toml`）。如实记一笔，不是这个 issue 范围内的缺陷（没有改
  `agent_transport::config`，035/031 早于本 issue 拍板的查找顺序原样复用）。
- 退出优雅关闭（`RunEvent::Exit` → `SessionsHandle::close_all`）代码路径按
  Tauri 文档标准写法接线、类型检查通过；冒烟测试只验证了裸进程 `kill -TERM`
  不崩（这条路径不经过 Tauri 事件循环，不代表 `RunEvent::Exit` 真的触发过）。
  真正验证「Cmd+Q / 关窗口 → 会话落盘」需要一次真实 GUI 退出交互，按任务边界
  留给主会话。
- 图标是 `tauri init` 生成的占位默认图标，未替换成品牌图标——发布工程不在
  M4 范围，`tauri.conf.json` 的 `bundle.icon` 指到这批默认文件够让
  `tauri build` 跑通。
- 磁盘：`/Volumes/work` 卷在开发过程中被并行 agent 的构建撞到过 100% 满
  （`No space left on device`）。用 `CARGO_TARGET_DIR` 把 `tauri build` 的
  产物临时挪到系统卷的 scratchpad 目录跑通验收，另清了一次
  `target/debug/incremental`（纯编译缓存，删了只影响下次重编译速度，
  不影响正确性）给共享 workspace 争取空间。这不是代码变更，记在这里供后人
  排障——`apps/desktop/src-tauri` 正常场景下用默认 target 目录即可，不需要
  这个环境变量。

### 合并记录 + M4 终局验收（主会话，2026-08-02）

真桌面 app 起窗、内嵌 server 127.0.0.1:60834、`GET /` 返回同一套 web 前端
（SHA256 逐文件同源已由实现证明，运行时 title 复核）、**真实一轮对话**
（问「M4 之后是第几个里程碑完成」，模型流式答「四」）、HTTP undo →
`applied{entries:2}`、退出干净。异议五条全收（裸二进制读到仓库配置的行为
如实记录；Cmd+Q 事件路径待真人 GUI 交互，非阻塞）。磁盘打满事故的处置合规。
