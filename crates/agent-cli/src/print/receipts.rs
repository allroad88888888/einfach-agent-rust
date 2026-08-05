//! 命令回执：`/model` `/undo` `/redo`、启动恢复、一轮收尾的一次性文案。
//!
//! 全是无状态的纯打印函数——**跟 [`super::events`] 的分界就是这个**：那边是一个
//! 跨整轮活着的状态机（上一段是思考还是正文、上一句是谁说的），这边每个函数
//! 各说一句就完事，互相之间没有顺序关系。
//!
//! 措辞不是装饰。027 的「注意」逐条落在这里：撞上不可逆操作要说清**哪一个**
//! 挡住了路，越过要说清越过的是**哪一个**，取消轮没能自动擦除要说清**为什么**
//! ——「诚实优于整洁」。

use agent_core::TurnStatus;

/// `/model <name>` 切成功了（014 缺口 1）：报新的 provider/model/endpoint，
/// **不打 key**——`model_switch::switch` 已经拿到了新 key，但这条确认行跟
/// 启动横幅那条（`main.rs`）同一个规矩，只报「配没配」不报内容。
pub fn model_switched(name: &str, model: &str, endpoint: &str) {
    println!("[已切换] provider={name} model={model} endpoint={endpoint}");
}

/// `/model <name>` 失败：未知名字、没有对应 adapter、或者没配 key。三种情况
/// 共用一行前缀，具体原因由调用方（`model_switch::switch`）拼好传进来。
pub fn model_switch_error(message: &str) {
    eprintln!("[切换失败] {message}");
}

/// 取消轮自动擦除成功（027 的正牌答案，取代 014/M1 时代「截断消息列表」那招）：
/// `undo_turn` 干净退掉了这一轮留下的全部痕迹。
pub fn cancelled_turn_erased(entries: usize, turn_id: u64) {
    println!("[已撤销] 取消的第 {turn_id} 轮留下的 {entries} 条痕迹已经擦除，没有计入历史");
}

/// 取消轮撞上屏障（这一轮已经执行过一个不可逆工具，比如 `shell/exec`）：
/// **保留该轮 + 打印说明**，不擅自越过用户没被问到的不可逆操作
/// （「诚实优于整洁」，027 已裁决）。
pub fn cancelled_turn_kept(entries: usize, what: &str) {
    println!(
        "[无法自动撤销] 取消的这一轮已经执行过一个不可逆操作：{what}，只退回了它之后的 {entries} 条。\
         这一轮的痕迹保留在历史里；需要连它也撤掉的话输入 /undo!"
    );
}

/// `/undo` 走完了：退了哪一轮、多少条目。
pub fn undo_applied(entries: usize, turn_id: u64) {
    println!("[已撤销] 第 {turn_id} 轮，{entries} 条");
}

/// `/undo` 无可做（游标已经在最底）。
pub fn undo_nothing() {
    println!("[没有可撤销的了]");
}

/// `/undo`（或 `/undo!` 撞上第二个屏障）停在一个不可逆操作门口。`forced`
/// 为真时说明这已经是 `/undo!` 越过第一条之后又撞上的下一条——用词要让人
/// 明白自己在确认什么：**哪一个**不可逆操作挡住了路。
pub fn undo_blocked(entries: usize, what: &str, forced: bool) {
    let prefix = if forced {
        "[仍有阻挡]"
    } else {
        "[撤销受阻]"
    };
    println!(
        "{prefix} 已经退了 {entries} 条，撞上了一个不可逆操作：{what}，undo 在这里停下不会自动越过。\
         确认要越过它（副作用不会跟着回滚）就输入 /undo!"
    );
}

/// `/undo!` 真的越过了一个屏障——明确说出越过的是哪一个，不是甩一句「已越过」
/// 让用户自己猜（措辞要求见 `docs/issues/027-cli-undo.md`「注意」）。
pub fn undo_force_crossed(what: &str) {
    println!("[已越过] {what}——它的副作用不会被回滚，只是不再挡住 undo 继续走。");
}

/// `/redo` 走完了。
pub fn redo_applied(entries: usize, turn_id: u64) {
    println!("[已重做] 第 {turn_id} 轮，{entries} 条");
}

/// `/redo` 无可做（游标已经在栈顶）。
pub fn redo_nothing() {
    println!("[没有可重做的了]");
}

/// 启动时从会话文件恢复成功（027）：报一句「接上了」+ 到第几轮，undo 栈的
/// 存在感在这里第一次向用户交代——不是新会话，是接着聊。
pub fn session_recovered(turn_id: u64) {
    println!(
        "[会话已恢复] 接着第 {turn_id} 轮继续（/undo 撤销、/redo 重做、/undo! 越过不可逆操作）"
    );
}

/// 恢复出来的会话里有一个工具调用「发出去了、还没等到结果」——上一个进程可能
/// 已经真的跑完了它，**不自动重发**（020 推迟的账，027 兑现）。
pub fn unresolved_tool_call_notice() {
    println!(
        "[可能已经执行过] 恢复出的会话里有一个工具调用还没收到结果——上一个进程可能已经执行过它，\
         这里不会自动重发。继续对话前请自行确认，需要的话可以 /undo 把这一轮撤掉。"
    );
}

/// 会话文件有内容，但翻译/重建失败（`RecoverError`：标签不认识，或者三元组
/// 违反 `History` 的不变量）——跟 011 的「尾部半行容忍、中部损坏拒绝」不是
/// 同一层：这是语义层面「读得出字节但拼不出一个自洽的会话」，明确报错，
/// 不硬凑一个能跑但是错的状态。
pub fn recovery_failed(err: &dyn std::fmt::Display) {
    eprintln!("[恢复失败] {err}——会话文件看起来存在但没法安全重建，拒绝启动。");
}

/// 一轮收尾：`run_turn` 的返回值是权威终态，这里换成一句人话。
pub fn turn_outcome(status: &TurnStatus) {
    match status {
        TurnStatus::Done { truncated: false } => println!("[本轮完成]"),
        TurnStatus::Done { truncated: true } => {
            println!("[本轮被截断：撞到了轮数/长度上限，模型本来还想继续]")
        }
        TurnStatus::Failed(failure) => println!("[本轮失败: {failure:?}]"),
        // `run_turn` 只在终态或者「转移表判了 ProtocolViolation 但没给出
        // 终态」两种情况下返回（agent-runtime::runner 模块文档）——后者打
        // 出来提醒这不是正常收尾，不是漏判。
        other => println!(
            "[本轮没有走到终态，卡在 {other:?}——上面应该已经有一条协议违规通报，可以 /quit 重开]"
        ),
    }
}
