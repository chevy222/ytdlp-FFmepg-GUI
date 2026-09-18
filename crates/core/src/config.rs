//! config.json 用户设置（§7.2 / §3.6）：原子写，损坏回退默认。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{paths::atomic_write_json, CoreError, Result};

/// 下载段（§3.6 下载分组 + §7.2）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadConfig {
    /// 画质上限（短边），超限自动降分辨率转码（DL-03/DL-04）
    #[serde(default)]
    pub max_h: u32,
    /// 下载高度硬上限（MAX_DL_H 语义）
    #[serde(default)]
    pub max_dl_h: u32,
    /// 并发分片数（默认 4）
    #[serde(default)]
    pub fragments: u32,
    /// 重试次数
    #[serde(default)]
    pub retries: u32,
    /// 仅音频默认
    #[serde(default)]
    pub audio_only: bool,
    /// 播放列表默认（默认关）
    #[serde(default)]
    pub playlist: bool,
    /// 嵌入封面/元数据
    #[serde(default)]
    pub embed_cover: bool,
    /// 文件名模板（纯标题/标题+ID/UP主-标题/日期-标题）
    #[serde(default)]
    pub filename_template: String,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            max_h: 1080,
            max_dl_h: 2160,
            fragments: 4,
            retries: 3,
            audio_only: false,
            playlist: false,
            embed_cover: true,
            filename_template: "纯标题".into(),
        }
    }
}

/// 转码段（§3.6 转码分组 + §7.2；x265 CRF 固定 23 不落配置项）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscodeConfig {
    #[serde(default)]
    pub max_w: u32,
    #[serde(default)]
    pub max_h: u32,
    /// 码率封顶 kbps
    #[serde(default)]
    pub brcap_kbps: Option<u32>,
    /// 兜底码率 kbps
    #[serde(default)]
    pub br_default_kbps: u32,
    /// 编码器模式：auto | libx265 | nvenc | amf
    #[serde(default)]
    pub force_encoder_mode: String,
    /// QSV low_power
    #[serde(default)]
    pub low_power: bool,
    /// 保留封面
    #[serde(default)]
    pub keep_cover: bool,
}

impl Default for TranscodeConfig {
    fn default() -> Self {
        Self {
            max_w: 1920,
            max_h: 1080,
            brcap_kbps: None,
            br_default_kbps: 8000,
            force_encoder_mode: "auto".into(),
            low_power: true,
            keep_cover: true,
        }
    }
}

/// 通用段（§3.6 通用分组：输出/音量/并发/检查更新/历史上限/清理解析缓存）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    /// 默认输出目录（默认桌面）
    #[serde(default)]
    pub default_output_dir: Option<String>,
    /// 碰撞命名策略：auto_inc | skip
    #[serde(default)]
    pub collision_policy: String,
    /// 音量归一化（下载后处理与转码共用）
    #[serde(default)]
    pub normalize_audio: bool,
    /// 音量增益上限 dB（默认 24）
    #[serde(default)]
    pub max_gain_db: f32,
    /// 并发任务数（全局：下载/转码/合并共享，默认 3）
    #[serde(default)]
    pub concurrency: u32,
    /// 启动时检查更新（默认开启）
    #[serde(default)]
    pub check_update: bool,
    /// 历史上限（默认 100，上限 200，N5 统一）
    #[serde(default)]
    pub history_limit: usize,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            default_output_dir: None,
            collision_policy: "auto_inc".into(),
            normalize_audio: true,
            max_gain_db: 24.0,
            concurrency: 3,
            check_update: true,
            history_limit: 100,
        }
    }
}

impl GeneralConfig {
    pub const HISTORY_LIMIT_MAX: usize = 200;
}

/// 依赖段（§3.6 依赖分组：路径输入框留空＝PATH，PO-Token 默认启用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependenciesConfig {
    #[serde(default)]
    pub yt_dlp_path: Option<String>,
    #[serde(default)]
    pub ffmpeg_path: Option<String>,
    #[serde(default)]
    pub ffprobe_path: Option<String>,
    #[serde(default)]
    pub deno_path: Option<String>,
    /// PO-Token 服务（YouTube 风控验证令牌，deno 跑 potoken 生成器），默认启用
    #[serde(default)]
    pub potoken_enabled: bool,
}

impl Default for DependenciesConfig {
    fn default() -> Self {
        Self {
            yt_dlp_path: None,
            ffmpeg_path: None,
            ffprobe_path: None,
            deno_path: None,
            potoken_enabled: true,
        }
    }
}

/// 网络段（§3.6 网络分组：代理地址 + 站点分流）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkConfig {
    #[serde(default)]
    pub proxy_url: String,
    /// 站点分流：站点 -> 是否走代理（其余直连）
    #[serde(default)]
    pub site_proxy: std::collections::HashMap<String, bool>,
}

impl NetworkConfig {
    /// 站点分流解析：返回该 URL 应使用的代理地址。
    /// - 未配置任何分流站点：全局 proxy_url（空则直连）
    /// - 已配置分流：命中站点且勾选 → proxy_url；其余一律直连
    pub fn resolve_proxy(&self, url: &str) -> Option<String> {
        if self.proxy_url.is_empty() {
            return None;
        }
        if self.site_proxy.is_empty() {
            return Some(self.proxy_url.clone());
        }
        let host = host_of(url)?;
        for (site, enabled) in &self.site_proxy {
            if site_matches(site, &host) {
                return if *enabled { Some(self.proxy_url.clone()) } else { None };
            }
        }
        None
    }
}

/// 从 URL 提取小写 host（去协议/端口/路径）。
fn host_of(url: &str) -> Option<String> {
    let u = url.trim();
    let after = u.split_once("://").map(|x| x.1).unwrap_or(u);
    let host = after.split(['/', '?', '#']).next()?.split(':').next()?.to_string();
    Some(host.to_lowercase())
}

/// 站点匹配：精确相等或子域后缀（bilibili.com 命中 www.bilibili.com）。
fn site_matches(site: &str, host: &str) -> bool {
    let site = site.trim().trim_start_matches('.').to_lowercase();
    if site.is_empty() {
        return false;
    }
    host == site || host.ends_with(&format!(".{site}"))
}

/// 根配置（§7.2）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub download: DownloadConfig,
    #[serde(default)]
    pub transcode: TranscodeConfig,
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub dependencies: DependenciesConfig,
    #[serde(default)]
    pub network: NetworkConfig,
}

impl AppConfig {
    /// 从文件加载；文件缺失返回默认；损坏则备份（config.json.corrupt-<ts>）后回退默认（§3.7 规则）。
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)?;
        match serde_json::from_str::<AppConfig>(&text) {
            Ok(c) => Ok(c),
            Err(e) => {
                let backup = path.with_extension(format!(
                    "corrupt-{}.json",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                ));
                let _ = std::fs::copy(path, &backup);
                Err(CoreError::ConfigCorrupt(format!(
                    "{}（已备份到 {}）",
                    e,
                    backup.display()
                )))
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        atomic_write_json(path, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn defaults_match_spec() {
        let c = AppConfig::default();
        assert_eq!(c.download.max_h, 1080);
        assert_eq!(c.download.fragments, 4);
        assert!(!c.download.playlist);
        assert!(c.download.embed_cover);
        assert_eq!(c.transcode.force_encoder_mode, "auto");
        assert!(c.transcode.low_power);
        assert_eq!(c.general.concurrency, 3);
        assert_eq!(c.general.max_gain_db, 24.0);
        assert!(c.general.check_update);
        assert_eq!(c.general.history_limit, 100);
        assert!(c.dependencies.potoken_enabled);
        assert!(c.dependencies.yt_dlp_path.is_none());
        assert_eq!(c.network.proxy_url, "");
    }

    #[test]
    fn history_limit_bounded() {
        assert!(GeneralConfig::HISTORY_LIMIT_MAX >= GeneralConfig::default().history_limit);
    }

    #[test]
    fn load_missing_file_returns_default() {
        let root = tempdir().unwrap();
        let c = AppConfig::load(&root.path().join("nope.json")).unwrap();
        assert_eq!(c.general.concurrency, 3);
    }

    #[test]
    fn save_then_load_roundtrip() {
        let root = tempdir().unwrap();
        let p = root.path().join("config.json");
        let mut c = AppConfig::default();
        c.general.concurrency = 5;
        c.network.site_proxy.insert("youtube.com".into(), true);
        c.save(&p).unwrap();
        let back = AppConfig::load(&p).unwrap();
        assert_eq!(back.general.concurrency, 5);
        assert_eq!(back.network.site_proxy.get("youtube.com"), Some(&true));
    }

    #[test]
    fn resolve_proxy_global_when_no_site_split() {
        let n = NetworkConfig {
            proxy_url: "socks5://127.0.0.1:10808".into(),
            ..Default::default()
        };
        assert_eq!(
            n.resolve_proxy("https://www.bilibili.com/video/BV1xx"),
            Some("socks5://127.0.0.1:10808".to_string())
        );
        assert_eq!(
            NetworkConfig::default().resolve_proxy("https://www.bilibili.com/video/BV1xx"),
            None
        );
    }

    #[test]
    fn resolve_proxy_site_split() {
        let mut n = NetworkConfig {
            proxy_url: "socks5://127.0.0.1:10808".into(),
            ..Default::default()
        };
        n.site_proxy.insert("bilibili.com".into(), false); // 勾选=走代理，此处为直连
        assert_eq!(n.resolve_proxy("https://www.bilibili.com/video/BV1xx"), None);
        n.site_proxy.insert("youtube.com".into(), true);
        assert_eq!(
            n.resolve_proxy("https://www.youtube.com/watch?v=abc"),
            Some("socks5://127.0.0.1:10808".to_string())
        );
        assert_eq!(n.resolve_proxy("https://vimeo.com/1"), None); // 未配置站点直连
        assert_eq!(n.resolve_proxy("https://api.bilibili.com/x"), None); // 子域命中 false
    }

    #[test]
    fn corrupt_config_backed_up_and_errors() {
        let root = tempdir().unwrap();
        let p = root.path().join("config.json");
        std::fs::write(&p, "{ not json !").unwrap();
        let err = AppConfig::load(&p).unwrap_err();
        assert!(matches!(err, CoreError::ConfigCorrupt(_)));
        let backups: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("corrupt"))
            .collect();
        assert_eq!(backups.len(), 1, "损坏配置应备份后回退");
    }

    #[test]
    fn partial_config_fills_missing_with_defaults() {
        // 旧版本/缺段配置应能加载并补默认（serde default）
        let json = r#"{"general":{"concurrency":6}}"#;
        let c: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(c.general.concurrency, 6);
        assert_eq!(c.download.fragments, 4);
        assert!(c.dependencies.potoken_enabled);
    }
}
