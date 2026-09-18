//! 内置登录 WebView2 登录窗（DL-05）：
//! - 登录窗顶部中心固定"登录完成"按钮 + 提示条
//! - SPA 站点（YouTube 等）注入脚本 setInterval 每 800ms 自检重建（#ytdlp-login-bar / #ytdlp-login-hint）
//! - 点击"登录完成"→ 跳转本地 done URL（携带 document.cookie）→ Rust 抓取保存
//! - Windows 上优先经 WebView2 CookieManager（COM）抓取含 HttpOnly 的 Cookie；失败回退 URL 携带的 cookie
//! - 保存后关闭登录窗并自动重解析 NeedLogin 条目

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::commands::save_cookies;

/// 已知支持内置登录的站点 → 登录 URL。
pub fn login_url_for_host(host: &str) -> Option<String> {
    let h = host.to_lowercase();
    if h.contains("youtube") || h.contains("youtu.be") {
        Some("https://www.youtube.com/".into())
    } else if h.contains("bilibili") {
        Some("https://www.bilibili.com/".into())
    } else if h.contains("x.com") || h.contains("twitter") {
        Some("https://x.com/".into())
    } else if h.contains("douyin") {
        Some("https://www.douyin.com/".into())
    } else {
        None
    }
}

/// 打开登录窗。
pub fn open_login(app: &AppHandle, host: &str, url: &str) -> Result<(), String> {
    // 已存在则聚焦
    if let Some(win) = app.get_webview_window("ytdlp-login") {
        let _ = win.set_focus();
        return Ok(());
    }
    let script = login_inject_script();
    let host2 = host.to_string();
    WebviewWindowBuilder::new(
        app,
        "ytdlp-login",
        WebviewUrl::External(url.parse().map_err(|e| format!("无效登录 URL：{}", e))?),
    )
    .title(format!("登录 - 影栈（{}）", host))
    .inner_size(1000.0, 760.0)
    .min_inner_size(720.0, 560.0)
    .center()
    .initialization_script(&script)
    .on_page_load(move |webview, payload| {
        let url = payload.url().to_string();
        if url.contains("ytdlp-login-done") {
            let app = webview.app_handle().clone();
            if let Some(win) = app.get_webview_window("ytdlp-login") {
                handle_login_done(&win, &host2, &url);
            }
        }
    })
    .build()
    .map_err(|e| format!("打开登录窗口失败：{}", e))?;
    Ok(())
}

fn handle_login_done(win: &tauri::WebviewWindow, host: &str, url: &str) {
    let app = win.app_handle().clone();
    // 1) Windows：尝试 COM 抓取（含 HttpOnly）
    #[cfg(windows)]
    {
        let (tx, rx) = std::sync::mpsc::channel::<Option<Vec<ytdlp_core::cookies::CookieEntry>>>();
        let host_owned = host.to_string();
        let _ = win.with_webview(move |webview| {
            let _ = tx.send(crate::login_win::fetch_cookies_com(&webview, &host_owned));
        });
        let got = rx.recv().ok().flatten();
        if let Some(cookies) = got {
            if !cookies.is_empty() {
                let _ = save_cookies(app.clone(), host.to_string(), cookies);
                finish_login(&app, host);
                return;
            }
        }
    }
    // 2) 回退：解析 URL 携带的 document.cookie
    if let Ok(parsed) = url::Url::parse(url) {
        let mut cookies = Vec::new();
        for (k, v) in parsed.query_pairs() {
            if k == "cookies" {
                for c in parse_cookie_header(v.as_ref()) {
                    cookies.push(ytdlp_core::cookies::CookieEntry {
                        name: c.0,
                        value: c.1,
                        domain: format!(".{}", host.trim_start_matches("www.")),
                        path: "/".into(),
                        expires: None,
                        http_only: false,
                        secure: true,
                        same_site: "lax".into(),
                    });
                }
            }
        }
        if !cookies.is_empty() {
            let _ = save_cookies(app.clone(), host.to_string(), cookies);
        }
    }
    finish_login(&app, host);
}
fn finish_login(app: &AppHandle, _host: &str) {
    // 关闭登录窗
    if let Some(win) = app.get_webview_window("ytdlp-login") {
        let _ = win.close();
    }
    // 通知前端（触发 NeedLogin 条目自动重解析）
    let _ = app.emit("login:done", serde_json::json!({ "host": _host }));
}

/// 解析 `a=b; c=d` 形式 cookie 字符串。
fn parse_cookie_header(s: &str) -> Vec<(String, String)> {
    s.split(';')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            let (k, v) = part.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

/// 注入脚本：顶部中心"登录完成"按钮 + 提示条 + SPA 800ms 保活重建（DL-05）。
fn login_inject_script() -> String {
    r#"
(function () {
  'use strict';
  function ensure() {
    if (document.getElementById('ytdlp-login-bar')) { return; }
    var bar = document.createElement('div');
    bar.id = 'ytdlp-login-bar';
    bar.setAttribute('style',
      'position:fixed;top:0;left:50%;transform:translateX(-50%);z-index:2147483647;' +
      'display:flex;flex-direction:column;align-items:center;gap:4px;' +
      'padding:6px 16px 8px;background:rgba(15,118,110,0.96);border-radius:0 0 10px 10px;' +
      'box-shadow:0 2px 10px rgba(0,0,0,0.35);font-family:system-ui,sans-serif;');
    var btn = document.createElement('button');
    btn.id = 'ytdlp-login-btn';
    btn.textContent = '登录完成';
    btn.setAttribute('style',
      'border:none;cursor:pointer;font-size:14px;font-weight:600;color:#ffffff;' +
      'background:#0f766e;padding:8px 22px;border-radius:6px;box-shadow:0 0 0 1px rgba(255,255,255,0.5);');
    btn.onclick = function () {
      var cookies = encodeURIComponent(document.cookie);
      var host = encodeURIComponent(location.hostname);
      window.location.href = 'http://127.0.0.1/ytdlp-login-done?host=' + host + '&cookies=' + cookies;
    };
    var hint = document.createElement('div');
    hint.id = 'ytdlp-login-hint';
    hint.textContent = '登录完成后点击上方"登录完成"按钮保存 Cookie';
    hint.setAttribute('style', 'font-size:12px;color:rgba(255,255,255,0.9);');
    bar.appendChild(btn);
    bar.appendChild(hint);
    var root = document.body || document.documentElement;
    root.appendChild(bar);
  }
  ensure();
  setInterval(ensure, 800);
  function failHint() {
    if (document.getElementById('ytdlp-fail-hint')) { return; }
    if (document.readyState === 'complete' && document.body && document.body.childElementCount === 0) {
      var el = document.createElement('div');
      el.id = 'ytdlp-fail-hint';
      el.textContent = '页面加载失败：登录窗走系统代理，请先在代理软件中开启"系统代理"后重试，或点右上角 × 关闭';
      el.setAttribute('style',
        'position:fixed;left:0;right:0;bottom:0;z-index:2147483646;' +
        'background:#7f1d1d;color:#fff;font-family:system-ui,sans-serif;font-size:13px;' +
        'padding:10px 16px;text-align:center;');
      (document.body || document.documentElement).appendChild(el);
    }
  }
  setInterval(failHint, 800);
})();
"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_url_mapping() {
        assert!(login_url_for_host("www.youtube.com")
            .unwrap()
            .contains("youtube"));
        assert!(login_url_for_host("www.bilibili.com")
            .unwrap()
            .contains("bilibili"));
        assert!(login_url_for_host("x.com").unwrap().contains("x.com"));
        assert!(login_url_for_host("www.douyin.com")
            .unwrap()
            .contains("douyin"));
        assert!(login_url_for_host("example.com").is_none());
    }

    #[test]
    fn parse_cookie_header_basic() {
        let c = parse_cookie_header("SID=abc; VISITOR_INFO1_LIVE=x; empty");
        assert_eq!(c.len(), 2);
        assert_eq!(c[0], ("SID".to_string(), "abc".to_string()));
        assert_eq!(c[1].0, "VISITOR_INFO1_LIVE");
    }

    #[test]
    fn inject_script_has_800ms_keepalive() {
        let s = login_inject_script();
        assert!(s.contains("setInterval(ensure, 800)"));
        assert!(s.contains("ytdlp-login-bar"));
        assert!(s.contains("ytdlp-login-hint"));
        assert!(s.contains("登录完成"));
    }
}
