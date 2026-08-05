# 092 远端工具认领、终态回执与结果协议

**里程碑** M12 · **依赖** 060 + 066 + 072 · **状态** 协议、Java 透传与 100 轮压测已完成，待双端真机 dogfood

## 目标

把前端 / Java 宿主执行 `web:` 工具的闭环从“发出结果就不管”升级为一份可判定协议：

- 多个执行端同时连接时，同一次调用只能被一个执行端原子认领；
- 结果回传要等 actor 明确答复，HTTP 成功表示结果已匹配并提交，而不只是进队列；
- 重试同一份结果不会重复推进模型，冲突结果会被明确拒绝；
- 成功、业务失败、宿主取消、未认领超时、已认领但结果未知必须分开表达；
- 前端能查询当前状态，UI / 日志能解释一次调用最终发生了什么。

## 先说不能承诺的事

本协议**不声称 exactly-once 外部执行**。如果宿主已经完成下单，恰好在回传结果前断网，
Rust 无法仅凭本地状态证明“下单成功”还是“根本没执行”。原子认领只能消除两个在线执行端
同时开工的重复；跨崩溃 exactly-once 仍需要业务系统接受 `tool_call_id` 作为幂等键。

因此超时必须区分：

- `unclaimed_timeout`：截止前无人认领，可确认宿主没有通过本协议开工；
- `outcome_unknown`：已认领但没有终态回传，副作用可能已经发生，禁止静默自动重试；
- `cancelled`：对话侧已取消，不等于外部副作用一定被撤销。

## 状态机

```text
                     claim
pending_unclaimed ─────────────→ claimed
        │                           │  │  │
        │ deadline                  │  │  └─ host cancelled ─→ cancelled
        ↓                           │  └──── host failed ────→ failed
unclaimed_timeout                   └─────── host succeeded ─→ succeeded
                                    │
                                    └─ deadline / lost host ─→ outcome_unknown

任意非终态 ── session cancel / undo / redo ─→ cancelled
```

终态不可逆；同一个 `submission_id` 重放只返回 `duplicate`，不再次写入 core。

## 协议草案 v2

### 1. 原子认领

`POST /sessions/{id}/tool_claim`

```json
{
  "agent": "root",
  "tool_call_id": "call-1",
  "claim_id": "executor-generated-random-id"
}
```

`claim_id` 由执行端在真正执行前生成，并在本次执行生命周期内保存。服务端在 actor 线程内
完成 compare-and-set：第一位认领者成功；相同 `claim_id` 重试仍成功；不同认领者得到冲突。
执行端只有收到 `claimed | already_claimed_by_you` 才能开始外部副作用；请求超时等同于
“认领结果未知”，必须用同一个 `claim_id` 重试，不能换 id 直接执行。

成功响应为 `200`：

```json
{
  "disposition": "claimed",
  "agent": "root",
  "tool_call_id": "call-1",
  "request": { "tool": "web:order/create", "input": {}, "location": "Web" }
}
```

`disposition` 为 `claimed | already_claimed_by_you`。其他结果使用结构化错误码：

- `409 tool_claimed_by_other`
- `410 tool_call_terminal`
- `404 tool_call_unknown`

### 2. 终态提交与强确认

保留 `POST /sessions/{id}/tool_result` 路径，v2 请求改为带认领凭据和提交幂等键：

```json
{
  "agent": "root",
  "tool_call_id": "call-1",
  "claim_id": "executor-generated-random-id",
  "submission_id": "one-final-submission-id",
  "outcome": {
    "status": "failed",
    "error": {
      "code": "inventory_shortage",
      "message": "库存不足",
      "retryable": false
    }
  }
}
```

`outcome` 是有标签联合：

- `succeeded { content: string }`
- `failed { error: { code, message, retryable, details? } }`
- `cancelled { reason: string }`

`details` 是可选 JSON，只给宿主观测；进入模型的失败正文由 Rust 按固定模板生成，避免各前端
随意拼接造成协议漂移。成功正文仍受现有 1 MiB HTTP 上限和 core 32 KiB prompt 截断约束。

HTTP 必须等待 actor 作答。只有结果已匹配认领、通过 epoch 闸并提交给当前工具槽，才返回：

```json
{
  "disposition": "committed",
  "terminal_status": "failed",
  "agent": "root",
  "tool_call_id": "call-1",
  "submission_id": "one-final-submission-id"
}
```

同一 `submission_id` + 同一 payload 重试返回 `200 duplicate`；同一 `submission_id` 换
payload，或同一认领换一个 `submission_id`，返回 `409 result_conflict`。这保证“actor 已提交、
HTTP 响应在路上丢失”时可安全重发。未认领、认领不匹配、已超时分别返回明确错误，不再用
SSE `transport_trouble` 代替 HTTP 应答。

### 3. 状态投影

把现有 `GET /sessions/{id}/pending_tools` 扩为 active 投影，每项增加：

```json
{
  "state": "pending_unclaimed",
  "revision": 3,
  "agent": "root",
  "call_id": "call-1",
  "request": {},
  "created_at_unix_ms": 1785912000000,
  "deadline_at_unix_ms": 1785912600000,
  "claimed_by_me": false
}
```

时间一律为 UTC Unix 毫秒整数，不接受本地时区字符串。活动态只有
`pending_unclaimed | claimed`；终态只有 `succeeded | failed | cancelled |
unclaimed_timeout | outcome_unknown`，合计七种状态。每次状态迁移递增 `revision`，状态查询还返回
`updated_at_unix_ms`；终态附 `terminal_origin: host | session | deadline` 和可选
`submission_id`，让 UI 不必从文字猜原因。认领后 `deadline_at_unix_ms` 表示等待终态的截止线，
不再沿用“无人认领”的语义。

最近状态另走 `GET /sessions/{id}/tool_status?agent=...&tool_call_id=...`。若调用方要判断
`claimed_by_me`，通过 `X-Tool-Claim-Id` 请求头携带凭据；禁止把 `claim_id` 放 query，避免被
反向代理访问日志记录。服务端只返回布尔值，不泄露其他执行端的凭据。终态回执使用有界内存
账本，随 session 生灭，不进入 prompt，不跨恢复伪造。状态快照暴露
`retention_floor_revision`（未淘汰时为空）：未发生过淘汰且查不到才返回
`404 tool_call_unknown`；一旦发生过淘汰，账本外的 id 只能诚实返回
`410 status_not_retained`，不能假装还能精确区分“从未存在”和“曾存在但已淘汰”。

`tool_executed` 保留作时间线事件，但它只是异步观测；`tool_result` 的同步响应才是提交确认。

## 兼容策略

- 本轮 v1 `{ result: { content, is_error } }` 继续接受，响应带 `Deprecation: true` 与
  `X-Remote-Tool-Protocol-Deprecated: v1`；它仅供单执行端迁移窗口使用。
- `remote_tool_protocol=v2_required` 的部署配置和拒绝无认领结果属于后续 issue（要和 Java
  网关的分批升级一起落地），本 092 不把该开关记作已实现或验收完成。
- Web demo 与 Java 网关优先迁到 v2，迁完再把服务端默认切为 `v2_required`。
- `srv:tool/activate` 等 Rust 内部工具不经过本协议；只有 `Location::Web` 的真实宿主工具进入。
- 不自动接管已被其他执行端认领的不可逆调用。未来若需要 takeover，必须结合工具可逆性和
  业务幂等能力单开 issue，不能把租约过期直接等同于“可以安全重跑”。

## 树形任务与模型分配

```text
092 root：协议边界与最终集成（gpt-5.6-sol / xhigh，主 agent）
├─ [x] 092-A actor 请求-应答接缝（gpt-5.6-sol / xhigh）
│  ├─ 内部 ActorMessage 携带一次性 reply，不把 sender 塞进可序列化 Command
│  ├─ 原子 claim、幂等 terminal receipt、有界回执账本
│  └─ 先提交 core 事件并答复，再继续慢 provider 调用
├─ [x] 092-B HTTP + TS 协议（gpt-5.6-terra / high）
│  ├─ claim/result/status 请求响应与结构化错误
│  ├─ ts-rs 生成、协议快照、1 MiB 边界
│  └─ v1 迁移期兼容与弃用标记（v2_required 开关另开 issue）
├─ [x] 092-C Web 执行器迁移（gpt-5.6-terra / high）
│  ├─ 稳定 claim_id、执行前认领、结果幂等重试
│  ├─ claimed_by_other 不执行；unknown/terminal 不执行
│  └─ UI 区分 failed/cancelled/outcome_unknown
├─ [x] 092-T 独立验收测试（codex-auto-review / xhigh，不读实现思路）
│  ├─ 两执行端并发认领只有一个成功
│  ├─ HTTP 200 必须代表 actor 已提交，不是仅入队
│  ├─ 相同提交重放不产生第二条 tool_result
│  └─ undo/超时/迟到回传与 epoch 闸
└─ [ ] 092-D 集成验证与真机 dogfood（主 agent）
   ├─ [x] Java 通配代理无损转发 claim/status/v2 result 协议
   ├─ [x] 100 轮真实 TCP 双客户端并发 claim，每轮恰好一个获胜
   ├─ [ ] 浏览器 + Java 网关同 chatid 并连，不重复副作用
   ├─ [ ] 回传响应丢失后重试得到 duplicate
   └─ [ ] 已认领断线显示 outcome_unknown，不谎报普通 timeout
```

执行结果：root 已冻结公开语义；A、B、C、T 已完成并通过自动化验收。D 的 Java 代理透传和
100 轮真实 TCP 并发压测已通过；浏览器与 Java 网关仍需以同一 `chatid` 真机共同接入，不能用
Rust/Web mock 冒充完成。

## 文件职责预案

- `actor/message.rs`：只定义 actor 内部命令信封与一次性回复。
- `remote_tool_claim.rs`：只实现等待槽的原子认领状态机。
- `remote_tool_receipt.rs`：只保存有界终态回执并判定重复提交。
- `http/tool_protocol.rs`：只定义 v2 wire 类型。
- `routes/tool_claim.rs`：只处理认领 HTTP 接口。
- `routes/tool_result.rs`：只处理终态提交 HTTP 接口。
- `routes/tool_status.rs`：只处理单次调用状态查询。
- `web/tool-claim.ts`：只负责认领与幂等提交。
- `web/tool-exec.ts`：只负责调用前端实现并把 outcome 交给提交层。

不往现有大文件硬塞状态机；新增普通文件均须 `wc -l <= 300`。

## 验收

- [x] actor 与 Web 双执行端测试均验证并发 claim 时恰好一个 `claimed`，另一方得到
  `tool_claimed_by_other`，且只有获胜方执行工具。
- [x] `tool_result` 只有在 core 终态已提交后才返回 `committed`，随后 provider 才继续。
- [x] 同一 `submission_id` 与 payload 重放得到 `duplicate`，不会再次推进 core。
- [x] 同一提交换 payload 或同一 claim 换提交得到 `result_conflict`，第一份结果不被覆盖。
- [x] 未认领截止得到 `unclaimed_timeout`；已认领失联得到 `outcome_unknown`。
- [x] undo/cancel 后迟到结果得到明确 `tool_call_terminal`，不会产生幽灵回写。
- [x] Web 在 claim/result 响应丢失和 429/5xx 时复用稳定 id；结果未知只重投，不重跑工具。
- [x] v1 客户端在兼容模式仍可用，且每次 v1 成功响应均带可识别的弃用标记；`v2_required`
  配置开关留待后续 issue 验收。
- [x] `cargo test -p agent-runtime`、`cargo test -p agent-server --features ts`、Web
  typecheck/协议验证/build、`scripts/check-invariants.sh --all` 通过。
- [x] Java 通配代理自动化验证 method/path/query/header/body/response 均可承载 v2 协议。
- [x] 100 轮真实 TCP 双客户端并发 claim 压测，每轮恰好一个 `claimed`、一个 409。
- [ ] Java 网关与浏览器使用同一 `chatid` 完成真机 dogfood。

## 红线与注意

- 红线 6：claim 与 result 都必须绑定 actor 保管的 epoch；客户端仍不能提交 epoch。
- 红线 3：一次性 reply sender 只存在 actor 信封里，绝不能进入 core/store/serde 类型。
- 红线 11：失败正文模板进入 prompt，必须逐字节确定；`details` 不得偷偷拼进正文。
- 回执账本必须有硬上限；不能按每条 1 MiB 正文长期保留，只留状态、id 与必要摘要。
- 有界账本不能承诺永久精确的 `unknown` / `expired` 判定；必须携带 retention 水位并使用
  `status_not_retained` 表达信息已经不足。
- “同步确认”不能等下一次 provider 网络调用完成，否则一个慢模型会把 HTTP 回传拖到超时。
- 当前工作树有大量用户改动，实施时只碰本 issue 列出的文件，遇到重叠先核对而非覆盖。
