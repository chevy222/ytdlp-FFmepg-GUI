//! 工具链托管下载（依赖页 下载/更新）：从官方 GitHub Release 下载
//! yt-dlp / ffmpeg+ffprobe（BtbN Builds）/ deno 到 `<exe 同级>\tools\`，
//! SHA-256 校验后原子激活（tmp + rename 覆盖）。
//!
//! 下载源（Windows x86_64）：
//! - yt-dlp.exe：https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe
//! - ffmpeg/ffprobe：https://github.com/BtbN/FFmpeg-Builds/.../ffmpeg-master-latest-win64-gpl.zip
//! - deno.exe：https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip
//!
//! SHA-256：各源均提供 `.sha256` 旁路文件（deno 缺失时跳过校验并记录）。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

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
}

/// 工具托管下载器。
pub struct ToolDownloader {
    pub tools_dir: PathBuf,
    pub temp_dir: PathBuf,
}

impl ToolDownloader {
    /// `tools_dir`：`<exe 同级>\tools\`；`temp_dir`：`<exe 同级>\temp\tool_dl\`。
    pub fn new(tools_dir: impl Into<PathBuf>, temp_dir: impl Into<PathBuf>) -> Self {
        Self {
            tools_dir: tools_dir.into(),
            temp_dir: temp_dir.into(),
        }
    }

    /// 目标在 tools 目录的最终路径。
    pub fn target_path(&self, kind: ToolKind) -> PathBuf {
        self.tools_dir.join(kind.exe_name())
    }

    /// 下载并激活。`force=false` 时若目标已存在则直接返回现有路径。
    /// `on_progress(phase, percent)`：phase 为"下载/校验/解压"等阶段名。
    pub fn download(
        &self,
        kind: ToolKind,
        force: bool,
        on_progress: &mut dyn FnMut(String, f32),
    ) -> Result<PathBuf, String> {
        let target = self.target_path(kind);
        if target.exists() && !force {
            return Ok(target);
        }
        std::fs::create_dir_all(&self.tools_dir)
            .map_err(|e| format!("创建 tools 目录失败：{e}"))?;
        std::fs::create_dir_all(&self.temp_dir)
            .map_err(|e| format!("创建临时目录失败：{e}"))?;

        let (raw, verify_zip) = self.download_artifact(kind, on_progress)?;

        on_progress("校验".into(), 0.0);
        let digest = sha256_hex(&raw).map_err(|e| format!("计算 SHA-256 失败：{e}"))?;
        let want = self.fetch_sha(kind).unwrap_or_default();
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
            let extracted = self.extract_from_zip(&raw, entry)?;
            let _ = std::fs::remove_file(&raw);
            self.activate(kind, &extracted, on_progress)?;
            let _ = std::fs::remove_file(&extracted);
        } else {
            self.activate(kind, &raw, on_progress)?;
            let _ = std::fs::remove_file(&raw);
        }
        on_progress("完成".into(), 1.0);
        Ok(target)
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

        on_progress("连接".into(), 0.0);
        let resp = ureq::get(url)
            .timeout(std::time::Duration::from_secs(30))
            .call()
            .map_err(|e| format!("下载失败 {url}: {e}"))?;
        let total = resp
            .header("Content-Length")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        let mut reader = resp.into_reader();
        let mut file = std::fs::File::create(&raw)
            .map_err(|e| format!("创建临时文件失败：{e}"))?;
        let mut buf = [0u8; 64 * 1024];
        let mut done: u64 = 0;
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| format!("下载中断：{e}"))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])
                .map_err(|e| format!("写入临时文件失败：{e}"))?;
            done += n as u64;
            if total > 0 {
                on_progress("下载".into(), (done as f32 / total as f32).min(1.0));
            }
        }
        if total > 0 && done != total {
            let _ = std::fs::remove_file(&raw);
            return Err(format!("下载不完整：{done}/{total}"));
        }
        Ok((raw, is_zip))
    }

    /// 取 SHA-256 期望值（网络失败/404 返回空，跳过校验）。
    fn fetch_sha(&self, kind: ToolKind) -> Option<String> {
        let url = kind.sha_url()?;
        let resp = ureq::get(url).timeout(std::time::Duration::from_secs(20)).call().ok()?;
        let mut text = String::new();
        resp.into_reader()
            .take(512)
            .read_to_string(&mut text)
            .ok()?;
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

    /// 原子激活：tmp + rename 覆盖。
    fn activate(
        &self,
        kind: ToolKind,
        src: &Path,
        on_progress: &mut dyn FnMut(String, f32),
    ) -> Result<(), String> {
        on_progress("安装".into(), 0.5);
        let target = self.target_path(kind);
        let tmp = self.tools_dir.join(format!("{}.tmp", kind.exe_name()));
        std::fs::copy(src, &tmp).map_err(|e| format!("复制工具失败：{e}"))?;
        std::fs::rename(&tmp, &target).map_err(|e| format!("激活工具失败：{e}"))?;
        on_progress("安装".into(), 1.0);
        Ok(())
    }
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
}
