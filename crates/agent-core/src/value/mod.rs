//! 值类型：agent 状态图里 primitive atom 会装的东西。
//!
//! 四个子模块按语义域拆分（红线 9，一个文件一件事）：原子图上流动的值本身、
//! 消息与内容块、工具描述与调用请求、会话配置与结果元数据。
//!
//! [`atom_value::AgentValue`] 是**唯一**真的进 atom 的类型（026）；其余三个模块
//! 定义的是它某几个变体的载荷。

pub mod atom_value;
pub mod message;
pub mod session;
pub mod str_set;
pub mod tool;
