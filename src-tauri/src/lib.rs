//! Tauri 应用壳：空窗口可运行（M0）；核心逻辑在 ytdlp-core。
//! 目录约定：启动时确保 exe 同级 config/ temp/ tools/ logs/（§3.7）。

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // 确保绿色便携目录结构（exe 同级）
            let paths = ytdlp_core::paths::Paths::from_exe();
            if let Err(e) = paths.ensure_dirs() {
                eprintln!("创建运行时目录失败：{}", e);
            }
            // 加载配置（损坏时回退默认，后续接入 UI）
            match ytdlp_core::config::AppConfig::load(&paths.config_file()) {
                Ok(_) => {}
                Err(e) => eprintln!("config 加载告警：{}", e),
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Tauri 应用启动失败");
}
