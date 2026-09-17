//! Windows 专用：经 WebView2 CookieManager（COM）抓取含 HttpOnly 的 Cookie（DL-05）。
//! 仅 Windows target 编译；任何失败返回 None，调用方回退到注入 URL 携带的 document.cookie。

#![cfg(windows)]

use std::sync::mpsc;

use tauri::webview::PlatformWebview;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_COOKIE_SAME_SITE_KIND, ICoreWebView2Cookie, ICoreWebView2CookieList, ICoreWebView2_2,
};
use webview2_com::GetCookiesCompletedHandler;
use windows::core::{Interface, PCWSTR, PWSTR};

use ytdlp_core::cookies::CookieEntry;

/// 同步抓取某 host 域下全部 Cookie（含 HttpOnly）。
///
/// 走 WebView2 CookieManager.GetCookies（COM，非 JS document.cookie，因此含 HttpOnly）。
/// GetCookies 异步完成，回调在 STA 消息泵执行；这里用 `wait_with_pump` 泵消息等待，
/// 不能裸阻塞 `recv()`（否则回调永远不执行，死锁）。
pub fn fetch_cookies_com(webview: &PlatformWebview, host: &str) -> Option<Vec<CookieEntry>> {
    let controller = webview.controller();
    let core = unsafe { controller.CoreWebView2().ok()? };
    // CookieManager 在 ICoreWebView2_2 上（基接口无此方法）
    let core2 = core.cast::<ICoreWebView2_2>().ok()?;
    let manager = unsafe { core2.CookieManager().ok()? };

    let uri = format!("https://{}/", host);
    let uri_wide: Vec<u16> = uri.encode_utf16().chain(std::iter::once(0)).collect();

    let (tx, rx) = mpsc::channel::<Option<Vec<CookieEntry>>>();
    let completed: webview2_com::CompletedClosure<
        windows::core::HRESULT,
        Option<ICoreWebView2CookieList>,
    > = Box::new(move |err: windows::core::Result<()>, cookies: Option<ICoreWebView2CookieList>| {
            let result = (|| -> windows::core::Result<Option<Vec<CookieEntry>>> {
                err?;
                let list = match cookies {
                    Some(l) => l,
                    None => return Ok(None),
                };
                let mut count = 0u32;
                unsafe { list.Count(&mut count) }?;
                let mut out = Vec::with_capacity(count as usize);
                for i in 0..count {
                    let cookie = unsafe { list.GetValueAtIndex(i) }?;
                    out.push(cookie_to_entry(&cookie));
                }
                Ok(Some(out))
            })();
            let _ = tx.send(result.ok().flatten());
            Ok(())
        });

    let handler = GetCookiesCompletedHandler::create(completed);
    unsafe {
        manager
            .GetCookies(PCWSTR(uri_wide.as_ptr()), &handler)
            .ok()?;
    }
    webview2_com::wait_with_pump(rx).ok()?
}

/// 读取 Cookie 的宽字符串属性（Name/Value/Domain/Path）。
unsafe fn get_wide(c: &ICoreWebView2Cookie, idx: u8) -> String {
    let mut p = PWSTR::null();
    let ok = match idx {
        0 => c.Name(&mut p),
        1 => c.Value(&mut p),
        2 => c.Domain(&mut p),
        _ => c.Path(&mut p),
    };
    if ok.is_ok() && !p.is_null() {
        p.to_string().unwrap_or_default()
    } else {
        String::new()
    }
}

fn bool_from(b: windows::core::BOOL) -> bool {
    b.0 != 0
}

/// WebView2 Cookie → 内部 CookieEntry。
fn cookie_to_entry(c: &ICoreWebView2Cookie) -> CookieEntry {
    let mut is_http = windows::core::BOOL(0);
    let mut is_secure = windows::core::BOOL(0);
    let mut same_site = COREWEBVIEW2_COOKIE_SAME_SITE_KIND::default();
    let mut expires = 0.0f64;
    unsafe {
        let _ = c.IsHttpOnly(&mut is_http);
        let _ = c.IsSecure(&mut is_secure);
        let _ = c.SameSite(&mut same_site);
        let _ = c.Expires(&mut expires);
    }
    CookieEntry {
        name: unsafe { get_wide(c, 0) },
        value: unsafe { get_wide(c, 1) },
        domain: unsafe { get_wide(c, 2) },
        path: unsafe { get_wide(c, 3) },
        expires: if expires > 0.0 { Some(expires) } else { None },
        http_only: bool_from(is_http),
        secure: bool_from(is_secure),
        same_site: match same_site.0 {
            1 => "lax".into(),
            2 => "strict".into(),
            _ => "none".into(),
        },
    }
}
