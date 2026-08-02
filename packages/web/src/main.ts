// 唯一职责：启动装配——建会话、开 SSE、把输入框/按钮接到 api.ts 的五个
// 请求上。不含渲染逻辑（render/）、不含协议解析（connection.ts）,纯接线,
// 跟 `crates/agent-cli/src/main.rs` 的自我定位（「纯接线」）是同一个判据。
import { cancelBtn, composerEl, inputEl, redoBtn, statusEl, undoBtn } from "./dom";
import { createSession, sendCancel, sendInput, sendRedo, sendUndo } from "./api";
import { connect, type ConnectionState } from "./connection";
import { createRenderer } from "./render/dispatch";

async function main(): Promise<void> {
  statusEl.textContent = "创建会话…";
  const sessionId = await createSession();

  const render = createRenderer(sessionId);
  connect(sessionId, render, (state) => updateStatus(sessionId, state));

  composerEl.addEventListener("submit", (event) => {
    event.preventDefault();
    const text = inputEl.value.trim();
    if (!text) return;
    inputEl.value = "";
    void sendInput(sessionId, text).catch(reportError);
  });

  // Undo/Redo 按钮只发默认档（turn 粒度、非 force）——`undo_blocked` 的确认
  // 弹层由收到的 `SessionEvent::Undo` 帧触发（`render/undo.ts`），不是按钮
  // 点击本身：031 的四个命令端点都是 fire-and-forget（202,不等结果),按钮
  // 点击这一刻还不知道会不会撞屏障。
  undoBtn.addEventListener("click", () => void sendUndo(sessionId, "turn", false).catch(reportError));
  redoBtn.addEventListener("click", () => void sendRedo(sessionId).catch(reportError));
  cancelBtn.addEventListener("click", () => void sendCancel(sessionId).catch(reportError));
}

function updateStatus(sessionId: string, state: ConnectionState): void {
  const label = state === "open" ? "已连接" : state === "error" ? "断线，自动重连中…" : "连接中…";
  statusEl.textContent = `session ${sessionId} · ${label}`;
}

function reportError(e: unknown): void {
  statusEl.textContent = `出错：${e instanceof Error ? e.message : String(e)}`;
}

void main();
