//! 元数据解析（§3.2 MD-01/MD-02/MD-06）：
//! - URL 解析：yt-dlp `-J`（JSON 元数据 + 格式列表 + 缩略图）
//! - 本地解析：ffprobe（流/格式/旋转/封面）+ volumedetect（音量）
//! - 失败分类（网络不可达 / 需要登录 / 链接无效 / 非视频 / 探测失败）
//!
//! 解析 JSON → MediaMeta 的转换均为纯函数，可单测；子进程调用薄封装在顶层。

use std::io::BufRead;
use std::path::Path;
use std::process::Stdio;

use serde_json::Value;

use crate::config::NetworkConfig;
use crate::exec::{ChildGuard, Tool, ToolResolver};
use crate::model::{AudioVolume, DownloadFormat, MediaMeta};
use crate::Result;

/// 解析来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeSource {
    YtDlp,
    Ffprobe,
}

/// 解析失败分类（MD-05）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeErrorKind {
    /// 网络不可达 / 超时
    Network,
    /// 需要登录（Cookie 缺失/过期/风控）
    NeedLogin,
    /// 链接无效 / 不存在 / 404
    InvalidLink,
    /// 非视频文件 / 探测失败（本地）
    NotVideo,
    /// 其他失败
    Failed,
}

/// URL 解析结果。
#[derive(Debug, Clone)]
pub struct UrlProbe {
    pub meta: MediaMeta,
    pub site: Option<String>,
    pub host: Option<String>,
    /// 是否需要登录才能下载
    pub needs_login: bool,
    /// 是否合集/多 P
    pub is_playlist: bool,
    pub playlist_count: Option<u32>,
    /// 缩略图 URL（缓存/封面用）
    pub thumbnail_url: Option<String>,
}

/// 本地解析结果。
#[derive(Debug, Clone)]
pub struct LocalProbe {
    pub meta: MediaMeta,
}

/// 播放列表单集条目（DL-09 平铺）。
#[derive(Debug, Clone)]
pub struct PlaylistEntry {
    pub url: String,
    pub title: String,
}

/// 展开播放列表（yt-dlp -J --flat-playlist）：快速拿每集 URL 与标题，
/// 命令层据此逐条平铺进统一列表。
pub fn list_playlist_entries(
    resolver: &ToolResolver,
    url: &str,
    cookies_file: Option<&Path>,
    network: &NetworkConfig,
) -> std::result::Result<Vec<PlaylistEntry>, ProbeFailure> {
    let mut cmd = resolver.command(Tool::YtDlp).map_err(|e| ProbeFailure {
        kind: ProbeErrorKind::Failed,
        message: e.to_string(),
    })?;
    cmd.arg("-J").arg("--flat-playlist").arg("--no-warnings");
    if let Some(cf) = cookies_file {
        cmd.arg("--cookies").arg(cf);
    }
    let proxy = if network.proxy_url.is_empty() {
        None
    } else {
        Some(network.proxy_url.as_str())
    };
    if let Some(p) = proxy {
        cmd.arg("--proxy").arg(p);
    }
    cmd.arg(url);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let guard = ChildGuard::spawn(&mut cmd).map_err(|e| ProbeFailure {
        kind: ProbeErrorKind::Failed,
        message: format!("启动 yt-dlp 失败：{}", e),
    })?;
    let output = guard.wait_with_output().map_err(|e| ProbeFailure {
        kind: ProbeErrorKind::Failed,
        message: format!("yt-dlp 退出异常：{}", e),
    })?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let mut f = classify_ytdlp_error(&err);
        f.message = format!("获取播放列表失败：{}", err.trim());
        return Err(f);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let v: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => {
            return Err(ProbeFailure {
                kind: ProbeErrorKind::Failed,
                message: "yt-dlp 返回无法解析的播放列表数据".into(),
            })
        }
    };
    let mut out = Vec::new();
    if let Some(entries) = v["entries"].as_array() {
        for e in entries {
            let u = e["url"]
                .as_str()
                .or_else(|| e["webpage_url"].as_str())
                .unwrap_or_default()
                .to_string();
            if u.is_empty() {
                continue;
            }
            let title = e["title"].as_str().unwrap_or("").to_string();
            out.push(PlaylistEntry { url: u, title });
        }
    }
    Ok(out)
}

/// 解析失败（分类 + 摘要）。
#[derive(Debug, Clone)]
pub struct ProbeFailure {
    pub kind: ProbeErrorKind,
    pub message: String,
}

impl std::fmt::Display for ProbeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// URL 解析（yt-dlp -J）。
/// `cookies_file`：Netscape 临时文件路径（None 则不带）。
pub fn probe_url(
    resolver: &ToolResolver,
    url: &str,
    cookies_file: Option<&Path>,
    network: &NetworkConfig,
    playlist: bool,
) -> std::result::Result<UrlProbe, ProbeFailure> {
    let mut cmd = resolver.command(Tool::YtDlp).map_err(|e| ProbeFailure {
        kind: ProbeErrorKind::Failed,
        message: e.to_string(),
    })?;
    cmd.arg("-J").arg("--no-warnings");
    if playlist {
        cmd.arg("--yes-playlist");
    } else {
        cmd.arg("--no-playlist");
    }
    if let Some(cf) = cookies_file {
        cmd.arg("--cookies").arg(cf);
    }
    let proxy = if network.proxy_url.is_empty() {
        None
    } else {
        Some(network.proxy_url.as_str())
    };
    if let Some(p) = proxy {
        cmd.arg("--proxy").arg(p);
    }
    cmd.arg(url);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let guard = ChildGuard::spawn(&mut cmd).map_err(|e| ProbeFailure {
        kind: ProbeErrorKind::Failed,
        message: format!("启动 yt-dlp 失败：{}", e),
    })?;
    let output = guard.wait_with_output().map_err(|e| ProbeFailure {
        kind: ProbeErrorKind::Failed,
        message: format!("yt-dlp 退出异常：{}", e),
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(classify_ytdlp_error(stderr.trim()));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_ytdlp_json(&text).map_err(|e| ProbeFailure {
        kind: ProbeErrorKind::Failed,
        message: format!("解析 yt-dlp 输出失败：{}", e),
    })
}

/// 本地文件解析（ffprobe + volumedetect）。
pub fn probe_local(
    resolver: &ToolResolver,
    path: &Path,
) -> std::result::Result<LocalProbe, ProbeFailure> {
    if !path.is_file() {
        return Err(ProbeFailure {
            kind: ProbeErrorKind::NotVideo,
            message: format!("文件不存在：{}", path.display()),
        });
    }
    // 1) ffprobe 基础信息
    let mut cmd = resolver.command(Tool::Ffprobe).map_err(|e| ProbeFailure {
        kind: ProbeErrorKind::Failed,
        message: e.to_string(),
    })?;
    cmd.args([
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
    ])
    .arg(path)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let guard = ChildGuard::spawn(&mut cmd).map_err(|e| ProbeFailure {
        kind: ProbeErrorKind::Failed,
        message: format!("启动 ffprobe 失败：{}", e),
    })?;
    let out = guard.wait_with_output().map_err(|e| ProbeFailure {
        kind: ProbeErrorKind::Failed,
        message: format!("ffprobe 退出异常：{}", e),
    })?;
    if !out.status.success() {
        return Err(ProbeFailure {
            kind: ProbeErrorKind::NotVideo,
            message: format!(
                "ffprobe 探测失败：{}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        });
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut meta = parse_ffprobe_json(&text);
    meta.size_bytes = std::fs::metadata(path).ok().map(|m| m.len());

    // 2) volumedetect（有音频流时）
    if meta.acodec.is_some() {
        if let Ok(vol) = probe_volume(resolver, path) {
            meta.audio_volume = vol;
        }
    }
    Ok(LocalProbe { meta })
}

/// 音量探测（ffmpeg volumedetect）。
pub fn probe_volume(resolver: &ToolResolver, path: &Path) -> Result<AudioVolume> {
    let mut cmd = resolver.command(Tool::Ffmpeg)?;
    cmd.args(["-i"])
        .arg(path)
        .args(["-af", "volumedetect", "-f", "null", "-"]);
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let guard = ChildGuard::spawn(&mut cmd)?;
    let out = guard.wait_with_output()?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    Ok(parse_volumedetect(&stderr))
}

/// 解析 yt-dlp `-J` JSON → UrlProbe（纯函数）。
pub fn parse_ytdlp_json(text: &str) -> Result<UrlProbe> {
    let v: Value = serde_json::from_str(text)?;
    let title = v["title"].as_str().map(str::to_string);
    let duration = v["duration"].as_f64();
    let thumbnail = v["thumbnail"].as_str().map(str::to_string);
    let is_playlist = v["_type"].as_str() == Some("playlist")
        || v["playlist_count"].as_u64().map(|c| c > 1).unwrap_or(false);
    let playlist_count = v["playlist_count"].as_u64().map(|c| c as u32);
    let webpage_url = v["webpage_url"].as_str().unwrap_or_default();
    let host = crate::cookies::host_from_url(webpage_url);
    let extractor = v["extractor"].as_str().unwrap_or_default();
    let site = if extractor.is_empty() {
        None
    } else {
        Some(extractor.to_string())
    };

    // 格式列表（fps/tbr/filesize 完整时才进入列表；带协议 video+audio 合并项）
    let mut formats: Vec<DownloadFormat> = Vec::new();
    if let Some(arr) = v["formats"].as_array() {
        for f in arr {
            let format_id = f["format_id"].as_str().unwrap_or_default().to_string();
            if format_id.is_empty() {
                continue;
            }
            let vcodec = f["vcodec"].as_str().map(str::to_string);
            let acodec = f["acodec"].as_str().map(str::to_string);
            let height = f["height"].as_u64().map(|h| h as u32);
            let fps = f["fps"].as_f64();
            let filesize = f.as_object().and_then(|o| {
                o.get("filesize")
                    .or_else(|| o.get("filesize_approx"))
                    .and_then(|x| x.as_u64())
            });
            let tbr = f["tbr"].as_f64().map(|t| t as u32);
            let ext = f["ext"].as_str().map(str::to_string);
            let has_video = match vcodec.as_deref() {
                None => false,
                Some(c) => !matches!(c.to_lowercase().as_str(), "none" | "n/a" | "images"),
            };
            let audio_only = !has_video && acodec.is_some();
            let note = if f["format_note"].as_str().is_some() {
                f["format_note"].as_str().map(str::to_string)
            } else {
                None
            };
            let df = DownloadFormat {
                label: String::new(),
                format_id,
                height,
                ext,
                vcodec,
                acodec,
                filesize_bytes: filesize,
                fps,
                tbr_kbps: tbr,
                note,
                audio_only,
            };
            let label = df.make_label();
            formats.push(DownloadFormat { label, ..df });
        }
    }
    // 去重（同 format_id 保留首个）
    let mut seen = std::collections::HashSet::new();
    formats.retain(|f| seen.insert(f.format_id.clone()));

    let meta = MediaMeta {
        title: title.clone(),
        duration_secs: duration,
        height: formats
            .iter()
            .filter(|f| !f.audio_only)
            .map(|f| f.height.unwrap_or(0))
            .max(),
        container: None, // 下载容器由所选格式决定，展示列用 URL 行内格式按钮
        formats: formats.iter().map(|f| f.label.clone()).collect(),
        download_formats: formats,
        has_cover: thumbnail.is_some(),
        ..Default::default()
    };
    Ok(UrlProbe {
        meta,
        site,
        host,
        needs_login: false,
        is_playlist,
        playlist_count,
        thumbnail_url: thumbnail,
    })
}

/// 解析 ffprobe JSON → MediaMeta（纯函数）。
pub fn parse_ffprobe_json(text: &str) -> MediaMeta {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return MediaMeta::default(),
    };
    let mut meta = MediaMeta::default();
    if let Some(fmt) = v["format"].as_object() {
        meta.container = fmt.get("format_name").and_then(|x| x.as_str()).map(|s| {
            let lower = s.to_lowercase();
            // mov,mp4,m4a,… 优先显示 MP4（用户习惯），否则取首个格式名大写
            if lower.contains("mp4") {
                "MP4".to_string()
            } else {
                let base = lower.split(',').next().unwrap_or(&lower);
                base.to_uppercase()
            }
        });
        meta.duration_secs = fmt
            .get("duration")
            .and_then(|x| x.as_str())
            .and_then(|s| s.parse().ok());
        meta.size_bytes = fmt
            .get("size")
            .and_then(|x| x.as_str())
            .and_then(|s| s.parse().ok());
        meta.has_cover = false;
    }
    let streams = v["streams"].as_array().cloned().unwrap_or_default();
    let mut has_audio = false;
    let mut audio_tracks = 0u32;
    let mut rotate: Option<i32> = None;
    for s in &streams {
        match s["codec_type"].as_str() {
            Some("video") => {
                meta.height = s["height"].as_u64().map(|h| h as u32);
                meta.vcodec = s["codec_name"].as_str().map(str::to_string);
                meta.fps = s["avg_frame_rate"].as_str().and_then(|r| {
                    let mut it = r.split('/');
                    let n: f64 = it.next()?.parse().ok()?;
                    let d: f64 = it.next()?.parse().ok()?;
                    if d == 0.0 {
                        None
                    } else {
                        Some(n / d)
                    }
                });
                meta.vbitrate_kbps = s["bit_rate"]
                    .as_str()
                    .and_then(|b| b.parse::<f64>().ok())
                    .map(|b| (b / 1000.0) as u32);
                meta.extradata = s["extradata"].as_str().map(str::to_string);
                if s["disposition"]["attached_pic"].as_u64() == Some(1)
                    || s["disposition"]["attached_pic"].as_str() == Some("1")
                {
                    meta.has_cover = true;
                }
                // 旋转标记（转码/后处理时清零，TC-09）
                if let Some(tags) = s["tags"].as_object() {
                    if let Some(r) = tags.get("rotate").and_then(|x| x.as_str()) {
                        if let Ok(deg) = r.parse::<i32>() {
                            rotate = Some(deg);
                        }
                    }
                }
            }
            Some("audio") => {
                has_audio = true;
                audio_tracks += 1;
                // 保留首个音频流的信息（后续流仅计数）
                if meta.acodec.is_none() {
                    meta.acodec = s["codec_name"].as_str().map(str::to_string);
                    meta.abitrate_kbps = s["bit_rate"]
                        .as_str()
                        .and_then(|b| b.parse::<f64>().ok())
                        .map(|b| (b / 1000.0) as u32);
                    meta.sample_rate = s["sample_rate"]
                        .as_str()
                        .and_then(|r| r.parse().ok());
                }
            }
            _ => {}
        }
    }
    meta.audio_tracks = if has_audio { Some(audio_tracks) } else { None };
    if meta.acodec.is_some() && meta.audio_tracks == Some(0) {
        meta.audio_tracks = Some(1);
    }
    // rotate 记录（旋转角来自文件标记；手动旋转以条目 rot_angle 为准）
    let _ = rotate;
    meta
}

/// 解析 volumedetect 输出 → AudioVolume（纯函数）。
pub fn parse_volumedetect(stderr: &str) -> AudioVolume {
    let mut v = AudioVolume::default();
    for line in stderr.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("[Parsed_volumedetect") {
            if let Some(rest) = rest.split("] ").nth(1) {
                if let Some(val) = rest.strip_prefix("mean_volume: ") {
                    v.mean_volume_db = val.trim_end_matches(" dB").trim().parse().ok();
                } else if let Some(val) = rest.strip_prefix("max_volume: ") {
                    v.max_volume_db = val.trim_end_matches(" dB").trim().parse().ok();
                }
            }
        }
    }
    v
}

/// 分类 yt-dlp 错误（MD-05）。
fn classify_ytdlp_error(stderr: &str) -> ProbeFailure {
    let lower = stderr.to_lowercase();
    let kind = if lower.contains("sign in")
        || lower.contains("login")
        || lower.contains("需要登录")
        || lower.contains("private")
        || lower.contains("log in")
        || lower.contains("members only")
    {
        ProbeErrorKind::NeedLogin
    } else if lower.contains("unable to download webpage")
        || lower.contains("timed out")
        || lower.contains("connection")
        || lower.contains("网络不可达")
    {
        ProbeErrorKind::Network
    } else if lower.contains("does not exist")
        || lower.contains("404")
        || lower.contains("invalid url")
    {
        ProbeErrorKind::InvalidLink
    } else if lower.contains("video unavailable") || lower.contains("unavailable") {
        ProbeErrorKind::NeedLogin
    } else {
        ProbeErrorKind::Failed
    };
    ProbeFailure {
        kind,
        message: stderr.lines().last().unwrap_or("未知错误").to_string(),
    }
}

/// 子进程逐行回调辅助（供下载进度等使用）。
pub fn read_lines<R: BufRead, F: FnMut(String)>(reader: R, mut on_line: F) {
    for line in reader.lines().map_while(|l| l.ok()) {
        on_line(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const YTDLP_JSON: &str = r#"{
      "id": "abc123",
      "title": "示例视频标题",
      "duration": 332.5,
      "thumbnail": "https://i.ytimg.com/vi/abc123/maxresdefault.jpg",
      "extractor": "youtube",
      "webpage_url": "https://www.youtube.com/watch?v=abc123",
      "formats": [
        {"format_id": "137", "ext": "mp4", "height": 1080, "vcodec": "avc1", "acodec": "none", "fps": 30.0, "filesize": 35651584, "tbr": 1200.0, "format_note": "1080p"},
        {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a", "fps": null, "filesize": 5242880, "tbr": 128.0, "format_note": "medium"}
      ]
    }"#;

    #[test]
    fn parse_ytdlp_json_basic() {
        let p = parse_ytdlp_json(YTDLP_JSON).unwrap();
        assert_eq!(p.meta.title.as_deref(), Some("示例视频标题"));
        assert_eq!(p.meta.duration_secs, Some(332.5));
        assert_eq!(p.host.as_deref(), Some("www.youtube.com"));
        assert_eq!(p.site.as_deref(), Some("youtube"));
        assert_eq!(
            p.thumbnail_url.as_deref(),
            Some("https://i.ytimg.com/vi/abc123/maxresdefault.jpg")
        );
        assert!(!p.is_playlist);
    }

    #[test]
    fn parse_ytdlp_json_formats() {
        let p = parse_ytdlp_json(YTDLP_JSON).unwrap();
        assert_eq!(p.meta.download_formats.len(), 2);
        let video = &p.meta.download_formats[0];
        assert_eq!(video.format_id, "137");
        assert_eq!(video.height, Some(1080));
        assert!(!video.audio_only);
        assert!(video.label.contains("1080P"));
        assert!(video.label.contains("H.264"));
        let audio = &p.meta.download_formats[1];
        assert!(audio.audio_only);
        assert!(audio.label.contains("仅音频"));
        assert!(audio.label.contains("AAC"));
        // 画质/格式展示列
        assert!(p.meta.formats.iter().any(|l| l.contains("1080P")));
    }

    #[test]
    fn parse_ytdlp_json_playlist() {
        let text = r#"{"_type":"playlist","playlist_count":5,"title":"合集","formats":[]}"#;
        let p = parse_ytdlp_json(text).unwrap();
        assert!(p.is_playlist);
        assert_eq!(p.playlist_count, Some(5));
    }

    #[test]
    fn parse_ytdlp_json_dedup_format_ids() {
        let text = r#"{"title":"t","formats":[{"format_id":"a","vcodec":"avc1","acodec":"none"},{"format_id":"a","vcodec":"avc1","acodec":"none"}]}"#;
        let p = parse_ytdlp_json(text).unwrap();
        assert_eq!(p.meta.download_formats.len(), 1);
    }

    #[test]
    fn parse_ffprobe_json_media() {
        let json = r#"{
          "streams": [
            {"codec_type":"video","codec_name":"hevc","width":1920,"height":1080,"avg_frame_rate":"60/1","bit_rate":"12000000","tags":{"rotate":"90"}},
            {"codec_type":"audio","codec_name":"aac","bit_rate":"320000"},
            {"codec_type":"audio","codec_name":"aac","bit_rate":"128000"}
          ],
          "format": {"format_name":"mov,mp4,m4a","duration":"332.000000","size":"35651584"}
        }"#;
        let m = parse_ffprobe_json(json);
        assert_eq!(m.height, Some(1080));
        assert_eq!(m.vcodec.as_deref(), Some("hevc"));
        assert_eq!(m.fps, Some(60.0));
        assert_eq!(m.vbitrate_kbps, Some(12000));
        assert_eq!(m.acodec.as_deref(), Some("aac"));
        assert_eq!(m.audio_tracks, Some(2));
        assert_eq!(m.abitrate_kbps, Some(320));
        assert_eq!(m.container.as_deref(), Some("MP4"));
        assert_eq!(m.duration_secs, Some(332.0));
        assert_eq!(m.size_bytes, Some(35651584));
        assert!(!m.has_cover);
    }

    #[test]
    fn parse_ffprobe_json_attached_pic_cover() {
        let json = r#"{"streams":[{"codec_type":"video","codec_name":"mjpeg","disposition":{"attached_pic":1}},{"codec_type":"video","codec_name":"hevc","height":1080},{"codec_type":"audio","codec_name":"aac"}]}"#;
        let m = parse_ffprobe_json(json);
        assert!(m.has_cover);
        assert_eq!(m.height, Some(1080));
    }

    #[test]
    fn parse_volumedetect_ok() {
        let stderr = "\
[Parsed_volumedetect_0 @ 0x7f] n_samples: 1000
[Parsed_volumedetect_0 @ 0x7f] mean_volume: -15.3 dB
[Parsed_volumedetect_0 @ 0x7f] max_volume: -8.2 dB
";
        let v = parse_volumedetect(stderr);
        assert_eq!(v.mean_volume_db, Some(-15.3));
        assert_eq!(v.max_volume_db, Some(-8.2));
    }

    #[test]
    fn parse_volumedetect_empty() {
        assert_eq!(parse_volumedetect("").max_volume_db, None);
    }

    #[test]
    fn classify_errors() {
        let e = classify_ytdlp_error("ERROR: Please sign in to view this video");
        assert_eq!(e.kind, ProbeErrorKind::NeedLogin);
        let e = classify_ytdlp_error("ERROR: Video unavailable. This video is private");
        assert_eq!(e.kind, ProbeErrorKind::NeedLogin);
        let e = classify_ytdlp_error("ERROR: Unable to download webpage: timed out");
        assert_eq!(e.kind, ProbeErrorKind::Network);
        let e = classify_ytdlp_error("ERROR: This video does not exist");
        assert_eq!(e.kind, ProbeErrorKind::InvalidLink);
        let e = classify_ytdlp_error("ERROR: something else happened");
        assert_eq!(e.kind, ProbeErrorKind::Failed);
    }

    #[test]
    fn download_format_label_audio() {
        let f = DownloadFormat {
            format_id: "140".into(),
            label: String::new(),
            height: None,
            ext: Some("m4a".into()),
            vcodec: None,
            acodec: Some("mp4a".into()),
            filesize_bytes: Some(5242880),
            fps: None,
            tbr_kbps: Some(128),
            note: None,
            audio_only: true,
        };
        let l = f.make_label();
        assert!(l.contains("仅音频"));
        assert!(l.contains("M4A"));
        assert!(l.contains("AAC"));
        assert!(l.contains("5.0MB"));
    }
}
