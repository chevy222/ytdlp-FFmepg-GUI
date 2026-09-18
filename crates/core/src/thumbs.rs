//! 封面缩略图：URL 解析结果下载 / 本地文件与下载产物抽帧。
//! 统一落在 `<exe 同级>\config\cache\thumbs\<id>.jpg`，前端经 asset 协议展示。

use crate::exec::{ChildGuard, Tool, ToolResolver};
use std::path::{Path, PathBuf};

/// 从远程缩略图 URL 下载到 dest（如 yt-dlp 的 thumbnail）。
pub fn save_remote_thumb(url: &str, dest: &Path) -> Result<(), String> {
    ensure_parent(dest).map_err(|e| e.to_string())?;
    let tmp = dest.with_extension("tmp.jpg");
    let tmp_str = tmp.to_string_lossy().into_owned();
    let mut cmd = std::process::Command::new("curl");
    cmd.args(["-L", "--fail", "-sS", "-o", &tmp_str, url]);
    // GUI 程序启动控制台子进程会弹出一个黑窗（一闪而过）；这里与其它调用点
    // 保持一致，显式隐藏控制台。
    crate::exec::hide_console(&mut cmd);
    let output = cmd
        .output()
        .map_err(|e| format!("无法调用 curl：{e}"))?;
    if !output.status.success() || !tmp.is_file() {
        let _ = std::fs::remove_file(&tmp);
        return Err("缩略图下载失败".into());
    }
    let size = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        let _ = std::fs::remove_file(&tmp);
        return Err("缩略图为空".into());
    }
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
    Ok(())
}

/// 用 ffmpeg 从视频文件抽取一帧做封面（-ss 0.5 首帧附近，等比缩放 ≤360 宽）。
pub fn extract_thumb(resolver: &ToolResolver, src: &Path, dest: &Path) -> Result<(), String> {
    ensure_parent(dest).map_err(|e| e.to_string())?;
    let mut cmd = resolver
        .command(Tool::Ffmpeg)
        .map_err(|e| e.to_string())?;
    cmd.args(["-y", "-ss", "0.5", "-i"])
        .arg(src)
        .args(["-frames:v", "1", "-vf", "scale=360:-2", "-q:v", "3"])
        .arg(dest);
    let child = ChildGuard::spawn(&mut cmd).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() || !dest.is_file() {
        return Err(format!("抽帧失败：{}", crate::exec::decode_text(&out.stderr)));
    }
    Ok(())
}

fn ensure_parent(dest: &Path) -> std::io::Result<()> {
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p)?;
    }
    Ok(())
}

/// thumb 目录 + 条目文件路径。
pub fn thumb_path(cache_dir: &Path, id: &str) -> PathBuf {
    cache_dir.join("thumbs").join(format!("{id}.jpg"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_path_layout() {
        let p = thumb_path(Path::new("/x/config/cache"), "abc");
        assert_eq!(p, Path::new("/x/config/cache/thumbs/abc.jpg"));
    }

    #[test]
    fn extract_thumb_bad_source() {
        let resolver = ToolResolver::default();
        let err = extract_thumb(
            &resolver,
            Path::new("no-such-file.mp4"),
            Path::new("/tmp/no-thumb.jpg"),
        );
        assert!(err.is_err(), "源不存在应报错");
    }
}
