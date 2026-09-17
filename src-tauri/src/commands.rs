//! Tauri 命令层：统一列表 CRUD + 解析/下载/取消/删除 + 配置 + Cookie + 依赖自检。
//!
//! 后台任务用 std::thread + 事件 `item:update`（payload = MediaItem）回推前端；
//! 关键状态变更才持久化 history.json（进度高频更新只 emit 不落盘）。

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, State};
use ytdlp_core::config::AppConfig;
use ytdlp_core::exec::ToolResolver;
use ytdlp_core::cookies::CookieStore;
use ytdlp_core::download::{
    self, cleanup_on_cancel, post_process, probe_output, run_download, DownloadParams,
};
use ytdlp_core::model::{ItemKind, MediaItem, Status};
use ytdlp_core::config::NetworkConfig;
use ytdlp_core::merge::{self, MergeParams};
use ytdlp_core::transcode::{self, TranscodeParams};
use ytdlp_core::probe::{self, ProbeErrorKind};
use ytdlp_core::worker::SubmitOutcome;
use ytdlp_core::{transition, CoreError};

use crate::login;
use crate::state::AppState;

/// 硬件编码器探测（TC-16）：QSV/NVENC/AMF 可用性，供设置页标注。
#[tauri::command]
pub fn probe_hw_encoders(app: AppHandle) -> CmdResult<serde_json::Value> {
    let state = app.state::<AppState>();
    let resolver = state.resolver();
    let hw = transcode::detect_hw_encoders(&resolver)
        .map_err(|e| format!("探测编码器失败：{}", e))?;
    Ok(serde_json::json!({ "qsv": hw.qsv, "nvenc": hw.nvenc, "amf": hw.amf }))
}

/// 依赖自检项（前端显示）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolStatus {
    pub tool: String,
    pub path: Option<String>,
    pub version: Option<String>,
    pub ok: bool,
}

type CmdResult<T> = Result<T, String>;

fn err_string(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// 更新条目并返回克隆（变更即原子写仅对状态迁移生效由调用方决定）。
fn update_item(app: &AppHandle, id: &str, f: impl FnOnce(&mut MediaItem)) -> Option<MediaItem> {
    let state = app.state::<AppState>();
    let mut hist = state.history.lock().unwrap();
    let item = hist.get(id)?.clone();
    let mut item = item;
    f(&mut item);
    hist.upsert(item.clone());
    drop(hist);
    let _ = app.emit("item:update", &item);
    Some(item)
}

/// 记录日志行并 emit。
fn log_item(app: &AppHandle, id: &str, line: impl Into<String>) {
    update_item(app, id, |it| {
        it.push_log(line);
    });
}

/// 持久化（状态迁移后调用）。
fn persist(app: &AppHandle) {
    app.state::<AppState>().persist();
}

// ---------- 添加与解析 ----------

#[tauri::command]
pub fn add_url(app: AppHandle, urls: Vec<String>) -> CmdResult<()> {
    let state = app.state::<AppState>();
    let mut hist = state.history.lock().unwrap();
    for raw in urls {
        let url = clean_url(&raw);
        if url.is_empty() {
            continue;
        }
        let item = MediaItem::from_url(url);
        let id = item.id.clone();
        hist.upsert(item);
        // 解析线程不占并发 slot
        let app2 = app.clone();
        std::thread::spawn(move || {
            run_probe(app2, id);
        });
    }
    drop(hist);
    persist(&app);
    let _ = app.emit("list:changed", ());
    Ok(())
}

#[tauri::command]
pub fn add_local(app: AppHandle, paths: Vec<String>, recursive: bool) -> CmdResult<()> {
    let mut files: Vec<PathBuf> = Vec::new();
    for p in paths {
        let pb = PathBuf::from(&p);
        if pb.is_dir() {
            scan_dir(&pb, recursive, &mut files);
        } else if pb.is_file() {
            files.push(pb);
        }
    }
    if files.is_empty() {
        return Err("没有找到可添加的文件".into());
    }
    let state = app.state::<AppState>();
    let mut hist = state.history.lock().unwrap();
    for f in files {
        let item = MediaItem::from_path(f.to_string_lossy().into_owned());
        let id = item.id.clone();
        hist.upsert(item);
        let app2 = app.clone();
        std::thread::spawn(move || {
            run_probe(app2, id);
        });
    }
    drop(hist);
    persist(&app);
    Ok(())
}

fn scan_dir(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if recursive {
                scan_dir(&p, true, out);
            }
        } else if download::is_video_file(&p) {
            out.push(p);
        }
    }
}

/// URL 清洗（DL-01）：去引号/空白，抖音 modal_id 归一化由 yt-dlp 处理。
fn clean_url(raw: &str) -> String {
    raw.trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .to_string()
}

/// 解析任务（URL 或本地；阻塞运行在线程中）。
fn run_probe(app: AppHandle, id: String) {
    let state = app.state::<AppState>();
    let item = {
        let hist = state.history.lock().unwrap();
        hist.get(&id).cloned()
    };
    let Some(item) = item else {
        return;
    };
    log_item(&app, &id, "开始解析元数据…");

    let resolver = state.resolver();
    let network = state.config.lock().unwrap().network.clone();
    let netscape = resolve_cookies(&state, &item);

    let playlist_on = state.config.lock().unwrap().download.playlist;
    let result = if item.url.is_some() {
        probe::probe_url(
            &resolver,
            item.url.as_deref().unwrap_or_default(),
            netscape.as_deref(),
            &network,
            playlist_on,
        )
    } else {
        let path = item.path.clone().unwrap_or_default();
        probe::probe_local(&resolver, Path::new(&path)).map(|p| {
            let mut mp = p;
            let title = item.title.clone();
            mp.meta.title = Some(title);
            ytdlp_core::probe::UrlProbe {
                meta: mp.meta,
                site: Some("本地文件".into()),
                host: None,
                needs_login: false,
                is_playlist: false,
                playlist_count: None,
                thumbnail_url: None,
            }
        })
    };

    match result {
        Ok(p) => {
            let url_src = item.url.is_some();
            update_item(&app, &id, |it| {
                it.meta = p.meta;
                it.site = p.site.clone();
                if p.host.is_some() {
                    it.host = p.host.clone();
                }
                it.status = Status::Ready;
                it.percent = 0.0;
                it.error = None;
                it.push_log("解析完成，已就绪".to_string());
                if url_src {
                    it.push_log(format!("可用格式：{} 项", it.meta.download_formats.len()));
                }
                let _ = &p.is_playlist;
                let _ = p.playlist_count;
            });
            // URL 已就绪：触发 5 秒倒计时自动下载（前端计时，后端只发可下载信号）
            if url_src {
                let _ = app.emit("item:ready", serde_json::json!({ "id": id }));
            }
            // 播放列表（DL-09）：开启时把合集展开为逐集条目平铺进列表
            if url_src && playlist_on && p.is_playlist {
                expand_playlist(&app, &id, &item, &resolver, netscape.as_deref(), &network);
            }
        }
        Err(f) => {
            let status = if f.kind == ProbeErrorKind::NeedLogin {
                Status::NeedLogin
            } else {
                Status::Failed
            };
            update_item(&app, &id, |it| {
                it.status = status;
                it.error = Some(f.to_string());
                it.push_log(format!("解析失败：{}", f));
            });
        }
    }
    persist(&app);
}

// ---------- 列表 ----------

#[tauri::command]
pub fn list_items(state: State<'_, AppState>) -> CmdResult<Vec<MediaItem>> {
    let hist = state.history.lock().unwrap();
    Ok(hist.items.clone())
}

#[tauri::command]
pub fn get_item(state: State<'_, AppState>, id: String) -> CmdResult<MediaItem> {
    let hist = state.history.lock().unwrap();
    hist.get(&id)
        .cloned()
        .ok_or_else(|| format!("条目不存在：{}", id))
}

// ---------- 动作 ----------

#[tauri::command]
pub fn start_download(
    app: AppHandle,
    id: String,
    format_id: Option<String>,
    audio_only: bool,
) -> CmdResult<()> {
    let state = app.state::<AppState>();
    {
        let mut hist = state.history.lock().unwrap();
        let item = hist.get(&id).cloned().ok_or("条目不存在")?;
        if item.status != Status::Ready {
            return Err(format!("当前状态不可下载：{}", item.status.label()));
        }
        let new_status = transition(item.status, Status::Downloading).map_err(err_string)?;
        hist.upsert(MediaItem {
            status: new_status,
            ..item.clone()
        });
    }
    // 提交并发队列
    let outcome = {
        let mut q = state.queue.lock().unwrap();
        q.submit(&id)
    };
    if outcome == SubmitOutcome::Queued {
        log_item(&app, &id, "已排队，等待并发 slot…");
        persist(&app);
        return Ok(());
    }
    log_item(&app, &id, "开始下载…");
    persist(&app);
    let app2 = app.clone();
    std::thread::spawn(move || {
        run_download_task(app2, id, format_id, audio_only);
    });
    Ok(())
}

fn run_download_task(app: AppHandle, id: String, format_id: Option<String>, audio_only: bool) {
    let state = app.state::<AppState>();
    let cancel = state.register_cancel(&id);

    let (url, cfg, general, resolver, out_dir, template, proxy, netscape, sections) = {
        let hist = state.history.lock().unwrap();
        let item = hist.get(&id).cloned();
        let Some(item) = item else { return };
        let url = item.url.clone().unwrap_or_default();
        let cfg = state.config.lock().unwrap().download.clone();
        let general = state.config.lock().unwrap().general.clone();
        let resolver = state.resolver();
        let out_dir = default_output_dir(&state, &item);
        let template = cfg.filename_template.clone();
        let proxy = state.config.lock().unwrap().network.proxy_url.clone();
        let netscape = prepare_cookies(&state, &item);
        let sections = item.sections.clone();
        (
            url, cfg, general, resolver, out_dir, template, proxy, netscape, sections,
        )
    };

    let params = DownloadParams {
        format_id: format_id.clone(),
        audio_only,
        out_dir: out_dir.clone(),
        filename_template: template,
        embed_cover: cfg.embed_cover,
        proxy: Some(proxy),
        cookies_file: netscape,
        sections,
    };

    let app2 = app.clone();
    let id2 = id.clone();
    let prog_result = run_download(&resolver, &url, &params, &cfg, &cancel, move |p| {
        update_item(&app2, &id2, |it| {
            it.percent = p.percent;
            if let Some(s) = p.speed {
                it.speed = Some(s);
            }
            if let Some(e) = p.eta {
                it.eta = Some(e);
            }
            if let Some(f) = p.file {
                it.file = Some(f);
            }
        });
    });

    let mut outcome = match prog_result {
        Ok(o) => o,
        Err(e) => {
            finish_download(&app, &id, &cancel, Err(e), &state.paths.temp_dir(), &params);
            return;
        }
    };

    // 后处理（DL-04）
    let mut first = outcome.output_paths.first().cloned();
    if let Some(path) = &first {
        log_item(&app, &id, "下载完成，开始后处理…");
        update_item(&app, &id, |it| {
            it.status = Status::PostProcessing;
        });
        let app2 = app.clone();
        let cfg2 = cfg.clone();
        let general2 = general.clone();
        let cancel2 = cancel.clone();
        let pp = post_process(&resolver, path, &cfg2, &general2, &cancel2, |line| {
            log_item(&app2, &id, line);
        });
        match pp {
            Ok(final_path) => {
                first = Some(final_path.clone());
                outcome.output_paths = vec![final_path];
            }
            Err(e) => {
                finish_download(&app, &id, &cancel, Err(e), &state.paths.temp_dir(), &params);
                return;
            }
        }
    }

    // 产物解析（MD-06）
    if let Some(path) = &first {
        if let Ok(meta) = probe_output(&resolver, path) {
            update_item(&app, &id, |it| {
                it.meta = meta;
            });
        }
    }
    finish_download(
        &app,
        &id,
        &cancel,
        Ok(outcome),
        &state.paths.temp_dir(),
        &params,
    );
}

fn finish_download(
    app: &AppHandle,
    id: &str,
    _cancel: &Arc<std::sync::atomic::AtomicBool>,
    result: Result<download::DownloadOutcome, CoreError>,
    temp_root: &Path,
    params: &DownloadParams,
) {
    let state = app.state::<AppState>();
    let final_status = match &result {
        Ok(o) => {
            log_item(
                app,
                id,
                format!("下载完成：{} 个文件", o.output_paths.len()),
            );
            Status::Done
        }
        Err(CoreError::Cancelled) => {
            cleanup_on_cancel(params, temp_root);
            log_item(app, id, "已取消，清理残留");
            Status::Canceled
        }
        Err(e) => {
            log_item(app, id, format!("下载失败：{}", e));
            Status::Failed
        }
    };
    update_item(app, id, |it| {
        it.status = final_status;
        it.percent = if final_status == Status::Done {
            100.0
        } else {
            it.percent
        };
        if let Err(e) = &result {
            it.error = Some(e.to_string());
        }
        if final_status == Status::Done {
            it.speed = None;
            it.eta = None;
        }
    });
    // 释放并发 slot，启动下一个等待任务（下载/转码/合并共用）
    let next = {
        let mut q = state.queue.lock().unwrap();
        q.finish(id)
    };
    if let Some(next_id) = next {
        launch_next(app, next_id);
    }
    persist(app);
}

fn default_output_dir(state: &AppState, _item: &MediaItem) -> PathBuf {
    if let Some(dir) = &state.cli.lock().unwrap().dir {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Some(dir) = &state.config.lock().unwrap().general.default_output_dir {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    // 默认桌面（§3.6 通用）
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|p| p.join("Desktop"))
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|p| p.join("Desktop"))
        })
        .unwrap_or_else(|| state.paths.root().join("output"))
}

/// Cookie 文件解析：CLI `--cookies` 优先；否则从 Cookie 库按站点导出 Netscape 临时文件。
fn resolve_cookies(state: &AppState, item: &MediaItem) -> Option<PathBuf> {
    if let Some(p) = &state.cli.lock().unwrap().cookies {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    let host = item.host.clone().or_else(|| {
        item.url
            .as_deref()
            .and_then(ytdlp_core::cookies::host_from_url)
    })?;
    let store = CookieStore::new(state.paths.cookies_dir());
    let tmp = state.paths.temp_dir().join(format!("cookies-{}.txt", host));
    store.export_netscape(&host, &tmp).ok().flatten()
}

/// 下载/解析共用 cookie 解析（历史名称保留）。
fn prepare_cookies(state: &AppState, item: &MediaItem) -> Option<PathBuf> {
    resolve_cookies(state, item)
}

/// 合并面板参数（MG-01/04：低频操作，仅面板内配置，不落 config.json）。
#[derive(Debug, Clone)]
pub struct MergeJob {
    pub ids: Vec<String>,
    pub filename: String,
    pub container: String,
    pub encoder_mode: String,
    pub normalize_audio: bool,
}

/// 批量合并（MG-01..04：多选按序拼接；参数在合并面板配置）。
#[tauri::command]
pub fn start_merge(
    app: AppHandle,
    ids: Vec<String>,
    filename: Option<String>,
    container: Option<String>,
    encoder: Option<String>,
    normalize: Option<bool>,
) -> CmdResult<()> {
    let state = app.state::<AppState>();
    if ids.len() < 2 {
        return Err("合并至少需要 2 个条目".into());
    }
    let mut jobs = Vec::new();
    {
        let mut hist = state.history.lock().unwrap();
        for id in &ids {
            let item = hist.get(id).cloned().ok_or("条目不存在")?;
            let has_file = item
                .path
                .as_deref()
                .map(|p| std::path::Path::new(p).is_file())
                .unwrap_or(false);
            if !has_file {
                log_item(&app, id, "合并被跳过：无本地输入文件");
                continue;
            }
            let to = match transition(item.status, Status::Merging) {
                Ok(t) => t,
                Err(e) => {
                    log_item(&app, id, format!("合并被跳过：{}", e));
                    continue;
                }
            };
            hist.upsert(MediaItem {
                status: to,
                ..item.clone()
            });
            jobs.push(id.clone());
        }
    }
    if jobs.len() < 2 {
        return Err("可合并条目不足 2 个".into());
    }
    let norm = normalize.unwrap_or_else(|| {
        state.config.lock().unwrap().general.normalize_audio
    });
    let job = MergeJob {
        ids: jobs,
        filename: filename.unwrap_or_else(default_merge_name),
        container: container.unwrap_or_else(|| "mp4".into()),
        encoder_mode: encoder.unwrap_or_else(|| "auto".into()),
        normalize_audio: norm,
    };
    for id in &job.ids {
        state
            .merge_jobs
            .lock()
            .unwrap()
            .insert(id.clone(), job.clone());
    }
    for id in &job.ids {
        let outcome = {
            let mut q = state.queue.lock().unwrap();
            q.submit(id)
        };
        if outcome == SubmitOutcome::Queued {
            log_item(&app, id, "已排队，等待并发 slot…");
        } else {
            log_item(&app, id, "开始合并…");
            let app2 = app.clone();
            let id2 = id.clone();
            std::thread::spawn(move || run_merge_task(app2, id2));
        }
    }
    persist(&app);
    Ok(())
}

/// 默认合并输出名：合并_<时间戳>（MG-06）。
fn default_merge_name() -> String {
    format!("合并_{}", today_stamp())
}

fn today_stamp() -> String {
    let s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (s / 86400) as i64 + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}{:02}{:02}", y, m, d)
}

/// 合并任务线程（提交队列后执行；进度经 `item:update` 回推）。
fn run_merge_task(app: AppHandle, id: String) {
    let state = app.state::<AppState>();
    let cancel = state.register_cancel(&id);
    let (inputs, params) = {
        let job = state
            .merge_jobs
            .lock()
            .unwrap()
            .get(&id)
            .cloned();
        let Some(job) = job else { return };
        let mut inputs = Vec::new();
        let mut anchor = None;
        {
            let hist = state.history.lock().unwrap();
            for jid in &job.ids {
                if let Some(item) = hist.get(jid) {
                    if anchor.is_none() {
                        anchor = Some(item.clone());
                    }
                    if let Some(p) = &item.path {
                        inputs.push(std::path::PathBuf::from(p));
                    }
                }
            }
        }
        let cfg = state.config.lock().unwrap().clone();
        let p = MergeParams {
            inputs,
            out_dir: default_output_dir(&state, &anchor.unwrap_or_else(|| MediaItem::new(ItemKind::MergeOut, "合并".into()))),
            filename: job.filename.clone(),
            container: job.container.clone(),
            encoder_mode: job.encoder_mode.clone(),
            low_power: cfg.transcode.low_power,
            collision_policy: cfg.general.collision_policy.clone(),
            normalize_audio: job.normalize_audio,
            max_gain_db: cfg.general.max_gain_db,
        };
        (p.inputs.clone(), p)
    };
    if inputs.len() < 2 {
        log_item(&app, &id, "合并输入不足，已取消");
        let _ = state.merge_jobs.lock().unwrap().remove(&id);
        return;
    }
    let app2 = app.clone();
    let id2 = id.clone();
    let result = merge::run_merge(
        &state.resolver(),
        &params,
        &cancel,
        move |pct| {
            update_item(&app2, &id2, |it| {
                it.percent = pct;
            });
        },
        |line| log_item(&app, &id, line),
    );
    finish_merge(&app, &id, result);
}

/// 合并收尾：原条目恢复、产物作为新条目回列表、释放队列 slot。
fn finish_merge(app: &AppHandle, id: &str, result: Result<std::path::PathBuf, CoreError>) {
    let state = app.state::<AppState>();
    let (orig_status, final_status) = match &result {
        Ok(out) => {
            log_item(app, id, format!("合并完成，产物回列表：{}", out.display()));
            (restore_status(app, id), Status::Done)
        }
        Err(CoreError::Cancelled) => {
            log_item(app, id, "合并已取消，清理残留");
            (restore_status(app, id), Status::Canceled)
        }
        Err(e) => {
            log_item(app, id, format!("合并失败：{}", e));
            (restore_status(app, id), Status::Failed)
        }
    };
    update_item(app, id, |it| {
        it.status = orig_status;
        it.percent = if final_status == Status::Done {
            100.0
        } else {
            it.percent
        };
        if let Err(e) = &result {
            it.error = Some(e.to_string());
        }
    });
    if let Ok(out) = &result {
        let meta = download::probe_output(&state.resolver(), out).unwrap_or_default();
        let title = out
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "合并产物".into());
        let mut prod = MediaItem::new(ItemKind::MergeOut, title);
        prod.path = Some(out.to_string_lossy().into_owned());
        prod.status = Status::Done;
        prod.percent = 100.0;
        prod.meta = meta;
        prod.updated_at = now_str();
        state.history.lock().unwrap().upsert(prod.clone());
        let _ = app.emit("item:ready", serde_json::json!({ "id": prod.id }));
        let _ = app.emit("list:changed", ());
    }
    state.merge_jobs.lock().unwrap().remove(id);
    let next = {
        let mut q = state.queue.lock().unwrap();
        q.finish(id)
    };
    if let Some(next_id) = next {
        launch_next(app, next_id);
    }
    persist(app);
}

/// 设置时间范围下载（DL-12）：起止 "HH:MM:SS"；空串清除。
#[tauri::command]
pub fn set_sections(app: AppHandle, id: String, start: String, end: String) -> CmdResult<()> {
    let valid = |t: &str| {
        if t.is_empty() {
            return true;
        }
        let parts: Vec<&str> = t.split(':').collect();
        if parts.len() != 3 {
            return false;
        }
        parts.iter().all(|p| {
            !p.is_empty()
                && p.chars().all(|c| c.is_ascii_digit())
                && p.len() <= 2
        })
    };
    if !valid(&start) || !valid(&end) {
        return Err("时间格式应为 HH:MM:SS（如 00:01:00）".into());
    }
    let sections = if start.is_empty() && end.is_empty() {
        None
    } else {
        Some((start, end))
    };
    let state = app.state::<AppState>();
    update_item(&app, &id, |it| {
        it.sections = sections.clone();
        match &sections {
            Some((s, e)) => {
                it.push_log(format!("已设置时间范围下载：{} - {}", s, e));
            }
            None => {
                it.push_log("已清除时间范围".to_string());
            }
        }
    });
    state.persist();
    Ok(())
}

/// 批量转码（TC-05：按 设置-转码/通用 默认参数执行，不弹确认窗）。
#[tauri::command]
pub fn start_transcode(app: AppHandle, ids: Vec<String>) -> CmdResult<()> {
    let state = app.state::<AppState>();
    if ids.is_empty() {
        return Err("未选择条目".into());
    }
    let mut to_run = Vec::new();
    {
        let mut hist = state.history.lock().unwrap();
        for id in &ids {
            let item = hist.get(id).cloned().ok_or("条目不存在")?;
            let has_file = item.path.as_deref().map(|p| std::path::Path::new(p).is_file()).unwrap_or(false);
            if !has_file {
                log_item(&app, id, "转码被跳过：无本地输入文件（先下载或添加本地文件）");
                continue;
            }
            let to = match transition(item.status, Status::Transcoding) {
                Ok(t) => t,
                Err(e) => {
                    log_item(&app, id, format!("转码被跳过：{}", e));
                    continue;
                }
            };
            hist.upsert(MediaItem {
                status: to,
                ..item.clone()
            });
            to_run.push(id.clone());
        }
    }
    if to_run.is_empty() {
        return Err("没有可转码的条目（需要已解析的本地文件）".into());
    }
    for id in to_run {
        let outcome = {
            let mut q = state.queue.lock().unwrap();
            q.submit(&id)
        };
        if outcome == SubmitOutcome::Queued {
            log_item(&app, &id, "已排队，等待并发 slot…");
        } else {
            log_item(&app, &id, "开始转码…");
            let app2 = app.clone();
            std::thread::spawn(move || run_transcode_task(app2, id));
        }
    }
    persist(&app);
    Ok(())
}

/// 播放列表展开（DL-09）：flat-playlist 拿每集 URL，逐条作为独立条目解析平铺。
fn expand_playlist(
    app: &AppHandle,
    id: &str,
    item: &MediaItem,
    resolver: &ToolResolver,
    cookies: Option<&Path>,
    network: &NetworkConfig,
) {
    let url = item.url.clone().unwrap_or_default();
    if url.is_empty() {
        return;
    }
    match probe::list_playlist_entries(resolver, &url, cookies, network) {
        Ok(entries) => {
            let n = entries.len();
            log_item(app, id, format!("播放列表展开：{} 集", n));
            let app2 = app.clone();
            for e in entries {
                let entry = MediaItem::from_url(e.url);
                let eid = entry.id.clone();
                {
                    let st = app2.state::<AppState>();
                    let mut hist = st.history.lock().unwrap();
                    hist.upsert(entry);
                }
                let app3 = app2.clone();
                std::thread::spawn(move || run_probe(app3, eid));
            }
        }
        Err(f) => {
            log_item(app, id, format!("播放列表展开失败：{}", f.message));
        }
    }
}

/// 转码任务线程（提交队列后执行；进度经 `item:update` 回推）。
fn run_transcode_task(app: AppHandle, id: String) {
    let state = app.state::<AppState>();
    let cancel = state.register_cancel(&id);
    let (_input, params, meta) = {
        let hist = state.history.lock().unwrap();
        let item = match hist.get(&id) {
            Some(i) => i.clone(),
            None => return,
        };
        let path = match &item.path {
            Some(p) => std::path::PathBuf::from(p),
            None => return,
        };
        let cfg = state.config.lock().unwrap().clone();
        let out_dir = default_output_dir(&state, &item);
        let params = TranscodeParams {
            input: path.clone(),
            out_dir,
            title: item.title.clone(),
            filename_template: cfg.download.filename_template.clone(),
            container: "mp4".into(),
            encoder_mode: cfg.transcode.force_encoder_mode.clone(),
            low_power: cfg.transcode.low_power,
            max_w: cfg.transcode.max_w,
            max_h: cfg.transcode.max_h,
            brcap_kbps: cfg.transcode.brcap_kbps,
            normalize_audio: cfg.general.normalize_audio,
            max_gain_db: cfg.general.max_gain_db,
            rot_angle: item.rot_angle,
            keep_cover: cfg.transcode.keep_cover,
            collision_policy: cfg.general.collision_policy.clone(),
        };
        (path, params, item.meta.clone())
    };
    let app2 = app.clone();
    let id2 = id.clone();
    let result = transcode::run_transcode(
        &state.resolver(),
        &params,
        &meta,
        &cancel,
        move |pct| {
            update_item(&app2, &id2, |it| {
                it.percent = pct;
            });
        },
        |line| log_item(&app, &id, line),
    );
    finish_transcode(&app, &id, result);
}

/// 转码收尾：原条目恢复、产物作为新条目回列表、释放队列 slot。
fn finish_transcode(
    app: &AppHandle,
    id: &str,
    result: Result<std::path::PathBuf, CoreError>,
) {
    let state = app.state::<AppState>();
    let (orig_status, final_status) = match &result {
        Ok(out) => {
            log_item(app, id, format!("转码完成，产物回列表：{}", out.display()));
            (restore_status(app, id), Status::Done)
        }
        Err(CoreError::Cancelled) => {
            log_item(app, id, "已取消，清理残留");
            (restore_status(app, id), Status::Canceled)
        }
        Err(e) => {
            log_item(app, id, format!("转码失败：{}", e));
            (restore_status(app, id), Status::Failed)
        }
    };
    update_item(app, id, |it| {
        it.status = orig_status;
        it.percent = if final_status == Status::Done {
            100.0
        } else {
            it.percent
        };
        if let Err(e) = &result {
            it.error = Some(e.to_string());
        }
    });
    // 成功：产物作为新条目回到列表（TC-11）
    if let Ok(out) = &result {
        let meta = probe_output(&state.resolver(), out).unwrap_or_default();
        let title = out
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "转码产物".into());
        let mut prod = MediaItem::new(ItemKind::TranscodeOut, title);
        prod.path = Some(out.to_string_lossy().into_owned());
        prod.status = Status::Done;
        prod.percent = 100.0;
        prod.meta = meta;
        prod.updated_at = now_str();
        state.history.lock().unwrap().upsert(prod.clone());
        let _ = app.emit("item:ready", serde_json::json!({ "id": prod.id }));
        let _ = app.emit("list:changed", ());
    }
    // 释放并发 slot，启动下一个等待任务
    let next = {
        let mut q = state.queue.lock().unwrap();
        q.finish(id)
    };
    if let Some(next_id) = next {
        launch_next(app, next_id);
    }
    persist(app);
}

/// 转码结束后原条目恢复状态：本地文件 → 已就绪；下载产物/转码产物 → 已完成（可再转码）。
fn restore_status(app: &AppHandle, id: &str) -> Status {
    let state = app.state::<AppState>();
    let hist = state.history.lock().unwrap();
    let Some(item) = hist.get(id) else { return Status::Ready };
    match item.kind {
        ItemKind::LocalFile | ItemKind::TranscodeOut => Status::Ready,
        _ => Status::Done,
    }
}

/// 队列 slot 释放后启动下一个等待任务（下载/转码按条目状态分流；M3 合并接入）。
fn launch_next(app: &AppHandle, next_id: String) {
    let app2 = app.clone();
    std::thread::spawn(move || {
        let st = app2.state::<AppState>();
        let item = {
            let h = st.history.lock().unwrap();
            h.get(&next_id).cloned()
        };
        let Some(item) = item else { return };
        log_item(&app2, &next_id, "开始执行…");
        match item.status {
            Status::Transcoding => run_transcode_task(app2, next_id),
            Status::Merging => run_merge_task(app2, next_id),
            _ => {
                log_item(&app2, &next_id, "开始下载…");
                let (fid, aonly) = (item.format_id.clone(), item.audio_only);
                run_download_task(app2, next_id, fid, aonly);
            }
        }
    });
}

fn now_str() -> String {
    let s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (s / 86400) as i64 + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let sec = s % 86400;
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        y,
        m,
        d,
        sec / 3600,
        (sec % 3600) / 60,
        sec % 60
    )
}

#[tauri::command]
pub fn cancel_item(app: AppHandle, id: String) -> CmdResult<()> {
    let state = app.state::<AppState>();
    // 等待中：直接移除
    let removed = {
        let mut q = state.queue.lock().unwrap();
        q.cancel_waiting(&id)
    };
    if removed {
        update_item(&app, &id, |it| {
            it.status = Status::Canceled;
            it.push_log("已取消（队列中移除）".to_string());
        });
        persist(&app);
        return Ok(());
    }
    // 运行中：置取消标志，进程树由任务线程终止
    if let Some(flag) = state.cancel_flag(&id) {
        flag.store(true, Ordering::Relaxed);
        update_item(&app, &id, |it| {
            it.status = Status::Canceled;
            it.push_log("正在取消…".to_string());
        });
        persist(&app);
        Ok(())
    } else {
        Err("该任务未在运行中".into())
    }
}

#[tauri::command]
pub fn remove_item(app: AppHandle, id: String) -> CmdResult<()> {
    let state = app.state::<AppState>();
    let mut hist = state.history.lock().unwrap();
    if hist.remove(&id) {
        drop(hist);
        persist(&app);
        let _ = app.emit("item:removed", id);
        Ok(())
    } else {
        Err("条目不存在".into())
    }
}

#[tauri::command]
pub fn clear_done(app: AppHandle) -> CmdResult<()> {
    let state = app.state::<AppState>();
    state.history.lock().unwrap().clear_done();
    persist(&app);
    let _ = app.emit("list:changed", ());
    Ok(())
}

#[tauri::command]
pub fn retry_item(app: AppHandle, id: String) -> CmdResult<()> {
    let state = app.state::<AppState>();
    let ok = {
        let mut hist = state.history.lock().unwrap();
        let item = hist.get(&id).cloned().ok_or("条目不存在")?;
        if !item.status.is_terminal() && item.status != Status::NeedLogin {
            return Err("仅失败/已取消/需要登录可重试".into());
        }
        let ns = transition(item.status, Status::Probing).map_err(err_string)?;
        hist.upsert(MediaItem {
            status: ns,
            error: None,
            ..item
        });
        true
    };
    if ok {
        let app2 = app.clone();
        std::thread::spawn(move || run_probe(app2, id));
        persist(&app);
    }
    Ok(())
}

#[tauri::command]
pub fn relogin_item(app: AppHandle, id: String) -> CmdResult<()> {
    let state = app.state::<AppState>();
    let host = {
        let hist = state.history.lock().unwrap();
        let item = hist.get(&id).ok_or("条目不存在")?;
        item.host.clone().or_else(|| {
            item.url
                .as_deref()
                .and_then(ytdlp_core::cookies::host_from_url)
        })
    };
    let host = host.ok_or("无法确定登录站点")?;
    let login_url = login::login_url_for_host(&host).ok_or("该站点不支持内置登录")?;
    login::open_login(&app, &host, &login_url).map_err(err_string)?;
    Ok(())
}

// ---------- 配置 ----------

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> CmdResult<AppConfig> {
    Ok(state.config.lock().unwrap().clone())
}

#[tauri::command]
pub fn save_config(app: AppHandle, config: AppConfig) -> CmdResult<()> {
    let state = app.state::<AppState>();
    {
        let mut cur = state.config.lock().unwrap();
        *cur = config.clone();
    }
    let path = state.paths.config_file();
    let _ = std::fs::create_dir_all(state.paths.config_dir());
    config.save(&path).map_err(err_string)?;
    // 并发上限即时生效
    state
        .queue
        .lock()
        .unwrap()
        .set_concurrency(config.general.concurrency as usize);
    Ok(())
}

// ---------- Cookie ----------

#[tauri::command]
pub fn list_cookies(state: State<'_, AppState>) -> CmdResult<Vec<serde_json::Value>> {
    let store = CookieStore::new(state.paths.cookies_dir());
    let mut out = Vec::new();
    for host in store.list_hosts().map_err(err_string)? {
        let cookies = store.load_host(&host).map_err(err_string)?;
        out.push(serde_json::json!({
            "host": host,
            "count": cookies.len(),
            "expires_at": cookies.iter().filter_map(|c| c.expires).fold(0.0, f64::max),
        }));
    }
    Ok(out)
}

#[tauri::command]
pub fn save_cookies(
    app: AppHandle,
    host: String,
    cookies: Vec<ytdlp_core::cookies::CookieEntry>,
) -> CmdResult<()> {
    let state = app.state::<AppState>();
    let store = CookieStore::new(state.paths.cookies_dir());
    store.save_host(&host, cookies).map_err(err_string)?;
    let _ = app.emit("cookies:changed", host);
    Ok(())
}

#[tauri::command]
pub fn delete_cookie(app: AppHandle, host: String) -> CmdResult<()> {
    let state = app.state::<AppState>();
    let store = CookieStore::new(state.paths.cookies_dir());
    store.delete_host(&host).map_err(err_string)?;
    let _ = app.emit("cookies:changed", host);
    Ok(())
}

// ---------- 依赖自检 ----------

#[tauri::command]
pub fn probe_dependencies(state: State<'_, AppState>) -> CmdResult<Vec<ToolStatus>> {
    let resolver = state.resolver();
    let mut out = Vec::new();
    for tool in [
        ytdlp_core::exec::Tool::YtDlp,
        ytdlp_core::exec::Tool::Ffmpeg,
        ytdlp_core::exec::Tool::Ffprobe,
        ytdlp_core::exec::Tool::Deno,
    ] {
        let resolved = resolver.resolve(tool);
        let (path, version, ok) = match &resolved {
            Ok(p) => {
                let v = ytdlp_core::exec::tool_version(&resolver, tool);
                (Some(p.to_string_lossy().into_owned()), v, true)
            }
            Err(_) => (None, None, false),
        };
        out.push(ToolStatus {
            tool: tool.name().to_string(),
            path,
            version,
            ok,
        });
    }
    Ok(out)
}

// ---------- 其他 ----------

#[tauri::command]
pub fn open_item_dir(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let hist = state.history.lock().unwrap();
    let item = hist.get(&id).ok_or("条目不存在")?;
    let target = item
        .file
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| item.path.clone().map(PathBuf::from))
        .ok_or("该条目没有本地文件")?;
    let dir = if target.is_dir() {
        target
    } else {
        target.parent().map(Path::to_path_buf).unwrap_or(target)
    };
    open_in_explorer(&dir)
}

#[cfg(windows)]
fn open_in_explorer(dir: &Path) -> CmdResult<()> {
    std::process::Command::new("explorer")
        .arg(dir)
        .spawn()
        .map_err(err_string)?;
    Ok(())
}

#[cfg(not(windows))]
fn open_in_explorer(_dir: &Path) -> CmdResult<()> {
    Err("仅 Windows 支持打开目录".into())
}

#[tauri::command]
pub fn clear_temp(state: State<'_, AppState>) -> CmdResult<()> {
    let dir = state.paths.temp_dir();
    if dir.is_dir() {
        for e in std::fs::read_dir(&dir).map_err(err_string)? {
            let e = e.map_err(err_string)?;
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    Ok(())
}

/// 设置条目旋转角度（UL-12：随条目保存，转码时生效；M2 使用）。
#[tauri::command]
pub fn rot_item(app: AppHandle, id: String, degrees: u16) -> CmdResult<()> {
    let angle = ytdlp_core::model::RotAngle::from_degrees(degrees);
    update_item(&app, &id, |it| {
        it.rot_angle = angle;
    });
    Ok(())
}
