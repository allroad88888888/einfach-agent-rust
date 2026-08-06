//! `SessionRegistry` 的打开协调：严格 `open` 与 HTTP 幂等 `open_or_get` 共用同一闸门。
//!
//! `actor::spawn` 不能在注册表锁内执行；`Slot::Opening` 先原子占位，再在锁外启动，
//! 防止两个 actor 写同一份持久化文件。严格调用方遇到占位立即失败；HTTP 的
//! get-or-create 调用方在条件变量上等待，启动成功便复用，失败则由一个等待方重试。

use std::collections::BTreeMap;

use agent_core::ExecutionProfileId;
use agent_runtime::ExecutionBinding;

use crate::actor::{self, OpenError};
use crate::handle::SessionHandle;

use super::{Entry, OpenOrGet, OpenOrGetError, OpenSpec, SessionId, SessionRegistry, Slot};

impl SessionRegistry {
    /// 严格打开：同一 id 已活着或正在打开时返回错误。
    pub fn open(&self, spec: OpenSpec) -> Result<SessionHandle, OpenError> {
        let id = spec.id.clone();
        {
            let mut sessions = self.sessions.lock().unwrap();
            match sessions.get(&id) {
                Some(Slot::Opening) => {
                    return Err(OpenError(format!(
                        "session \"{id}\" 正在被另一次 open() 调用起，等它完成或者失败再试"
                    )));
                }
                Some(Slot::Ready(existing)) if existing.died.lock().unwrap().is_none() => {
                    return Err(OpenError(format!(
                        "session \"{id}\" 已经 open 着，先 close 或者等它自己死了再重开"
                    )));
                }
                Some(Slot::Ready(_)) | None => {}
            }
            sessions.insert(id.clone(), Slot::Opening);
        }
        self.finish_open(id, spec, BTreeMap::new())
    }

    /// 幂等打开：先原子占住 id，再延迟构造 spec；等待者不会误读赢家刚写的历史文件。
    pub(crate) fn open_or_get_with<T, E>(
        &self,
        id: SessionId,
        execution_bindings: BTreeMap<ExecutionProfileId, ExecutionBinding>,
        build: impl FnOnce() -> Result<(OpenSpec, T), E>,
    ) -> Result<OpenOrGet<T>, OpenOrGetError<E>> {
        let mut build = Some(build);
        loop {
            let mut sessions = self.sessions.lock().unwrap();
            match sessions.get(&id) {
                Some(Slot::Opening) => {
                    sessions = self.opening_changed.wait(sessions).unwrap();
                    drop(sessions);
                }
                Some(Slot::Ready(existing)) if existing.died.lock().unwrap().is_none() => {
                    return Ok(OpenOrGet::Existing);
                }
                Some(Slot::Ready(_)) | None => {
                    sessions.insert(id.clone(), Slot::Opening);
                    drop(sessions);
                    let (spec, opened_value) = match build.take().unwrap()() {
                        Ok(built) => built,
                        Err(error) => {
                            self.release_opening(&id);
                            return Err(OpenOrGetError::Build(error));
                        }
                    };
                    return self
                        .finish_open(id, spec, execution_bindings)
                        .map(|_| OpenOrGet::Opened(opened_value))
                        .map_err(OpenOrGetError::Open);
                }
            }
        }
    }

    fn release_opening(&self, id: &SessionId) {
        self.sessions.lock().unwrap().remove(id);
        self.opening_changed.notify_all();
    }

    fn finish_open(
        &self,
        id: super::SessionId,
        spec: OpenSpec,
        execution_bindings: BTreeMap<ExecutionProfileId, ExecutionBinding>,
    ) -> Result<SessionHandle, OpenError> {
        let spawn_result = actor::spawn(spec, execution_bindings);
        let mut sessions = self.sessions.lock().unwrap();
        let result = match spawn_result {
            Ok(spawned) => {
                let handle = spawned.handle.clone();
                sessions.insert(
                    id,
                    Slot::Ready(Entry {
                        handle: spawned.handle,
                        join: spawned.join,
                        died: spawned.died,
                    }),
                );
                Ok(handle)
            }
            Err(error) => {
                sessions.remove(&id);
                Err(error)
            }
        };
        self.opening_changed.notify_all();
        result
    }
}
