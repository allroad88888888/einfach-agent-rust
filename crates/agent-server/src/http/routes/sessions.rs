//! `POST /sessions`：校验宿主声明并原子地 get-or-create 一个会话。

mod query;

pub(in crate::http) use query::{agents, status};

use std::path::PathBuf;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::http::capabilities::{self, Capabilities};
use crate::http::error::ApiError;
use crate::http::json::ApiJson;
use crate::http::state::AppState;
use crate::registry::{OpenOrGet, OpenOrGetError, SessionId};

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
    /// 宿主这一次要声明的 tool/skill（061，形状见
    /// [`crate::http::capabilities`]）。省略 → `None`，行为跟 061 之前**逐字节
    /// 一致**：既有调用方一个字都不用改。
    #[serde(default)]
    capabilities: Option<Capabilities>,
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
///
/// # 061/062：`capabilities` 先校验，再装进**这一个**会话
///
/// 请求体带 `capabilities` 时，在**任何文件系统副作用之前**过一遍
/// [`crate::http::capabilities::validate`]（工具名前缀/字符集、skill id、重名），
/// 不合规一律 400 且这一次的会话根本不会被 `open`——跟上面 chatid 那条同一处、
/// 同一套「白名单 + 拒绝，绝不 sanitize」。
///
/// 校验通过之后（062）翻成 `(ToolSpec, Reversibility)`
/// （[`crate::http::capabilities::host_tools`]）当参数交给
/// [`SessionTemplate::open_spec`](crate::http::config::SessionTemplate::open_spec)：
/// 它只落进这一次的 `OpenSpec`，最终进的是这个会话在自己 actor 线程里现造的那张
/// `ToolTable`。**全局的 `SessionTemplate` 一个字节不动**，别的 chatid 看不见
/// （docs/HOST-CAPABILITIES.md §二）。
///
/// `existing` 那一支（会话已经活在 registry 里，或刚由并发请求打开）压根不 `open_spec`，
/// 所以这次的声明被忽略——会话中途换工具表 = 前缀缓存那一刻全断（红线 11），而
/// 「运行时增删」HOST-CAPABILITIES §三 明确不做。
///
/// # 073：有历史的会话**不接受再声明**（用户 2026-08-04 拍板）
///
/// 注入的声明**是会话状态，不是部署配置**：它在建会话那一次被 journaled 地写进
/// 会话状态（`agent_core::Session::declare_host_tools` → `Slot::HostTools`），恢复时
/// 跟别的 primitive 一样**从日志回放自动回来**，宿主**不必也不该**在重连时再声明
/// 一遍——历史对话是在**那一份**工具表下产生的，用今天的新清单重建就自相矛盾
/// （模型当初说「我调了 `web:crm/lookup`」，而今天的清单里可能没有它了），而且
/// 工具表在 prompt 最前面，换一份 = 恢复出来的第一轮前缀全断。
///
/// 所以「这个 chatid 在磁盘上已经有会话文件」+「这次又带了 `capabilities`」=
/// **400 `session_has_history`**，不忽略、不比对、不合并：
///
/// - **忽略**会制造本仓最讨厌的那种 bug——前端以为登记上了、其实没有，没有任何
///   报错，症状是「模型死活不用某个工具」，离现场十万八千里；
/// - **不一致才报错**要先定义「一致」（逐字节？名字集合？描述算不算？），每一种
///   定义都有人踩到边界，而且它默认了「一致时可以重复声明」，等于给「恢复时重新
///   注入」留了个后门。
///
/// **客户端契约（先查再建）**：`GET /sessions/{id}` → 404 就带声明建、200
/// （`alive`/`dormant`/`dead`）就不带。[`status`] 因此认识 `dormant`——不然「磁盘
/// 上有历史但此刻没活着」这个**恰恰就是恢复**的情况会被答成 404，契约当场作废。
/// 完整说明见 `docs/INTEGRATION.md` §chatid 与 `docs/issues/065-frontend-inject.md`。
///
/// # 064：skill 那一半走完全相同的路
///
/// `capabilities.skills` 经 [`crate::http::capabilities::host_skills`] 翻成
/// `Vec<HostSkill>`，跟工具一样当参数交给 `open_spec` → `OpenSpec` → 这个会话在自己
/// actor 线程里现造的 `SkillRegistry`。上面那条 073 的闸对它一视同仁：`capabilities`
/// 是整体判断的，带 skill 的声明撞上有历史的会话同样 **400 `session_has_history`**。
///
/// **server 不从磁盘 `./skills/` 装载**（069 §拍板「顺带定死 064 第 3 条」）：宿主
/// 已经有声明入口，两个来源合流会造出「同一份请求在不同部署上行为不同」的面；而且
/// 073 之后宿主声明是**会话状态**（恢复时逐字节复刻），磁盘上那份不是——部署者改一下
/// `./skills/` 就能悄悄改写一段历史对话该长什么样。
///
/// # 076：`capabilities.disable_builtin` 是同一条路上的**减法**
///
/// 前两样是「宿主往这个会话里加什么」，这一样是「这个会话不启用部署方给的哪几件
/// 内置工具」——列出来的**连名字带描述都不进 prompt**，模型压根不知道有它。
///
/// **只能减不能加**：名字必须在 `template().tools` 这一档装配出来的表里，不认识的
/// 一律 **400 且点名**（`capabilities::check_builtin_switch`）。反过来（客户端说
/// 「给我开 `srv:shell/exec`」）意味着前端一句 JSON 就能突破部署方的决定，而
/// `capabilities` 这条路上的客户端是浏览器。
///
/// **为什么必须报错而不是静默忽略**：拼错一个名字被忽略 → 客户端以为关掉了、其实
/// 没关 → 模型照样调得到 `srv:shell/exec`，**没有任何报错**。这一刻客户端还在线、
/// 能改，所以该在这里失败（对比 064 的 `skill_injection` 过滤：每轮都跑、作者早不
/// 在场，那里绝不能报错）。
///
/// 上面 073 那条闸对它同样一视同仁：`capabilities` 是整体判断的，只带
/// `disable_builtin` 的请求撞上有历史的会话照样 **400 `session_has_history`**——
/// 开关跟声明一样是会话状态，那段历史就是在**那一份减过的表**下产生的。
pub(in crate::http) async fn create(
    State(state): State<AppState>,
    ApiJson(body): ApiJson<CreateSessionRequest>,
) -> Result<Response, ApiError> {
    let CreateSessionRequest {
        id: requested_id,
        session_path,
        capabilities,
    } = body;
    let (id, client_supplied_id) = match requested_id {
        Some(id) => {
            if !is_valid_client_session_id(&id) {
                return Err(ApiError::bad_request(
                    "id 只能包含 ASCII 字母、数字、连字符和下划线，长度最多 128",
                ));
            }
            (SessionId::from(id), true)
        }
        None => (state.generate_id(), false),
    };
    let outcome = match state
        .registry()
        .open_or_get_with(
            id.clone(),
            state.execution_bindings(),
            || {
            // 只有原子占住 chatid 的赢家检查历史。并发输家等赢家启动完直接复用，
            // 不能把赢家刚创建的 jsonl 误判成一次“带声明恢复历史”的新请求。
            let store_path = session_path
                .clone()
                .or_else(|| state.template().default_session_path(&id));
            let has_persisted_history = client_supplied_id
                && store_path.as_ref().is_some_and(|path| path.is_file());
            if has_persisted_history && capabilities.is_some() {
                return Err(ApiError::session_has_history(format!(
                    "session \"{id}\" 已经有历史了：它的能力从历史来（建会话那一次已经写进会话状态），这次请求不要再带 capabilities。\
                     判断办法：先 GET /sessions/{id}——404 才是新会话、才带声明；200（alive/dormant/dead）一律不带"
                )));
            }
            // 只有真正创建者校验并翻译声明；并发等待者和既有会话都不会中途换表。
            if let Some(declared) = &capabilities {
                capabilities::validate(declared)
                    .map_err(|rejection| ApiError::bad_request(rejection.to_string()))?;
                capabilities::check_builtin_switch(declared, state.template().tools)
                    .map_err(|rejection| ApiError::bad_request(rejection.to_string()))?;
            }
            let host_tools = capabilities::host_tools(capabilities.as_ref());
            let host_skills = capabilities::host_skills(capabilities.as_ref());
            let disable_builtin = capabilities::disabled_builtins(capabilities.as_ref());
            let spec = state
                .template()
                .open_spec(
                    id.clone(),
                    session_path,
                    host_tools,
                    host_skills,
                    disable_builtin,
                )
                .map_err(|error| {
                    ApiError::conflict(format!(
                        "session \"{id}\" 的工具根目录建不起来：{error}"
                    ))
                })?;
            Ok((spec, has_persisted_history))
            },
        )
        .map_err(|error| match error {
            OpenOrGetError::Build(error) => error,
            OpenOrGetError::Open(error) => ApiError::conflict(error.to_string()),
        })?
    {
        OpenOrGet::Existing => CreateSessionOutcome::Existing,
        OpenOrGet::Opened(true) => CreateSessionOutcome::Recovered,
        OpenOrGet::Opened(false) => CreateSessionOutcome::Created,
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
    let outcome = if client_supplied_id {
        Some(outcome)
    } else {
        None
    };
    Ok((
        status,
        Json(CreateSessionResponse {
            id: id.to_string(),
            outcome,
        }),
    )
        .into_response())
}

/// 白名单：非空、`[A-Za-z0-9_-]`、≤128 字节。点号也不在表里，于是 `.`/`..`
/// 连同 `/`、`\`、NUL、非 ASCII 一起被同一条规则挡掉——不必再单列一串「危险
/// 序列」黑名单（黑名单漏一个就是漏一个）。
fn is_valid_client_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_CLIENT_SESSION_ID_LEN
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
