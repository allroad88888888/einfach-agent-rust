// 唯一职责：把模型点名的 `web:` 工具在这个浏览器里**执行掉并回传结果**——
// 整条注入链的最后一跳。065 只把「我有哪些能力」声明出去，模型真的调用时得
// 有人去跑，那就是这个文件。
//
// 三样东西都不在这里：画卡片是 `render/tool.ts` 的事（它收同一帧 `tool_executing`
// 但只管显示，两边互不知道对方存在）、HTTP 细节是 `api.ts` 的 `sendToolResult`、
// 「有哪些工具、怎么执行」是 `capabilities/` 的 `findWebTool`。**067 的 MCP
// 工具也是经 `registerWebTool` 进的同一张表**（`mcp/register.ts`），所以这里
// 一行 MCP 特判都没有——本地示例工具和 MCP 转发在这一层长得一模一样。
//
// # 三条判断
//
// 1. **只认 `request.location === "Web"`**。位置是 server 按名字推的
//    （`agent-runtime` 的 `tool_table::location_of`：`web:` 前缀 → `Location::Web`），
//    前端**不重新推一遍**——同一件事两处判据迟早会分叉。`Desktop` 不归浏览器
//    管（全仓也还没有它的执行路径，060 注），`Server` 压根不该走到这儿。
// 2. **每一次派发都必须有一次回传。** 找不到实现 → `is_error`；实现抛异常 →
//    `is_error`。**沉默是最坏的选项**：060 给远端等待补了截止线之后不回传不再
//    挂死会话，但要一直等到那条截止线（分钟级）才拿到 `is_error`，中间模型
//    什么也做不了；当场回传能让它立刻自纠（065 留的接线点原话）。
// 3. **超限自己截断。** server 侧 `content` 超 1 MiB 直接 400
//    （`routes/tool_result.rs` 的 `MAX_RESULT_BYTES`），那一下这次调用既没结果
//    也没回传，正好退化成第 2 条要避免的沉默。
//
// # 4. 帧只是触发器，服务端的等待槽才是判据（072）
//
// 没带 `Last-Event-ID` 的新连接（同一个 chatid 上的新 tab / 网关重启 / 网关背后
// 浏览器刷新）server 会把整个 ring 补发一遍（`http/hub/ring.rs` 的 `replay(None)`），
// 其中包含**早就回传过**的 `tool_executing`。进程内的集合挡不住它——新实例的集合
// 是空的。而「这帧是不是补发的」**根本不是正确的判据**：派了活、还没执行就换了
// 客户端，那帧确实是补发的、活却真的还欠着，按「补发就跳过」办 = 这活永远没人干
// （而且要等 060 的十分钟截止线才有症状，比重复执行更隐蔽）。
//
// 唯一权威的判据是「这次调用现在**是否还在服务端的等待槽里**」——那是服务端状态，
// 刷新掉不了。于是这一层剩下的规则只有两条，都在下面 `createToolExecutor` 里：
//
// - 收到 `tool_executing` 先向 `GET /pending_tools` 求证，命中才跑；
// - **每次连上**（首连 + 每次自动重连）拉一次那份投影，把欠着的活补执行掉——顺手
//   覆盖「帧根本到不了」的两种情况：ring 被挤爆（`Gap`，那帧永久没了）和断线期间
//   派下来的活。
//
// 残留一格（写进宿主契约，本层治不了）：**同一个 chatid 同时挂多个执行端**时，
// 两边对同一条**仍在等待**的调用求证都会说「还欠着」，于是各执行一次。真要
// exactly-once 得把「认领」做成服务端的状态变更，见 issue 072 §诚实的代价。
import type { AgentId, Frame, PendingTool, ToolCallId, ToolCallRequest } from "@agent/protocol";

import { fetchPendingTools, sendToolResult, type ToolResultBody } from "./api";
import { findWebTool } from "./capabilities";

/** 跟 `crates/agent-server/src/http/routes/tool_result.rs` 的 `MAX_RESULT_BYTES`
 * 同一个数。**那边量的是 `String::len()`——UTF-8 字节**，不是 JS 字符串的
 * `.length`（UTF-16 码元）：一个汉字是 1 个 `.length`、3 个字节，按 `.length`
 * 判会漏判到三倍，照样撞 400。所以下面一律拿 `TextEncoder` 量字节。 */
const MAX_RESULT_BYTES = 1024 * 1024;

/** 截断说明写进 `content` 本身——模型只看得到 `content`，没有第二个通道告诉
 * 它「后面还有」。不说明的话它会把半截结果当成全部。 */
const TRUNCATION_NOTE = "\n\n[前端截断：完整结果超过 1 MiB 的回传上限，这里只带回了开头这一段。需要更多请缩小查询范围再调一次。]";

const encoder = new TextEncoder();

/** 一个执行器有两个入口，因为「该执行什么」有两条来路：帧推过来的，和连上之后
 * 主动去拉的。两者**必须共用同一份认领集合**——不然一条慢工具刚被帧触发、
 * `sweep` 就会在投影里又看见它（服务端那一刻确实还欠着），当场执行第二次。 */
export interface ToolExecutor {
  /** 喂一帧。形状故意跟 `render/dispatch.ts` 的 `createRenderer(sessionId)`
   * 一样——`main.ts` 把两个并排接到同一条 SSE 上，**渲染和执行是两件事**。 */
  (frame: Frame): void;
  /** 每次连上调一次（首连 + `EventSource` 每次自动重连）：拉一次待办投影，把还
   * 欠着的活补执行掉。**不依赖帧还在不在**，所以 ring 被挤爆（`Gap`）或者断线
   * 期间派的活也不会漏。 */
  sweep(): Promise<void>;
}

/** 起一个执行器。每次调用起一份独立的认领集合（不是模块级单例）：一个 session
 * 一份，跟 `createRenderer` 的 `StreamCursor` 同一个道理。
 *
 * **这份集合不是正确性边界**（072）——它只挡「同一个页面生命周期内同一条调用被
 * 触发两次」，真正判「这活还要不要干」的是服务端的等待槽。押在客户端存储上的
 * 修法（`localStorage` 之类）结构上就不成立：能中这条 bug 的宿主是一个开放集合
 * （浏览器、JVM 网关、明天第三种），正确性边界不能放在一个「每加一个集成方就要
 * 重新实现一遍、而且漏了不报错」的地方。 */
export function createToolExecutor(sessionId: string): ToolExecutor {
  // 一个 call_id 最多认领一次：认领即执行 + 回传。同一帧被投递两次
  // （`FrameWatermark` 对不带 id 的帧保守放行）、或者帧和 `sweep` 撞上同一条调用，
  // 都不该变成两次副作用。
  const claimed = new Set<ToolCallId>();

  function execute(frame: Frame): void {
    const { agent, event } = frame;
    if (event.type !== "tool_executing") return;
    if (event.data.request.location !== "Web") return;
    void verifyThenRun(sessionId, claimed, { agent, call_id: event.data.call_id, request: event.data.request });
  }

  async function sweep(): Promise<void> {
    for (const owed of await fetchPendingTools(sessionId)) {
      // 投影本身就是判据，这条路不用再求证一次。位置过滤跟收帧那条同一条判据：
      // `desk:` 不归浏览器管。
      if (owed.request.location !== "Web") continue;
      if (claimed.has(owed.call_id)) continue;
      claimed.add(owed.call_id);
      void runWebTool(sessionId, owed.agent, owed.call_id, owed.request);
    }
  }

  return Object.assign(execute, { sweep });
}

/** 帧退化成「去问一下」：先向服务端的待办投影求证，命中才真的执行。
 *
 * 认领（`claimed.add`）发生在第一个 `await` **之前**，所以同一个 tick 内投递
 * 两次同一帧不会变成两次求证。求证问不到（网络断了、会话没了）时**不执行**并把
 * 认领退回去：不知道就别做副作用，等下一次 `sweep` 重新问——重连本来就会调它。 */
async function verifyThenRun(sessionId: string, claimed: Set<ToolCallId>, owed: PendingTool): Promise<void> {
  if (claimed.has(owed.call_id)) return;
  claimed.add(owed.call_id);
  let stillOwed: boolean;
  try {
    const pending = await fetchPendingTools(sessionId);
    stillOwed = pending.some((entry) => entry.call_id === owed.call_id && entry.agent === owed.agent);
  } catch (error) {
    claimed.delete(owed.call_id);
    console.error(`[web-tool] 求证 ${owed.request.tool}（${owed.call_id}）是否还欠着失败：${describe(error)}——这次不执行，等下一次重连拉待办时补上`);
    return;
  }
  // 不在投影里 = 这次调用早就收场了（回传过 / 超时判失败过 / 被取消过）。那帧是
  // 补发的历史，不是派给我的活。
  if (!stillOwed) return;
  await runWebTool(sessionId, owed.agent, owed.call_id, owed.request);
}

/** 执行 + 回传。**不抛**：这是从 `execute` 那个同步回调里 `void` 掉的 promise，
 * 抛出去只会变成一条没人接的 unhandledrejection。 */
async function runWebTool(sessionId: string, agent: AgentId, callId: ToolCallId, request: ToolCallRequest): Promise<void> {
  const result = await produceResult(request);
  try {
    await sendToolResult(sessionId, agent, callId, result);
  } catch (error) {
    // 回传本身失败（网络断了、会话没了、body 被拒）——这次调用于是真的没人
    // 回答了,只能等 server 的远端截止线兜底。**不静默**：这条日志是排查
    // 「模型为什么卡了几分钟才拿到 is_error」的唯一线索。
    console.error(`[web-tool] ${request.tool}（${callId}）的结果回传失败：${describe(error)}——这次调用要等到服务端的远端截止线才会拿到 is_error`);
  }
}

/** 跑一次实现，把三种结局都翻成一个 `{ content, is_error }`。 */
async function produceResult(request: ToolCallRequest): Promise<ToolResultBody> {
  const impl = findWebTool(request.tool);
  if (impl === undefined) {
    // 声明与实现在 `registerWebTool` 那一刻是绑在一起进来的,所以走到这里
    // 基本只有一种可能：模型编了个本会话没声明过的 `web:` 名字。回一句
    // 它能据此自纠的话,别只回「not found」。
    console.warn(`[web-tool] 本前端没有实现 ${request.tool}——回传 is_error 让模型自纠（不能沉默：那样它要等到截止线）`);
    return { content: `本前端没有实现工具 ${request.tool}。这个会话可用的工具以模型收到的工具表为准，请换一个已声明的工具，或者换个办法完成这件事。`, is_error: true };
  }
  try {
    return { content: fitToLimit(await impl(request.input)), is_error: false };
  } catch (error) {
    // `WebToolImpl` 的约定：抛异常 = 这次调用失败（`capabilities/index.ts`）。
    // MCP 的 `isError: true` 已经在 `mcp/register.ts` 里对齐成同一个形状。
    console.warn(`[web-tool] ${request.tool} 执行失败：${describe(error)}`);
    return { content: fitToLimit(`工具 ${request.tool} 执行失败：${describe(error)}`), is_error: true };
  }
}

/** 按 **UTF-8 字节**裁到上限以内，末尾补一句说明。已经在上限内的原样返回
 * （不复制、不改一个字）。 */
export function fitToLimit(content: string): string {
  const bytes = encoder.encode(content);
  if (bytes.length <= MAX_RESULT_BYTES) return content;

  // 说明本身也占字节,得从预算里先扣掉——不扣的话「截断后的结果」正好又超限。
  let keep = Math.max(MAX_RESULT_BYTES - encoder.encode(TRUNCATION_NOTE).length, 0);
  // 退到字符边界：UTF-8 的续字节形如 `10xxxxxx`,而 `bytes[keep]` 是第一个
  // **不**保留的字节——它要是续字节,说明这一刀切在了某个多字节字符中间,往前
  // 退到那个字符的首字节。不退的话 `TextDecoder` 会把残缺序列换成 U+FFFD
  // （重新编码是 3 字节）,反而可能把结果顶回上限之上。
  while (keep > 0 && (bytes[keep] & 0xc0) === 0x80) keep -= 1;
  return new TextDecoder().decode(bytes.subarray(0, keep)) + TRUNCATION_NOTE;
}

function describe(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
