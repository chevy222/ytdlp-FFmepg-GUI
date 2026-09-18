//! 工具链托管下载（依赖页 下载/更新）：从官方 GitHub Release 下载
//! yt-dlp / ffmpeg+ffprobe（BtbN Builds）/ deno，SHA-256 校验后原子激活
//! （tmp + rename 覆盖）。
//!
//! 安装位置由调用方给定：依赖页「下载」固定装到 `<exe 同级>\tools\`，
//! 「更新」装到该工具**当前生效**的那个文件（设置里填的路径或托管副本）。
//!
//! "有没有新版本"的判定：
//! - yt-dlp / deno 有版本号，直接比 release tag；
//! - ffmpeg / ffprobe 是 BtbN 的滚动构建（tag 恒为 `latest`），只能比
//!   "上次安装时的远端产物 SHA-256"（`tools/installed.json` 记录）。
//!
//! 下载源（Windows x86_64）：
//! - yt-dlp.exe：https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe
//! - ffmpeg/ffprobe：https://github.com/BtbN/FFmpeg-Builds/.../ffmpeg-master-latest-win64-gpl.zip
//! - deno.exe：https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip
//!
//! SHA-256：各源均提供 `.sha256` 旁路文件（deno 缺失时跳过校验并记录）。

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::exec::Tool;

/// 可托管的工具类型（依赖页四个入口）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    YtDlp,
    Ffmpeg,
    Ffprobe,
    Deno,
}

impl ToolKind {
    /// 依赖页 data-tool 键（dependencies.* 配置键名）。
    pub fn from_config_key(key: &str) -> Option<Self> {
        match key {
            "yt_dlp_path" => Some(Self::YtDlp),
            "ffmpeg_path" => Some(Self::Ffmpeg),
            "ffprobe_path" => Some(Self::Ffprobe),
            "deno_path" => Some(Self::Deno),
            _ => None,
        }
    }

    /// 激活后的文件名（Windows exe；非 Windows 环境无后缀）。
    pub fn exe_name(&self) -> &'static str {
        match self {
            Self::YtDlp => "yt-dlp.exe",
            Self::Ffmpeg => "ffmpeg.exe",
            Self::Ffprobe => "ffprobe.exe",
            Self::Deno => "deno.exe",
        }
    }

    /// 对应的 Tool（用于执行/校验）。
    pub fn tool(&self) -> Tool {
        match self {
            Self::YtDlp => Tool::YtDlp,
            Self::Ffmpeg => Tool::Ffmpeg,
            Self::Ffprobe => Tool::Ffprobe,
            Self::Deno => Tool::Deno,
        }
    }

    /// 下载 URL。
    pub fn url(&self) -> &'static str {
        match self {
            Self::YtDlp => {
                "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
            }
            Self::Ffmpeg | Self::Ffprobe => {
                "https://github.com/BtbN/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-win64-gpl.zip"
            }
            Self::Deno => {
                "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip"
            }
        }
    }

    /// SHA-256 校验文件 URL（可能不存在，404 时跳过校验）。
    pub fn sha_url(&self) -> Option<&'static str> {
        match self {
            Self::YtDlp => Some(
                "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe.sha256",
            ),
            Self::Ffmpeg | Self::Ffprobe => Some(
                "https://github.com/BtbN/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-win64-gpl.zip.sha256",
            ),
            Self::Deno => Some(
                "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip.sha256",
            ),
        }
    }

    /// zip 压缩包内的目标文件路径（zip 型工具）。
    pub fn zip_entry(&self) -> Option<&'static str> {
        match self {
            Self::Ffmpeg => Some("ffmpeg-master-latest-win64-gpl/bin/ffmpeg.exe"),
            Self::Ffprobe => Some("ffmpeg-master-latest-win64-gpl/bin/ffprobe.exe"),
            Self::Deno => Some("deno.exe"),
            _ => None,
        }
    }

    /// 发布页 `releases/latest` 地址（用于反查最新版本号）。
    /// ffmpeg/ffprobe 走 BtbN 的滚动构建（tag 恒为 `latest`），没有版本号可比 → None。
    pub fn latest_release_url(&self) -> Option<&'static str> {
        match self {
            Self::YtDlp => Some("https://github.com/yt-dlp/yt-dlp/releases/latest"),
            Self::Deno => Some("https://github.com/denoland/deno/releases/latest"),
            Self::Ffmpeg | Self::Ffprobe => None,
        }
    }
}

/// 一次安装的结果：落地路径 + 远端产物指纹（拿不到远端 `.sha256` 时为 None）。
#[derive(Debug, Clone)]
pub struct DownloadedTool {
    pub path: PathBuf,
    pub remote_sha256: Option<String>,
}

/// 单个工具的安装指纹：上一次由本程序安装到的位置 + 当时远端产物的 SHA-256。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledFingerprint {
    /// 安装目标（绝对路径）
    pub path: String,
    /// 远端产物 SHA-256：yt-dlp 是 exe 本身；ffmpeg / deno 是 zip 包
    pub sha256: String,
}

/// `tools/installed.json` 索引（key = 设置页的配置键名，如 `ffmpeg_path`）。
///
/// 只服务于「更新」的"有没有新版本"判定：文件丢失/损坏只会让下一次「更新」
/// 退化成"重新下载一次"，不影响安装，也不需要用户处理。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InstalledIndex {
    entries: std::collections::BTreeMap<String, InstalledFingerprint>,
}

impl InstalledIndex {
    /// 索引文件路径（`<exe 同级>\tools\installed.json`）。
    pub fn file_in(tools_dir: &Path) -> PathBuf {
        tools_dir.join(INSTALLED_INDEX_FILE)
    }

    /// 读取索引（不存在/损坏 → 空表）。
    pub fn load(tools_dir: &Path) -> Self {
        std::fs::read_to_string(Self::file_in(tools_dir))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn get(&self, key: &str) -> Option<&InstalledFingerprint> {
        self.entries.get(key)
    }

    /// 记录一次安装（`sha256` 为空表示远端没给可校验的指纹，此时不记录）。
    pub fn record(&mut self, key: &str, path: &Path, sha256: &str) {
        if sha256.is_empty() {
            return;
        }
        self.entries.insert(
            key.to_string(),
            InstalledFingerprint {
                path: path.to_string_lossy().into_owned(),
                sha256: sha256.to_ascii_lowercase(),
            },
        );
    }

    /// 原子写回索引。
    pub fn save(&self, tools_dir: &Path) -> Result<(), String> {
        let file = Self::file_in(tools_dir);
        crate::paths::atomic_write_json(&file, self)
            .map_err(|e| format!("写入 {} 失败：{e}", file.display()))
    }
}

/// 工具安装指纹索引文件名。
pub const INSTALLED_INDEX_FILE: &str = "installed.json";

/// 「远端没变过」判定：远端产物指纹与上次安装记录一致，且目标路径没变过。
///
/// 任一条件不满足（含拿不到远端指纹）都返回 false —— 宁可多下一次，
/// 也不要把"其实有新版本"误报成"已是最新"。
pub fn installed_matches(
    rec: Option<&InstalledFingerprint>,
    target: &Path,
    remote_sha: Option<&str>,
) -> bool {
    let (Some(rec), Some(remote)) = (rec, remote_sha) else {
        return false;
    };
    !rec.sha256.is_empty()
        && rec.path == target.to_string_lossy().as_ref()
        && rec.sha256.eq_ignore_ascii_case(remote)
}

/// 从 `curl -w %{url_effective}` 的结果里取版本号：`…/releases/tag/<tag>` → `<tag>`（去前缀 `v`）。
/// 滚动标签（`latest`）或不是版本形态的 tag 一律 None，避免拿它当版本比较。
pub fn parse_release_tag(effective_url: &str) -> Option<String> {
    let (_, tag) = effective_url.split_once("/tag/")?;
    let tag = tag.trim().trim_end_matches('/').trim_start_matches('v');
    if tag.is_empty() || !tag.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some(tag.to_string())
}

/// 工具托管下载器。
#[derive(Debug, Clone)]
pub struct ToolDownloader {
    pub tools_dir: PathBuf,
    pub temp_dir: PathBuf,
    /// 取消标志（下载中点"取消"置 true → kill curl）
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl ToolDownloader {
    /// `tools_dir`：`<exe 同级>\tools\`；`temp_dir`：`<exe 同级>\temp\tool_dl\`。
    pub fn new(tools_dir: impl Into<PathBuf>, temp_dir: impl Into<PathBuf>) -> Self {
        Self {
            tools_dir: tools_dir.into(),
            temp_dir: temp_dir.into(),
            cancel: None,
        }
    }

    /// 设置取消标志。
    pub fn with_cancel(
        mut self,
        cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> Self {
        self.cancel = cancel;
        self
    }

    /// 托管副本的目标路径（`tools\<工具名>`）——「下载」的固定落点。
    pub fn target_path(&self, kind: ToolKind) -> PathBuf {
        self.tools_dir.join(kind.exe_name())
    }

    /// 该工具是否已经有托管副本。
    pub fn is_installed(&self, kind: ToolKind) -> bool {
        self.target_path(kind).is_file()
    }

    /// 远端最新版本号（读 `releases/latest` 的最终跳转地址）。
    /// 不支持的源（ffmpeg/ffprobe 是滚动构建）或查询失败返回 None。
    pub fn latest_version(&self, kind: ToolKind) -> Option<String> {
        let url = kind.latest_release_url()?;
        std::fs::create_dir_all(&self.temp_dir).ok()?;
        // 文件名带工具名：同时点两个工具的"更新"时互不覆盖（进程 id 是同一个）
        let head = self
            .temp_dir
            .join(format!("head-{}-{}.txt", kind.exe_name(), std::process::id()));
        let mut cmd = std::process::Command::new("curl");
        cmd.args(["-sIL", "--fail", "-o"])
            .arg(&head)
            .args(["-w", "%{url_effective}"])
            .arg(url);
        crate::exec::hide_console(&mut cmd);
        let out = cmd.output().ok();
        let _ = std::fs::remove_file(&head);
        let out = out?;
        if !out.status.success() {
            return None;
        }
        parse_release_tag(&String::from_utf8_lossy(&out.stdout))
    }

    /// 远端产物 SHA-256（`.sha256` 旁路文件；404/网络失败返回 None）。
    pub fn remote_sha(&self, kind: ToolKind) -> Option<String> {
        let url = kind.sha_url()?;
        let mut cmd = std::process::Command::new("curl");
        cmd.args(["-L", "--fail", "-sS", url]);
        crate::exec::hide_console(&mut cmd);
        let output = cmd.output().ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        // 格式："<64hex>  filename" 或 裸 64hex
        let hex = text
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if hex.len() == 64 {
            Some(hex)
        } else {
            None
        }
    }

    /// 下载并安装到 `dest`（目标文件绝对路径：`tools\<工具名>` 或设置里填的位置）。
    /// `on_progress(phase, percent)`：phase 为"下载/校验/解压/安装"等阶段名。
    pub fn download(
        &self,
        kind: ToolKind,
        dest: &Path,
        on_progress: &mut dyn FnMut(String, f32),
    ) -> Result<DownloadedTool, String> {
        // 目标目录可能是托管 `tools\`，也可能是用户自填位置 → 提前建好并尽早报错
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建目标目录失败：{e}"))?;
        }
        std::fs::create_dir_all(&self.temp_dir)
            .map_err(|e| format!("创建临时目录失败：{e}"))?;

        let (raw, verify_zip) = self.download_artifact(kind, on_progress)?;

        on_progress("校验".into(), 0.0);
        let digest = sha256_hex(&raw).map_err(|e| format!("计算 SHA-256 失败：{e}"))?;
        let want = self.remote_sha(kind).unwrap_or_default();
        if !want.is_empty() && !digest.eq_ignore_ascii_case(&want) {
            let _ = std::fs::remove_file(&raw);
            return Err(format!(
                "SHA-256 校验失败：期望 {} 实际 {}",
                &want[..16.min(want.len())],
                &digest[..16]
            ));
        }
        on_progress("校验".into(), 1.0);

        // 解压或直取
        if verify_zip {
            let entry = kind
                .zip_entry()
                .ok_or_else(|| "内部错误：zip 型工具缺少目标条目".to_string())?;
            on_progress("解压".into(), 0.0);
            let extracted = self.extract_from_zip(&raw, entry)?;
            on_progress("解压".into(), 1.0);
            let _ = std::fs::remove_file(&raw);
            self.activate(&extracted, dest, on_progress)?;
            let _ = std::fs::remove_file(&extracted);
        } else {
            self.activate(&raw, dest, on_progress)?;
            let _ = std::fs::remove_file(&raw);
        }
        on_progress("完成".into(), 1.0);
        Ok(DownloadedTool {
            path: dest.to_path_buf(),
            remote_sha256: if want.is_empty() { None } else { Some(want) },
        })
    }

    /// 下载原始文件到 temp，返回 (路径, 是否为 zip)。
    fn download_artifact(
        &self,
        kind: ToolKind,
        on_progress: &mut dyn FnMut(String, f32),
    ) -> Result<(PathBuf, bool), String> {
        let is_zip = kind.zip_entry().is_some();
        let ext = if is_zip { "zip" } else { "bin" };
        let raw = self
            .temp_dir
            .join(format!("{}-{}.{}", kind.exe_name(), std::process::id(), ext));
        let url = kind.url();
        // 先问总大小：拿不到（CDN 不给 content-length）时进度退化为阶段提示，不影响下载
        let total = self.remote_size(url);

        on_progress("连接".into(), 0.0);
        // 用系统 curl（Windows 10+ 自带 curl.exe）下载，避免 TLS 库交叉编译问题
        let out_str = raw.to_string_lossy().into_owned();
        let mut cmd = std::process::Command::new("curl");
        cmd.args(["-L", "--fail", "-sS", "-o", &out_str, url]);
        // stderr 必须 pipe：否则 wait_with_output 拿不到 curl 的报错，
        // 下载失败时用户只能看到"下载失败 <url>"而无任何原因。
        // -sS 已静默进度，出错才输出，短文本不会撑爆管道缓冲。
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        crate::exec::hide_console(&mut cmd);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("无法调用 curl：{e}"))?;
        // curl -sS 自身不输出进度，这里按已写入的字节数估算百分比上报，
        // 否则整个下载过程前端只能停在 0%（大文件动辄几分钟）。
        on_progress("下载".into(), 0.0);
        let mut last_pct = 0.0f32;
        // 轮询等待 + 检查取消标志（点"取消"则 kill）
        loop {
            if let Some(flag) = &self.cancel {
                if flag.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = std::fs::remove_file(&raw);
                    return Err("已取消".to_string());
                }
            }
            if let Some(total) = total.filter(|t| *t > 0) {
                let done = std::fs::metadata(&raw).map(|m| m.len()).unwrap_or(0);
                let pct = (done as f64 / total as f64).min(1.0) as f32;
                // 限流：每前进 1% 才上报一次，避免 150ms 一条事件打爆前端
                if pct - last_pct >= 0.01 {
                    last_pct = pct;
                    on_progress("下载".into(), pct);
                }
            }
            match child
                .try_wait()
                .map_err(|e| format!("curl 等待失败：{e}"))?
            {
                Some(_) => break,
                None => std::thread::sleep(std::time::Duration::from_millis(150)),
            }
        }
        let output = child
            .wait_with_output()
            .map_err(|e| format!("curl 读取输出失败：{e}"))?;
        if !output.status.success() {
            let _ = std::fs::remove_file(&raw);
            let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if err.is_empty() {
                format!("下载失败 {url}")
            } else {
                format!("下载失败 {url}: {err}")
            });
        }
        let done = std::fs::metadata(&raw).map(|m| m.len()).unwrap_or(0);
        if done == 0 {
            let _ = std::fs::remove_file(&raw);
            return Err(format!("下载失败 {url}: 文件为空"));
        }
        on_progress("下载".into(), 1.0);
        Ok((raw, is_zip))
    }

    /// 查询远端文件总大小（HEAD 跟随重定向）。
    /// 取不到（CDN 不给 content-length / 网络异常）返回 None，调用方退化为阶段提示。
    fn remote_size(&self, url: &str) -> Option<u64> {
        let mut cmd = std::process::Command::new("curl");
        cmd.args(["-sIL", "--fail", url]);
        crate::exec::hide_console(&mut cmd);
        let out = cmd.output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        last_content_length(&text)
    }

    /// 从 zip 提取单个条目到 temp 目录。
    fn extract_from_zip(&self, zip_path: &Path, entry_name: &str) -> Result<PathBuf, String> {
        let file =
            std::fs::File::open(zip_path).map_err(|e| format!("打开压缩包失败：{e}"))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("解析压缩包失败：{e}"))?;
        let mut entry = archive
            .by_name(entry_name)
            .map_err(|_| format!("压缩包内未找到 {entry_name}"))?;
        let out = self
            .temp_dir
            .join(format!("extract-{}-{}", std::process::id(), std::path::Path::new(entry_name).file_name().and_then(|s| s.to_str()).unwrap_or("out")));
        let mut file = std::fs::File::create(&out).map_err(|e| format!("创建解压文件失败：{e}"))?;
        std::io::copy(&mut entry, &mut file).map_err(|e| format!("解压失败：{e}"))?;
        Ok(out)
    }

    /// 原子激活：写到目标同目录的 `<文件名>.tmp` 再 rename 覆盖
    /// （同目录才能保证 rename 是原子替换；用户自填目录不可写时在这里报错）。
    fn activate(
        &self,
        src: &Path,
        dest: &Path,
        on_progress: &mut dyn FnMut(String, f32),
    ) -> Result<(), String> {
        on_progress("安装".into(), 0.5);
        let dir = dest
            .parent()
            .ok_or_else(|| format!("无效的目标路径：{}", dest.display()))?;
        let name = dest
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| format!("无效的目标文件名：{}", dest.display()))?;
        let tmp = dir.join(format!("{name}.tmp"));
        std::fs::copy(src, &tmp)
            .map_err(|e| format!("写入 {} 失败（目标目录不可写？）：{e}", tmp.display()))?;
        if let Err(e) = std::fs::rename(&tmp, dest) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("激活工具失败：{e}"));
        }
        on_progress("安装".into(), 1.0);
        Ok(())
    }
}

/// 从 curl `-I` 输出里取最后一个 `content-length`。
/// 带 `-L` 时输出含每一次跳转的响应头，只有最终响应的大小是真实文件大小。
fn last_content_length(headers: &str) -> Option<u64> {
    headers
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            if k.trim().eq_ignore_ascii_case("content-length") {
                v.trim().parse::<u64>().ok()
            } else {
                None
            }
        })
        .next_back()
}

fn sha256_hex(path: &Path) -> Result<String, std::io::Error> {
    use sha2::Digest;
    use std::io::BufReader;
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = sha2::Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn config_key_roundtrip() {
        assert_eq!(ToolKind::from_config_key("yt_dlp_path"), Some(ToolKind::YtDlp));
        assert_eq!(ToolKind::from_config_key("deno_path"), Some(ToolKind::Deno));
        assert_eq!(ToolKind::from_config_key("nope"), None);
    }

    #[test]
    fn urls_and_entries() {
        assert!(ToolKind::YtDlp.url().ends_with("yt-dlp.exe"));
        assert!(ToolKind::Ffmpeg.url().contains("ffmpeg-master-latest-win64-gpl.zip"));
        assert_eq!(
            ToolKind::Ffmpeg.zip_entry(),
            Some("ffmpeg-master-latest-win64-gpl/bin/ffmpeg.exe")
        );
        assert_eq!(ToolKind::Deno.zip_entry(), Some("deno.exe"));
        assert_eq!(ToolKind::YtDlp.exe_name(), "yt-dlp.exe");
    }

    #[test]
    fn downloader_target_path() {
        let dl = ToolDownloader::new("/x/tools", "/x/temp");
        assert_eq!(dl.target_path(ToolKind::Deno), PathBuf::from("/x/tools/deno.exe"));
    }

    #[test]
    fn last_content_length_takes_final_hop() {
        // GitHub → S3 的两次跳转：302 只有 content-length: 0，最终响应才是文件大小
        let head = concat!(
            "HTTP/2 302\r\ncontent-length: 0\r\nlocation: https://example/x\r\n\r\n",
            "HTTP/2 200\r\nContent-Length: 172693744\r\n",
        );
        assert_eq!(last_content_length(head), Some(172693744));
        // 没有该头（分块传输 / HEAD 被拒）→ None，进度退化为阶段提示
        assert_eq!(last_content_length("HTTP/2 200\r\ntransfer-encoding: chunked\r\n"), None);
    }

    #[test]
    fn parse_release_tag_reads_version_only() {
        let yt_dlp = "https://github.com/yt-dlp/yt-dlp/releases/tag/2026.08.19\n";
        assert_eq!(parse_release_tag(yt_dlp).as_deref(), Some("2026.08.19"));
        // deno 的 tag 带 v 前缀
        let deno = "https://github.com/denoland/deno/releases/tag/v2.9.7";
        assert_eq!(parse_release_tag(deno).as_deref(), Some("2.9.7"));
        // BtbN 是滚动发布，tag 恒为 latest → 没有版本可比
        let btb = "https://github.com/BtbN/FFmpeg-Builds/releases/tag/latest";
        assert_eq!(parse_release_tag(btb), None);
        assert_eq!(parse_release_tag("https://github.com/yt-dlp/yt-dlp"), None);
    }

    #[test]
    fn installed_index_roundtrip() {
        let root = tempdir().unwrap();
        let tools = root.path().join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        let mut idx = InstalledIndex::load(&tools);
        assert!(idx.get("ffmpeg_path").is_none());

        let target = tools.join("ffmpeg.exe");
        idx.record("ffmpeg_path", &target, "ABCDEF01");
        // 空指纹（远端没给 .sha256）不记录
        idx.record("deno_path", &tools.join("deno.exe"), "");
        idx.save(&tools).unwrap();

        let back = InstalledIndex::load(&tools);
        let rec = back.get("ffmpeg_path").unwrap();
        assert_eq!(PathBuf::from(&rec.path), target);
        assert_eq!(rec.sha256, "abcdef01");
        assert!(back.get("deno_path").is_none());
        // 索引缺失/损坏时退化为空表（不影响安装，只是下次更新会重新比一次）
        std::fs::write(InstalledIndex::file_in(&tools), "{ not json").unwrap();
        assert!(InstalledIndex::load(&tools).get("ffmpeg_path").is_none());
    }

    #[test]
    fn installed_matches_requires_same_path_and_sha() {
        let target = PathBuf::from("/x/tools/ffmpeg.exe");
        let rec = InstalledFingerprint {
            path: "/x/tools/ffmpeg.exe".to_string(),
            sha256: "abc123".to_string(),
        };
        assert!(installed_matches(Some(&rec), &target, Some("abc123")));
        assert!(installed_matches(Some(&rec), &target, Some("ABC123")));
        // 远端有新构建 → 需要更新
        assert!(!installed_matches(Some(&rec), &target, Some("def456")));
        // 用户改过设置、目标换到别处 → 记录失效，需要更新
        assert!(!installed_matches(Some(&rec), Path::new("/y/ffmpeg.exe"), Some("abc123")));
        // 没有记录 / 拿不到远端指纹 → 一律按"需要更新"处理（宁可多下一次）
        assert!(!installed_matches(None, &target, Some("abc123")));
        assert!(!installed_matches(Some(&rec), &target, None));
    }
}
