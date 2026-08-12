# 156 server 全链：`capabilities.prefix` 协议 + 校验 + 落店 + 装配

**里程碑** M17 · **依赖** [154](154-host-prefix-slot.md) + [155](155-with-host-prefix.md) · **模型** sonnet · **独测** ✅ · **状态** 完成（见文末，2026-08-12）

## 目标

HTTP 宿主从声明到首轮 prompt 的整条路通：`POST /sessions` 带
`capabilities.prefix` → 校验 → 写 `Slot::HostPrefix`（154）→ 装配
`with_host_prefix`（155）→ `session_start::maybe_run` 落前缀块。
恢复路从店里读回来重新合成，**不重跑**（既有语义）。

## 协议形状（HOST-CAPABILITIES §八之三，决策 31 原文）

```jsonc
"capabilities": {
  "prefix": [ { "name": "web:crm/briefing", "text": "今天的客户上下文：……" } ]
}
```

## 做什么

1. `http/capabilities/mod.rs`：`Capabilities.prefix: Vec<CapabilityPrefix>`
   （`#[serde(default)]` + `ts(optional, as = ...)`，照 `tools` 字段的形状）；
   `CapabilityPrefix { name: String, text: String }` 两个字段都 `#[serde(default)]`
   （缺了由 validate 用结构化错误拒，不让 serde 给通用 400——mod.rs 既有拍板）。
   `ts` feature 导出，`packages/protocol` 生成物跟着更新。
2. `validate.rs` 三条新规（全部 400 且点名是哪一项、为什么）：
   - name 必须带 `web:`/`desk:` 前缀（复用工具名的既有前缀规则；`srv:` 结构性
     撞不了内置 timed 名，这就是撞名判据——ROADMAP §四那条欠的账）；
   - 声明内部重名拒 + 与 `capabilities.tools` 的名字重名拒（两处最后都成为表里
     的名字，一个模型面一个 timed 区，同名会让 `init:<name>` label 与路由说不清）；
   - **text 为空 → 400**。本地 timed 工具空文本跳过是「执行结果」语义（135）；
     声明一段常量空文本只能是笔误，按 069「在最早能报给作者的点上失败」办。
3. 落店/装配：create 路 `declare_host_prefix`（154），恢复路 `host_prefix()`
   读回；两路都在 per-session 装配链**尾部**接 `with_host_prefix`（155 的表尾
   约定）。`session_start::maybe_run` 的位置**一个字不动**（139 修过的时序 bug
   别再引进来：必须在 `persist::seed_after_recover` 之后）。
4. `session_has_history` 闸：已有历史的会话再带 `capabilities`（含只带 `prefix`）
   → 400，**应当是既有闸自动罩住的**——验证并加一条测试钉住，不写新逻辑。

## 验收

- e2e：声明两块 → 首轮 encode body 的 system 段里两块都在、排在内置 init 块
  （如 skills 索引）之后、块间 name 序；不带 `prefix` 的会话请求体与本条落地前
  **逐字节相同**（红线 11）。
- 重启恢复：kill 后重开，首轮 body 与崩溃前**逐字节一致**（回放不重跑）；
  恢复后 spawn 子 agent `inherit_prefix: ["<声明名>"]` 成功、点未声明名照旧拒。
- 校验矩阵：坏前缀 / 内部重名 / 与 tools 重名 / 空 text 各得 400 且错误文本
  点名；`"prefix": []` 与不带字段行为逐字节相同。
- 有历史 + 再声明 → 400 `session_has_history`。

## 注意

- 长度上限**不加**：与 skill `body` 同一笔账（HOST-CAPABILITIES §九 待拍的
  安全项），本条只在 §九 那张单子上补一行「prefix.text 同此案」。
- 老 server 收到带 `prefix` 的请求会静默忽略（`deny_unknown_fields` 没开，
  mod.rs 既有取向「宿主比 server 先升级是常态」）——文档如实写，不改。

## 实做记录（2026-08-12）

- 落点：`CapabilityPrefix`（新文件 `capability_prefix.rs`，53 行）+
  `validate_prefix.rs`（新文件，247 行——塞进 `validate.rs` 会顶破 300，按职责拆）+
  `assemble.rs` 翻料 + `OpenSpec.host_prefix` + `actor/capabilities.rs` 建时
  `declare_host_prefix`/恢复时 `host_prefix()` 读回、装配链 `.with_host_tools(..)`
  之后接 `.with_host_prefix(..)`。`actor/session_start.rs`/`body.rs` **零改动**
  （139 的时序位置一字未动）。TS 生成物：`Capabilities.ts` 加 `prefix?`、新
  `CapabilityPrefix.ts`，一致性测试红转绿。
- **独测抓到一个真缺口并已收紧**（主会话拍板）：初版 name 校验只查 `web:`/`desk:`
  前缀、没复用工具名的本体白名单——`"web:"`、`"web:crm briefing"` 在 tools 侧 400、
  在 prefix 侧 201。收紧到与 `capabilities.tools` **一字不差**（本体非空、全名
  ≤128、`[A-Za-z0-9_/-]`，新 `PrefixRejection::NameShape` 变体）；`"web:/"` 两边
  一致合法（`/` 在白名单内）。理由：名字进 journaled 的 `init:<name>` label、模型
  要在 `inherit_prefix` 里逐字打它、同名不同判是 HOST-CAPABILITIES §三之二点名的
  说不清的面。独测那条「如实钉住宽松行为」的测试翻转为断言收紧后行为。
- 实现方 e2e 6 条 + 独测（盲）11 条：单块特殊字符（引号/换行/中文/emoji）逐字节、
  三块乱序按 name 字节序、prefix+disable_builtin 并用互不干扰、`[]` 与不带字段
  字节相同、恢复回放字节一致 + 恢复后 spawn 收/拒、两种 400 可判别（矩阵 +
  `session_has_history` 对照）、undo 一轮不动声明块。
- 实现方测试助手里发现一个既有模式的真坑：`wait_for_terminal` 不按 `frame.agent`
  过滤，有子 agent 的场景会在**子**的终态帧上提前返回——已在
  `http_capabilities_prefix_inherit_after_restart.rs` 里修正并注释。
- 验收：`cargo test -p agent-server` 93+134 全绿；workspace 2062 过 0 挂；
  `check-invariants --all` 退出码 0（无新增超限）；ts 一致性测试过。
