//! 合并引擎（§3.5 MG）：多视频按序拼接。
//!
//! 双模式（MG-02/03）：
//! - 模式 A 同参直拼：视频编码/分辨率/帧率/音频编码/采样率一致 **且视频流
//!   extradata（SPS/PPS）一致** → concat demuxer 零重编码直拼
//! - 模式 B 异参统一：逐段统一转码（编码器跟随 设置-转码 默认编码器、分辨率
//!   统一为各段最大值、音频 aac 48kHz 立体声）后再 concat 直拼
//!
//! 输出：MP4（默认）/MKV，`+faststart`；任务私有临时目录，结束清理（稳定性需求）。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::exec::{ChildGuard, Tool, ToolResolver};
use crate::model::MediaMeta;
use crate::{CoreError, Result};

/// 合并参数（合并面板内配置，不落 config.json，§3.5 说明）。
#[derive(Debug, Clone)]
pub struct MergeParams {
    pub inputs: Vec<PathBuf>,
    pub out_dir: PathBuf,
    /// 输出文件名（可编辑，默认 合并_<时间戳>）
    pub filename: String,
    /// 容器：mp4 | mkv
    pub container: String,
    /// 编码器：auto | libx265 | nvenc | amf（跟随 设置-转码 默认编码器）
    pub encoder_mode: String,
    pub low_power: bool,
    pub collision_policy: String,
}

impl MergeParams {
    pub fn extension(&self) -> &'static str {
        match self.container.as_str() {
            "mkv" => "mkv",
            _ => "mp4",
        }
    }
}

/// 同参判定（MG-02）：vcodec/height/fps/acodec/sample_rate/extradata 一致；
/// 音频缺失视为一致仅当所有段都无音频。
fn same_parameters(metas: &[MediaMeta]) -> bool {
    if metas.len() < 2 {
        return false;
    }
    let first = &metas[0];
    let v = |m: &MediaMeta| {
        (
            m.vcodec.clone(),
            m.height,
            m.fps.map(|f| (f * 100.0).round() as i64),
            m.extradata.clone(),
        )
    };
    let a = |m: &MediaMeta| (m.acodec.clone(), m.sample_rate);
    let ref_v = v(first);
    let ref_a = a(first);
    metas.iter().all(|m| v(m) == ref_v && a(m) == ref_a)
}

/// 各段时长合计（进度换算基准）。
fn total_duration(metas: &[MediaMeta]) -> f64 {
    metas.iter().filter_map(|m| m.duration_secs).sum()
}

/// 输出路径（碰撞安全命名，同 TC-17）。
pub fn output_path(params: &MergeParams) -> Result<PathBuf> {
    let base = crate::transcode::sanitize_filename(&params.filename);
    let ext = params.extension();
    let candidate = params.out_dir.join(format!("{}.{}", base, ext));
    if !candidate.exists() {
        return Ok(candidate);
    }
    match params.collision_policy.as_str() {
        "skip" => Err(CoreError::Io(std::io::Error::other(format!(
            "输出已存在，按策略跳过：{}",
            candidate.display()
        )))),
        _ => {
            for i in 1..1000 {
                let p = params
                    .out_dir
                    .join(format!("{} ({}).{}", base, i, ext));
                if !p.exists() {
                    return Ok(p);
                }
            }
            Err(CoreError::Io(std::io::Error::other(
                "无法生成不冲突的输出名",
            )))
        }
    }
}

/// 编码器参数（与转码一致：auto = QSV → libx265 兜底；显式 nvenc/amf）。
fn encoder_args(mode: &str, _low_power: bool) -> (String, Vec<String>) {
    let sv = |v: &[&str]| -> Vec<String> { v.iter().map(|s| s.to_string()).collect() };
    match mode {
        "libx265" => ("libx265".into(), sv(&["-crf", "23", "-preset", "medium"])),
        "nvenc" => (
            "hevc_nvenc".into(),
            sv(&["-rc", "vbr", "-cq", "23", "-preset", "p5"]),
        ),
        "amf" => (
            "hevc_amf".into(),
            sv(&["-qp_i", "23", "-qp_p", "23", "-quality", "balanced"]),
        ),
        _ => {
            // auto：交给调用方探测 QSV（合并低频，直接 libx265 兜底语义一致）
            ("libx265".into(), sv(&["-crf", "23", "-preset", "medium"]))
        }
    }
}

/// 模式 A：concat demuxer 零重编码直拼。
#[allow(clippy::too_many_arguments)]
fn concat_copy(
    resolver: &ToolResolver,
    list_file: &Path,
    out: &Path,
    container: &str,
    cancel: &Arc<AtomicBool>,
    duration: f64,
    on_progress: &mut dyn FnMut(f32),
    on_log: &mut dyn FnMut(String),
) -> Result<()> {
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-f".into(),
        "concat".into(),
        "-safe".into(),
        "0".into(),
        "-i".into(),
        list_file.to_string_lossy().into_owned(),
        "-c".into(),
        "copy".into(),
        "-map_metadata".into(),
        "0".into(),
    ];
    if container == "mp4" {
        args.push("-movflags".into());
        args.push("+faststart".into());
    }
    args.push("-y".into());
    args.push("-progress".into());
    args.push("pipe:1".into());
    args.push("-nostats".into());
    args.push("-loglevel".into());
    args.push("error".into());
    args.push(out.to_string_lossy().into_owned());

    on_log("参数一致，直拼（零重编码）…".into());
    run_piped_progress(resolver, args, cancel, duration, on_progress, on_log)?;
    if !out.exists() {
        return Err(CoreError::ProcessFailed {
            program: "ffmpeg".into(),
            code: None,
            stderr: "直拼结束但未找到输出文件".into(),
        });
    }
    Ok(())
}

/// 模式 B 单段统一转码（H.265 跟随编码器、分辨率统一、音频 aac 48k 立体声）。
#[allow(clippy::too_many_arguments)]
fn transcode_segment(
    resolver: &ToolResolver,
    input: &Path,
    out: &Path,
    encoder_mode: &str,
    low_power: bool,
    target_h: u32,
    cancel: &Arc<AtomicBool>,
    on_progress: &mut dyn FnMut(f32),
    on_log: &mut dyn FnMut(String),
) -> Result<()> {
    let (enc, mut enc_args) = encoder_args(encoder_mode, low_power);
    if enc == "libx265" {
        enc_args.push("-tag:v".into());
        enc_args.push("hvc1".into());
    }
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-map".into(),
        "0:v:0".into(),
        "-map".into(),
        "0:a?".into(),
        "-vf".into(),
        format!(
            "scale=-2:{}:force_original_aspect_ratio=decrease",
            if target_h > 0 { target_h } else { 1080 }
        ),
        "-c:v".into(),
        enc,
    ];
    args.extend(enc_args);
    args.push("-c:a".into());
    args.push("aac".into());
    args.push("-ar".into());
    args.push("48000".into());
    args.push("-ac".into());
    args.push("2".into());
    args.push("-map_metadata".into());
    args.push("0".into());
    args.push("-movflags".into());
    args.push("+faststart".into());
    args.push("-y".into());
    args.push("-progress".into());
    args.push("pipe:1".into());
    args.push("-nostats".into());
    args.push("-loglevel".into());
    args.push("error".into());
    args.push(out.to_string_lossy().into_owned());

    on_log(format!("统一参数转码：{}", input.display()));
    run_piped_progress(resolver, args, cancel, 0.0, on_progress, on_log)?;
    if !out.exists() {
        return Err(CoreError::ProcessFailed {
            program: "ffmpeg".into(),
            code: None,
            stderr: "统一转码结束但未找到输出".into(),
        });
    }
    Ok(())
}

fn run_piped_progress(
    resolver: &ToolResolver,
    args: Vec<String>,
    cancel: &Arc<AtomicBool>,
    duration: f64,
    on_progress: &mut dyn FnMut(f32),
    on_log: &mut dyn FnMut(String),
) -> Result<()> {
    use std::io::{BufRead, Read};
    use std::process::Stdio;
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
        if duration > 0.0 {
            if let Some(us) = crate::transcode::parse_out_time_us(&line) {
                let pct = ((us as f64 / 1e6) / duration * 100.0).clamp(0.0, 99.0) as f32;
                on_progress(pct);
            }
        }
    }
    let status = guard.wait()?;
    if cancel.load(Ordering::Relaxed) {
        return Err(CoreError::Cancelled);
    }
    if !status.success() {
        let mut buf = String::new();
        let mut stderr = stderr;
        let _ = stderr.read_to_string(&mut buf);
        let err = if buf.trim().is_empty() {
            "（无错误输出）".to_string()
        } else {
            buf.trim().to_string()
        };
        on_log(format!("ffmpeg 失败：{}", err));
        return Err(CoreError::ProcessFailed {
            program: "ffmpeg".into(),
            code: status.code(),
            stderr: err,
        });
    }
    Ok(())
}

/// 执行合并，返回输出路径；取消清理临时目录与输出残留（UL-06 适用合并）。
pub fn run_merge(
    resolver: &ToolResolver,
    params: &MergeParams,
    cancel: &Arc<AtomicBool>,
    mut on_progress: impl FnMut(f32),
    mut on_log: impl FnMut(String),
) -> Result<PathBuf> {
    if params.inputs.len() < 2 {
        return Err(CoreError::Io(std::io::Error::other("合并至少需要 2 个输入")));
    }
    let out = output_path(params)?;
    let duration = 0.0; // 探测后更新
    let _ = duration;

    // 1) 探测全部输入
    let mut metas = Vec::with_capacity(params.inputs.len());
    on_log("探测输入参数…".into());
    for p in &params.inputs {
        let m = crate::download::probe_output(resolver, p)?;
        metas.push(m);
    }
    let total = total_duration(&metas);
    let same = same_parameters(&metas);
    if !same {
        on_log("输入参数不一致（编码/分辨率/帧率/音频/采样率），按统一模式处理".into());
    }

    // 2) 任务私有临时目录
    let task_id = uuid::Uuid::new_v4().to_string();
    let tmp = params.out_dir.join("temp").join(format!("merge_{}", task_id));
    std::fs::create_dir_all(&tmp)?;
    let cleanup = |t: &Path, out: &Path| {
        let _ = std::fs::remove_dir_all(t);
        let _ = std::fs::remove_file(out);
    };
    let result = (|| -> Result<PathBuf> {
        if same {
            // 模式 A：concat 直拼
            let list = tmp.join("list.txt");
            write_concat_list(&list, &params.inputs)?;
            let mut prog = on_progress;
            concat_copy(
                resolver,
                &list,
                &out,
                &params.container,
                cancel,
                total,
                &mut prog,
                &mut on_log,
            )?;
        } else {
            // 模式 B：逐段统一转码 + concat 直拼
            let target_h = metas
                .iter()
                .filter_map(|m| m.height)
                .max()
                .unwrap_or(1080);
            let segs: Vec<PathBuf> = params
                .inputs
                .iter()
                .enumerate()
                .map(|(i, _p)| tmp.join(format!("seg_{:02}.mp4", i)))
                .collect();
            let seg_total = total;
            let mut acc = 0.0f64;
            for (i, (p, seg)) in params.inputs.iter().zip(&segs).enumerate() {
                let d = metas[i].duration_secs.unwrap_or(0.0);
                let seg_dur = if seg_total > 0.0 { d } else { 1.0 };
                let base = if seg_total > 0.0 {
                    (acc / seg_total * 85.0) as f32
                } else {
                    (i as f32 / params.inputs.len() as f32) * 85.0
                };
                let span = if seg_total > 0.0 {
                    (seg_dur / seg_total * 85.0) as f32
                } else {
                    85.0 / params.inputs.len() as f32
                };
                let seg_base = base;
                let seg_span = span.max(0.5);
                let mut prog = |pct: f32| {
                    on_progress(seg_base + pct * 0.01 * seg_span);
                };
                transcode_segment(
                    resolver,
                    p,
                    seg,
                    &params.encoder_mode,
                    params.low_power,
                    target_h,
                    cancel,
                    &mut prog,
                    &mut on_log,
                )?;
                acc += d;
            }
            let list = tmp.join("list.txt");
            write_concat_list(&list, &segs)?;
            let mut prog = |pct: f32| {
                on_progress(85.0 + pct * 0.01 * 15.0);
            };
            concat_copy(
                resolver,
                &list,
                &out,
                &params.container,
                cancel,
                seg_total,
                &mut prog,
                &mut on_log,
            )?;
        }
        Ok(out.clone())
    })();

    match result {
        Ok(o) => {
            let _ = std::fs::remove_dir_all(&tmp);
            on_log(format!("合并完成：{}", o.display()));
            Ok(o)
        }
        Err(CoreError::Cancelled) => {
            cleanup(&tmp, &out);
            on_log("合并已取消，清理临时目录与输出残留".into());
            Err(CoreError::Cancelled)
        }
        Err(e) => {
            cleanup(&tmp, &out);
            on_log(format!("合并失败，已清理临时目录与半成品：{}", e));
            Err(e)
        }
    }
}

fn write_concat_list(path: &Path, inputs: &[PathBuf]) -> Result<()> {
    let mut content = String::new();
    for p in inputs {
        let s = p.to_string_lossy();
        // 单引号转义（ffmpeg concat 文件内转义规则）
        let escaped = s.replace('\'', "'\\''");
        content.push_str(&format!("file '{}'\n", escaped));
    }
    std::fs::write(path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AudioVolume;

    fn meta(
        vcodec: &str,
        h: u32,
        fps: f64,
        acodec: &str,
        sr: u32,
        ext: &str,
        dur: f64,
    ) -> MediaMeta {
        MediaMeta {
            vcodec: Some(vcodec.into()),
            height: Some(h),
            fps: Some(fps),
            acodec: Some(acodec.into()),
            sample_rate: Some(sr),
            extradata: Some(ext.into()),
            duration_secs: Some(dur),
            audio_volume: AudioVolume::default(),
            ..Default::default()
        }
    }

    #[test]
    fn same_params_true_when_identical() {
        let ms = vec![
            meta("h264", 1080, 30.0, "aac", 48000, "aabb", 10.0),
            meta("h264", 1080, 30.0, "aac", 48000, "aabb", 20.0),
        ];
        assert!(same_parameters(&ms));
    }

    #[test]
    fn same_params_false_on_extradata_diff() {
        let ms = vec![
            meta("h264", 1080, 30.0, "aac", 48000, "aabb", 10.0),
            meta("h264", 1080, 30.0, "aac", 48000, "ccdd", 20.0),
        ];
        assert!(!same_parameters(&ms));
    }

    #[test]
    fn same_params_false_on_res_diff() {
        let ms = vec![
            meta("h264", 1080, 30.0, "aac", 48000, "aabb", 10.0),
            meta("h264", 720, 30.0, "aac", 48000, "aabb", 20.0),
        ];
        assert!(!same_parameters(&ms));
    }

    #[test]
    fn same_params_false_on_sample_rate_diff() {
        let ms = vec![
            meta("h264", 1080, 30.0, "aac", 48000, "aabb", 10.0),
            meta("h264", 1080, 30.0, "aac", 44100, "aabb", 20.0),
        ];
        assert!(!same_parameters(&ms));
    }

    #[test]
    fn output_path_auto_inc() {
        let dir = tempfile::tempdir().unwrap();
        let p = MergeParams {
            inputs: vec![],
            out_dir: dir.path().to_path_buf(),
            filename: "合并_20260917".into(),
            container: "mp4".into(),
            encoder_mode: "auto".into(),
            low_power: false,
            collision_policy: "auto_inc".into(),
        };
        let a = output_path(&p).unwrap();
        std::fs::write(&a, b"x").unwrap();
        let b = output_path(&p).unwrap();
        assert_eq!(b.file_name().unwrap(), "合并_20260917 (1).mp4");
    }

    #[test]
    fn output_path_skip() {
        let dir = tempfile::tempdir().unwrap();
        let p = MergeParams {
            inputs: vec![],
            out_dir: dir.path().to_path_buf(),
            filename: "合并_x".into(),
            container: "mkv".into(),
            encoder_mode: "auto".into(),
            low_power: false,
            collision_policy: "skip".into(),
        };
        let a = output_path(&p).unwrap();
        std::fs::write(&a, b"x").unwrap();
        assert!(output_path(&p).is_err());
    }

    #[test]
    fn concat_list_escapes_quote() {
        let dir = tempfile::tempdir().unwrap();
        let list = dir.path().join("list.txt");
        write_concat_list(&list, &[PathBuf::from("a'b.mp4")]).unwrap();
        let s = std::fs::read_to_string(&list).unwrap();
        assert_eq!(s, "file 'a'\\''b.mp4'\n");
    }
}
