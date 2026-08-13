# 168 补 GitHub repo 元信息

**里程碑** L · **依赖** [165](165-launch-positioning-decision.md) · **模型** haiku · **状态** 完成（2026-08-13）

## 目标

description / topics / homepage 三个字段全空。GitHub 的搜索、topic 页、
「你可能感兴趣」推荐**全靠这三个字段**——空着等于把仓库从发现路径里摘出去。
这是全部推广工作里成本最低的一件（一条命令），做完立刻生效。

## 做了什么

`gh repo edit` 一次写入：

**description**（承载 [165](165-launch-positioning-decision.md) 的一句话定位，压进 GitHub 的显示长度）：

> An embeddable agent runtime with a real ledger: undo, redo, crash recovery and audit
> replay are one mechanism, not four features. Runs on a server, in a desktop app, or
> entirely in the browser via wasm.

**topics**（10 个）：`rust` `llm` `agent` `wasm` `agent-runtime` `ai-agents`
`undo-redo` `event-sourcing` `mcp` `deepseek`

选词理由：前五个是品类词（有人在 topic 页浏览）；`undo-redo` / `event-sourcing`
是**差异词**——按 L2「不进 Rust agent 框架品类」，要被那些在找「状态/账本」的人搜到，
而不是被找「LLM 框架」的人搜到之后失望；`deepseek` 是当前唯一实测过的 provider，
诚实且有搜索量。

## 验收

- [x] `gh repo view --json description,repositoryTopics` 两项都非空并回显预期内容
- [x] topics 含 `rust` `llm` `agent` `wasm` `agent-runtime`

## 留的尾巴

**homepage 仍然空着，这是有意的**——它该填 [170](170-pages-workflow.md) 的 Pages demo URL。
现在随便填个链接（比如文档）会占掉那个位置，而 demo 链接是全部推广里点击率最高的一个坑位。
见 [173](173-readme-demo-hero.md)。
