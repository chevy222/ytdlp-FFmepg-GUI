# ytdlp-FFmpeg-GUI（影栈）开发里程碑

| 项 | 内容 |
| --- | --- |
| 项目代号 | ytdlp-FFmpeg-GUI |
| 产品中文名 | 影栈 |
| 代码仓库 | https://github.com/chevy222/ytdlp-FFmpeg-GUI.git |
| 分支 | main |
| 文档日期 | 2026-09-18 |
| 关联文档 | 需求文档 v1.0 / 效果图 / 架构总览图（同目录 docs/） |

---

## 里程碑总览

| 阶段 | Commit | 日期 | 内容 | 验证状态 |
| --- | --- | --- | --- | --- |
| M0 工程初始化 | `8e0d1e2` | 09-17 | workspace 骨架 + 核心层（模型/状态机/配置/历史，48 单测）+ Tauri v2 空窗口 + Windows CI 占位 | 通过 |
| M1 解析 + 列表 + 下载 | `863c7cd` | 09-17 | 核心五模块（exec/cookies/probe/download/worker）+ src-tauri 18 命令 + 前端效果图接入（真实 invoke） | 通过 |
| 交叉编译修复 | `3947696` | 09-17 | 修通 Linux 环境交叉编译 Windows 单 EXE（cargo-xwin + zig），webview2-com/windows 版本对齐，流程固化到 scripts/cross-build/ | 通过 |
| M2 转码 | `1fda862` | 09-17 | 转码引擎（H.265：QSV→libx265 兜底/NVENC/AMF，旋转/分辨率/码率/增益/封面）+ 批量转码命令 + 列表封面旋转交互 | 通过 |
| M3 合并 | `a4a9077` | 09-17 | 合并引擎（同参直拼/异参统一）+ 合并面板 + 产物回列表 | 通过 |
| M4 CLI 入口 | `86a5163` | 09-17 | CLI 解析（--url/--cookies/--dir/--yt-dlp-path/--deno-path）+ 单实例转发（UL-09） | 通过 |
| P1 增强 | `0f9fe6a` | 09-17 | 播放列表展开平铺（DL-09）+ 时间范围下载（DL-12）+ 合并音量归一化（MG-05） | 通过 |
| P2/TC-16 | `33dc546` | 09-18 | 硬件编码器探测（QSV/NVENC/AMF）+ 硬编失败自动回退 libx265 | 通过 |
| 发布收尾 | `ef9cc88` | 09-18 | Release 附件补单 EXE 直传（zip + exe）；README 里程碑对齐 | 通过 |
| CI 修复 | `223e5b0` | 09-18 | exec 单测平台无关化（tool_names cfg windows 分支） | 通过 |

> 后续规划（P2/P3 尾项，未被要求实现）：MG-07 音视频合成、Q11 浏览器扩展桥接。

---

## 各阶段详情

### M0 工程初始化（8e0d1e2）

- workspace 三 crate：`crates/core`（平台无关可单测）、`src-tauri`（Tauri v2 应用壳）、`ui/`（单页原生 JS 前端）。
- 核心层落地：`MediaItem`/`MediaMeta` 模型、状态机（含迁移白名单）、`AppConfig`（JSON 原子写、损坏备份回退）、历史持久化。
- `scripts/build.ps1`（pwsh 7，与 CI 同构）、`.github/workflows/build.yml`（Windows runner）。
- 空窗口可运行（Tauri v2）。

### M1 解析 + 列表 + 下载（863c7cd）

- `exec.rs`：ToolResolver（PATH / 配置 / 托管 tools 目录三级解析）、进程管理、kill_tree 取消。
- `cookies.rs`：Cookie 按 HOST 匹配 + 站点级回退 + X↔twitter 姊妹域名互退。
- `probe.rs`：URL 元数据（yt-dlp -j）+ 本地文件（ffprobe + volumedetect：分辨率/编码/帧率/音轨/封面/最大音量）。
- `download.rs`：yt-dlp 下载（格式选择、PO-Token、cookie、进度解析、取消清残留）。
- `worker.rs` + src-tauri 18 命令：添加/解析/下载/登录（WebView2 内嵌登录，YouTube SPA 800ms 重注入按钮兜底）/取消/重试等。
- 前端效果图接入真实 invoke：统一列表、5s 倒计时自动下载、画质下拉、批量栏、日志按条目。

### 交叉编译修复（3947696）

- Linux 直接 `cargo check src-tauri` 因 gdk-sys 缺 GTK 必失败 → 统一 xwin 交叉检查/构建 Windows 目标（`x86_64-pc-windows-msvc`）。
- webview2-com 0.38 / windows 0.61 版本分裂修复；login.rs/login_win.rs 重写。
- 构建三件套固化在 `scripts/cross-build/README.md` + 4 个 wrapper（cargo-xwin/zig 等）。
- 产物：`target/x86_64-pc-windows-msvc/release/ytdlp-FFmpeg-GUI.exe`（~11MB）。

### M2 转码（1fda862）

- `transcode.rs`：编码器协商（auto=QSV→libx265 兜底、nvenc cq23 p5、amf qp23）、旋转 transpose、分辨率上限、码率封顶、音量增益上限、保留封面、mp4 hvc1 tag、+faststart、文件名模板（纯标题/标题+ID/日期-标题）、碰撞 auto_inc/skip、`-progress pipe:1` 进度、取消 kill_tree + 删残留。
- src-tauri：start_transcode/run_transcode_task/finish_transcode（产物 TranscodeOut 回列表、release slot）+ launch_next 统一调度（下载/转码按状态分流）。
- 状态机加 `Done→Transcoding`；UI 封面旋转交互（缩略图上旋转按钮，封面按目标方向旋转）。

### M3 合并（a4a9077）

- `merge.rs`：同参直拼（MG-02：vcodec/height/fps/acodec/sample_rate/extradata SPS-PPS 一致 → concat demuxer 零重编码）/ 异参统一（MG-03：逐段 H.265 转码后直拼）；输出 MP4/MKV + faststart；temp/merge_<uuid>/ 私有目录结束清理；取消清理。
- MediaMeta 增 extradata/sample_rate 字段（probe 采集，直拼硬性判据）。
- 状态机加 `Done→Merging`；UI 合并面板（顺序上移下移/移除、容器/编码器/文件名、参数差异预警）。

### M4 CLI 入口（86a5163，UL-09）

- `cli.rs` 纯解析：`--url`（可重复）/`--cookies`/`--dir`/`--yt-dlp-path`/`--deno-path`/裸位置参数当 URL。
- `tauri-plugin-single-instance`：第二实例 argv 转发给主实例（存 CliOverrides + add_url）；本次调用级覆盖不写 config.json。
- `--dir` 输出目录优先、`--cookies` resolve_cookies 优先并统一 probe/下载路径、工具路径进 ToolResolver。

### P1 增强（0f9fe6a）

- **DL-09 播放列表展开**：probe_url 支持 `--yes-playlist`（设置-下载 播放列表开关）；解析出合集时 `--flat-playlist` 展开每集 URL 平铺进统一列表逐条解析/下载。
- **DL-12 时间范围下载**：MediaItem 增 sections 字段，下载传 yt-dlp `--download-sections`；行操作"剪辑"按钮（modal 输起止 HH:MM:SS，清空移除）。
- **MG-05 合并音量归一化**：合并完成后 probe 产物音量 → 视频 copy、音频 aac 增益至峰值 0dBFS（上限来自通用设置）；合并面板开关默认跟随通用。

### P2/TC-16（33dc546）

- `HwEncoders{qsv,nvenc,amf}` + `detect_hw_encoders`（ffmpeg -encoders，解析纯函数可单测）。
- `run_transcode` 包一层：显式 NVENC/AMF 或自动探测 QSV 运行时失败（非取消）自动用 libx265 重试一次，进度/日志延续。
- UI 设置-转码默认编码器下拉加载时探测硬件，未检测到的选项标注"（未检测到）"（仍可选）。

### 发布收尾（ef9cc88）+ CI 修复（223e5b0）

- `release.yml`：push `v*` tag → Windows runner → 单测 → clippy(-D warnings) → release 构建 → GitHub Release 草稿上传 **zip + 单 EXE 直传**。
- CI 平台化修复：`exec.rs` tool_names 测试按 `cfg(windows)` 断言 `.exe` 后缀，全仓排查无其它硬编码文件名。

---

## 质量基线（当前）

- `cargo test -p ytdlp-core`：**110 passed**。
- `cargo clippy -p ytdlp-core --all-targets -- -D warnings` 与 `cargo xwin clippy -p ytdlp-gui`：**0 警告**。
- Release 单 EXE 构建通过：`target/x86_64-pc-windows-msvc/release/ytdlp-FFmpeg-GUI.exe`（~11MB）。
- UI 自检（html skill shot.py）：无控制台错误、无 lint 触发。
- 待办：打 `v1.0.0` tag 触发 CI 出 Release 草稿；Windows 真机端到端验证（下载→转码→合并实链路）。
