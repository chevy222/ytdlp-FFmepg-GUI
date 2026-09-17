# ytdlp-FFmpeg-GUI（影栈）

下载 · 转码 · 合并 融为一体的本地视频工具箱 —— **Windows 桌面 GUI（Rust + Tauri v2）**，绿色便携单 EXE。

- **统一列表即工作台**：下载任务、下载产物、本地文件、处理任务全部在同一份列表；下载 / 转码 / 合并是作用于条目的动作，不做分模块页面。
- **元数据先行**：URL 或本地文件添加后先解析元数据（URL 用 yt-dlp 解析格式清单；本地文件用 ffprobe + volumedetect 探测分辨率/编码/音量）再进入可操作状态。
- **能力**：yt-dlp 1000+ 站点下载（内置 WebView2 登录）、批量转码（H.265：QSV → libx265 兜底，可强制 NVENC / AMF）、多文件合并（同参直拼 / 异参统一）。
- **绿色便携**：所有产生文件（config/ temp/ tools/ logs/）均在 exe 同级，不写注册表、不依赖 %APPDATA%。

## 仓库结构

```
crates/core     平台无关核心层（模型/状态机/配置原子写/历史持久化，可单测）
src-tauri       Tauri v2 应用壳（空窗口骨架，M0）
ui              前端静态资源（效果图同款界面，里程碑接入）
scripts/build.ps1        本地构建（pwsh 7，与 CI 同构）
scripts/cross-build/     Linux 环境交叉编译 Windows 单 EXE（cargo-xwin / zigbuild）
.github/workflows/       Windows runner CI（build + release）
```

## 构建

Windows 本地（PowerShell 7）：

```powershell
pwsh ./scripts/build.ps1
```

产物：`src-tauri/target/release/ytdlp-FFmpeg-GUI.exe`（单 EXE，无安装包）。

CI：push/PR 在 **Windows runner** 上跑 单测 → Clippy(-D warnings) → Release 构建；
push `v*` tag 发布 **zip + 单 EXE 直传**到 GitHub Release。

## 里程碑

| 阶段 | 内容 |
| --- | --- |
| M0 工程初始化 | workspace 骨架 + 目录约定 + 核心层（模型/状态机/配置/历史）+ build.ps1 + CI + 空窗口可运行 |
| M1 解析 + 列表 + 下载 | 元数据解析（URL/本地/产物）+ 统一列表 + 下载能力 + WebView2 登录 + 依赖/设置 |
| M2 转码 | 批量转码引擎（H.265：QSV→libx265 兜底/NVENC/AMF）+ 旋转/分辨率/码率/增益/封面 + 产物回列表 |
| M3 合并 | 合并面板 + 同参直拼（extradata 校验）/ 异参统一 + 产物回列表 |
| M4 CLI 入口 | --url/--cookies/--dir/--yt-dlp-path/--deno-path + 单实例转发 |
| P1 增强 | 播放列表展开平铺、时间范围下载（剪辑）、合并音量归一化 |
| P2 完善 | TC-16 硬件编码器探测 + 硬编失败自动回退 libx265 |

## 需求文档

完整需求 v1.0（终版）：统一列表、元数据先行、下载/转码/合并动作、设置分组（依赖/网络/Cookie/下载/转码/通用）、
目录存储约定（§3.7）、数据模型（§7）等，见 `docs/ytdlp-FFmpeg-GUI-doubao-需求文档.md`（效果图与架构图同目录）。

## License

MIT
