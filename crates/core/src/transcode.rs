//! 转码引擎（§3.4 TC）：ffmpeg 子进程。
//!
//! 输出 H.265（QSV/NVENC/AMF/libx265），支持：手动旋转（条目 rot_angle，TC-04）、
//! 分辨率上限（MAXW/MAXH）、码率封顶、音量归一化（增益上限，通用段）、保留封面、
//! 文件名模板（下载段共用）+ 碰撞安全命名（auto_inc/skip）、取消清理输出残留（UL-06）。
//! x265 CRF 固定 23（不落配置，TC 需求）。
//!
//! 进度：`ffmpeg -progress pipe:1 -nostats`，按 `out_time_us` 相对探测时长换算百分比。

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::exec::{decode_text, ChildGuard, Tool, ToolResolver};
use crate::model::{MediaMeta, RotAngle};
use crate::{CoreError, Result};

/// 转码参数（来自 设置-转码/通用/下载 + 条目 rot_angle，TC-05）。
#[derive(Debug, Clone)]
pub struct TranscodeParams {
    pub input: PathBuf,
    pub out_dir: PathBuf,
    /// 输出命名标题（文件名模板输入）
    pub title: String,
    /// 文件名模板：纯标题/标题+ID/UP主-标题/日期-标题（下载段共用）
    pub filename_template: String,
    /// 容器：mp4 | mkv（P0 仅两者，§TC-12）
    pub container: String,
    /// 编码器模式：auto | libx265 | nvenc | amf（自动 = QSV → libx265 兜底）
    pub encoder_mode: String,
    /// QSV low_power（仅 auto→QSV 时生效）
    pub low_power: bool,
    pub max_w: u32,
    pub max_h: u32,
    pub brcap_kbps: Option<u32>,
    pub normalize_audio: bool,
    pub max_gain_db: f32,
    pub rot_angle: RotAngle,
    pub keep_cover: bool,
    /// auto_inc | skip（§TC-11 碰撞命名策略）
    pub collision_policy: String,
}

impl TranscodeParams {
    /// 目标扩展名（容器 → 扩展）。
    pub fn extension(&self) -> &'static str {
        match self.container.as_str() {
            "mkv" => "mkv",
            _ => "mp4",
        }
    }

    /// 输出路径（模板命名 + 碰撞策略；skip 且已存在时返回 Err）。
    pub fn output_path(&self) -> Result<PathBuf> {
        let base = apply_filename_template(&self.filename_template, &self.title);
        let ext = self.extension();
        let file = format!("{}.{}", base, ext);
        let candidate = self.out_dir.join(&file);
        if !candidate.exists() {
            return Ok(candidate);
        }
        match self.collision_policy.as_str() {
            "skip" => Err(CoreError::Io(std::io::Error::other(format!(
                "输出已存在，按策略跳过：{}",
                candidate.display()
            )))),
            _ => {
                // auto_inc：名 (1).ext / (2).ext …
                for i in 1..1000 {
                    let p = self
                        .out_dir
                        .join(format!("{} ({}).{}", base, i, ext));
                    if !p.exists() {
                        return Ok(p);
                    }
                }
                Err(CoreError::Io(std::io::Error::other("无法生成不冲突的输出名")))
            }
        }
    }
}

/// 文件名模板（§设置-下载，下载/转码/合并输出共用）。
///
/// - 纯标题：`{title}`
/// - 标题+ID：`{title}-{id 前 8 位}`
/// - UP主-标题：本地条目无 UP 主字段，回退为 `{title}`（下载侧由 yt-dlp 模板实现）
/// - 日期-标题：`{YYYY-MM-DD}-{title}`
pub fn apply_filename_template(tmpl: &str, title: &str) -> String {
    let title = sanitize_filename(title);
    match tmpl {
        "标题+ID" => format!("{}-{}", title, id_hint(title.as_str())),
        "日期-标题" => format!("{}-{}", today(), title),
        "UP主-标题" => title,
        _ => title,
    }
}

/// 文件名清洗（Windows 非法字符 / 截断）。
pub fn sanitize_filename(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    let t = cleaned.trim().trim_end_matches('.').to_string();
    if t.is_empty() {
        "未命名".into()
    } else {
        t.chars().take(120).collect()
    }
}

fn id_hint(_title: &str) -> String {
    // 本地转码无稳定 ID 字段：用标题长度做短指纹，保证不同文件不互相覆盖
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    _title.hash(&mut h);
    format!("{:08x}", h.finish() & 0xFFFF_FFFF)
}

/// 今天日期 YYYY-MM-DD（无 chrono，手算公历）。
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86400) as i64 + 719_468; // 1970-01-01 → Rata Die
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// 编码器选择（TC-16 简化：auto = QSV 可用则 hevc_qsv，否则 libx265 兜底；
/// 显式 nvenc/amf/libx265 不被自动覆盖）。
fn pick_encoder(
    resolver: &ToolResolver,
    mode: &str,
    low_power: bool,
) -> Result<(String, Vec<String>)> {
    let sv = |v: &[&str]| -> Vec<String> { v.iter().map(|s| s.to_string()).collect() };
    match mode {
        "libx265" => Ok(("libx265".into(), sv(&["-crf", "23", "-preset", "medium"]))),
        "nvenc" => Ok((
            "hevc_nvenc".into(),
            sv(&["-rc", "vbr", "-cq", "23", "-preset", "p5"]),
        )),
        "amf" => Ok((
            "hevc_amf".into(),
            sv(&["-qp_i", "23", "-qp_p", "23", "-quality", "balanced"]),
        )),
        _ => {
            if qsv_available(resolver)? {
                let mut args = sv(&["-global_quality", "23"]);
                if low_power {
                    args.push("-low_power".into());
                    args.push("1".into());
                }
                Ok(("hevc_qsv".into(), args))
            } else {
                Ok(("libx265".into(), sv(&["-crf", "23", "-preset", "medium"])))
            }
        }
    }
}

/// 可用硬件编码器探测结果（TC-16）。
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct HwEncoders {
    pub qsv: bool,
    pub nvenc: bool,
    pub amf: bool,
}

/// 解析 `ffmpeg -encoders` 输出（纯函数，可单测）。
pub fn parse_encoders_output(text: &str) -> HwEncoders {
    let mut hw = HwEncoders::default();
    for line in text.lines() {
        if line.contains("hevc_qsv") {
            hw.qsv = true;
        } else if line.contains("hevc_nvenc") {
            hw.nvenc = true;
        } else if line.contains("hevc_amf") {
            hw.amf = true;
        }
    }
    hw
}

/// 探测 ffmpeg 是否内置 QSV（hevc_qsv）编码器。
pub fn qsv_available(resolver: &ToolResolver) -> Result<bool> {
    Ok(detect_hw_encoders(resolver)?.qsv)
}

/// 探测可用硬件编码器（QSV/NVENC/AMF，TC-16）。
pub fn detect_hw_encoders(resolver: &ToolResolver) -> Result<HwEncoders> {
    let out = crate::exec::run_tool_capture(resolver, Tool::Ffmpeg, &["-encoders"])?;
    let text = decode_text(&out.stdout);
    Ok(parse_encoders_output(&text))
}

/// 视频滤镜链：旋转（transpose）→ 分辨率上限（不放大，force_original_aspect_ratio）。
fn build_vf(rot: RotAngle, max_w: u32, max_h: u32) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    match rot.degrees() {
        90 => parts.push("transpose=1".into()),
        180 => parts.push("transpose=1,transpose=1".into()),
        270 => parts.push("transpose=2".into()),
        _ => {}
    }
    if max_w > 0 || max_h > 0 {
        let w = if max_w > 0 {
            format!("min(iw,{})", max_w)
        } else {
            "iw".into()
        };
        let h = if max_h > 0 {
            format!("min(ih,{})", max_h)
        } else {
            "ih".into()
        };
        parts.push(format!(
            "scale={}:{}:force_original_aspect_ratio=decrease",
            w, h
        ));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(","))
    }
}

/// 构造 ffmpeg 参数（TC-03/TC-07/TC-08/TC-09/TC-10）。
pub fn build_args(resolver: &ToolResolver, params: &TranscodeParams, meta: &MediaMeta) -> Result<Vec<String>> {
    let (encoder, mut enc_args) = pick_encoder(resolver, &params.encoder_mode, params.low_power)?;
    if params.container == "mp4" && encoder == "libx265" {
        // hvc1 标签（Apple 兼容）
        enc_args.push("-tag:v".into());
        enc_args.push("hvc1".into());
    }
    // 码率封顶（kbps；None 不限制）
    if let Some(br) = params.brcap_kbps {
        if br > 0 {
            enc_args.push("-maxrate".into());
            enc_args.push(format!("{}k", br));
            enc_args.push("-bufsize".into());
            enc_args.push(format!("{}k", br * 2));
        }
    }
    // 音频增益（normalize_audio + 解析音量；接近满度/无音量不处理）
    let need_gain = params.normalize_audio
        && meta
            .audio_volume
            .max_volume_db
            .map(|v| v < -0.5 && v > -100.0)
            .unwrap_or(false);
    let gain = if need_gain {
        let max_v = meta.audio_volume.max_volume_db.unwrap_or(0.0);
        Some((-max_v).clamp(0.0, params.max_gain_db))
    } else {
        None
    };

    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-i".into(),
        params.input.to_string_lossy().into_owned(),
        "-map".into(),
        "0:v:0".into(),
        "-map".into(),
        "0:a?".into(),
    ];
    if params.keep_cover {
        args.push("-map".into());
        args.push("0:t?".into());
    }
    if let Some(vf) = build_vf(params.rot_angle, params.max_w, params.max_h) {
        args.push("-vf".into());
        args.push(vf);
    }
    args.push("-c:v".into());
    args.push(encoder);
    args.extend(enc_args);
    args.push("-c:a".into());
    if let Some(g) = gain {
        if g > 0.1 {
            args.push("-af".into());
            args.push(format!("volume={:.2}dB", g));
        }
        args.push("aac".into());
    } else {
        args.push("copy".into());
    }
    if params.keep_cover {
        args.push("-c:t".into());
        args.push("copy".into());
    }
    args.push("-map_metadata".into());
    args.push("0".into());
    match params.container.as_str() {
        "mkv" => args.push("-f".into()),
        _ => {
            args.push("-movflags".into());
            args.push("+faststart".into());
            args.push("-f".into());
        }
    }
    if params.container == "mkv" {
        args.push("matroska".into());
    } else {
        args.push("mp4".into());
    }
    args.push("-y".into());
    args.push("-progress".into());
    args.push("pipe:1".into());
    args.push("-nostats".into());
    args.push("-loglevel".into());
    args.push("error".into());
    Ok(args)
}

/// 解析 `-progress` 输出中的 `out_time_us=`（微秒）。
pub(crate) fn parse_out_time_us(line: &str) -> Option<u64> {
    let line = line.trim();
    if let Some(v) = line.strip_prefix("out_time_us=") {
        return v.trim().parse().ok();
    }
    if let Some(v) = line.strip_prefix("out_time_ms=") {
        return v.trim().parse::<u64>().ok().map(|ms| ms * 1000);
    }
    None
}

/// 执行转码。
///
/// 返回输出路径；取消时终止子进程树并删除输出残留（UL-06）；失败删除半成品保留原文件。
/// 执行转码（TC-16 硬编失败自动回退 libx265：显式 NVENC/AMF 或自动探测
/// 出的 QSV 运行时失败，非取消时用 CPU 编码重试一次，进度/日志延续）。
pub fn run_transcode(
    resolver: &ToolResolver,
    params: &TranscodeParams,
    meta: &MediaMeta,
    cancel: &Arc<AtomicBool>,
    mut on_progress: impl FnMut(f32),
    mut on_log: impl FnMut(String),
) -> Result<PathBuf> {
    match run_transcode_once(resolver, params, meta, cancel, &mut on_progress, &mut on_log) {
        Err(e)
            if !matches!(e, CoreError::Cancelled)
                && !matches!(params.encoder_mode.as_str(), "libx265") =>
        {
            on_log("硬编失败，自动回退 libx265 重试…".to_string());
            let mut p2 = params.clone();
            p2.encoder_mode = "libx265".into();
            run_transcode_once(resolver, &p2, meta, cancel, &mut on_progress, &mut on_log)
        }
        r => r,
    }
}

fn run_transcode_once(
    resolver: &ToolResolver,
    params: &TranscodeParams,
    meta: &MediaMeta,
    cancel: &Arc<AtomicBool>,
    on_progress: &mut dyn FnMut(f32),
    on_log: &mut dyn FnMut(String),
) -> Result<PathBuf> {
    let out = params.output_path()?;
    let args = build_args(resolver, params, meta)?;
    on_log(format!(
        "转码 {} → {}（{}）",
        params.input.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        out.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        params.encoder_mode
    ));

    let mut cmd = resolver.command(Tool::Ffmpeg)?;
    cmd.args(&args);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut guard = ChildGuard::spawn(&mut cmd)?;
    let stdout = guard
        .stdout()
        .ok_or_else(|| CoreError::Io(std::io::Error::other("无法读取 ffmpeg 输出")))?;
    let stderr = guard
        .stderr()
        .ok_or_else(|| CoreError::Io(std::io::Error::other("无法读取 ffmpeg 错误输出")))?;

    let duration = meta.duration_secs.unwrap_or(0.0);
    let reader = std::io::BufReader::new(stdout);
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if cancel.load(Ordering::Relaxed) {
            guard.kill_tree();
            break;
        }
        if let Some(us) = parse_out_time_us(&line) {
            if duration > 0.0 {
                let pct = ((us as f64 / 1e6) / duration * 100.0).clamp(0.0, 99.0) as f32;
                on_progress(pct);
            }
        }
    }

    let status = guard.wait()?;
    if cancel.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(&out);
        return Err(CoreError::Cancelled);
    }
    if !status.success() {
        let err = read_stderr(stderr);
        on_log(format!("转码失败（已删除半成品，保留原文件）：{}", err));
        let _ = std::fs::remove_file(&out);
        return Err(CoreError::ProcessFailed {
            program: "ffmpeg".into(),
            code: status.code(),
            stderr: err,
        });
    }
    if !out.exists() {
        return Err(CoreError::ProcessFailed {
            program: "ffmpeg".into(),
            code: None,
            stderr: "转码结束但未找到输出文件".into(),
        });
    }
    on_log(format!("转码完成：{}", out.display()));
    Ok(out)
}

fn read_stderr(mut stderr: std::process::ChildStderr) -> String {
    use std::io::Read;
    let mut buf = String::new();
    let _ = stderr.read_to_string(&mut buf);
    let t = buf.trim().to_string();
    if t.is_empty() {
        "（无错误输出）".into()
    } else {
        t
    }
}

/// 判断是否本地可转码输入（文件存在）。
pub fn is_transcode_input(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_pure_title() {
        assert_eq!(apply_filename_template("纯标题", "a/b:c.mp4"), "a_b_c.mp4");
    }

    #[test]
    fn template_date_prefix() {
        let s = apply_filename_template("日期-标题", "你好");
        assert!(s.starts_with("20") && s.contains('-') && s.ends_with("你好"), "{}", s);
    }

    #[test]
    fn sanitize_windows_chars() {
        assert_eq!(sanitize_filename("a<b>:c\"d/e\\f|g?h*i"), "a_b__c_d_e_f_g_h_i");
        assert_eq!(sanitize_filename("  "), "未命名");
        assert_eq!(sanitize_filename("abc."), "abc");
    }

    #[test]
    fn output_collision_auto_inc() {
        let dir = tempfile::tempdir().unwrap();
        let p = TranscodeParams {
            input: "x.mp4".into(),
            out_dir: dir.path().to_path_buf(),
            title: "t".into(),
            filename_template: "纯标题".into(),
            container: "mp4".into(),
            encoder_mode: "libx265".into(),
            low_power: false,
            max_w: 0,
            max_h: 0,
            brcap_kbps: None,
            normalize_audio: false,
            max_gain_db: 24.0,
            rot_angle: RotAngle::ZERO,
            keep_cover: true,
            collision_policy: "auto_inc".into(),
        };
        let a = p.output_path().unwrap();
        assert_eq!(a.file_name().unwrap(), "t.mp4");
        std::fs::write(&a, b"x").unwrap();
        let b = p.output_path().unwrap();
        assert_eq!(b.file_name().unwrap(), "t (1).mp4");
    }

    #[test]
    fn output_collision_skip() {
        let dir = tempfile::tempdir().unwrap();
        let p = TranscodeParams {
            out_dir: dir.path().to_path_buf(),
            title: "t".into(),
            filename_template: "纯标题".into(),
            container: "mkv".into(),
            collision_policy: "skip".into(),
            ..mk()
        };
        let a = p.output_path().unwrap();
        std::fs::write(&a, b"x").unwrap();
        assert!(p.output_path().is_err());
    }

    fn mk() -> TranscodeParams {
        TranscodeParams {
            input: "x.mp4".into(),
            out_dir: PathBuf::new(),
            title: "t".into(),
            filename_template: "纯标题".into(),
            container: "mp4".into(),
            encoder_mode: "libx265".into(),
            low_power: false,
            max_w: 0,
            max_h: 0,
            brcap_kbps: None,
            normalize_audio: false,
            max_gain_db: 24.0,
            rot_angle: RotAngle::ZERO,
            keep_cover: true,
            collision_policy: "auto_inc".into(),
        }
    }

    #[test]
    fn vf_rotate_and_scale() {
        assert_eq!(build_vf(RotAngle::ZERO, 0, 0), None);
        let vf = build_vf(RotAngle::from_degrees(90), 1920, 1080).unwrap();
        assert!(vf.starts_with("transpose=1,"), "{}", vf);
        let vf = build_vf(RotAngle::from_degrees(270), 0, 0).unwrap();
        assert_eq!(vf, "transpose=2");
    }

    #[test]
    fn parse_encoders_detects_hw() {
        let text = [
            " V....D hevc_qsv            HEVC (Intel Quick Sync Video acceleration)",
            " V....D hevc_nvenc          HEVC (NVIDIA NVENC)",
            " V....D libx265             libx265 H.265 / HEVC",
        ]
        .join("\n");
        let hw = parse_encoders_output(&text);
        assert!(hw.qsv && hw.nvenc && !hw.amf);
        assert!(!parse_encoders_output("V....D libx265").qsv);
    }

    #[test]
    fn parse_us() {
        assert_eq!(parse_out_time_us("out_time_us=1234567"), Some(1234567));
        assert_eq!(parse_out_time_us("out_time_ms=999"), Some(999000));
        assert_eq!(parse_out_time_us("progress=end"), None);
    }
}
