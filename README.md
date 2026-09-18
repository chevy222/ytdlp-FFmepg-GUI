# ytdlp-FFmpeg-GUI（影栈）

**下载 · 转码 · 合并** 融为一体的 Windows 桌面视频工具箱 —— Rust + Tauri v2，绿色便携单 EXE，无安装包、不写注册表。

## 功能特性

- **统一列表即工作台**：下载任务、下载产物、本地文件、处理任务全部在同一份列表里；下载 / 转码 / 合并是**作用于条目的动作**，不做分模块页面。列表行内提供转码 / 打开目录 / 重试 / 取消 / 去登录 / 日志等上下文操作；勾选多条后出现批量操作栏（旋转 / 批量转码 / 合并 / 删除）。
- **元数据先行**：粘贴 URL 或添加本地文件/目录后，条目先以"解析中"入列，解析完成才进入可操作状态——
  - URL 用 yt-dlp 解析标题、时长、封面、可用清晰度/格式列表；解析完默认 **5 秒倒计时自动下载**，倒计时期间点画质下拉可取消改手动；
  - 本地文件用 ffprobe + volumedetect 探测容器、分辨率、编码器、视频码率、帧率、音频编码、采样率、音频码率、音轨数、**最大音量**、时长、大小，下载完成的产物也按同一标准再解析一遍，可直接进入转码/合并。
- **下载**：yt-dlp 驱动 1000+ 站点（B 站 / 抖音 / YouTube / X 等）；格式选择（七级格式回落 + 排序串，MP4 容器统一）；下载后自动后处理（音量归一化 + 封面/元数据嵌入 + 产物本地解析）；Cookie 按站点 JSON 本地管理，缺失/过期自动弹 **WebView2 内置登录窗**（顶部中心"登录完成"按钮保存 Cookie，YouTube 等 SPA 站点注入按钮每 800ms 保活重建；登录窗走系统代理，需在代理软件开启"系统代理"）。
- **转码**：批量转 **H.265**（自动 Intel QSV → libx265 兜底，可强制 NVENC / AMF / libx265，x265 CRF 固定 23）；支持**手动旋转**（列表封面缩略图上的旋转箭头，0°/90°/180°/270°，角度随条目保存）、分辨率上限、码率封顶、音频自动增益（峰值 → 0dBFS，24dB 封顶）、保留封面、碰撞安全命名；转码产物作为新条目回到列表。
- **合并**：勾选多条目 → 合并面板排序 → 参数一致（含编码器 SPS/PPS）零重编码直拼，参数不一致自动统一后拼接；产物回列表。
- **绿色便携**：所有产生文件都在 exe 同级（`config/` 配置与 Cookie / `temp/` 临时 / `tools/` 工具链 / `logs/` 日志），所有固化内容以 JSON 形式存放；单实例运行。
- **站点分流代理**：站点列表按需勾选"走代理"（白名单，未勾选一律直连），代理地址可配置（预设常用代理工具端口）。

## 快速上手

1. **下载**：粘贴视频链接 → 添加 → 自动解析（5 秒倒计时后自动下载，或点画质下拉选格式手动下载）。B 站 / YouTube 等需登录的站点会自动弹出登录窗。
2. **转码**：添加本地文件/目录（支持拖放）→ 解析完自动"已就绪"→ 用封面缩略图上的旋转箭头纠正方向（可选）→ 勾选（或直接行内"转码"）→ 按设置-转码的默认参数批量转 H.265。
3. **合并**：勾选多段素材 → 批量操作栏"合并…"→ 排序 → 确认输出。
4. **设置**：齿轮图标进入——依赖（yt-dlp / ffmpeg / ffprobe / deno 路径 + 下载/更新）、网络（代理 + 站点分流）、Cookie 管理、下载（并发分片 / 仅音频 / 播放列表 / 文件名模板）、转码（分辨率 / 码率 / 编码器 / 保留封面）、通用（输出目录 / 音量归一化与增益上限 / 并发任务数 / 检查更新 / 历史上限 / 清理解析缓存）。

## 目录与数据存储（exe 同级）

```
config/     config.json（设置）· history.json（列表/队列/条目日志）· cookies/（站点 Cookie）· cache/（解析缓存）
temp/       下载分片 / 中间产物（任务结束清理，可一键清理残留）
tools/      yt-dlp / ffmpeg / ffprobe / deno（托管模式工具链）
```

## 构建

**Windows 本地**（PowerShell 7）：

```powershell
pwsh ./scripts/build.ps1
```

产物：`src-tauri/target/release/ytdlp-FFmpeg-GUI.exe`（单 EXE）。

**CI**：push/PR 在 **Windows runner** 上跑核心层单测 → Clippy（`-D warnings`）→ Release 构建；push `v*` tag 发布 **zip** 到 GitHub Release。

**Linux 环境交叉编译 Windows 单 EXE**（cargo-xwin / zig，作为本地备用通道）：详见 `scripts/cross-build/README.md`。

## 技术栈

| 层 | 选型 |
| --- | --- |
| 桌面框架 | Rust + Tauri v2（单窗口） |
| 前端 | 原生 HTML / CSS / JS（浅色 Soft UI + 磨砂玻璃） |
| 下载引擎 | yt-dlp（外部子进程，JSON 输出） |
| 处理引擎 | ffmpeg / ffprobe（转码 / 合并 / 探测 / 封面 / 音量） |
| JS 运行时 | deno（yt-dlp YouTube 组件必需） |
| 内置登录 | WebView2 登录窗 + COM CookieManager 抓取含 HttpOnly 的 Cookie |
| CI | GitHub Actions（windows-latest） |

## 仓库结构

```
crates/core     平台无关核心层（模型/状态机/配置原子写/历史持久化/探测/下载/转码/合并，可单测）
src-tauri       Tauri v2 应用壳（命令桥接、登录窗、CLI 入口）
ui              前端静态资源
scripts/build.ps1        本地构建（pwsh 7，与 CI 同构）
scripts/cross-build/     Linux 环境交叉编译（cargo-xwin / zigbuild）
.github/workflows/       Windows runner CI（build + release）
docs/           需求文档 / 效果图 / 架构总览图 / 主要功能流程图
```

## License

MIT
