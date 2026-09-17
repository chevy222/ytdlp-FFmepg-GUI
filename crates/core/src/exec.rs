//! 外部进程与工具链定位（§5.1/§3.6 依赖 / §4 安全性-路径参数化）。
//!
//! - 工具定位：config 指定路径（含 `<exe 同级>\tools\` 托管）优先，否则系统 PATH。
//! - 进程执行：Command 参数化（不拼接 shell，防注入）；取消时终止进程树（Windows taskkill /T）。
//! - 平台：Linux 上编译/测试，Windows 上生产运行；取消用条件编译。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};

use crate::config::DependenciesConfig;
use crate::CoreError;

/// 工具种类（§3.6 依赖：yt-dlp / ffmpeg / ffprobe / deno）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    YtDlp,
    Ffmpeg,
    Ffprobe,
    Deno,
}

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Self::YtDlp => "yt-dlp",
            Self::Ffmpeg => "ffmpeg",
            Self::Ffprobe => "ffprobe",
            Self::Deno => "deno",
        }
    }

    pub fn exe_name(self) -> &'static str {
        #[cfg(windows)]
        {
            match self {
                Self::YtDlp => "yt-dlp.exe",
                Self::Ffmpeg => "ffmpeg.exe",
                Self::Ffprobe => "ffprobe.exe",
                Self::Deno => "deno.exe",
            }
        }
        #[cfg(not(windows))]
        {
            self.name()
        }
    }
}

/// 工具解析器：指定路径（config 依赖段）优先，否则 PATH 探测。
#[derive(Debug, Clone, Default)]
pub struct ToolResolver {
    yt_dlp: Option<PathBuf>,
    ffmpeg: Option<PathBuf>,
    ffprobe: Option<PathBuf>,
    deno: Option<PathBuf>,
}

impl ToolResolver {
    /// 从配置构造；路径留空 = 走 PATH。
    pub fn from_config(cfg: &DependenciesConfig) -> Self {
        Self {
            yt_dlp: cfg.yt_dlp_path.as_ref().map(PathBuf::from),
            ffmpeg: cfg.ffmpeg_path.as_ref().map(PathBuf::from),
            ffprobe: cfg.ffprobe_path.as_ref().map(PathBuf::from),
            deno: cfg.deno_path.as_ref().map(PathBuf::from),
        }
    }

    /// 显式指定工具根目录（tools/ 托管模式，exe 同级）。
    pub fn with_tools_dir(mut self, tools_dir: impl Into<PathBuf>) -> Self {
        let dir = tools_dir.into();
        self.yt_dlp = self.yt_dlp.or(Some(dir.join(Tool::YtDlp.exe_name())));
        self.ffmpeg = self.ffmpeg.or(Some(dir.join(Tool::Ffmpeg.exe_name())));
        self.ffprobe = self.ffprobe.or(Some(dir.join(Tool::Ffprobe.exe_name())));
        self.deno = self.deno.or(Some(dir.join(Tool::Deno.exe_name())));
        self
    }

    /// 解析工具可执行文件路径（存在校验；未配置且 PATH 无 → Err）。
    pub fn resolve(&self, tool: Tool) -> crate::Result<PathBuf> {
        let configured = match tool {
            Tool::YtDlp => self.yt_dlp.clone(),
            Tool::Ffmpeg => self.ffmpeg.clone(),
            Tool::Ffprobe => self.ffprobe.clone(),
            Tool::Deno => self.deno.clone(),
        };
        if let Some(p) = configured {
            if p.is_file() {
                return Ok(p);
            }
            return Err(CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} 指定路径不存在：{}", tool.name(), p.display()),
            )));
        }
        find_in_path(tool.exe_name()).ok_or_else(|| {
            CoreError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("未找到 {}（请设置路径或加入系统 PATH）", tool.name()),
            ))
        })
    }

    /// 解析并生成命令（已设好程序路径）。
    pub fn command(&self, tool: Tool) -> crate::Result<Command> {
        Ok(Command::new(self.resolve(tool)?))
    }

    /// 全部工具是否可用（依赖自检，§3.8）。
    pub fn all_available(&self) -> bool {
        [Tool::YtDlp, Tool::Ffmpeg, Tool::Ffprobe]
            .iter()
            .all(|t| self.resolve(*t).is_ok())
    }
}

/// 在系统 PATH 中查找可执行文件。
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
        #[cfg(windows)]
        {
            // Windows 上补充 .exe/.cmd/.bat 扩展名
            for ext in ["exe", "cmd", "bat"] {
                let cand = dir.join(format!("{}.{}", name, ext));
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

/// 可取消的受控子进程：Drop 时若未退出则强制终止（防泄漏）。
pub struct ChildGuard {
    child: Option<Child>,
    /// 已取消（外部标志；取消时终止进程树）
    cancelled: bool,
}

impl ChildGuard {
    pub fn spawn(cmd: &mut Command) -> crate::Result<Self> {
        let child = cmd.spawn().map_err(|e| {
            CoreError::Io(std::io::Error::new(
                e.kind(),
                format!("启动进程失败：{}", e),
            ))
        })?;
        Ok(Self {
            child: Some(child),
            cancelled: false,
        })
    }

    /// 终止进程树（Windows taskkill /T /F；其他平台 kill 主进程）。
    pub fn kill_tree(&mut self) {
        self.cancelled = true;
        if let Some(child) = self.child.as_mut() {
            kill_tree_of(child);
        }
    }

    /// 等待退出并返回完整输出（已取消时返回 Err(Canceled)）。
    pub fn wait_with_output(mut self) -> crate::Result<Output> {
        let mut child = self
            .child
            .take()
            .ok_or_else(|| CoreError::Io(std::io::Error::other("子进程句柄已丢失")))?;
        if self.cancelled {
            let _ = child.kill();
        }
        let out = child.wait_with_output().map_err(CoreError::Io)?;
        if self.cancelled {
            return Err(CoreError::Cancelled);
        }
        Ok(out)
    }

    /// 获取子进程 id（供 taskkill 使用）。
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(|c| c.id())
    }

    /// 取 stdout（调用后由调用方接管）。
    pub fn stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.child.as_mut().and_then(|c| c.stdout.take())
    }

    /// 取 stderr（调用后由调用方接管）。
    pub fn stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.child.as_mut().and_then(|c| c.stderr.take())
    }

    /// 非阻塞检查是否退出。
    pub fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        match self.child.as_mut() {
            Some(c) => c.try_wait(),
            None => Ok(None),
        }
    }

    /// 阻塞等待退出。
    pub fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        match self.child.as_mut() {
            Some(c) => c.wait(),
            None => Err(std::io::Error::other("子进程句柄已丢失")),
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            if let Ok(Some(_)) = child.try_wait() {
                // 已退出
            } else if self.cancelled {
                let _ = child.kill();
            }
        }
    }
}

/// 终止子进程树（Windows：taskkill /PID <pid> /T /F；其余：直接 kill）。
fn kill_tree_of(child: &mut Child) {
    #[cfg(windows)]
    {
        let pid = child.id();
        // taskkill 需先不 kill 掉主进程句柄，直接用 PID 命令
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = child.kill();
    }
}

/// 等待非零退出的子进程完成（输出缓冲，供版本查询等短命令）。
pub fn run_capture(mut cmd: Command) -> crate::Result<Output> {
    let out = cmd.output().map_err(CoreError::Io)?;
    if !out.status.success() {
        return Err(CoreError::ProcessFailed {
            program: cmd.get_program().to_string_lossy().into_owned(),
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(out)
}

/// 运行并捕获输出的便捷封装（工具版本查询等）。
pub fn run_tool_capture(
    resolver: &ToolResolver,
    tool: Tool,
    args: &[&str],
) -> crate::Result<Output> {
    let mut cmd = resolver.command(tool)?;
    cmd.args(args);
    run_capture(cmd)
}

/// 解析工具版本字符串（首行；ffmpeg/ffprobe 用 `-version`，其余用 `--version`）。
pub fn tool_version(resolver: &ToolResolver, tool: Tool) -> Option<String> {
    let arg = match tool {
        Tool::Ffmpeg | Tool::Ffprobe => "-version",
        _ => "--version",
    };
    let out = run_tool_capture(resolver, tool, &[arg]).ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let first = text.lines().next()?.trim().to_string();
    Some(first)
}

/// 校验指定路径可执行（依赖路径输入框失焦校验）。
pub fn validate_tool_path(path: &Path, _tool: Tool) -> crate::Result<()> {
    if !path.is_file() {
        return Err(CoreError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} 不存在", path.display()),
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn tool_names() {
        assert_eq!(Tool::YtDlp.name(), "yt-dlp");
        #[cfg(windows)]
        {
            assert_eq!(Tool::Ffmpeg.exe_name(), "ffmpeg.exe");
            assert_eq!(Tool::Ffprobe.exe_name(), "ffprobe.exe");
        }
        #[cfg(not(windows))]
        {
            assert_eq!(Tool::Ffmpeg.exe_name(), "ffmpeg");
            assert_eq!(Tool::Ffprobe.exe_name(), "ffprobe");
        }
        assert_eq!(Tool::Deno.name(), "deno");
    }

    #[test]
    fn default_resolver_uses_path() {
        let r = ToolResolver::default();
        // PATH 里一定有 sh（unix）或 system32（windows），resolve 不应 panic；
        // 具体工具可能缺失，故只验证"未配置时走 PATH 探测"这一行为不报配置错误。
        let _ = r.resolve(Tool::YtDlp);
    }

    #[test]
    fn configured_path_must_exist() {
        let root = tempdir().unwrap();
        let cfg = DependenciesConfig {
            ffmpeg_path: Some(
                root.path()
                    .join("nonexistent-ffmpeg")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ..Default::default()
        };
        let r = ToolResolver::from_config(&cfg);
        let err = r.resolve(Tool::Ffmpeg).unwrap_err();
        assert!(matches!(err, CoreError::Io(_)));
    }

    #[test]
    fn with_tools_dir_fills_managed_paths() {
        let root = tempdir().unwrap();
        let tools = root.path().join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        std::fs::write(tools.join(Tool::YtDlp.exe_name()), "x").unwrap();
        std::fs::write(tools.join(Tool::Deno.exe_name()), "x").unwrap();
        let r = ToolResolver::default().with_tools_dir(&tools);
        assert_eq!(
            r.resolve(Tool::YtDlp).unwrap(),
            tools.join(Tool::YtDlp.exe_name())
        );
        assert_eq!(
            r.resolve(Tool::Deno).unwrap(),
            tools.join(Tool::Deno.exe_name())
        );
    }

    #[test]
    fn configured_overrides_tools_dir() {
        let root = tempdir().unwrap();
        let custom = root.path().join("custom-ffmpeg");
        std::fs::write(&custom, "x").unwrap();
        let cfg = DependenciesConfig {
            ffmpeg_path: Some(custom.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let r = ToolResolver::from_config(&cfg).with_tools_dir(root.path());
        assert_eq!(r.resolve(Tool::Ffmpeg).unwrap(), custom);
    }

    #[test]
    fn tool_version_reads_first_line() {
        let r = ToolResolver::default();
        // 不假设工具存在：可用且版本输出可解析时返回非空首行
        if let Some(v) = tool_version(&r, Tool::Ffmpeg) {
            assert!(!v.is_empty());
        }
    }

    #[test]
    fn validate_tool_path_rejects_missing() {
        let root = tempdir().unwrap();
        assert!(validate_tool_path(&root.path().join("nope"), Tool::Ffmpeg).is_err());
    }

    #[test]
    fn run_capture_success_and_failure() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "exit 0"]);
        assert!(run_capture(cmd).is_ok());
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo boom >&2; exit 3"]);
        let err = run_capture(cmd).unwrap_err();
        match err {
            CoreError::ProcessFailed { code, stderr, .. } => {
                assert_eq!(code, Some(3));
                assert!(stderr.contains("boom"));
            }
            other => panic!("意外错误：{:?}", other),
        }
    }
}
