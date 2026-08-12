# 156 server 全链：`capabilities.prefix` 协议 + 校验 + 落店 + 装配

**里程碑** M17 · **依赖** [154](154-host-prefix-slot.md) + [155](155-with-host-prefix.md) · **模型** sonnet · **独测** ✅ · **状态** 未开始

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
