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

pub mod atom_value;
pub mod host_skills;
pub mod host_tools;
pub mod message;
pub mod session;
pub mod str_set;
pub mod tool;
