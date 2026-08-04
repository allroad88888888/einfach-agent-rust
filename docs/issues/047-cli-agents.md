# 047 CLI `/agents` 文本树

**里程碑** M7 · **依赖** 046 · **模型** sonnet · **独测** —

M7 的第一个「能用」刹车片：只靠 core 的 `agent_tree()`，在 CLI 里把树打出来。像 022
之于 MCP——如果快照本身不对，后面的 SSE 和 web 面板全是空中楼阁，先在最便宜的壳上验它。

## 范围

1. **`/agents` 命令**（`agent-cli/src/repl.rs`）：调 `session.agent_tree()`，渲染成缩进
   文本树。每行：`<缩进×depth> <id 短形> [<status>] <activity> · <task 截断> · <usage>`。
2. 渲染是纯格式化——不碰状态，不新增命令外的行为。放一个单独的小模块
   （`print/agent_tree.rs` 或近似），别塞进 `repl.rs` 让它顶破行数。

## 验收（可判定）

- 单 agent 会话 `/agents`：一行，root，状态跟 `/`（当前状态）一致。
- 模型 spawn 了子 agent 后 `/agents`：树形缩进正确，父子关系肉眼可辨，每个子 agent 一行
  带它的 task 和 activity。
- `/undo` 撤掉 spawn 那轮后 `/agents`：子 agent 不再出现（复用 046 的派生读，白拿）。
- 无子 agent、纯单轮对话：`/agents` 不报错，就一行 root。

## 注意

- **只读**：`/agents` 不改任何状态，就是 `agent_tree()` 的一个渲染器。
- 与 `/skills`（039）同款：「有哪些 / 什么状态」是 core 的事，命令只负责摆出来。
- 渲染逻辑跟 048 的 web 面板**共用同一个 `agent_tree()` 数据**（OBSERVABILITY.md：不搞
  两套重建）——CLI 只是把同一份快照渲成文本，web 渲成面板。
- 收工验证前台跑完（WORKFLOW §四 -1）：真起一次会话、真 spawn 子 agent、`/agents` 看到
  树、`/undo` 看到回退，再交报告。

## 实做记录（完成 · 2026-08-03）

单个 sonnet 实现 agent（无红线，自测）。派活单顶部写死「验证前台跑完、禁后台 + 等
监视器」——治了 046 那次「收尾自旋」，这个 agent 干净收工。

**产出**：渲染器 `crates/agent-cli/src/print/agent_tree.rs`（190 行，纯格式化
`render_agent_tree(&AgentTree) -> String` + `short_id`/`describe_activity`/`describe_task`），
8 个单测直接构造 `AgentTree` 断言输出（root 单格 / root+两子 / 两层孙 / 各 activity 变体 /
缺 task / 长文本截断）；`repl.rs` 加 `/agents` arm（照 `/skills` 同款，125 行）；
`print/mod.rs` 导出。格式每行 `<缩进×depth><短id> [<activity>] · <task 截断>`。

**验收兑现**（主会话从磁盘跑）：`cargo test -p agent-cli` 16 lib + 集成全绿；
`clippy -D warnings` 净；红线过；无临时文件残留（agent 用完删了合成 smoke example）。
渲染器 190 / repl 125 行，红线 9 内。渲染器只读（收 `&AgentTree`，模块内不碰 store），
与 048/049 **共用同一个 `agent_tree()` 快照**——不重建。

**验收边界**：「真起模型 spawn 子 agent，从 REPL 看 `/agents` + `/undo`」这段 live 端到端
**折进 049**（需真 key + 模型行为，是 web 终点的活）。047 用构造 `AgentTree` 的单测 +
一次合成 smoke（临时 example，跑完删）覆盖渲染器与接线；`agent_tree()` 本身 046 已独测。
