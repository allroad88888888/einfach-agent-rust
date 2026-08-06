# 093 — 非视觉 Agent 委派视觉子 Agent

> 本文是辅助中文摘要。规范主文档为
> [093-vision-subagent-delegation.md](./093-vision-subagent-delegation.md)。

## 核心能力

DeepSeek 不需要假装自己能看图。它在需要视觉证据时调用专用
`srv:vision/inspect`，Rust 只把模型选中的图片和问题交给受信任的 Kimi 子 Agent，
再把子 Agent 的文字观察作为工具结果还给 DeepSeek，让原对话继续完成。

```text
DeepSeek -> 选择图片句柄 -> Rust 受信任路由 -> Kimi 隔离子 Agent -> 文字结果 -> DeepSeek
```

这条能力最强的地方是边界清楚：AI 只决定“看哪张图、解决什么问题”，不能决定 API
地址、密钥或原始模型；Rust 决定可信执行环境；Kimi 子 Agent 不带父对话、不带其他图片、
不带任何工具，因此不会把完整上下文和权限一起扩散出去。

## 已拍板

- V1 用一个小而专用的 Tool，不额外套 Skill；以后视觉工作流变多时再做延迟 Skill 目录。
- 子 Agent 底层采用通用启动协议，但视觉 Tool 固定使用 `vision` 能力、空前文、显式图片、空工具集。
- 非视觉入口不再丢图片字节；图片进入有配额、会话隔离的临时附件仓。
- 模型只看到 `img_*` 安全句柄，永远看不到原始字节、本地路径、`ms://`、endpoint 或 key。
- core 只持久化不透明执行配置 ID；runtime 才保存真实 provider/client/model/secret 绑定。
- V1 附件是临时状态。关闭、过期、淘汰或进程重启后明确返回
  `attachment_unavailable`，绝不静默降级。
- 视觉检查由模型按需触发；自动识别所有图片是后续独立产品模式。

## 小任务树

```text
093
├─ B core：子 Agent 执行配置持久化、undo/redo/restore
├─ C runtime：多 provider 可信路由与 fail-closed
├─ D runtime：拆出 spawn schema/parser，修复超限文件
├─ E server：会话级附件注册、租约、配额、过期与隔离
├─ F ingress：DeepSeek 图片保留、安全句柄与占位提示
├─ G vision：专用 Tool、Kimi 隔离 child、结果和终态错误
├─ H config：命名执行配置与 `vision` 能力映射
└─ I tests：持久化、并发、取消、恢复、安全与 DeepSeek→Kimi E2E
```

第一批并行做 B、D、E；第二批接 C/H；随后串起 F/G，最后补齐全链路验证。每批独立
commit，且不会把当前工作区里无关的已有修改混进提交。
