//! 封面缩略图：URL 解析结果下载 / 本地文件与下载产物抽帧。
//! 统一落在 `<exe 同级>\config\cache\thumbs\<id>.jpg`，前端经 asset 协议展示。

use crate::exec::{ChildGuard, Tool, ToolResolver};
use std::io::Read;
use std::path::{Path, PathBuf};

/// 从远程缩略图 URL 下载到 dest（如 yt-dlp 的 thumbnail）。
pub fn save_remote_thumb(url: &str, dest: &Path) -> Result<(), String> {
    ensure_parent(dest).map_err(|e| e.to_string())?;
    let resp = ureq::get(url)
        .call()
        .map_err(|e| format!("缩略图下载失败：{e}"))?;
    let mut body = Vec::new();
    let mut reader = resp.into_reader();
    reader
        .read_to_end(&mut body)
        .map_err(|e| format!("缩略图读取失败：{e}"))?;
    if body.is_empty() {
        return Err("缩略图为空".into());
    }
    let tmp = dest.with_extension("tmp.jpg");
    std::fs::write(&tmp, &body).map_err(|e| e.to_string())?;
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
