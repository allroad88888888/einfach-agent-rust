# 164 认领收尾：浏览器宿主的 skills 声明与落店恢复

**里程碑** M17 补做线（157 的地基） · **依赖** — · **模型** 主会话前台（认领） · **独测** —（见「验证」） · **状态** 完成（2026-08-13）

## 来龙去脉（先说清这活儿是谁的）

这批改动（11 个文件 + 新 `capabilities.rs`）是**另一个会话的在飞工作**，一直以
未提交状态躺在主检出里——M17/M18 期间三个会话靠「谁都不碰别人的脏区」并行。
2026-08-13 用户确认那个会话已不存在，指示本会话收尾。**代码是它写的，本会话
只做验证、补文档、提交**；下面的描述按实读代码回填。

## 它做了什么

浏览器宿主（`agent-wasm`）的页面能力声明从「只有 tools」补齐到与 server 侧
（M10/M15）同构的形状：

1. **新 `capabilities.rs`**：页面 JSON 入口。旧 `{"tools":[…]}` 形状完全兼容；
   新增 `skills`（id 白名单 `[A-Za-z0-9_-]`≤128 / 重名拒 / **skill 自带 tools
   即拒**——140 同判）。顶层 tools 继续复用 runtime 既有校验。
2. **声明落店**（073 同构）：新会话 `declare_host_tools`/`declare_host_skills`
   journaled 写入 + `persist::sync`；**恢复只认 journal**——当前宿主构造参数
   不参与，不覆盖历史（`capabilities_for_session` 按 `restored` 二选一，
   有单测钉住）。
3. **装表**：`browser_tool_table` 接 `SkillRegistry::from_host_skills`——声明
   skill 时表里多出 `srv:skill/read` + 开局索引 timed 工具；「业务 `srv:` 结构性
   不进表」的既有承诺收窄为「唯一例外是 runtime 自己的 skill read」。
4. **开局驱动**：`run_session_start` 只挂新建路（`restored=false`），位置在
   `persist::seed_after_recover` 之后——135/139 那个时序坑没有踩。
5. **`HOST_TOOL_TIMEOUT` 60s → runtime 默认 10 分钟**（推翻 123 §4 的默认值，
   见下）。
6. **transport 三个 fetch\_\* 文件**：`Headers`/`Request` 对象换成裸 JS 对象 +
   `fetch(url, init)` 两参调用——所有依赖收敛到「全局作用域有一个 `fetch`
   函数」，与 `call_global_fetch` 注释里既有的取向一致（主线程 / Worker /
   Node 测试环境通吃）。

## 123 默认截止线的变更（值得单独一段）

123 当年把 60s 的理由写得很硬（「这一头是个 JS 回调——机器干活」）。但那是
M14 之前：M14 之后页面工具包含**人工参与**的形状（提问 / 上传 / 确认），
「机器干活」的前提不再普遍成立。改回 runtime 默认（10 分钟）消除两个宿主的
约定漂移；取消仍走既有即时信号，不因预算变慢。原作者留了钉值测试
（`human_host_tool_wait_budget_is_ten_minutes`）。123 文件已加追记指回本条。

## 验证（2026-08-13，本会话）

- `scripts/build-wasm.sh --dev` 绿（wasm32 全量编译）。
- `cargo check --tests --target wasm32-unknown-unknown` 绿——原作者写的两个
  单测（恢复只认 journal / 钉 10 分钟）**编译验证过**；但本仓今天没有 wasm
  测试运行 harness（无 `wasm-bindgen-test` 依赖），它们**跑不起来**，等
  harness 进来自动生效。原生 `cargo test` 在这个 crate 从来不通（wasm32-only，
  存量事实，非本批改动引入）。
- `cargo test -p agent-transport` 53 过 0 挂（fetch\_\* 是 wasm32 门控，原生面
  零影响）；workspace 其余不受影响。
- `check-invariants.sh --all` 退出 0；所有改动文件 ≤300 行。
- 真机浏览器 smoke 与 157 合跑，见 [157](157-wasm-prefix-declaration.md)
  实做记录。

## 没做的

- wasm 测试 harness（`wasm-pack test`）——原作者没建，本会话也不顺手建：
  那是一条独立的基建决定，不该藏在收尾提交里。
- `capabilities.prefix`（决策 31 的 wasm 半边）——那是 [157](157-wasm-prefix-declaration.md)
  的活，踩着本条的地基做。
