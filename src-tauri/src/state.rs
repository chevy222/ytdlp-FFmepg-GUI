//! 应用全局状态：路径、配置、历史列表、并发队列、取消注册表。

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use ytdlp_core::config::AppConfig;
use ytdlp_core::exec::ToolResolver;
use ytdlp_core::history::History;
use ytdlp_core::paths::Paths;
use ytdlp_core::worker::TaskQueue;

/// 应用全局状态（Tauri State）。
pub struct AppState {
    pub paths: Paths,
    pub history: Mutex<History>,
    pub config: Mutex<AppConfig>,
    pub queue: Mutex<TaskQueue>,
    /// 任务取消标志注册表（id -> flag）
    pub cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// 解析缓存（host|normalized-url -> 缓存时间；M1 内存级，落盘 P2）
    pub probe_cache: Mutex<HashMap<String, i64>>,
}

impl AppState {
    pub fn new() -> Self {
        let paths = Paths::from_exe();
        let _ = paths.ensure_dirs();
        let config = AppConfig::load(&paths.config_file()).unwrap_or_default();
        let history = History::load(&paths.history_file()).unwrap_or_default();
        Self {
            paths,
            history: Mutex::new(history),
            config: Mutex::new(config),
            queue: Mutex::new(TaskQueue::new(config.general.concurrency)),
            cancels: Mutex::new(HashMap::new()),
            probe_cache: Mutex::new(HashMap::new()),
        }
    }

    /// 当前工具解析器（按 config 依赖段构造）。
    pub fn resolver(&self) -> ToolResolver {
        let cfg = self.config.lock().unwrap();
        ToolResolver::from_config(&cfg.dependencies)
    }

    /// 注册取消标志并返回。
    pub fn register_cancel(&self, id: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.cancels
            .lock()
            .unwrap()
            .insert(id.to_string(), flag.clone());
        flag
    }

    pub fn cancel_flag(&self, id: &str) -> Option<Arc<AtomicBool>> {
        self.cancels.lock().unwrap().get(id).cloned()
    }

    /// 持久化历史（变更即原子写，写失败降级为内存态并告警）。
    pub fn persist(&self) {
        let history = self.history.lock().unwrap();
        if let Err(e) = history.save(&self.paths.history_file()) {
            eprintln!("history 持久化失败（保持内存态）：{}", e);
        }
    }
}
