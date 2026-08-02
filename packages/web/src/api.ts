// 唯一职责：跟 agent-server 的 HTTP 面说话（issue 031 的路由表）——五个
// `fetch` 调用 + 一个 SSE 地址拼接函数,不掺渲染逻辑。请求体的形状照抄
// `crates/agent-server/src/http/routes/*.rs` 里各自的 wire 形状（`InputRequest`
// `UndoRequest` 等），那些是 axum 端点自己的入参结构,不是 `packages/protocol`
// 导出的协议类型——031 的实做记录写得很清楚：`Command`/`SessionEvent` 是给
// SSE **下行**用的判别联合,HTTP **上行**请求体是路由自己定的另一套形状,两者
// 故意不是同一套（`undo.rs` 的 `UndoRequest` 甚至不是 `Command::Undo` 本身）。
// 这里手写这几个请求体字面量不违反决策 2——决策 2 管的是「不要重新发明
// SessionEvent/Command 那份判别联合」,不是禁止写任何请求体。
import type { Granularity } from "@agent/protocol";

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
 * （见包内 README「持久化」一节，要接落盘照这里的形状加一个字段）。 */
export async function createSession(): Promise<string> {
  const res = await fetch("/sessions", { method: "POST", headers: JSON_HEADERS, body: JSON.stringify({}) });
  if (!res.ok) throw new Error(await describeError(res));
  const body = (await res.json()) as { id: string };
  return body.id;
}

export function eventsUrl(id: string): string {
  return `/sessions/${encodeURIComponent(id)}/events`;
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
