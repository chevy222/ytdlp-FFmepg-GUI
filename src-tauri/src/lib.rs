//! Tauri 应用壳：全局状态 + 命令注册 + 目录约定（§3.7）。
//! 核心逻辑在 ytdlp-core；本层负责界面桥接与外部进程任务调度。

mod commands;
mod login;
#[cfg(windows)]
mod login_win;
mod state;

use tauri::Manager;
use ytdlp_core::model::Status;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(state::AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::add_url,
            commands::add_local,
            commands::list_items,
            commands::get_item,
            commands::start_download,
            commands::cancel_item,
            commands::remove_item,
            commands::clear_done,
            commands::retry_item,
            commands::relogin_item,
            commands::get_config,
            commands::save_config,
            commands::list_cookies,
            commands::save_cookies,
            commands::delete_cookie,
            commands::probe_dependencies,
            commands::open_item_dir,
            commands::clear_temp,
            commands::rot_item,
        ])
        .setup(|app| {
            // 确保绿色便携目录结构（exe 同级）
            let state = app.state::<state::AppState>();
            if let Err(e) = state.paths.ensure_dirs() {
                eprintln!("创建运行时目录失败：{}", e);
            }
            // 启动时恢复队列（UL-10）：中断的任务标记失败，可重试
            {
                let mut hist = state.history.lock().unwrap();
                for item in hist.items.iter_mut() {
                    if !item.status.is_terminal() && item.status != Status::Ready {
                        item.status = Status::Failed;
                        item.error = Some("应用重启，任务中断".into());
                        item.push_log("应用重启，任务中断（可重试）".to_string());
                    }
                }
            }
            state.persist();
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Tauri 应用启动失败");
}
