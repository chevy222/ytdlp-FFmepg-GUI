//! 统一列表条目 MediaItem 与状态机（需求文档 §7.1 / §3.1）。

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

/// 条目状态（与 UI"已就绪"对应的内部枚举名为 `Ready`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Status {
    /// 解析中（URL 取信息 / 本地 ffprobe 探测）
    Probing,
    /// 已就绪（已解析完成、可执行下载/转码/合并动作；UI 术语"已就绪"）
    Ready,
    /// 下载中
    Downloading,
    /// 后处理中（下载产物画质上限/封面/元数据/音量归一化）
    PostProcessing,
    /// 转码中
    Transcoding,
    /// 合并中
    Merging,
    /// 已完成（含下载完成）
    Done,
    /// 失败（含解析失败/下载失败/转码失败）
    Failed,
    /// 已取消（取消时清理 temp 与输出目录残留）
    Canceled,
    /// 需要登录（可恢复：WebView2 登录后续传）
    NeedLogin,
}

impl Status {
    /// 是否为终态（不可再执行动作，仅可删除/重试）。
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Canceled)
    }

    /// 是否为可恢复状态（失败分级：可恢复给操作指引）。
    pub fn is_recoverable(self) -> bool {
        matches!(self, Self::NeedLogin | Self::Failed)
    }

    /// UI 文案（状态用颜色 + 文字双表达，见 §6.3）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Probing => "解析中",
            Self::Ready => "已就绪",
            Self::Downloading => "下载中",
            Self::PostProcessing => "后处理中",
            Self::Transcoding => "转码中",
            Self::Merging => "合并中",
            Self::Done => "已完成",
            Self::Failed => "失败",
            Self::Canceled => "已取消",
            Self::NeedLogin => "需要登录",
        }
    }

    /// 状态分组（筛选下拉：全部/解析中/已就绪/处理中/已完成/失败/需要登录）。
    pub fn group(self) -> StatusGroup {
        match self {
            Self::Downloading | Self::PostProcessing | Self::Transcoding | Self::Merging => {
                StatusGroup::Working
            }
            _ => StatusGroup::Single(self),
        }
    }
}

/// 筛选分组（下载中/后处理/转码/合并归入"处理中"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusGroup {
    All,
    Single(Status),
    Working,
}

/// 旋转角度（0°/90°/180°/270°，封面旋转箭头指定，随条目保存，转码生效）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RotAngle(u16);

impl RotAngle {
    pub const ZERO: Self = Self(0);

    pub fn from_degrees(deg: u16) -> Self {
        Self((deg / 90) % 4 * 90)
    }

    pub fn degrees(self) -> u16 {
        self.0
    }

    /// 顺时针旋转 90°。
    pub fn rotate_cw(self) -> Self {
        Self::from_degrees(self.0 + 90)
    }

    /// 逆时针旋转 90°。
    pub fn rotate_ccw(self) -> Self {
        Self::from_degrees(self.0 + 270)
    }
}

impl Default for RotAngle {
    fn default() -> Self {
        Self::ZERO
    }
}

/// 条目来源类型（统一列表：URL 任务 / 本地文件 / 转码产物 / 合并产物）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ItemKind {
    /// URL 下载任务
    UrlTask,
    /// 本地文件/目录（添加后先解析）
    LocalFile,
    /// 转码产物（回到列表）
    TranscodeOut,
    /// 合并产物（回到列表）
    MergeOut,
}

impl ItemKind {
    pub fn is_download_source(self) -> bool {
        matches!(self, Self::UrlTask)
    }
}

/// 音频音量探测结果（volumedetect，供转码增益决策，§MD-02/MD-06/TC-07）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioVolume {
    pub mean_volume_db: Option<f32>,
    pub max_volume_db: Option<f32>,
}

/// 解析元数据（ffprobe / yt-dlp -j 结果，画质/格式列 11 项字段，§6.2）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaMeta {
    pub title: Option<String>,
    pub duration_secs: Option<f64>,
    /// 容器格式，如 MP4 / MKV
    pub container: Option<String>,
    /// 分辨率（短边），如 2160/1080/720
    pub height: Option<u32>,
    /// 视频编码器，如 HEVC / H.264 / AV1
    pub vcodec: Option<String>,
    /// 视频码率 kbps
    pub vbitrate_kbps: Option<u32>,
    /// 帧率
    pub fps: Option<f64>,
    /// 音频编码器
    pub acodec: Option<String>,
    /// 音频码率 kbps
    pub abitrate_kbps: Option<u32>,
    /// 音轨数
    pub audio_tracks: Option<u32>,
    /// 最大音量（转码增益依据）
    pub audio_volume: AudioVolume,
    /// 文件大小字节
    pub size_bytes: Option<u64>,
    /// 是否含封面
    pub has_cover: bool,
    /// 视频流 extradata（SPS/PPS 等，hex）——合并直拼硬性条件（MG-02）
    #[serde(default)]
    pub extradata: Option<String>,
    /// 首音频流采样率 Hz
    #[serde(default)]
    pub sample_rate: Option<u32>,
    /// URL 解析所得清晰度/格式列表（MD-01）
    pub formats: Vec<String>,
    /// 结构化下载格式列表（DL-02 格式选择；含 format_id 供下载使用）
    #[serde(default)]
    pub download_formats: Vec<DownloadFormat>,
}

/// 下载格式（DL-02：清晰度/编码/大小/帧率/码率，格式弹窗展示项）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DownloadFormat {
    pub format_id: String,
    /// 可读标签（如 `1080P · MP4 · H.264 · 5.2MB · 30fps`）
    pub label: String,
    pub height: Option<u32>,
    pub ext: Option<String>,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    pub filesize_bytes: Option<u64>,
    pub fps: Option<f64>,
    pub tbr_kbps: Option<u32>,
    /// 附加说明（如"需登录"、"AV1"）
    pub note: Option<String>,
    /// 是否仅音频格式
    #[serde(default)]
    pub audio_only: bool,
}

impl DownloadFormat {
    /// 生成可读标签（清晰度 · 容器 · 编码 · 大小 · 帧率）。
    pub fn make_label(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(h) = self.height {
            parts.push(format!("{}P", h));
        } else if self.audio_only {
            parts.push("仅音频".into());
        } else {
            parts.push("自适应".into());
        }
        if let Some(e) = &self.ext {
            parts.push(e.to_uppercase());
        }
        if let Some(c) = &self.vcodec {
            let cl = c.to_lowercase();
            let short = match cl.as_str() {
                "av01" => "AV1",
                "avc1" | "h264" | "h.264" => "H.264",
                "hevc" | "h265" | "h.265" => "H.265",
                "vp9" => "VP9",
                other => other,
            };
            parts.push(short.to_string());
        }
        if let Some(a) = &self.acodec {
            let al = a.to_lowercase();
            let short = match al.as_str() {
                "mp4a" | "aac" => "AAC",
                "opus" => "Opus",
                other => other,
            };
            parts.push(short.to_string());
        }
        if let Some(f) = self.filesize_bytes {
            parts.push(human_size(f));
        }
        if let Some(f) = self.fps {
            parts.push(format!("{:.0}fps", f));
        }
        parts.join(" · ")
    }
}

impl MediaMeta {
    /// 画质/格式列渲染（11 项字段，自动换行；需求文档 §6.2 / C8）。
    pub fn quality_line(&self) -> String {
        let mut parts = Vec::new();
        if let Some(c) = &self.container {
            parts.push(c.clone());
        }
        if let Some(h) = self.height {
            parts.push(format!("{}P", h));
        }
        if let Some(c) = &self.vcodec {
            parts.push(c.clone());
        }
        if let Some(b) = self.vbitrate_kbps {
            parts.push(format!("{}Mbps", (b + 500) / 1000));
        }
        if let Some(f) = self.fps {
            parts.push(format!("{:.0}fps", f));
        }
        if let Some(c) = &self.acodec {
            parts.push(c.clone());
        }
        if let Some(b) = self.abitrate_kbps {
            parts.push(format!("{}k", b));
        }
        if let Some(t) = self.audio_tracks {
            parts.push(format!("{}轨", t));
        }
        if let Some(v) = self.audio_volume.max_volume_db {
            parts.push(format!("{:.1}dB", v));
        }
        if let Some(d) = self.duration_secs {
            let m = (d / 60.0) as u64;
            let s = (d as u64) % 60;
            parts.push(format!("{:02}:{:02}", m, s));
        }
        if let Some(b) = self.size_bytes {
            parts.push(human_size(b));
        }
        parts.join(" · ")
    }
}

/// 人类可读大小（KB/MB/GB）。
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1}GB", b / GB)
    } else if b >= MB {
        format!("{:.1}MB", b / MB)
    } else if b >= KB {
        format!("{:.1}KB", b / KB)
    } else {
        format!("{}B", bytes)
    }
}

/// 统一列表条目（§7.1）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    pub id: String,
    pub kind: ItemKind,
    pub title: String,
    pub path: Option<String>,
    pub url: Option<String>,
    pub site: Option<String>,
    pub host: Option<String>,
    pub status: Status,
    pub percent: f32,
    pub error: Option<String>,
    /// 下载进度附加信息（§7.1 speed/eta/file）
    #[serde(default)]
    pub speed: Option<String>,
    #[serde(default)]
    pub eta: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    /// 最近日志行（≤300 行，按条目查看，§3.8/§6.2）
    #[serde(default)]
    pub log: VecDeque<String>,
    #[serde(default)]
    pub meta: MediaMeta,
    /// 封面缩略图本地路径（config/cache/thumbs/<id>.jpg）
    #[serde(default)]
    pub thumb: Option<String>,
    pub rot_angle: RotAngle,
    /// 下载任务专属：选中格式
    #[serde(default)]
    pub format_id: Option<String>,
    #[serde(default)]
    pub audio_only: bool,
    /// 队列持久化标记
    #[serde(default)]
    pub persist: bool,
    #[serde(default)]
    pub updated_at: String,
    /// 时间范围下载（DL-12）：起止 "HH:MM:SS"（yt-dlp --download-sections）
    #[serde(default)]
    pub sections: Option<(String, String)>,
}

impl MediaItem {
    pub fn new(kind: ItemKind, title: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            kind,
            title,
            path: None,
            url: None,
            site: None,
            host: None,
            status: Status::Probing,
            percent: 0.0,
            sections: None,
            error: None,
            speed: None,
            eta: None,
            file: None,
            log: VecDeque::new(),
            meta: MediaMeta::default(),
            thumb: None,
            rot_angle: RotAngle::ZERO,
            format_id: None,
            audio_only: false,
            persist: false,
            updated_at: String::new(),
        }
    }

    pub fn from_url(url: String) -> Self {
        let mut it = Self::new(ItemKind::UrlTask, url.clone());
        it.url = Some(url);
        it
    }

    pub fn from_path(path: String) -> Self {
        let title = std::path::Path::new(&path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        let mut it = Self::new(ItemKind::LocalFile, title);
        it.path = Some(path);
        it
    }

    /// 日志追加，保留最近 `MAX_LOG_LINES` 行（300）。
    pub fn push_log(&mut self, line: impl Into<String>) {
        const MAX_LOG_LINES: usize = 300;
        self.log.push_back(line.into());
        while self.log.len() > MAX_LOG_LINES {
            self.log.pop_front();
        }
    }

    /// 名称副行：站点/本地 · URL 或本地路径（§6.1）。
    pub fn subline(&self) -> String {
        if let Some(url) = &self.url {
            let site = self.site.as_deref().unwrap_or("站点");
            format!("{} · {}", site, url)
        } else if let Some(p) = &self.path {
            format!("本地文件 · {}", p)
        } else {
            String::new()
        }
    }
}

/// 状态机：合法迁移校验（§3.1 UL-06）。
///
/// 允许的迁移：
/// ```text
/// Probing -> Ready | Failed | Canceled | NeedLogin
/// Ready   -> Downloading | Transcoding | Merging | Canceled
/// Downloading -> PostProcessing | Failed | Canceled | NeedLogin
/// PostProcessing -> Done | Failed | Canceled
/// Transcoding -> Done | Failed | Canceled
/// Merging -> Done | Failed | Canceled
/// Done/Failed/Canceled -> Probing (重试/重新解析)
/// Failed -> NeedLogin (登录后转解析)
/// NeedLogin -> Probing (登录完成自动重新解析)
/// ```
// 白名单迁移表用显式 match 保持可读，禁用 matches! 风格提示。
#[allow(clippy::match_like_matches_macro)]
pub fn transition(from: Status, to: Status) -> Result<Status, crate::CoreError> {
    let ok = match (from, to) {
        (Status::Probing, Status::Ready)
        | (Status::Probing, Status::Failed)
        | (Status::Probing, Status::Canceled)
        | (Status::Probing, Status::NeedLogin)
        | (Status::Ready, Status::Downloading)
        | (Status::Ready, Status::Transcoding)
        | (Status::Ready, Status::Merging)
        | (Status::Ready, Status::Canceled)
        | (Status::Downloading, Status::PostProcessing)
        | (Status::Downloading, Status::Failed)
        | (Status::Downloading, Status::Canceled)
        | (Status::Downloading, Status::NeedLogin)
        | (Status::PostProcessing, Status::Done)
        | (Status::PostProcessing, Status::Failed)
        | (Status::PostProcessing, Status::Canceled)
        | (Status::Done, Status::Transcoding)
        | (Status::Done, Status::Merging)
        | (Status::Transcoding, Status::Done)
        | (Status::Transcoding, Status::Failed)
        | (Status::Transcoding, Status::Canceled)
        | (Status::Merging, Status::Done)
        | (Status::Merging, Status::Failed)
        | (Status::Merging, Status::Canceled)
        | (Status::Done, Status::Probing)
        | (Status::Failed, Status::Probing)
        | (Status::Failed, Status::NeedLogin)
        | (Status::Canceled, Status::Probing)
        | (Status::NeedLogin, Status::Probing) => true,
        _ => false,
    };
    if ok {
        Ok(to)
    } else {
        Err(crate::CoreError::InvalidTransition { from, to })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> MediaItem {
        MediaItem::new(ItemKind::LocalFile, "测试.mp4".into())
    }

    #[test]
    fn new_item_starts_probing() {
        assert_eq!(item().status, Status::Probing);
    }

    #[test]
    fn from_url_sets_kind_and_url() {
        let it = MediaItem::from_url("https://www.bilibili.com/video/BV1xx".into());
        assert_eq!(it.kind, ItemKind::UrlTask);
        assert_eq!(
            it.url.as_deref(),
            Some("https://www.bilibili.com/video/BV1xx")
        );
        assert!(it.status == Status::Probing);
    }

    #[test]
    fn from_path_uses_filename_as_title() {
        let it = MediaItem::from_path("D:/Videos/手机录像/VID_1.mp4".into());
        assert_eq!(it.kind, ItemKind::LocalFile);
        assert_eq!(it.title, "VID_1.mp4");
        assert_eq!(it.path.as_deref(), Some("D:/Videos/手机录像/VID_1.mp4"));
    }

    #[test]
    fn subline_url_uses_site_prefix() {
        let mut it = MediaItem::from_url("https://x.com/status/123".into());
        it.site = Some("X/Twitter".into());
        assert_eq!(it.subline(), "X/Twitter · https://x.com/status/123");
    }

    #[test]
    fn subline_path_uses_local_prefix() {
        let it = MediaItem::from_path("/tmp/a.mp4".into());
        assert_eq!(it.subline(), "本地文件 · /tmp/a.mp4");
    }

    #[test]
    fn log_keeps_last_300_lines() {
        let mut it = item();
        for i in 0..320 {
            it.push_log(format!("line {}", i));
        }
        assert_eq!(it.log.len(), 300);
        assert_eq!(it.log.front().map(String::as_str), Some("line 20"));
        assert_eq!(it.log.back().map(String::as_str), Some("line 319"));
    }

    #[test]
    fn rot_angle_cw_cycles() {
        let mut a = RotAngle::ZERO;
        a = a.rotate_cw();
        assert_eq!(a.degrees(), 90);
        a = a.rotate_cw();
        assert_eq!(a.degrees(), 180);
        a = a.rotate_cw();
        assert_eq!(a.degrees(), 270);
        a = a.rotate_cw();
        assert_eq!(a.degrees(), 0);
    }

    #[test]
    fn rot_angle_ccw() {
        assert_eq!(RotAngle::ZERO.rotate_ccw().degrees(), 270);
    }

    #[test]
    fn rot_angle_from_degrees_normalizes() {
        assert_eq!(RotAngle::from_degrees(540).degrees(), 180);
        assert_eq!(RotAngle::from_degrees(135).degrees(), 90);
    }

    #[test]
    fn terminal_statuses() {
        assert!(Status::Done.is_terminal());
        assert!(Status::Failed.is_terminal());
        assert!(Status::Canceled.is_terminal());
        assert!(!Status::Ready.is_terminal());
        assert!(!Status::Downloading.is_terminal());
    }

    #[test]
    fn recoverable_statuses() {
        assert!(Status::NeedLogin.is_recoverable());
        assert!(Status::Failed.is_recoverable());
        assert!(!Status::Done.is_recoverable());
    }

    #[test]
    fn status_groups_working() {
        assert_eq!(Status::Downloading.group(), StatusGroup::Working);
        assert_eq!(Status::Transcoding.group(), StatusGroup::Working);
        assert_eq!(Status::Merging.group(), StatusGroup::Working);
        assert_eq!(Status::PostProcessing.group(), StatusGroup::Working);
        assert_eq!(Status::Ready.group(), StatusGroup::Single(Status::Ready));
    }

    #[test]
    fn status_labels_chinese() {
        assert_eq!(Status::Ready.label(), "已就绪");
        assert_eq!(Status::NeedLogin.label(), "需要登录");
        assert_eq!(Status::Probing.label(), "解析中");
    }

    #[test]
    fn transition_probing_to_ready_ok() {
        assert_eq!(
            transition(Status::Probing, Status::Ready).unwrap(),
            Status::Ready
        );
    }

    #[test]
    fn transition_download_to_postprocess_ok() {
        assert_eq!(
            transition(Status::Downloading, Status::PostProcessing).unwrap(),
            Status::PostProcessing
        );
    }

    #[test]
    fn transition_transcode_to_done_ok() {
        assert_eq!(
            transition(Status::Transcoding, Status::Done).unwrap(),
            Status::Done
        );
    }

    #[test]
    fn transition_merge_to_done_ok() {
        assert_eq!(
            transition(Status::Merging, Status::Done).unwrap(),
            Status::Done
        );
    }

    #[test]
    fn transition_needlogin_to_probing_ok() {
        assert_eq!(
            transition(Status::NeedLogin, Status::Probing).unwrap(),
            Status::Probing
        );
    }

    #[test]
    fn transition_failed_to_needlogin_ok() {
        assert_eq!(
            transition(Status::Failed, Status::NeedLogin).unwrap(),
            Status::NeedLogin
        );
    }

    #[test]
    fn transition_terminal_to_probing_for_retry() {
        assert_eq!(
            transition(Status::Failed, Status::Probing).unwrap(),
            Status::Probing
        );
        assert_eq!(
            transition(Status::Canceled, Status::Probing).unwrap(),
            Status::Probing
        );
        assert_eq!(
            transition(Status::Done, Status::Probing).unwrap(),
            Status::Probing
        );
    }

    #[test]
    fn transition_done_to_transcoding_allowed() {
        // TC 语义：下载完成（Done）的产物可直接进入转码
        assert_eq!(
            transition(Status::Done, Status::Transcoding).unwrap(),
            Status::Transcoding
        );
    }

    #[test]
    fn transition_ready_to_probing_rejected() {
        assert!(transition(Status::Ready, Status::Probing).is_err());
    }

    #[test]
    fn transition_probing_to_done_rejected() {
        assert!(transition(Status::Probing, Status::Done).is_err());
    }

    #[test]
    fn quality_line_renders_11_fields() {
        let meta = MediaMeta {
            container: Some("MP4".into()),
            height: Some(2160),
            vcodec: Some("HEVC".into()),
            vbitrate_kbps: Some(12000),
            fps: Some(60.0),
            acodec: Some("AAC".into()),
            abitrate_kbps: Some(320),
            audio_tracks: Some(2),
            audio_volume: AudioVolume {
                max_volume_db: Some(-8.2),
                ..Default::default()
            },
            duration_secs: Some(332.0),
            size_bytes: Some(35_651_584),
            ..Default::default()
        };
        let line = meta.quality_line();
        assert_eq!(
            line,
            "MP4 · 2160P · HEVC · 12Mbps · 60fps · AAC · 320k · 2轨 · -8.2dB · 05:32 · 34.0MB"
        );
    }

    #[test]
    fn quality_line_empty_meta() {
        assert_eq!(MediaMeta::default().quality_line(), "");
    }

    #[test]
    fn human_size_formats() {
        assert_eq!(human_size(1024), "1.0KB");
        assert_eq!(human_size(1024 * 1024), "1.0MB");
        assert_eq!(human_size(35_651_584), "34.0MB");
        assert_eq!(human_size(2 * 1024 * 1024 * 1024), "2.0GB");
        assert_eq!(human_size(512), "512B");
    }

    #[test]
    fn meta_serde_roundtrip() {
        let meta = MediaMeta {
            height: Some(1080),
            formats: vec!["1080P".into(), "720P".into()],
            ..Default::default()
        };
        let json = serde_json::to_string(&meta).unwrap();
        let back: MediaMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(back.height, Some(1080));
        assert_eq!(back.formats.len(), 2);
    }

    #[test]
    fn item_serde_roundtrip_preserves_log_and_rot() {
        let mut it = item();
        it.status = Status::Ready;
        it.rot_angle = RotAngle::from_degrees(90);
        it.push_log("解析完成");
        it.meta.audio_volume.max_volume_db = Some(-8.2);
        let json = serde_json::to_string(&it).unwrap();
        let back: MediaItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, Status::Ready);
        assert_eq!(back.rot_angle.degrees(), 90);
        assert_eq!(back.log.front().map(String::as_str), Some("解析完成"));
        assert_eq!(back.meta.audio_volume.max_volume_db, Some(-8.2));
    }

    #[test]
    fn item_kind_download_source() {
        assert!(ItemKind::UrlTask.is_download_source());
        assert!(!ItemKind::LocalFile.is_download_source());
        assert!(!ItemKind::TranscodeOut.is_download_source());
    }
}
