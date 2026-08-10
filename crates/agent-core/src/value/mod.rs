//! 值类型：agent 状态图里 primitive atom 会装的东西。
//!
//! 按语义域拆分（红线 9，一个文件一件事）：原子图上流动的值本身、消息与内容块、
//! 工具描述与调用请求、会话配置与结果元数据。
//!
//! [`atom_value::AgentValue`] 是**唯一**真的进 atom 的类型（026）；别的模块要么
//! 定义它某几个变体的载荷（[`message`] / [`tool`] / [`session`]），要么是「某个
//! 槽位的值 ↔ [`atom_value::AgentValue::Json`]」的一处编解码（[`str_set`] 的有序
//! 字符串集、[`host_tools`] 与 [`host_skills`] 的宿主注入声明）——后者存在的理由都是红线 11：排序
//! 这一步不能在两个地方各写一遍，写漏一处就是那一处每轮全价且不报错。

//! [`send_plan`] 是第三类：它不是某个 `AgentValue` 变体的载荷，是「这一轮发什么」
//! 这件事本身的坐标（099）——完整历史入库永不压缩，压缩只改它（095 §2）。
//! [`send_plan_codec`] 把它接回第二类：`SendPlan` ↔ `AgentValue::Json` 的编解码
//! （100），跟 `str_set` / `host_tools` 同一层，只是分开一个文件——`send_plan.rs`
//! 自己的范围只管那个纯值，AgentValue 编码是另一件事。
//! [`summaries`] 是 `SendPlan` 那条线的另一半（107）：引用住 `SendPlan`，
//! **正文住 `Slot::Summaries`**（红线 5），这个文件是那个槽位的一处编解码。

pub mod atom_value;
pub mod host_skills;
pub mod host_tools;
pub mod message;
pub mod send_plan;
pub mod send_plan_codec;
pub mod session;
pub mod str_set;
pub mod summaries;
pub mod tool;
