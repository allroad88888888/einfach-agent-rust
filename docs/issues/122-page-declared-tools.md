# 122 页面声明自己的工具表

**里程碑** M14 · **依赖** [121](121-js-tool-callback.md) · **模型** opus · **独测** 真机 + native 字节比对 · **状态** 待做

## 目标

工具表从「编译期写死的两条」变成「页面声明的一份」。有了通用 executor 而表还写死，
就是假通用。

## 现状

```rust
// agent-wasm/src/tools.rs:43
pub(crate) fn browser_tool_table() -> ToolTable {
    ToolTable::empty().with_host_tools(host_tools())   // 编译期固定的两条
}
```

M10 的 `declare_host_tools` 早在 core 里（`Slot::HostTools`，`graph/slot.rs:87`），
server 侧 `agent-server/src/http/capabilities/assemble.rs` 也有现成的组装。
**这条是接一条现成的线，不是造机制。**

## 做什么

1. `AgentHost` 收一份页面给的工具声明 JSON（构造函数收，还是单独一个方法收，
   由实现者定并在实做记录里写明理由——但**必须在第一次 `send()` 之前定死**，
   会话中途换表就是前缀缓存全断）。
2. 声明形状对齐 `capabilities/assemble.rs` 已有的那份（`name` / `description` /
   `schema` / `reversibility`），**不要另起一套**。
3. 两条内建 `web:page/*` 的处置：由实现者定——保留为「总是存在」，还是也改成
   页面可选声明。**在实做记录里写明理由**，并保证 [121](121-js-tool-callback.md)
   那条「内建优先于回调」的派发顺序不被弄坏。
4. `reversibility` 缺省落 `Irreversible`——HOST-CAPABILITIES §五：「没说」不能推定
   为「安全」。`tools.rs:48-49` 已经写过这条理由。

## 验收

- **红线 11 主证据（真机）**：页面声明同一份 JSON，刷新前后 `toolTableJson()`
  **逐字节相同**；且刷新后第一轮的实际请求体里工具段与关闭前最后一轮逐字节相同。
  这是 M13 验收第三条的同一条，只是表的来源换了。
- **native 可测的那一半**：把「声明 JSON → `ToolTable`」这一步做成纯函数并在
  `agent-runtime` 或 `agent-tools` 侧用单测钉住——同一份输入 JSON 转 1000 次，
  `specs()` 序列化结果逐字节相同；字段顺序被打乱的两份 JSON 转出来**相同**
  （`with_host_tools` 按名字排序，`ToolSpec` 的字段顺序由 Rust 类型定死）。
- 真机：页面声明一条自定义工具 + [121](121-js-tool-callback.md) 的回调，
  模型调得到、答得出。
- 真机反向锁：页面声明一条 `srv:` 前缀的工具 → **拒绝**，不是默默接受。
  `tools.rs:5-9` 那条「从空表起步，不靠名称黑名单回减」的纪律在这里第一次被外部输入考验。

## 注意

- ⚠️ **这条把红线 11 的责任推给了页面**（[119](119-browser-host-capability-decision.md) §七-3）。
  `with_host_tools` 只帮你按名字排序，帮不了字段顺序和描述文案——页面每次刷新
  给的 JSON 必须逐字节一样。**这条契约要写进 `tools.rs` 的模块文档和给页面的 API
  文档两处**，并且 `www/index.html` 里的示例必须是一个模块级常量，不是每次现拼的字面量。
- `tools.rs` 今天 93 行，加一个 JSON → `ToolTable` 的解析大概率顶破 300 行上限。
  **拆分是本次改动的一部分**（红线 9），不留「下次再拆」。建议切法：
  「表怎么装配」与「声明 JSON 怎么解析校验」是两件事。
- 声明里的 `description` 是模型看得见的文本。**不要在解析层做任何规范化**
  （trim、大小写、补标点）——那等于替页面改了进 prompt 的字节，而且不报错。
