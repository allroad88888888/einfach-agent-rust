# 147 五条既有截获迁移进注册表，dispatch 只剩查表

**里程碑** M16 · **依赖** [146](146-intercept-registry.md) · **模型** sonnet · **独测** — · **状态** 完成（2026-08-12）

## 目标

spawn / collect / status / skill-read 四条工具截获迁进 146 的注册表，
`dispatch.rs` 的手工 if 链删掉——**行为逐字节零变化**。这一步同时是对
146 机制的 dogfood：注册表若装不下最复杂的 spawn（要 subtree、要产
`Dispatched::Events`），说明签名设计漏了，宁可现在红。

## 现状

- 四条截获在 `dispatch.rs` 手工排队，`Effect::Compact` 的路由**不迁**
  （它不是工具调用，模型看不见它，保持原位）。
- 迁移的注册点：各工具自己的模块暴露「注册函数」，宿主装配（cli/server/wasm
  的 RunnerCtx builder）统一调——谁开哪档能力，注册哪几条，与 `with_*`
  的授权档位一一对应。

## 做什么

1. 四个 `intercept` 适配到 `InterceptFn` 签名（实现体不动，只包一层）。
2. 装配点按既有授权档位接线：开了 spawn 档才注册 spawn 截获，与
   `with_spawn` 同点同条件——**声明与执行路径同开同关**，不许出现
   「表里有 spec 但没注册截获」的半开状态（那是 unknown_tool 假阳性）。
3. dispatch 删 if 链，`ExecuteTool` 分支只剩：查注册表 → MCP 路 → 远端路 →
   executor 路。
4. `dispatch.rs` 模块文档改写（「截获点就在这里，但实现不在」那节的新形状）。

## 验收

- 全部既有测试零变化（spawn/collect/status/skill-read 的单测、集成、wire 级
  全绿，一个断言都不用改——改了就说明行为变了，回去查）。
- 半开状态的看门狗：只 `with_spawn` 不注册截获（或反过来）→ debug_assert
  或既有测试红（挑一条能兑现的写清楚）。
- `dispatch.rs` 行数下降，`rg "SPAWN_TOOL|COLLECT_TOOL|STATUS_TOOL|SKILL_READ" src/dispatch.rs`
  零命中。

## 注意

- 纯迁移，一行为变化都不许有——这条的价值就是「零变化」本身。
- 若 spawn 的 `Dispatched::Events` 或 collect 的槽绑定装不进 146 的签名，
  **回 146 改签名**再来，别在本条打补丁绕。

## 实做记录（2026-08-12）

- 注册点选 A：`RunnerCtx::new` 末尾按 `declares` 条件注册四条（`builtin_intercepts.rs`），
  三宿主零改动，声明与执行同开同关自动成立；看门狗双向 debug_assert。
- **双重记账陷阱的正解**：把 027 包装从通用 `dispatch()` 挪进 SessionToolFn 适配器
  ——通用路纯查表直调，四条老截获原样直转（它们各自管自己的事件），既有 179 条
  集成测试零断言改动全绿。`InterceptArgs.input` 调成 `&Arc<Value>`（Arc::clone 需要）。
- dispatch.rs 262→231 行，四个工具名常量在其中零命中。
