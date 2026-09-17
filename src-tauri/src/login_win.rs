//! Windows 专用：经 WebView2 CookieManager（COM）抓取含 HttpOnly 的 Cookie（DL-05）。
//! 仅在 Windows target 编译；任何失败返回 None，调用方回退到注入 URL 携带的 document.cookie。
//! 注意：本文件无法在 Linux 编译验证，签名以 webview2-com 0.30 为准，CI（Windows）验证。

#![cfg(windows)]

use std::sync::mpsc;

use webview2_com::core::{IUnknown, HRESULT, PCWSTR};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2CookieList, ICoreWebView2GetCookiesCompletedHandler,
};

use ytdlp_core::cookies::CookieEntry;

/// 同步抓取某 host 域下全部 Cookie（含 HttpOnly）。
pub fn fetch_cookies_com(webview: &wry::WebView, host: &str) -> Option<Vec<CookieEntry>> {
    // 1) 取 CookieManager
    let controller = webview.controller();
    let core = controller.CoreWebView2().ok()?;
    let manager = core.CookieManager().ok()?;

    // 2) GetCookiesAsync（HTTP/HTTPS 协议前缀 + 根路径覆盖子域）
    let uri = format!("https://{}/", host);
    let uri_wide: Vec<u16> = uri.encode_utf16().chain(std::iter::once(0)).collect();
    let (tx, rx) = mpsc::channel::<Option<Vec<CookieEntry>>>();
    let mut handler = CookieHandler { tx };
    unsafe {
        manager
            .GetCookiesAsync(PCWSTR(uri_wide.as_ptr()), &mut handler)
            .ok()?;
    }
    // GetCookiesAsync 是异步 COM；完成回调在主线程执行，这里阻塞等待
    rx.recv().ok()?
}

struct CookieHandler {
    tx: mpsc::Sender<Option<Vec<CookieEntry>>>,
}

unsafe impl IUnknown for CookieHandler {
    unsafe fn QueryInterface(
        &mut self,
        riid: *const webview2_com::core::GUID,
        ppv: *mut *mut std::ffi::c_void,
    ) -> HRESULT {
        // 简化：仅支持自身接口
        if ppv.is_null() {
            return webview2_com::core::E_POINTER;
        }
        *ppv = self as *mut _ as *mut std::ffi::c_void;
        webview2_com::core::S_OK
    }
    unsafe fn AddRef(&mut self) -> u32 {
        1
    }
    unsafe fn Release(&mut self) -> u32 {
        1
    }
}

impl ICoreWebView2GetCookiesCompletedHandler for CookieHandler {
    unsafe fn Invoke(
        &mut self,
        result: HRESULT,
        cookie_list: *mut ICoreWebView2CookieList,
    ) -> HRESULT {
        if !result.is_ok() || cookie_list.is_null() {
            let _ = self.tx.send(None);
            return result;
        }
        let list = &*cookie_list;
        let mut out: Vec<CookieEntry> = Vec::new();
        match list.get_Count() {
            Ok(count) => {
                for i in 0..count {
                    if let Ok(cookie) = list.GetValueAtIndex(i) {
                        if let Some(entry) = cookie_to_entry(&cookie) {
                            out.push(entry);
                        }
                    }
                }
            }
            Err(_) => {}
        }
        let _ = self.tx.send(Some(out));
        webview2_com::core::S_OK
    }
}

fn cookie_to_entry(c: &ICoreWebView2Cookie) -> Option<CookieEntry> {
    Some(CookieEntry {
        name: c.get_Name().ok()?.to_string(),
        value: c.get_Value().ok()?.to_string(),
        domain: c.get_Domain().ok()?.to_string(),
        path: c.get_Path().ok()?.to_string(),
        expires: c.get_Expires().ok(),
        http_only: c.get_IsHttpOnly().ok()? != 0,
        secure: c.get_IsSecure().ok()? != 0,
        same_site: format!("{:?}", c.get_SameSite().ok()?).to_lowercase(),
    })
}
