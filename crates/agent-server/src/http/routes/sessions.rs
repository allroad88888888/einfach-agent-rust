//! 会话生命周期的两个端点（issue 031「会话创建」）：`POST /sessions` 取用一个
//! session（055 起是**幂等 getOrCreate**，见 [`create`]），`GET /sessions/:id`
//! 查它现在是活着还是死了。

use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use agent_core::AgentTree;

use crate::http::error::ApiError;
use crate::http::json::ApiJson;
use crate::http::state::AppState;
use crate::registry::{SessionId, SessionQuery};

const MAX_CLIENT_SESSION_ID_LEN: usize = 128;

#[derive(Deserialize)]
pub(in crate::http) struct CreateSessionRequest {
    /// 业务 chatid（055）。省略 → 服务端生成（031 以来的旧行为）。
    /// 部署契约见 [`create`] 文档「安全点二」。
    #[serde(default)]
    id: Option<String>,
    /// `Some` → 落盘（跟 CLI `--session <path>` 同款语义）；`None`/省略 → 内存
    /// 临时会话。
    #[serde(default)]
    session_path: Option<PathBuf>,
}

#[derive(Serialize)]
struct CreateSessionResponse {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<CreateSessionOutcome>,
}

/// 只有调用方指定业务 chatid 时才返回，避免改变旧 `{}` 请求的响应形状。
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum CreateSessionOutcome {
    Created,
    Existing,
    Recovered,
}

/// `POST /sessions`：**幂等 getOrCreate**（055，接缝见 `docs/INTEGRATION.md` §三）。
///
/// 请求体的 `id`（业务 chatid）省略时行为跟 031 以来一字不差：服务端
/// `generate_id()`、201、响应体只有 `{ "id": ... }`。给了 `id` 就按它当稳定
/// 会话身份，三态各自可判定：
///
/// | chatid 的状态 | 行为 | 状态码 | `outcome` |
/// |---|---|---|---|
/// | registry 里活着 | 直接接上，不新建、不清空历史 | 200 | `existing` |
/// | registry 没有、`<sessions-dir>/<chatid>.jsonl` 在 | 恢复 | 200 | `recovered` |
/// | 都没有 | 新建 | 201 | `created` |
///
/// 「查历史」没有新机制：磁盘上有那个文件就是有历史，恢复走的还是 kill -9
/// 重启那条既有路（`registry.open` → `agent_runtime::recover`）。这里只是
/// 抢在 `open` **之前**看一眼文件在不在，好把「新建」和「恢复」分别映射成
/// 201/200——`open` 之后再看就永远是「在」了。
///
/// # 安全点一：路径穿越只拒绝，不 sanitize
///
/// chatid 由客户端给、又会被拼进 `<sessions-dir>/<chatid>.jsonl`
/// （[`SessionTemplate::open_spec`](crate::http::config::SessionTemplate::open_spec)）
/// 和工具监狱目录名。校验（[`is_valid_client_session_id`]）就放在**收下这个 id
/// 的这一处**，不是每个拼路径的地方各防一遍。不合规一律 400，**绝不改写**：
/// 悄悄把 `a/b` 洗成 `a_b` 会让两个不同的 chatid 撞进同一个会话文件——静默
/// 串会话比拒绝更坏。拒绝发生在任何 `create_dir_all`/`open` 之前，因此坏 id
/// 在文件系统上留不下任何痕迹。
///
/// # 安全点二：chatid 即身份，归属由网关保证（部署契约）
///
/// server 无鉴权是 by design。但 chatid 一旦是会话身份，**猜到别人的 chatid
/// 就能接上别人的会话**——这条代码解决不了：上游网关必须保证 chatid 的归属
/// （`user → chatid` 授权，或让 chatid 含 uuid 这种不可猜的部分）。裸奔的
/// server + 可猜的 chatid = 越权读别人的对话。这里的 `id` 不是租户隔离边界，
/// 本 issue 也不做多租户鉴权（`X-Agent-Tenant-Id` 是未排期项）。
pub(in crate::http) async fn create(State(state): State<AppState>, ApiJson(body): ApiJson<CreateSessionRequest>) -> Result<Response, ApiError> {
    let CreateSessionRequest { id: requested_id, session_path } = body;
    let (id, client_supplied_id) = match requested_id {
        Some(id) => {
            if !is_valid_client_session_id(&id) {
                return Err(ApiError::bad_request("id 只能包含 ASCII 字母、数字、连字符和下划线，长度最多 128"));
            }
            (SessionId::from(id), true)
        }
        None => (state.generate_id(), false),
    };
    // `agent_runtime::recover` 会在 `registry.open` 之后读取该文件。先在这里
    // 判断是否已有文件，才可把「新建」和「恢复」准确地映射为 HTTP 201/200。
    let has_persisted_history = client_supplied_id
        && session_path
            .clone()
            .or_else(|| state.template().default_session_path(&id))
            .is_some_and(|path| path.is_file());
    let outcome = match state.registry().get(&id) {
        Some(SessionQuery::Alive(_)) => CreateSessionOutcome::Existing,
        Some(SessionQuery::Dead { .. }) | None => {
            let spec = state
                .template()
                .open_spec(id.clone(), session_path)
                .map_err(|e| ApiError::conflict(format!("session \"{id}\" 的工具根目录建不起来：{e}")))?;
            state.registry().open(spec).map_err(|e| ApiError::conflict(e.to_string()))?;
            if has_persisted_history { CreateSessionOutcome::Recovered } else { CreateSessionOutcome::Created }
        }
    };
    // 立刻把 SSE hub 造出来（不等第一次 `GET /events` 才现造）——不然「先
    // `POST /input` 好几轮，稍后才第一次连 SSE」这种顺序会在 hub 存在之前的
    // 这段时间里彻底丢事件（连「补不上」的 gap 都判不出来，因为环形缓冲那时
    // 还不存在）。`hub_for` 内部会先查 registry 确认活着——这里 `open` 刚成功
    // 返回，几乎不可能失败，真失败了也只是这次请求晚一点点再有人重试
    // `GET /events` 时现造，不影响 `POST /sessions` 本身的成功语义。
    let _ = state.hub_for(&id);
    let status = match outcome {
        CreateSessionOutcome::Created => StatusCode::CREATED,
        CreateSessionOutcome::Existing | CreateSessionOutcome::Recovered => StatusCode::OK,
    };
    let outcome = if client_supplied_id { Some(outcome) } else { None };
    Ok((status, Json(CreateSessionResponse { id: id.to_string(), outcome })).into_response())
}

/// 白名单：非空、`[A-Za-z0-9_-]`、≤128 字节。点号也不在表里，于是 `.`/`..`
/// 连同 `/`、`\`、NUL、非 ASCII 一起被同一条规则挡掉——不必再单列一串「危险
/// 序列」黑名单（黑名单漏一个就是漏一个）。
fn is_valid_client_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_CLIENT_SESSION_ID_LEN
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(in crate::http) enum SessionStatusResponse {
    Alive,
    Dead { reason: String },
}

pub(in crate::http) async fn status(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<SessionStatusResponse>, ApiError> {
    let id = SessionId::from(id);
    match state.registry().get(&id) {
        None => Err(ApiError::not_found(format!("session \"{id}\" 不存在（从没 open 过，或者已经被 close 摘表）"))),
        Some(SessionQuery::Alive(_)) => Ok(Json(SessionStatusResponse::Alive)),
        Some(SessionQuery::Dead { reason }) => Ok(Json(SessionStatusResponse::Dead { reason })),
    }
}

/// `GET /sessions/:id/agents`（048）：整棵活 agent 树此刻的快照——
/// [`crate::handle::SessionHandle::agent_tree`] 直接读共享单元格,**不走
/// actor 的 `mpsc` 命令队列**（048 issue 范围条款 4：一轮跑到一半也能立刻
/// 拿到当下的活树,不用排在 in-flight 的 `Command::Input` 后面）。开页/
/// reconnect 用它做种,之后靠 `GET /sessions/:id/events` 的 `agent_tree`
/// 帧增量更新（同一份 `Session::agent_tree()`,推和拉两条路给出同一棵树）。
///
/// 死会话报 410（跟 `input`/`undo`/`redo`/`cancel` 同一条判据,`state.
/// session_handle` 这一个函数——见该方法文档），不像 [`status`] 那样把
/// `dead` 当成 200 的一种正常结果：那是「问问这个 id 现在死没死」,这里
/// 问的是「给我看现在的活树」,树只在活着的 actor 手上才有意义。
pub(in crate::http) async fn agents(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<AgentTree>, ApiError> {
    let handle = state.session_handle(&SessionId::from(id))?;
    Ok(Json(handle.agent_tree()))
}
