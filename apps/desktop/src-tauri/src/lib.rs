//! issue 036：Tauri 内嵌同一个 agent-server 库，前端代码一套不变——这个 crate
//! 唯一的活是把 [`server::start`]（起内嵌 server）、[`first_run`]（缺配置时的
//! 提示页）接到 Tauri 的生命周期钩子上：`setup()` 里决定主窗口第一次该导航去
//! 哪个 URL，退出时（`RunEvent::Exit`）优雅关掉所有会话（issue 原文「退出时
//! 优雅 close 会话（快照落盘）」）。
//!
//! 真正的装配逻辑（读配置/拼 `SessionTemplate`/挂静态托管/绑端口）都在
//! [`server`]；平台目录在 [`paths`]；找 `packages/web` 构建产物在
//! [`dist`]；缺配置的提示页在 [`first_run`]——这个文件只做粘合，不重复它们的
//! 职责。

mod dist;
mod first_run;
mod paths;
mod server;

use std::sync::Mutex;

use tauri::{Manager, RunEvent, Url};

/// `setup()` 起完服务器之后存进 `app.manage`，退出钩子里取出来关——`Mutex` 只是
/// 因为 Tauri 的托管状态要求 `Sync`，`take()` 一次性拿走，不存在并发访问
/// （只有退出这一次读取）。
struct Shutdown(Mutex<Option<agent_server::SessionsHandle>>);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().setup(|app| {
        #[cfg(debug_assertions)]
        app.handle().plugin(tauri_plugin_log::Builder::default().level(log::LevelFilter::Info).build())?;

        let handle = app.handle().clone();
        let window = app.get_webview_window("main").expect("tauri.conf.json 只定义了 label=\"main\" 这一个窗口");

        match tauri::async_runtime::block_on(server::start(&handle)) {
            Ok(started) => {
                let url = Url::parse(&format!("http://{}", started.addr)).expect("SocketAddr 格式化出来的地址总是合法 URL");
                log::info!("内嵌 agent-server 监听 {}，导航主窗口过去", started.addr);
                window.navigate(url)?;
                app.manage(Shutdown(Mutex::new(Some(started.sessions))));
            }
            Err(err) => {
                log::error!("内嵌 agent-server 没能起来: {err}");
                if !err.is_missing_config() {
                    // 配置以外的失败（造目录、绑端口失败……）依然不 panic——
                    // 提示页文案照实转述这个 Display,用户看到的是「起不来，
                    // 原因是……」而不是一片空白窗口。
                    log::warn!("这不是「缺配置」——多半是宿主环境问题（磁盘/端口），提示页仍会显示，但措辞按缺配置写，先看日志。");
                }
                let config_dir = paths::config_dir(&handle)?;
                let providers_toml = paths::providers_toml_path(&handle)?;
                let page = first_run::write_page(&config_dir, &providers_toml, &err.to_string())?;
                window.navigate(page)?;
                app.manage(Shutdown(Mutex::new(None)));
            }
        }
        Ok(())
    });

    let app = builder.build(tauri::generate_context!()).expect("构建 tauri 应用失败");
    app.run(|app_handle, event| {
        // issue 036「退出时优雅 close 会话（快照落盘）」：`Exit` 是事件循环
        // 真正结束前的最后一站,`SessionsHandle::close_all` 是阻塞调用
        // （内部 `join` 每个 session 的 actor 线程),在这里等它跑完再让进程
        // 退出,不是提前放行。
        if !matches!(event, RunEvent::Exit) {
            return;
        }
        let Some(state) = app_handle.try_state::<Shutdown>() else { return };
        let sessions = state.0.lock().unwrap().take();
        let Some(sessions) = sessions else { return };
        for (id, outcome) in sessions.close_all() {
            if let Err(e) = outcome {
                log::warn!("退出时关闭会话 {id} 失败: {e:?}");
            }
        }
    });
}
