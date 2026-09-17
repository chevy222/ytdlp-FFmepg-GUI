// 入口：初始化日志/配置后启动 Tauri。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    ytdlp_gui_lib::run();
}
