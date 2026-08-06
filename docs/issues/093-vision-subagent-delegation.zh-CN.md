# 093 — 非视觉 Agent 委派视觉子 Agent

> 本文是辅助中文摘要；规范、完整任务树和验收口径以
> [英文主 issue](./093-vision-subagent-delegation.md) 为准。

**里程碑：** M12 · **状态：** 进行中 · **主模型：** `gpt-5.6-sol`

## 核心能力

DeepSeek 不需要假装自己能看图。它在需要视觉证据时调用专用
`srv:vision/inspect`，Rust 只把模型选中的图片和问题交给受信任的 Kimi 子 Agent，
再把子 Agent 的文字观察作为工具结果还给 DeepSeek，让原对话继续完成。

```text
DeepSeek 根 Agent
  -> 选择会话内图片句柄和问题
  -> Rust 固定可信 vision 执行配置
  -> Kimi 隔离子 Agent（无父前文、无工具）
  -> 文字 Tool 结果
  -> DeepSeek 完成回答
```

最重要的能力边界是：AI 只决定“看哪张图、解决什么问题”，不能决定 provider、
endpoint、key、原始模型或上传引用；Rust 决定可信执行环境；Kimi 只获得本次检查需要的
图片和问题，不会继承完整上下文与权限。这是可观察的显式委派，不是偷偷切换 provider。

## 已拍板的协议

- V1 使用一个小而专用的 Tool，不额外套 Skill；视觉工作流扩展后再考虑延迟 Skill 目录。
- 模型只看到会话隔离的 `img_*` 句柄；原始字节、路径、provider 引用和 secret 都留在 runtime。
- core 只持久化不透明 execution profile ID；runtime 解析真实绑定，找不到就必须在 I/O 前失败。
- 附件所有权记录是有界临时状态：同会话 tombstone 尚保留时返回
  `attachment_unavailable`；tombstone 被裁剪后与未知句柄无法区分，返回
  `attachment_not_found`。任何情况都不能假装已经看过图片。
- 本地取消或超时会锁死当前 attempt，停止后续上传和 chat，并忽略晚到结果。已经开始的同步
  上传可以完成，返回后 preparation 才退出并释放租约；V1 不承诺物理中断或立即释放租约。
- 除“包含它的父 turn 被取消”外，每条检查路径都要结算一个 Tool result。父/session 取消会终止并
  擦除该 turn，因此不承诺再写 Tool result；逻辑 child/前端回执立即失效，已开始的同步工作所持
  租约在它返回后释放。
- 自动检查全部图片是另一种产品模式；本 issue 只实现模型按需调用。
- 图片物化原样复用稳定的同步 `Client::upload_image`；`agent-transport` 不属于本 issue 范围，
  没有为视觉取消或超时做任何改动。

## 关键收口树

完整 A–J 叶子树在英文主 issue。下面把仍需收口或容易误解的叶子独立列出；格式为
`[状态 | owner/模型级别 | wave | 依赖] 目标 — 证据`。

```text
093 [进行中 | lead/SOL]
├─ F. 入口与直接视觉
│  ├─ F4 [review|Maxwell/SOL|W3|E3,C] 用既有上传客户端物化 Kimi 图片 — image_materialization.rs
│  ├─ F5 [review|Maxwell/SOL|W3|F4] 保持现有纯文本请求形状 — http_image_input.rs
│  └─ F6 [review|name-privacy/TERRA|W3|E2] 只接受 basename — validation.rs
├─ G. 视觉 facade 与 child 执行
│  ├─ G3 [review|Maxwell/SOL|W4|E3,F4] 租约内解析所选句柄 — image_resolver.rs
│  ├─ G4 [review|Maxwell/SOL|W4|G3] 只上传所选图片，ref 仅本次请求有效 — image_materialization.rs
│  ├─ G7 [review|Maxwell/SOL|W4|G4] 上传失败映射稳定错误码 — image_preparation_failure_tests.rs
│  ├─ G9 [review|Maxwell/SOL|W4|G3] 超时锁死并阻止后续工作 — deadline.rs, provider_call.rs
│  ├─ G10 [review|Maxwell/SOL|W4|G3] 放弃 attempt，同步工作返回后释放租约 — runner.rs
│  ├─ G11 [review|Maxwell/SOL|W4|G9–G10] 忽略阻塞 I/O 晚到结果 — io_thread.rs
│  ├─ G12 [done|vision/SOL|W4|G6] 子 Agent/provider 错误脱敏 — vision_child_outcome.rs
│  └─ G13 [active|output-privacy/SOL|W4|G6] 精确 provider ref 回显不得进入公共/持久输出 — vision_output_privacy.rs, output_privacy.rs
├─ I. 独立验证叶子
│  ├─ I6a [done|core-audit/SOL|W5|B] restore 保留 profile 身份 — execution_profile.rs
│  ├─ I6b [done|runtime-audit/SOL|W5|C6] 缺 profile 时在 I/O 前失败 — provider_call_tests.rs
│  ├─ I7a [done|Maxwell/SOL|W5|G9] 阻塞上传超时与晚到结果 — timeout.rs, timeout/upstream.rs
│  ├─ I7b [done|Maxwell/SOL|W5|G10] 同步上传中取消并停止后续批次 — image_materialization_tests.rs
│  ├─ I7c [done|e2e/SOL|W5|G4] 连续两次检查互不串扰 — repeated_inspection.rs
│  ├─ I8 [ready|operator/SOL|W6|I15] 可选真实 Kimi 付费验证，尚未运行 — 需明确批准；不阻塞
│  ├─ I9 [done|recovery/SOL|W5|E6,G] 重启丢失在 Kimi I/O 前失败 — restart.rs
│  ├─ I10 [review|error-audit/SOL|W5|G7,G12] 错误与状态脱敏矩阵 — failures.rs
│  ├─ I11 [review|name-privacy/TERRA|W5|F6] 名称校验/隐私 E2E — http_image_name_privacy.rs
│  ├─ I12 [done|privacy/SOL|W5|I3] 无前文/工具/secret/原始字节泄漏 — success.rs
│  ├─ I13 [review|direct-vision/SOL|W5|F4] Kimi 根模型回归 — http_image_input.rs
│  ├─ I14 [review|direct-vision/SOL|W5|F5] 纯文本请求形状回归 — http_image_input.rs
│  └─ I15 [blocked|acceptance/SOL|W6|G13,必需 I] 干净已提交工作树验收 — 预期：测试记录
└─ J. 后续，不阻塞 V1
   └─ J5 [ready|future/SOL|-|-] 持久/无界所有权与 tombstone 元数据 — Decision 7 延后
```

I6a 与 I6b 是两份独立证据：当前不声称已经存在一个“restore 后再发 HTTP”的组合 E2E。
G13 保持 active：把本次请求的精确 ref 带到唯一终态输出闸，抑制不可安全拼接的视觉 raw delta，
递归清理 text/JSON 的精确匹配，并验证 SSE、journal、Tool outcome、下一次根请求都不泄漏；正常
观察仍保留。不得凭 `ms://` 前缀猜测并误删无关内容，完成实现与测试前不能算 done。
I8 只有在操作者明确允许真实付费调用时才执行；当前未运行，跳过它不影响 issue 完成。

取消/超时会放弃精确的 `(agent, attempt)`，并在每次上传前、图片之间、Kimi chat 前检查
call-local latch。已经开始的同步上传可以物理完成；它返回后才释放所持租约，晚到结果被丢弃，
不会再启动下一次上传或 chat。协议不承诺物理 abort 或立即释放租约。每个 provider ref 只属于
一次 inspection 请求，绝不复用，也不进入公共输出或持久状态。

## 验收口径

- DeepSeek 能检查选定的会话图片；Kimi 只看到选中图片、显式问题、固定系统提示和空工具表。
- 所有非取消失败都返回稳定、脱敏的 Tool result；父/session 取消按上面的擦除语义处理。
- Kimi 直接图片输入与现有纯文本请求形状都不能回归；这里不声称请求字节完全一致。
- 所有必需测试最终必须在隔离、干净、已提交的工作树执行，状态在验收前保持“进行中”。
- 本次新增普通源码和测试文件不超过 300 行。`runner.rs` 当前 331 行，是强内聚事件泵/状态机；
  拆开 in-flight 表、deadline 与取消迁移会割裂同一个状态机，因此记录为不超过 500 行的复杂核心例外。
- 两份仅被小改的 legacy test 在本 issue 前已超过 300 行，没有顺手扩大范围重构：
  `agent-core/tests/it/observe_046.rs` 与 `agent-server/tests/it/http_capabilities_survive_restart.rs`。
