//! 下载能力（§3.3 DL-01~DL-11 + DL-04 后处理 + §3.2 MD-06 产物解析）。
//!
//! - 参数构造（纯函数可测）：格式回落 / 排序串 / 画质上限 / MP4 统一 / 不覆盖 / 模板
//! - 进度解析（纯函数可测）：`[download] xx% of xx at xx/s ETA xx`
//! - 执行：yt-dlp 子进程（--newline 逐行回调），取消杀进程树 + 清理输出残留
//! - 后处理（M1 基础版）：超画质上限降分辨率（libx265，QSV 协商留 M2）+ 音量归一化

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::config::{DownloadConfig, GeneralConfig};
use crate::exec::{ChildGuard, Tool, ToolResolver};
use crate::model::MediaMeta;
use crate::probe;
use crate::{CoreError, Result};

/// 下载进度回调数据。
#[derive(Debug, Clone)]
pub struct Progress {
    pub percent: f32,
    pub speed: Option<String>,
    pub eta: Option<String>,
    pub file: Option<String>,
}

/// 下载参数（由 config 与用户选择组装）。
#[derive(Debug, Clone)]
pub struct DownloadParams {
    pub format_id: Option<String>,
    pub audio_only: bool,
    pub out_dir: PathBuf,
    pub filename_template: String,
    pub embed_cover: bool,
    pub proxy: Option<String>,
    pub cookies_file: Option<PathBuf>,
    pub sections: Option<(String, String)>,
}

/// 文件名模板 → yt-dlp 输出模板（DL-10）。
pub fn output_template(tmpl: &str, playlist: bool) -> &'static str {
    if playlist {
        return "%(playlist_title)s/%(playlist_index)s - %(title)s.%(ext)s";
    }
    match tmpl {
        "标题+ID" => "%(title)s [%(id)s].%(ext)s",
        "UP主-标题" => "%(uploader)s - %(title)s.%(ext)s",
        "日期-标题" => "%(upload_date)s %(title)s.%(ext)s",
        _ => "%(title)s.%(ext)s",
    }
}

/// 默认格式串（DL-03 七级回落 + 画质上限 MAX_H，短边）。
pub fn default_format(max_h: u32) -> String {
    format!("bv*[height<={}]+ba/b[height<={}]/bv*+ba/b", max_h, max_h)
}

/// 排序串（DL-03）。
pub const SORT_SPEC: &str = "vcodec:h264,lang,quality,res,fps,acodec:aac,size,proto,ext";

/// 构造 yt-dlp 下载参数（纯函数）。
pub fn build_args(url: &str, p: &DownloadParams, cfg: &DownloadConfig) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    // 格式
    let format = match &p.format_id {
        Some(fid) if !fid.is_empty() => {
            if p.audio_only {
                format!("{}/bestaudio/best", fid)
            } else {
                format!("{}+ba/b", fid)
            }
        }
        _ => {
            if p.audio_only {
                "bestaudio/best".to_string()
            } else {
                default_format(cfg.max_h)
            }
        }
    };
    args.push("--format".into());
    args.push(format);
    args.push("-S".into());
    args.push(SORT_SPEC.into());
    // 合并容器统一 MP4
    args.push("--merge-output-format".into());
    args.push("mp4".into());
    // 输出
    let tmpl = output_template(&p.filename_template, cfg.playlist);
    let out = p.out_dir.join(tmpl);
    args.push("-o".into());
    args.push(out.to_string_lossy().into_owned());
    // 不覆盖
    args.push("--no-overwrites".into());
    // 并发分片
    args.push("-N".into());
    args.push(cfg.fragments.to_string());
    // 重试
    args.push("--retries".into());
    args.push(cfg.retries.to_string());
    args.push("--retry-sleep".into());
    args.push("3".into());
    // 封面/元数据
    if cfg.embed_cover {
        args.push("--embed-thumbnail".into());
        args.push("--embed-metadata".into());
    }
    // 音频仅提取
    if p.audio_only {
        args.push("-x".into());
        args.push("--audio-format".into());
        args.push("mp3".into());
        args.push("--audio-quality".into());
        args.push("0".into());
    }
    // 播放列表
    if cfg.playlist {
        args.push("--yes-playlist".into());
    } else {
        args.push("--no-playlist".into());
    }
    // 代理
    if let Some(proxy) = &p.proxy {
        if !proxy.is_empty() {
            args.push("--proxy".into());
            args.push(proxy.clone());
        }
    }
    // Cookie
    if let Some(cf) = &p.cookies_file {
        args.push("--cookies".into());
        args.push(cf.to_string_lossy().into_owned());
    }
    // 时间范围（DL-12）
    if let Some((start, end)) = &p.sections {
        args.push("--download-sections".into());
        args.push(format!("*{}-{}", start, end));
    }
    // 进度输出（逐行，供解析）
    args.push("--newline".into());
    args.push("--no-progress".into());
    // 文件名安全
    args.push("--windows-filenames".into());
    args.push("--trim-filenames".into());
    args.push("120".into());
    // 其他
    args.push("--no-warnings".into());
    args.push("--ignore-errors".into());
    // URL 最后
    args.push(url.to_string());
    args
}

/// 解析 yt-dlp 进度行（纯函数）。返回 None 表示非进度行。
pub fn parse_progress_line(line: &str) -> Option<Progress> {
    let t = line.trim();
    if let Some(rest) = t.strip_prefix("[download]") {
        let rest = rest.trim();
        // Destination: <path>
        if let Some(path) = rest.strip_prefix("Destination:") {
            return Some(Progress {
                percent: 0.0,
                speed: None,
                eta: None,
                file: Some(path.trim().to_string()),
            });
        }
        // 45.2% of 123.4MiB at 5.2MiB/s ETA 00:15
        if let Some(caps) = FULL_RE.get_or_init(full_re).captures(rest) {
            let percent: f32 = caps[1].parse().ok()?;
            let speed = format!("{}{}/s", &caps[4], &caps[5]);
            let eta = caps[6].to_string();
            return Some(Progress {
                percent,
                speed: Some(speed),
                eta: Some(eta),
                file: None,
            });
        }
        // 100% 完成行等无 ETA 情况
        if let Some(caps) = PLAIN_RE.get_or_init(plain_re).captures(rest) {
            let percent: f32 = caps[1].parse().ok()?;
            return Some(Progress {
                percent,
                speed: None,
                eta: None,
                file: None,
            });
        }
        // Merger / has already been downloaded 等忽略
    }
    None
}

fn full_re() -> regex::Regex {
    regex::Regex::new(
        r"^(\d+\.\d+)% of ~?([\d.]+)([KMG]i?B|B) at ([\d.]+)([KMG]i?B|B)/s ETA (\d+:\d+)",
    )
    .expect("无效正则 full")
}

fn plain_re() -> regex::Regex {
    regex::Regex::new(r"^(\d+\.\d+)% of ~?([\d.]+)([KMG]i?B|B)").expect("无效正则 plain")
}

static FULL_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
static PLAIN_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();

/// 下载结果。
#[derive(Debug, Clone)]
pub struct DownloadOutcome {
    pub output_paths: Vec<PathBuf>,
}

/// 执行下载（阻塞；逐行回调进度；取消置位后杀进程树）。
pub fn run_download(
    resolver: &ToolResolver,
    url: &str,
    p: &DownloadParams,
    cfg: &DownloadConfig,
    cancel: &Arc<AtomicBool>,
    mut on_progress: impl FnMut(Progress),
) -> Result<DownloadOutcome> {
    let args = build_args(url, p, cfg);
    let mut cmd = resolver.command(Tool::YtDlp)?;
    cmd.args(&args);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut guard = ChildGuard::spawn(&mut cmd)?;
    let stdout = guard
        .stdout()
        .ok_or_else(|| CoreError::Io(std::io::Error::other("无法读取 yt-dlp 输出")))?;
    let stderr = guard
        .stderr()
        .ok_or_else(|| CoreError::Io(std::io::Error::other("无法读取 yt-dlp 错误输出")))?;

    let mut output_paths: Vec<PathBuf> = Vec::new();
    let mut saw_dest = std::collections::HashSet::new();

    let stdout_reader = std::io::BufReader::new(stdout);
    let mut lines = stdout_reader.lines();
    for line in lines.by_ref() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if cancel.load(Ordering::Relaxed) {
            guard.kill_tree();
            break;
        }
        if let Some(prog) = parse_progress_line(&line) {
            if let Some(f) = &prog.file {
                let p = PathBuf::from(f);
                if saw_dest.insert(p.clone()) {
                    output_paths.push(p);
                }
            }
            on_progress(prog);
        }
    }
    // 取消/等待
    let status = guard.wait()?;
    if cancel.load(Ordering::Relaxed) {
        return Err(CoreError::Cancelled);
    }
    if !status.success() {
        let err = read_stderr(stderr);
        return Err(CoreError::ProcessFailed {
            program: "yt-dlp".into(),
            code: status.code(),
            stderr: err,
        });
    }
    let _ = stderr;
    // 兜底：无 Destination 时按 out_dir 最新视频文件
    if output_paths.is_empty() {
        output_paths = latest_media_in(&p.out_dir).into_iter().collect();
    }
    // 过滤存在的路径
    output_paths.retain(|p| p.exists());
    if output_paths.is_empty() {
        return Err(CoreError::ProcessFailed {
            program: "yt-dlp".into(),
            code: None,
            stderr: "下载完成但未找到产物文件".into(),
        });
    }
    Ok(DownloadOutcome { output_paths })
}

/// 取消后清理：删除本次输出残留（§UL-06 取消清理）。
pub fn cleanup_on_cancel(p: &DownloadParams, temp_root: &Path) {
    let _ = std::fs::remove_dir_all(temp_root);
    if let Some(dir) = p.out_dir.parent() {
        let _ = dir;
    }
    // 输出目录内由本任务产生的 .part/.ytdl 残留由 yt-dlp 取消时自行清理；
    // 已完成的临时转码产物在 post_process 的 temp 中，一并删除。
    let _ = p;
}

fn read_stderr(mut stderr: std::process::ChildStderr) -> String {
    use std::io::Read;
    let mut buf = String::new();
    let _ = stderr.read_to_string(&mut buf);
    buf.trim().to_string()
}

/// 后处理（DL-04 M1 基础版）：超画质上限降分辨率 + 音量归一化。
/// 返回最终产物路径（处理失败时返回原路径并附告警日志）。
pub fn post_process(
    resolver: &ToolResolver,
    input: &Path,
    cfg: &DownloadConfig,
    general: &GeneralConfig,
    cancel: &Arc<AtomicBool>,
    mut on_log: impl FnMut(String),
) -> Result<PathBuf> {
    // 先解析产物（MD-06 与后处理共用一次探测）
    let probe = probe::probe_local(resolver, input)
        .map_err(|e| CoreError::Io(std::io::Error::other(format!("产物解析失败：{}", e))))?;
    let meta = &probe.meta;

    let need_downscale = meta
        .height
        .map(|h| h > cfg.max_h && cfg.max_h > 0)
        .unwrap_or(false);
    let need_gain = general.normalize_audio
        && meta
            .audio_volume
            .max_volume_db
            .map(|v| v < -0.5 && v > -100.0)
            .unwrap_or(false);

    if !need_downscale && !need_gain {
        return Ok(input.to_path_buf());
    }

    let out = input.with_extension("processed.mp4");
    let mut args: Vec<String> = vec![
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-map".into(),
        "0".into(),
    ];
    // 视频
    if need_downscale {
        let target = cfg.max_h;
        args.push("-vf".into());
        args.push(format!("scale=-2:'min(ih,{})'", target));
        args.push("-c:v".into());
        args.push("libx265".into());
        args.push("-crf".into());
        args.push("23".into());
        args.push("-preset".into());
        args.push("medium".into());
        args.push("-tag:v".into());
        args.push("hvc1".into());
    } else {
        args.push("-c:v".into());
        args.push("copy".into());
    }
    // 音频（增益到峰值 0dBFS，MAXGAIN 封顶 24dB，TC-07 语义）
    if need_gain {
        let max_v = meta.audio_volume.max_volume_db.unwrap_or(0.0);
        let gain = (-max_v).clamp(0.0, general.max_gain_db);
        if gain > 0.1 {
            on_log(format!(
                "音量归一化：max_volume {:.1}dB → +{:.1}dB 增益",
                max_v, gain
            ));
            args.push("-af".into());
            args.push(format!("volume={:.2}dB", gain));
        }
    }
    if !need_gain && !need_downscale {
        // 仅封面处理无需重编码音频
    }
    args.push("-c:a".into());
    if need_gain {
        args.push("aac".into());
    } else {
        args.push("copy".into());
    }
    args.push("-movflags".into());
    args.push("+faststart".into());
    args.push("-y".into());
    args.push(out.to_string_lossy().into_owned());

    let mut cmd = resolver.command(Tool::Ffmpeg)?;
    cmd.args(&args);
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let mut guard = ChildGuard::spawn(&mut cmd)?;
    // 简单等待（后处理通常较快；取消支持）
    loop {
        match guard.try_wait()? {
            Some(_) => break,
            None => {
                if cancel.load(Ordering::Relaxed) {
                    guard.kill_tree();
                    let _ = std::fs::remove_file(&out);
                    return Err(CoreError::Cancelled);
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
    }
    let status = guard.wait()?;
    if !status.success() {
        let err = read_stderr_opt(guard.stderr());
        on_log(format!("后处理失败（保留原文件）：{}", err));
        let _ = std::fs::remove_file(&out);
        return Ok(input.to_path_buf());
    }
    // 原子替换
    let tmp = out.with_extension("mp4.tmp");
    let _ = std::fs::rename(&out, &tmp);
    let _ = std::fs::rename(&tmp, input);
    Ok(input.to_path_buf())
}

fn read_stderr_opt(mut stderr: Option<std::process::ChildStderr>) -> String {
    use std::io::Read;
    match &mut stderr {
        Some(e) => {
            let mut buf = String::new();
            let _ = e.read_to_string(&mut buf);
            buf.trim().to_string()
        }
        None => String::new(),
    }
}

/// 目录内最新媒体文件（兜底产物定位）。
pub fn latest_media_in(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if is_video_file(&p) {
                files.push(p);
            }
        }
    }
    files.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    files
}

/// 是否视频扩展名。
pub fn is_video_file(p: &Path) -> bool {
    matches!(
        p.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .as_deref(),
        Some("mp4" | "mkv" | "mov" | "webm" | "avi" | "flv" | "ts" | "m4v")
    )
}

/// 解析产物元数据（MD-06 下载完成产物解析；与本地文件相同链路）。
pub fn probe_output(resolver: &ToolResolver, path: &Path) -> Result<MediaMeta> {
    let p = probe::probe_local(resolver, path)
        .map_err(|e| CoreError::Io(std::io::Error::other(format!("产物解析失败：{}", e))))?;
    Ok(p.meta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DownloadConfig;

    fn params() -> DownloadParams {
        DownloadParams {
            format_id: None,
            audio_only: false,
            out_dir: PathBuf::from("D:/videos"),
            filename_template: "纯标题".into(),
            embed_cover: true,
            proxy: Some("socks5://127.0.0.1:10808".into()),
            cookies_file: None,
            sections: None,
        }
    }

    #[test]
    fn output_template_mapping() {
        assert_eq!(output_template("纯标题", false), "%(title)s.%(ext)s");
        assert_eq!(
            output_template("标题+ID", false),
            "%(title)s [%(id)s].%(ext)s"
        );
        assert_eq!(
            output_template("UP主-标题", false),
            "%(uploader)s - %(title)s.%(ext)s"
        );
        assert_eq!(
            output_template("日期-标题", false),
            "%(upload_date)s %(title)s.%(ext)s"
        );
        assert_eq!(output_template("未知", false), "%(title)s.%(ext)s");
        assert!(output_template("纯标题", true).contains("playlist"));
    }

    #[test]
    fn default_format_includes_max_h() {
        let f = default_format(1080);
        assert!(f.contains("height<=1080"));
        let f = default_format(720);
        assert!(f.contains("height<=720"));
    }

    #[test]
    fn build_args_defaults() {
        let args = build_args(
            "https://example.com/v",
            &params(),
            &DownloadConfig::default(),
        );
        let joined = args.join(" ");
        assert!(joined.contains("--format"));
        assert!(joined.contains("bv*[height<=1080]+ba"));
        assert!(joined.contains("--merge-output-format mp4"));
        assert!(joined.contains("--no-overwrites"));
        assert!(joined.contains("-N 4"));
        assert!(joined.contains("--embed-thumbnail"));
        assert!(joined.contains("--no-playlist"));
        assert!(joined.contains("--proxy socks5://127.0.0.1:10808"));
        assert!(joined.contains("--newline"));
        assert!(joined.ends_with("https://example.com/v"));
    }

    #[test]
    fn build_args_selected_format() {
        let mut p = params();
        p.format_id = Some("137".into());
        let args = build_args("u", &p, &DownloadConfig::default());
        let joined = args.join(" ");
        assert!(joined.contains("137+ba/b"));
    }

    #[test]
    fn build_args_audio_only() {
        let mut p = params();
        p.audio_only = true;
        p.format_id = Some("140".into());
        let args = build_args("u", &p, &DownloadConfig::default());
        let joined = args.join(" ");
        assert!(joined.contains("-x"));
        assert!(joined.contains("--audio-format mp3"));
        assert!(joined.contains("140/bestaudio/best"));
    }

    #[test]
    fn parse_progress_full_line() {
        let p = parse_progress_line("[download]  45.2% of 123.4MiB at 5.2MiB/s ETA 00:15").unwrap();
        assert!((p.percent - 45.2).abs() < 0.01);
        assert_eq!(p.speed.as_deref(), Some("5.2MiB/s"));
        assert_eq!(p.eta.as_deref(), Some("00:15"));
    }

    #[test]
    fn parse_progress_destination() {
        let p = parse_progress_line("[download] Destination: D:/videos/测试视频.mp4").unwrap();
        assert_eq!(p.file.as_deref(), Some("D:/videos/测试视频.mp4"));
    }

    #[test]
    fn parse_progress_plain_percent() {
        let p = parse_progress_line("[download] 100.0% of 35.6MiB").unwrap();
        assert_eq!(p.percent, 100.0);
        assert!(p.speed.is_none());
    }

    #[test]
    fn parse_progress_ignores_other_lines() {
        assert!(parse_progress_line("[youtube] abc: Downloading webpage").is_none());
        assert!(parse_progress_line("Merging formats into ...").is_none());
        assert!(parse_progress_line("").is_none());
    }

    #[test]
    fn is_video_file_detects_exts() {
        assert!(is_video_file(Path::new("a.MP4")));
        assert!(is_video_file(Path::new("a.mkv")));
        assert!(!is_video_file(Path::new("a.jpg")));
        assert!(!is_video_file(Path::new("a.mp3")));
    }
}
