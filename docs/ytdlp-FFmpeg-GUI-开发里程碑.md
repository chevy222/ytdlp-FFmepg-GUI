# ytdlp-FFmpeg-GUI（影栈）工程与构建说明

> 本文档描述当前仓库的**工程配置、构建流程、CI 门禁与代码约定**，与代码同步；不含提交历史与阶段记录。
> 功能行为与实现细节见 `docs/ytdlp-FFmpeg-GUI-doubao-需求文档.md`。

| 项 | 内容 |
| --- | --- |
| workspace | 根 `Cargo.toml`（`resolver = "2"`，members = `crates/core`、`src-tauri`） |
| crate | `ytdlp-core`（库，平台无关核心层）· `ytdlp-gui`（bin `ytdlp-FFmpeg-GUI` + rlib `ytdlp_gui_lib`） |
| edition / MSRV | edition 2021 · `rust-version = 1.85`（workspace 统一） |
| 许可证 | MIT |
| 目标平台 | Windows x64（MSVC）；核心层保持平台无关以便在 Linux 上单测 |

## 1. 工具链与依赖

`rust-toolchain.toml`：`channel = "stable"`，`components = ["rustfmt", "clippy"]`，`targets = ["x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"]`。

workspace 依赖（`Cargo.toml`）：

| 依赖 | 版本 | 用途 |
| --- | --- | --- |
| serde / serde_json | 1 | 全部模型的序列化与 JSON 存储 |
| thiserror | 2 | `CoreError` 错误类型 |
| uuid (v4) | 1 | 条目 id、任务临时目录名 |
| url | 2 | Cookie/站点 host 解析 |
| regex | 1 | yt-dlp 进度行解析 |

`crates/core` 额外依赖：`zip` 0.6（`default-features = false, features = ["deflate"]`，解压工具链 zip）、`sha2` 0.10（工具链校验）、`encoding_rs` 0.8（子进程输出 GBK 回退解码）；dev 依赖 `tempfile` 3。

`src-tauri` 依赖：`tauri` 2（`protocol-asset`）、`tauri-plugin-single-instance` 2、`tauri-plugin-dialog` 2、`ytdlp-core`（path）；Windows 目标额外 `webview2-com` 0.38、`windows` 0.61（`Win32_Foundation`，用于 COM CookieManager）；build 依赖 `tauri-build` 2。

应用配置（`src-tauri/tauri.conf.json`）：单窗口「影栈（ytdlp-FFmpeg-GUI）」，1280×800（最小 1024×640，居中）；`frontendDist = ../ui`（编译期由 `generate_context!` 嵌入单 EXE）；`withGlobalTauri = true`；`assetProtocol` 开启（用于展示 `config/cache/thumbs` 缩略图）；能力文件 `capabilities/default.json` 仅授予 `core:default` 与 `dialog:default`。

## 2. 本地构建

```powershell
pwsh ./scripts/build.ps1
```

`scripts/build.ps1`（PowerShell 7，`$ErrorActionPreference = 'Stop'`）步骤：

1. `cargo test -p ytdlp-core` —— 失败即中断；
2. `cargo clippy -p ytdlp-core --all-targets -- -D warnings` —— 失败即中断；
3. `cargo audit` —— 未安装或发现公告只告警，不中断；
4. `cargo build --release`；
5. 校验 `target\release\ytdlp-FFmpeg-GUI.exe` 存在并输出体积。

产物：`target/release/ytdlp-FFmpeg-GUI.exe`（workspace 根即仓库根，target 在仓库根下）。单 EXE 绿色便携：前端资源在编译期嵌入，运行时目录（`config/`、`temp/`、`tools/`）在 exe 同级自动创建，不写注册表、不依赖安装包。

前置条件：Windows 10/11 x64 + WebView2 运行时（Win11 自带）+ rustup stable（含 rustfmt/clippy）。

### Linux 环境交叉编译（本地备用通道）

见 `scripts/cross-build/README.md`：用 cargo-xwin / zigbuild 交叉编译 `x86_64-pc-windows-msvc`，产物在 `target/x86_64-pc-windows-msvc/release/ytdlp-FFmpeg-GUI.exe`。该通道**不用于 CI**。

## 3. CI（GitHub Actions，windows-latest）

`.github/workflows/build.yml`（push main / PR）：

```
checkout → 安装 rust stable(+rustfmt,clippy) → 缓存
→ cargo test -p ytdlp-core
→ cargo clippy -p ytdlp-core --all-targets -- -D warnings
→ cargo clippy -p ytdlp-gui  --all-targets -- -D warnings
→ cargo install cargo-audit --locked && cargo audit      # continue-on-error：信息性，不阻塞
→ cargo build --release
→ 校验 target\release\ytdlp-FFmpeg-GUI.exe 存在
→ 上传 artifact ytdlp-FFmpeg-GUI-win64
```

`.github/workflows/release.yml`（push tag `v*`）：同样的门禁步骤，随后把单 EXE 压成 `ytdlp-FFmpeg-GUI-<tag>-win64.zip`，用 `softprops/action-gh-release` 创建**草稿** Release（`permissions: contents: write`）。

## 4. 质量基线

- `cargo test -p ytdlp-core`：**149 个用例**（`#[cfg(test)]` 静态计数，分布：model 31、download 15、exec 13、probe 13、config 12、transcode 11、history 10、merge 8、tool_download 8、cookies 7、worker 6、cli 5、paths 4、timefmt 4、thumbs 2）。另有 `src-tauri`（login）4 个用例，`cargo test --workspace` 时一并执行（CI 门禁只跑 core）。
- `cargo clippy -p ytdlp-core --all-targets -- -D warnings` 与 `cargo clippy -p ytdlp-gui --all-targets -- -D warnings` 均为 CI 门禁。
- 核心层测试全部平台无关（不依赖 Windows 特有 API、不硬编码 `.exe` 后缀——按 `cfg(windows)` 断言）；涉及子进程的测试只做存在性/解析断言，不依赖外部工具是否安装。

## 5. 代码约定

- **分层**：`crates/core` 只做纯逻辑与子进程编排，不依赖 GUI；`src-tauri` 负责命令桥接、全局状态与后台线程；`ui/index.html` 为单文件前端（原生 JS，通过 `window.__TAURI__.core.invoke` 与事件通信）。
- **错误类型**：核心层统一 `CoreError`（`Io` / `Json` / `InvalidTransition` / `NotFound` / `ConfigCorrupt` / `Cancelled` / `ProcessFailed`）；应用层命令统一 `Result<T, String>` 回传前端。
- **状态变更**：业务状态迁移必须走 `model::transition` 白名单校验；进度类高频更新只 `emit` 前端不落盘，状态迁移才持久化 history.json。
- **文件安全**：JSON 原子写（临时文件 + 单次 rename）；媒体产物先写临时文件、校验后 rename 覆盖；取消/失败清理本任务临时目录与半成品，绝不删除 exe 同级目录之外的内容。
- **子进程**：全部经 `Command` 参数化调用（不拼 shell）；Windows 下加 `CREATE_NO_WINDOW`；取消用 `taskkill /PID <pid> /T /F` 终止进程树。
- **锁纪律**：`history` 等全局 `std::sync::Mutex` 不可重入——持锁期间不做文件 IO、不 `emit` 事件、不调用会再次加锁的函数（`log_item`/`update_item` 先出锁再调用）；Cookie 导出等 IO 在锁外做（锁内只克隆数据）。
- **日期/时间**：一律走 `crates/core/src/timefmt.rs`（epoch 秒 + 本地时区偏移），不按 UTC 手算日期。
- **注释语言**：中文；注释说明"为什么"（约束、坑、外部工具行为），不复述代码。
- 外部工具参数约束（封面映射、`-tag:v:0`、偶数对齐、`--ignore-errors` 等）见实现说明 §11 —— **改动 ffmpeg/yt-dlp 参数前必读**。

## 6. 发布

- 交付物：**单个 exe**（`target/release/ytdlp-FFmpeg-GUI.exe`），不做安装包（无 NSIS/MSI、不写注册表）。
- CI release 只做"单 EXE 套一层 zip"，不做打包器。

## 7. 手动验收清单

发版前在 Windows 真机按顺序走一遍（本仓库的自动化测试只覆盖核心层纯逻辑，外部工具链路需要真机验收）：

1. 首次启动：exe 同级生成 `config/`、`temp/`、`tools/`；设置-依赖自检显示 yt-dlp / ffmpeg / ffprobe / deno 状态与版本——**PATH 里已安装的工具同样要显示"已找到"+ 干净版本号**（`tools\` 为空不应影响）。
2. 依赖下载/更新：设置-依赖 对四个工具执行"下载"，确认落在 `tools\` 且路径被写回输入框；再点"更新"——yt-dlp / deno 与 ffmpeg / ffprobe 都应给出"已是最新（版本号），无需更新"或更新到新版本（ffmpeg/ffprobe 的版本号来自 gyan.dev `release-version`）；若把某个路径填成 PATH 里的工具（如 `C:\Windows\System32` 下），点"更新"应提示手动更新而不是去改它。每行"链接"按钮弹窗展示下载地址与构建页、可复制。下载中按钮显示"取消 <阶段> <百分比>"并持续前进（连接 → 下载 → 校验 → 解压 → 安装）。
3. 下载：粘贴一个 URL → 5 秒倒计时自动下载 → 状态依次 解析中 → 已就绪 → 下载中 → 后处理中 → 已完成；产物出现在默认输出目录，条目 `path` 已回填（行内出现"转码"按钮）。
4. 封面/画质列：条目缩略图出现，画质列显示 12 项源元数据（含采样率与音轨数）。
5. 下载→转码：对刚下载的条目点"转码" → 产物回列表并标"已完成"；设置-转码勾选"保留封面"时产物仍带封面。
6. 本地批量：拖入手机竖屏视频目录 → 解析 → 用缩略图旋转箭头纠正方向 → 批量转码（含 9:16 等非标比例素材，确认不因奇数宽高失败）。
7. 合并：勾选 ≥2 段（含参数一致与不一致各一组）→ 合并面板排序 → 输出可播放、参数一致组日志显示"直拼（零重编码）"。
8. 取消：下载/转码中断时点"取消"，确认进程树被终止、本任务临时目录被清理、状态为"已取消"，且**其它并发任务的临时目录不受影响**。
9. 需要登录：选一个需登录站点触发"需要登录" → "去登录"完成登录 → 自动重新解析并可继续。
10. CLI/单实例：`ytdlp-FFmpeg-GUI --url <URL>` 启动新实例；应用已运行时再次执行该命令，URL 应进入现有实例列表；`--dir`/`--cookies` 等覆盖项**单独出现**（不带 URL）也应在本实例生效（本次调用内）。
