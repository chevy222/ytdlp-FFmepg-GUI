//! 站点 Cookie 管理（§3.6 Cookie / §3.7 / DL-06）。
//!
//! - 存储：`config/cookies/<host>.txt`（Netscape 格式，yt-dlp 直接可用）。
//! - 匹配：精确 HOST → 站点级回退（父域/常见子域）→ X↔twitter 姊妹域名互退。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;

/// 单个 Cookie（WebView2 CookieManager 全字段，§3.7）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieEntry {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: Option<f64>,
    pub http_only: bool,
    pub secure: bool,
    #[serde(default)]
    pub same_site: String,
}

/// 站点 Cookie 存储（按 HOST 分文件，Netscape 格式）。
#[derive(Debug, Clone)]
pub struct CookieStore {
    dir: PathBuf,
}

/// 导出到 Netscape 格式时的大小上限（防畸形 cookie 撑爆文件）。
const MAX_COOKIE_VALUE_LEN: usize = 4096;

impl CookieStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn host_file(&self, host: &str) -> PathBuf {
        self.dir.join(format!("{}.txt", sanitize_host(host)))
    }

    /// 保存/覆盖某站点 cookie（Netscape 格式，原子写）。
    pub fn save_host(&self, host: &str, cookies: Vec<CookieEntry>) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let f = self.host_file(host);
        let tmp = f.with_extension("txt.tmp");
        std::fs::write(&tmp, netscape_format(&cookies))?;
        std::fs::rename(&tmp, &f)?;
        Ok(())
    }

    /// 读取某站点 cookie（Netscape 格式）。
    pub fn load_host(&self, host: &str) -> Result<Vec<CookieEntry>> {
        let f = self.host_file(host);
        if !f.exists() {
            return Ok(Vec::new());
        }
        let s = std::fs::read_to_string(f)?;
        Ok(netscape_parse(&s))
    }

    /// 已保存站点列表（按文件名排序，不含扩展名）。
    pub fn list_hosts(&self) -> Result<Vec<String>> {
        if !self.dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut hosts = Vec::new();
        for e in std::fs::read_dir(&self.dir)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(host) = name.strip_suffix(".txt") {
                hosts.push(host.to_string());
            }
        }
        hosts.sort();
        Ok(hosts)
    }

    /// 删除某站点 cookie。
    pub fn delete_host(&self, host: &str) -> Result<()> {
        let f = self.host_file(host);
        if f.exists() {
            std::fs::remove_file(f)?;
        }
        Ok(())
    }

    /// 获取匹配站点的 cookie 文件路径（供 yt-dlp --cookies）。
    /// 合并所有匹配候选的 cookie；无任何 cookie 返回 None。
    pub fn cookies_file(&self, host: &str) -> Result<Option<PathBuf>> {
        let mut entries: Vec<CookieEntry> = Vec::new();
        for h in cookie_candidates(host) {
            if let Ok(list) = self.load_host(&h) {
                for c in list {
                    if !entries
                        .iter()
                        .any(|e| e.name == c.name && e.domain == c.domain)
                    {
                        entries.push(c);
                    }
                }
            }
        }
        if entries.is_empty() {
            return Ok(None);
        }
        // 用实际有 cookie 的 host 作为文件名
        let actual_host = cookie_candidates(host)
            .into_iter()
            .find(|h| self.host_file(h).exists())
            .unwrap_or_else(|| host.to_string());
        let f = self.host_file(&actual_host);
        std::fs::write(&f, netscape_format(&entries))?;
        Ok(Some(f))
    }
}

/// 把 cookie 列表格式化成 Netscape 文本。
fn netscape_format(cookies: &[CookieEntry]) -> String {
    let mut out = String::from("# Netscape HTTP Cookie File\n");
    for c in cookies {
        if c.value.len() > MAX_COOKIE_VALUE_LEN {
            continue;
        }
        let (domain, include_sub) = if c.domain.starts_with('.') {
            (c.domain.as_str(), "TRUE")
        } else {
            (c.domain.as_str(), "FALSE")
        };
        let secure = if c.secure { "TRUE" } else { "FALSE" };
        let expires = match c.expires {
            Some(e) => format!("{}", e as i64),
            None => String::new(),
        };
        let value = c.value.replace(['\t', '\n'], " ");
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            domain, include_sub, c.path, secure, expires, c.name, value
        ));
    }
    out
}

/// 从 Netscape 文本解析 cookie 列表。
fn netscape_parse(s: &str) -> Vec<CookieEntry> {
    let mut out = Vec::new();
    for line in s.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 7 {
            continue;
        }
        let domain = parts[0].to_string();
        let include_sub = parts[1] == "TRUE";
        let path = parts[2].to_string();
        let secure = parts[3] == "TRUE";
        let expires = if parts[4].is_empty() {
            None
        } else {
            parts[4].parse::<f64>().ok()
        };
        let name = parts[5].to_string();
        let value = parts[6].to_string();
        let http_only = false; // Netscape 格式不区分 HttpOnly
        let same_site = String::new();
        out.push(CookieEntry {
            name,
            value,
            domain: if include_sub && !domain.starts_with('.') {
                format!(".{}", domain)
            } else {
                domain
            },
            path,
            expires,
            http_only,
            secure,
            same_site,
        });
    }
    out
}

/// 从 URL 中提取 HOST（小写，去端口）。
pub fn host_from_url(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    Some(u.host_str()?.to_lowercase())
}

/// 文件名安全化（host 中不允许的字符替换）。
pub fn sanitize_host(host: &str) -> String {
    host.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Cookie 匹配候选列表（DL-06）：精确 host → 站点级回退 → X↔twitter 互退。
pub fn cookie_candidates(host: &str) -> Vec<String> {
    let h = host.to_lowercase();
    let mut out = vec![h.clone()];
    let push_unique = |out: &mut Vec<String>, s: String| {
        if !out.contains(&s) {
            out.push(s);
        }
    };
    // 站点级回退：去掉常见子域前缀取父域
    let parent = h
        .rsplit_once('.')
        .and_then(|(sub, tld)| {
            if sub.contains('.') {
                Some(format!(
                    "{}.{}",
                    sub.rsplit_once('.').map(|(_, s)| s).unwrap_or(sub),
                    tld
                ))
            } else {
                None // 已是最短二级域
            }
        })
        .unwrap_or_else(|| h.clone());
    if parent != h {
        push_unique(&mut out, parent.clone());
        // 补充常见子域（www）
        if !parent.starts_with("www.") {
            push_unique(&mut out, format!("www.{}", parent));
        }
    }
    // X ↔ twitter 姊妹域名互退
    match h.as_str() {
        "x.com" | "www.x.com" => push_unique(&mut out, "twitter.com".into()),
        "twitter.com" | "www.twitter.com" => push_unique(&mut out, "x.com".into()),
        _ => {}
    }
    // YouTube ↔ youtu.be 姊妹域名互退
    match h.as_str() {
        "youtube.com" | "www.youtube.com" => push_unique(&mut out, "youtu.be".into()),
        "youtu.be" => {
            push_unique(&mut out, "youtube.com".into());
            push_unique(&mut out, "www.youtube.com".into());
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn ck(name: &str, value: &str, domain: &str) -> CookieEntry {
        CookieEntry {
            name: name.into(),
            value: value.into(),
            domain: domain.into(),
            path: "/".into(),
            expires: None,
            http_only: true,
            secure: true,
            same_site: "lax".into(),
        }
    }

    #[test]
    fn host_from_url_works() {
        assert_eq!(
            host_from_url("https://www.BiLiBiLi.com/video/BV1xx?a=1").as_deref(),
            Some("www.bilibili.com")
        );
        assert_eq!(
            host_from_url("https://x.com/status/1").as_deref(),
            Some("x.com")
        );
        assert!(host_from_url("not a url").is_none());
    }

    #[test]
    fn candidates_exact_then_parent_then_www() {
        let c = cookie_candidates("www.bilibili.com");
        assert_eq!(c[0], "www.bilibili.com");
        assert!(c.contains(&"bilibili.com".to_string()));
        // 去重：不应重复出现 www.bilibili.com
        assert_eq!(
            c.len(),
            c.iter().collect::<std::collections::HashSet<_>>().len()
        );
    }

    #[test]
    fn candidates_x_twitter_cross() {
        let c = cookie_candidates("x.com");
        assert!(c.contains(&"twitter.com".to_string()));
        let c2 = cookie_candidates("twitter.com");
        assert!(c2.contains(&"x.com".to_string()));
    }

    #[test]
    fn save_load_delete_roundtrip() {
        let root = tempdir().unwrap();
        let store = CookieStore::new(root.path().join("cookies"));
        store
            .save_host("youtube.com", vec![ck("SID", "abc", ".youtube.com")])
            .unwrap();
        assert_eq!(store.list_hosts().unwrap(), vec!["youtube.com".to_string()]);
        let loaded = store.load_host("youtube.com").unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "SID");
        assert!(loaded[0].http_only);
        store.delete_host("youtube.com").unwrap();
        assert!(store.list_hosts().unwrap().is_empty());
    }

    #[test]
    fn export_netscape_merges_and_formats() {
        let root = tempdir().unwrap();
        let store = CookieStore::new(root.path().join("cookies"));
        store
            .save_host("youtube.com", vec![ck("SID", "v1", ".youtube.com")])
            .unwrap();
        // host-only cookie（无前导点）应输出 FALSE
        store
            .save_host("www.youtube.com", vec![ck("VISITOR", "v2", "youtube.com")])
            .unwrap();
        let dest = root.path().join("netscape.txt");
        let out = store.export_netscape("www.youtube.com", &dest).unwrap();
        assert!(out.is_some());
        let text = std::fs::read_to_string(&dest).unwrap();
        // 带点 domain 保留前导点 + TRUE（跨子域）
        assert!(text.contains(".youtube.com\tTRUE\t/\tTRUE\t\tSID\tv1"));
        // 无点 domain + FALSE（host-only）
        assert!(text.contains("youtube.com\tFALSE\t/\tTRUE\t\tVISITOR\tv2"));
    }

    #[test]
    fn export_netscape_session_cookie_empty_expires() {
        let root = tempdir().unwrap();
        let store = CookieStore::new(root.path().join("cookies"));
        // expires=None（会话 cookie）：expires 字段必须为空，不能是 0
        store
            .save_host("example.com", vec![ck("SESS", "v", ".example.com")])
            .unwrap();
        let dest = root.path().join("n.txt");
        store.export_netscape("example.com", &dest).unwrap();
        let text = std::fs::read_to_string(&dest).unwrap();
        assert!(text.contains("\tTRUE\t/\tTRUE\t\tSESS\tv"), "got: {text}");
        assert!(!text.contains("TRUE\t0\tSESS"));
    }

    #[test]
    fn export_netscape_none_when_empty() {
        let root = tempdir().unwrap();
        let store = CookieStore::new(root.path().join("cookies"));
        assert!(store
            .export_netscape("nope.com", &root.path().join("n.txt"))
            .unwrap()
            .is_none());
    }
}
