//! 目录与存储约定（§3.7）：所有产生文件均在 exe 同级，不写注册表、不依赖 %APPDATA%。

use std::path::{Path, PathBuf};

/// exe 同级目录约定（绿色便携，§3.7）。
pub const DIR_CONFIG: &str = "config";
pub const DIR_TEMP: &str = "temp";
pub const DIR_TOOLS: &str = "tools";
pub const DIR_COOKIES: &str = "cookies";
pub const DIR_CACHE: &str = "cache";

/// 路径解析器：以 exe 所在目录为根（测试中可替换为任意根）。
#[derive(Debug, Clone)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    /// 使用 exe 所在目录作为根。
    pub fn from_exe() -> Self {
        let exe = std::env::current_exe().expect("无法定位当前可执行文件");
        Self {
            root: exe
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
        }
    }

    /// 显式指定根（测试/开发用）。
    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_dir(&self) -> PathBuf {
        self.root.join(DIR_CONFIG)
    }

    pub fn temp_dir(&self) -> PathBuf {
        self.root.join(DIR_TEMP)
    }

    pub fn tools_dir(&self) -> PathBuf {
        self.root.join(DIR_TOOLS)
    }

    pub fn cookies_dir(&self) -> PathBuf {
        self.config_dir().join(DIR_COOKIES)
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.config_dir().join(DIR_CACHE)
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir().join("config.json")
    }

    pub fn history_file(&self) -> PathBuf {
        self.config_dir().join("history.json")
    }

    /// 创建全部运行时目录（幂等）。
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for d in [
            self.config_dir(),
            self.temp_dir(),
            self.tools_dir(),
            self.cookies_dir(),
            self.cache_dir(),
        ] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }

    /// 任务私有临时目录（temp/<task_id>/，任务结束清理，§3.7/UL-08）。
    pub fn task_temp_dir(&self, task_id: &str) -> PathBuf {
        self.temp_dir().join(task_id)
    }
}

/// JSON 原子写：先写临时文件再 rename，避免中途损坏（§3.7 规则）。
pub fn atomic_write_json<T: serde::Serialize>(path: &Path, value: &T) -> crate::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value)?;
    std::fs::write(&tmp, &bytes)?;
    // rename 原子替换；Windows 上目标存在时先尝试替换
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn paths_resolve_exe_relative_dirs() {
        let root = tempdir().unwrap();
        let p = Paths::from_root(root.path());
        assert_eq!(p.config_dir(), root.path().join("config"));
        assert_eq!(p.temp_dir(), root.path().join("temp"));
        assert_eq!(p.tools_dir(), root.path().join("tools"));
        assert_eq!(p.cookies_dir(), root.path().join("config/cookies"));
        assert_eq!(p.cache_dir(), root.path().join("config/cache"));
        assert_eq!(p.config_file(), root.path().join("config/config.json"));
        assert_eq!(p.history_file(), root.path().join("config/history.json"));
    }

    #[test]
    fn ensure_dirs_creates_all() {
        let root = tempdir().unwrap();
        let p = Paths::from_root(root.path());
        p.ensure_dirs().unwrap();
        for d in [DIR_CONFIG, DIR_TEMP, DIR_TOOLS] {
            assert!(root.path().join(d).is_dir(), "缺失目录 {}", d);
        }
        assert!(root.path().join("config/cookies").is_dir());
        assert!(root.path().join("config/cache").is_dir());
    }

    #[test]
    fn task_temp_dir_isolated_by_id() {
        let root = tempdir().unwrap();
        let p = Paths::from_root(root.path());
        assert_eq!(p.task_temp_dir("abc"), root.path().join("temp/abc"));
    }

    #[test]
    fn atomic_write_then_read_back() {
        let root = tempdir().unwrap();
        let p = Paths::from_root(root.path());
        p.ensure_dirs().unwrap();
        let f = p.config_file();
        atomic_write_json(&f, &serde_json::json!({"a": 1})).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["a"], 1);
        // 覆盖写
        atomic_write_json(&f, &serde_json::json!({"a": 2})).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&f).unwrap()).unwrap();
        assert_eq!(v["a"], 2);
    }
}
