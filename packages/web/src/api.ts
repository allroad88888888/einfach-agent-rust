// 唯一职责：跟 agent-server 的 HTTP 面说话（issue 031 的路由表）——七个
// `fetch` 调用 + 一个 SSE 地址拼接函数,不掺渲染逻辑。请求体的形状照抄
// `crates/agent-server/src/http/routes/*.rs` 里各自的 wire 形状（`InputRequest`
// `UndoRequest` 等），那些是 axum 端点自己的入参结构,不是 `packages/protocol`
// 导出的协议类型——031 的实做记录写得很清楚：`Command`/`SessionEvent` 是给
// SSE **下行**用的判别联合,HTTP **上行**请求体是路由自己定的另一套形状,两者
// 故意不是同一套（`undo.rs` 的 `UndoRequest` 甚至不是 `Command::Undo` 本身）。
// 这里手写这几个请求体字面量不违反决策 2——决策 2 管的是「不要重新发明
// SessionEvent/Command 那份判别联合」,不是禁止写任何请求体。
// 065：`Capabilities` 是这个入口出去的**唯一一个请求体类型**——上面那段说的
// 「HTTP 上行请求体是路由自己定的形状」对它依然成立,区别是 061 把它挂上了
// ts-rs 导出（`crates/agent-server/src/http/capabilities/`），所以它跟下行那些
// 一样是生成物,不该在前端手写镜像（决策 2；`packages/protocol/src/index.ts`
// 那段 061 的注释记的是同一件事）。
import type { AgentId, AgentTree, Capabilities, Granularity, PendingTool, PendingToolsResponse, ToolCallId } from "@agent/protocol";

const JSON_HEADERS = { "Content-Type": "application/json" };

async function postJson(path: string, body?: unknown): Promise<void> {
  const res = await fetch(path, {
    method: "POST",
    headers: body === undefined ? undefined : JSON_HEADERS,
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) throw new Error(await describeError(res));
}

/** 统一错误形状是 `{"error":{"code","message"}}`（issue 031）——尽量把它
 * 拆出来给人看,拿不到就退回裸的状态码,不管哪种都不静默吞掉。 */
async function describeError(res: Response): Promise<string> {
  const text = await res.text();
  try {
    const parsed = JSON.parse(text) as { error?: { code?: string; message?: string } };
    if (parsed.error) return `${parsed.error.code}: ${parsed.error.message}`;
  } catch {
    // 不是 JSON（比如 axum rejection 的纯文本）——落到下面的兜底。
  }
  return `HTTP ${res.status}${text ? `: ${text}` : ""}`;
}

/** `POST /sessions`。最小客户端不暴露 `session_path` 字段——固定内存会话
 * （见包内 README「持久化」一节，要接落盘照这里的形状加一个字段）。
 *
 * 065：`capabilities` 是**宿主能力注入**的唯一入口（`docs/HOST-CAPABILITIES.md`
 * §四）——前端在这一次、也只有这一次把「我有哪些工具/skill」报给 server（§三：
 * 建会话时一次性声明,不做运行时增删;§二：只对这个会话生效,不进全局表）。
 * 声明源是 `./capabilities`,`main.ts` 传 `webCapabilities()` 进来。
 *
 * **省略这个参数时请求体逐字节还是 `{}`**——「不声明也能用」那条老路原样保留,
 * 这个可选参数本身就是那个开关（065 验收「不带声明的旧路径仍可用」）。 */
export async function createSession(capabilities?: Capabilities): Promise<string> {
  const request = capabilities === undefined ? {} : { capabilities };
  const res = await fetch("/sessions", { method: "POST", headers: JSON_HEADERS, body: JSON.stringify(request) });
  if (!res.ok) throw new Error(await describeError(res));
  const body = (await res.json()) as { id: string };
  return body.id;
}

export function eventsUrl(id: string): string {
  return `/sessions/${encodeURIComponent(id)}/events`;
}

/** `GET /sessions/:id/agents`（048）：整棵活 agent 树此刻的快照——开页 /
 * reconnect 用它做种,之后靠 SSE 的 `agent_tree` 帧增量更新（issue 049 范围
 * 条款 3）。跟 `POST /sessions` 一样不走 `postJson`（这条是 `GET`）,直接
 * `fetch` + 复用 `describeError` 的统一错误形状。 */
export async function fetchAgentTree(id: string): Promise<AgentTree> {
  const res = await fetch(`/sessions/${encodeURIComponent(id)}/agents`);
  if (!res.ok) throw new Error(await describeError(res));
  return (await res.json()) as AgentTree;
}

/** `GET /sessions/:id/pending_tools`（072）：此刻**还欠着**回传的远端工具调用。
 *
 * 这是「这次调用要不要执行」的唯一权威判据。收到一帧 `tool_executing` 判不出来
 * ——同一个 chatid 上一个没有游标的新客户端（刷新 / 新 tab / 网关重启）拿到的是
 * 整个 ring 的重放，里面混着早就干完的活；而「是不是补发」也不是正确的判据，
 * 派了活还没干就换客户端时，那帧确实是补发的、活却真的还欠着。
 *
 * 返回的是**全部**远端等待（含 `desk:`），按位置过滤是调用方的事
 * （`./tool-exec` 只认 `Location === "Web"`，跟它处理帧时同一条判据）。 */
export async function fetchPendingTools(id: string): Promise<PendingTool[]> {
  const res = await fetch(`/sessions/${encodeURIComponent(id)}/pending_tools`);
  if (!res.ok) throw new Error(await describeError(res));
  return ((await res.json()) as PendingToolsResponse).pending;
}

export function sendInput(id: string, text: string): Promise<void> {
  return postJson(`/sessions/${encodeURIComponent(id)}/input`, { text });
}

export function sendUndo(id: string, granularity: Granularity, force: boolean): Promise<void> {
  return postJson(`/sessions/${encodeURIComponent(id)}/undo`, { granularity, force });
}

export function sendRedo(id: string): Promise<void> {
  return postJson(`/sessions/${encodeURIComponent(id)}/redo`);
}

export function sendCancel(id: string): Promise<void> {
  return postJson(`/sessions/${encodeURIComponent(id)}/cancel`);
}

/** 一次前端工具调用的结果。字段名是 wire 形状（`snake_case`），照抄
 * `crates/agent-server/src/http/routes/tool_result.rs` 的 `ToolResult`——
 * `is_error` 那边带 `#[serde(default)]`，但这边始终显式发，别让「没写 = false」
 * 变成一条要靠记忆维护的默认值。 */
export interface ToolResultBody {
  content: string;
  is_error: boolean;
}

/** `POST /sessions/:id/tool_result`（066）：把一次由 SSE 派发下来的远端工具
 * （`Location::Web`）的结果送回去，server 侧 `resolve_remote_tool` 让**这一轮**
 * 当场续上（202 Accepted，跟另外四个命令端点一样是 fire-and-forget）。
 *
 * **没有 epoch 参数，这不是遗漏**：epoch 由 server 侧的 `RunnerCtx` 保管，
 * 结果必须精确匹配仍在等待的 `(agent, call_id)`——客户端伪造不了，也就不该在
 * 这个签名里出现（`tool_result.rs` 模块文档写死了这条）。
 *
 * `content` 超过 1 MiB 会被那边 400 掉（`MAX_RESULT_BYTES`）——**截断是调用方
 * 的事**（`./tool-exec` 的 `fitToLimit`）：截多少、怎么在内容里说明，都是
 * 「给模型看的东西」，不是 HTTP 这一层该替它做的决定。 */
export function sendToolResult(id: string, agent: AgentId, toolCallId: ToolCallId, result: ToolResultBody): Promise<void> {
  return postJson(`/sessions/${encodeURIComponent(id)}/tool_result`, { agent, tool_call_id: toolCallId, result });
}
