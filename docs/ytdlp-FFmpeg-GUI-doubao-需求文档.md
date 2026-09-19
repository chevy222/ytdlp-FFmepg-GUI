# ytdlp-FFmpeg-GUI（影栈）实现说明

> 本文档描述**当前代码的实际行为**，与仓库代码同步；不含规划、里程碑与评审过程。
> 编号（UL-/MD-/DL-/TC-/MG-）沿用历史编号，便于与代码、issue 互相追溯。
> 代码中**没有**的部分集中列在 §12，正文只写已经实现的行为。

| 项 | 内容 |
| --- | --- |
| 产品中文名 | 影栈 |
| 产品形态 | Windows 桌面 GUI（Rust + Tauri v2），绿色便携单 EXE |
| 代码仓库 | https://github.com/chevy222/ytdlp-FFmpeg-GUI.git |
| 工程结构 | `crates/core`（平台无关核心层，可单测）· `src-tauri`（Tauri 应用壳）· `ui`（单页原生 JS 前端） |
| 外部依赖 | yt-dlp 2026.08+ / ffmpeg + ffprobe / deno（yt-dlp 的 YouTube 组件需要） |
| 本地构建 | `pwsh ./scripts/build.ps1` → `target/release/ytdlp-FFmpeg-GUI.exe` |

---

## 1. 统一列表与任务（UL）

| 编号 | 行为 | 实现位置 |
| --- | --- | --- |
| UL-01 | 解析中/已就绪/下载中/后处理中/转码中/合并中/已完成/失败/已取消/需要登录 的条目全部在同一列表展示，无分模块页面 | `ui/index.html`、`model.rs::Status` |
| UL-02 | 条目一行：勾选框 · 名称（封面缩略图 + 旋转按钮 + 标题 + 副行"站点/本地 · URL 或路径"）· 画质/格式 · 状态 · 进度 · 操作 | `ui/index.html::rowHtml`、`model.rs::MediaItem::subline` |
| UL-03 | 列表按 `updated_at` 倒序渲染；筛选分组把下载中/后处理/转码/合并归为"处理中" | `ui/index.html::render`、`model.rs::Status::group` |
| UL-04 | 行内操作按状态出现：URL 已就绪 → 下载 / 剪辑 / 格式选择；Ready 或 Done 且有本地文件 → 转码；需要登录 → 去登录；处理中 → 取消；失败/已取消/需要登录 → 重试；已完成 → 打开目录；终态 → 删除；所有状态 → 日志 | `ui/index.html::ops` |
| UL-05 | 勾选后出现批量操作栏：已选 N 项 · 顺/逆时针旋转（仅图标）· 批量转码 · 合并… · 删除；工具栏右侧垃圾桶清空列表 | `ui/index.html::renderBatch` |
| UL-06 | 统一状态机（白名单迁移见 §9）+ 全局并发队列；进度实时回推；取消立即置标志并终止进程树；失败/已取消/需要登录可"重试"（即重新解析）；可删除 | `model.rs::transition`、`worker.rs::TaskQueue`、`commands.rs` |
| UL-07 | 列表与队列持久化在 `config/history.json`（含每条目 ≤300 行日志），变更即原子写 | `history.rs`、`state.rs::persist` |
| UL-08 | 临时文件按任务私有目录隔离（`temp/<任务id>/`、`temp/merge_<uuid>/`），任务结束或取消即清理；底部"清理临时文件"只清历史残留（跳过运行中任务的私有目录与 `temp/tool_dl/`，不影响进行中的任务） | `paths.rs::task_temp_dir`、`commands.rs::clear_temp` |
| UL-09 | CLI 入口 + 单实例参数转发（见 §10） | `cli.rs`、`src-tauri/src/lib.rs` |
| UL-10 | 启动恢复：非终态且非"已就绪"/"需要登录"的条目一律标记失败（"应用重启，任务中断"），可重试；"需要登录"跨重启保持原状（未登录事实不因重启改变） | `src-tauri/src/lib.rs::setup` |
| UL-11 | 封面缩略图：URL 条目取远程缩略图（系统 `curl` 下载，直连失败自动按设置代理重试一次），本地文件/下载产物用 ffmpeg 抽帧（`-ss 0.5`，宽 ≤360），统一落在 `config/cache/thumbs/<条目id>.jpg`；下载完成时条目仍无封面则对最终产物抽帧兜底（已有封面不覆盖）；解析中（Probing）显示"影"占位 | `thumbs.rs`、`commands.rs::finish_download` |
| UL-16 | **工具栏不放品牌 logo/应用名称区块**（左上角不显示 logo 图标 +"影栈 | 本地视频工作台"之类）——工具栏直接从 URL 输入框开始，保持紧凑。应用图标只出现在窗口标题栏/任务栏/欢迎页中央，不在工具栏重复占位 | `ui/index.html::toolbar` |
| UL-12 | 手动旋转：缩略图上的顺/逆时针箭头 → 角度 0/90/180/270 随条目保存（`rot_angle`，设置即落盘，重启不丢），转码时生效；角度 ≠ 0 时缩略图右上角显示角标。旋转按钮只对**文件已存在**的条目显示：本地文件/转码产物/合并产物解析完即可设，URL 条目要等下载产物落地（后处理中起）才出现——`rot_angle` 是转码参数，下载中显示只会误导 | `commands.rs::rot_item`、`model.rs::RotAngle`、`ui/index.html::rowHtml` |
| UL-13 | 每条目独立日志（最近 300 行），弹窗可查看/复制/仅清空视图。yt-dlp / ffmpeg / ffprobe 实际执行的完整命令行（参数经引号转义、可直接复制执行）在启动前写入条目日志：yt-dlp 解析/下载、ffprobe 本地与产物解析、ffmpeg 转码/后处理/合并/归一化/抽帧全覆盖 | `exec.rs::display_command`、`probe.rs`、`download.rs`、`transcode.rs`、`merge.rs::run_piped_progress`、`thumbs.rs` |
| UL-14 | 底部状态栏：左端 `运行中 n/m（并发上限）`——n = 处于下载中/后处理中/转码中/合并中的条目数，m = `general.concurrency`（悬停显示口径）；右端四个工具自检徽标，解析到可执行文件且版本可读时显示 `工具 版本`，解析到但版本探测失败显示 `工具（已找到）`，未解析到显示 `工具（未找到）`（悬停显示解析到的绝对路径） | `ui/index.html::renderQueue`/`refreshDeps`、`commands.rs::probe_dependencies`、`exec.rs::parse_version_line` |
| UL-15 | 列表排序：按 `updated_at` **降序**（最新的在最上）；条目创建即打时间戳（`YYYY-MM-DD HH:MM:SS` 本地时区），旧版 history 中无时间戳的条目排在最末、相对顺序不变 | `model.rs::MediaItem::new`、`ui/index.html::render` |

## 2. 元数据解析（MD）

| 编号 | 行为 | 实现位置 |
| --- | --- | --- |
| MD-01 | URL 解析：`yt-dlp -J --no-warnings [--yes-playlist\|--no-playlist] [--cookies] [--proxy] [--js-runtimes] <url>`，取标题/时长/缩略图/格式列表（format_id、容器、编码、大小、帧率、码率、是否仅音频），格式按 format_id 去重；解析阶段不判定登录状态 | `probe.rs::probe_url` / `parse_ytdlp_json` |
| MD-02 | 本地解析：`ffprobe -v error -print_format json -show_format -show_streams -show_data` 取容器/时长/大小/分辨率/编码器/帧率/视频码率/音频编码/采样率/音频码率/声道/音轨数/封面流/`extradata`/流绝对索引/容器旋转标记；再用 `ffmpeg -af volumedetect -f null -` 取 `mean_volume`/`max_volume`（有音频流时） | `probe.rs::probe_local` / `parse_ffprobe_json` / `probe_volume` |
| MD-03 | 条目先以"解析中"入列，前端显示"解析中…"占位进度；解析成功变"已就绪"，失败按分类变"失败"或"需要登录" | `commands.rs::add_url`/`add_local`/`run_probe` |
| MD-04 | 解析结果写入条目 `meta`，用于 12 项画质列展示、转码音频增益依据、合并参数对比；下载格式列表直接来自解析结果 | `commands.rs::run_probe` |
| MD-05 | 解析失败分类：网络不可达/超时 → `Network`；需登录/私有/会员/新鲜 Cookie → `NeedLogin`（条目变"需要登录"）；链接无效/404 → `InvalidLink`；非视频/探测失败 → `NotVideo`；其余 `Failed`。条目记录错误摘要，可"重试"（重新解析）或删除 | `probe.rs::classify_ytdlp_error` |
| MD-06 | 下载产物落盘后按 MD-02 同一链路再解析一次，并用产物抽帧更新缩略图；条目 `path`/`file` 回填为产物路径，可直接转码/合并 | `commands.rs::run_download_task` / `finish_download` |

## 3. 下载能力（DL）

| 编号 | 行为 | 实现位置 |
| --- | --- | --- |
| DL-01 | 顶部输入框可一次粘贴多条 URL（按空白切分、只接受 `http(s)://`），清洗去首尾引号与空白后逐条入列 | `commands.rs::clean_url`、`ui/index.html::addUrls` |
| DL-02 | 解析完成后画质列内联"格式选择 (N)"，弹窗列出解析结果（可读标签 + format_id），选定后按该格式下载：`<fid>+ba/b`，仅音频为 `<fid>/bestaudio/best` | `ui/index.html::openFmt`、`download.rs::build_args` |
| DL-03 | 默认格式四级回落 `bv*[height<=H]+ba / b[height<=H] / bv*+ba / b`（H = `download.max_h`）+ 排序串 `vcodec:h264,lang,quality,res,fps,acodec:aac,size,proto,ext`；`--merge-output-format mp4`；`--no-overwrites` 不覆盖同名文件 | `download.rs::default_format` / `SORT_SPEC` / `build_args` |
| DL-04 | 下载后自动后处理，仅在需要时执行：① **短边**超 `download.max_h`（`short_edge()`，竖屏源依赖 width 采集）→ `scale='trunc(iw*s/2)*2':'trunc(ih*s/2)*2'`（`s=min(1,max_h/min(iw,ih))`，旋转不变量）+ libx265 CRF23 重编码；② 开启音量归一化且 `max_volume ∈ (-100,-0.5)dB` → 增益至峰值 0dBFS（`general.max_gain_db` 封顶）并重编码 AAC。有封面流时主视频走 filter_complex、封面按索引 `copy`；产物经 ffprobe 校验后**单次 rename 原子替换**，任一步失败保留原文件并记日志 | `download.rs::post_process` / `verify_video` |
| DL-05 | "需要登录"条目与设置页均可打开 WebView2 登录窗（label `ytdlp-login`）：开窗走 async 命令 + `spawn_blocking`（Windows 上主线程建窗会死锁）；注入顶部中心"登录完成"按钮 + 提示条（内含"关闭"），`setInterval(800ms)` 自检重建（SPA 路由变化不触发页面加载事件）；页面加载失败时显示底部红色提示（提醒检查系统代理）；关闭手段两种——提示条"关闭"或 Esc（跳 `http://127.0.0.1/ytdlp-login-close` 魔法 URL，在 `on_navigation` 嗅探并取消导航；127.0.0.1 无监听，靠页面加载事件永远等不到）；点"登录完成"跳 done URL，Windows 优先经 COM CookieManager 抓取（含 HttpOnly，带 8s 截止的消息泵），失败回退 URL 携带的 `document.cookie`；保存后关窗并广播 `login:done`（前端收到后对全部"需要登录"条目自动重试，即重新解析） | `login.rs`、`login_win.rs` |
| DL-06 | Cookie 存 `config/cookies/<host>.json`（name/value/domain/path/expires/http_only/secure/same_site）；匹配顺序：精确 host → 父域 → 补 `www.` → X↔twitter 互退；导出 Netscape 文件到**任务私有目录**（`temp/<任务id>/cookies-<host>.txt`）供 `--cookies` 使用，任务结束随目录删除；单条 value >4096 字节不导出 | `cookies.rs`、`commands.rs::resolve_cookies` |
| DL-07 | 站点分流白名单：只有 `network.site_proxy` 中显式为 true 的站点走 `network.proxy_url`，其余一律直连；代理地址为空则全部直连 | `config.rs::NetworkConfig::resolve_proxy` |
| DL-08 | 仅音频：选中仅音频格式后加 `-x --audio-format mp3 --audio-quality 0`（格式固定 mp3） | `download.rs::build_args` |
| DL-09 | 播放列表（`download.playlist` 开）解析合集后用 `-J --flat-playlist` 展开每集 URL，逐条平铺进列表；每集在单个后台线程中**顺序**解析（大合集避免瞬间并发起等量 yt-dlp 子进程），每集就绪即回显 | `probe.rs::list_playlist_entries`、`commands.rs::expand_playlist` |
| DL-10 | 文件名模板：纯标题 `%(title)s.%(ext)s` / 标题+ID / UP主-标题 / 日期-标题（日期按本地时区）；播放列表模式 `%(playlist_title)s/%(playlist_index)s - %(title)s.%(ext)s` | `download.rs::output_template` |
| DL-11 | 并发分片 `-N <fragments>`（默认 4）、`--retries <retries>`（默认 3）、`--retry-sleep 3` | `download.rs::build_args` |
| DL-12 | 时间范围下载：条目 `sections` 设置后传 `--download-sections *<start>-<end>`（HH:MM:SS，前端校验格式） | `commands.rs::set_sections`、`download.rs::build_args` |
| DL-13 | 产物定位：解析 yt-dlp 输出中的 `[Merger] Merging formats into "<path>"`、`[download] Destination:`、`[download] <file> has already been downloaded` 三类行，"存在即本次产物"；一条都没解析到时才扫描输出目录，且只取**本次启动后新增的最新一个**视频文件；仍为空则判失败（不触碰目录内既有文件）。若本次只有"已下载过"的既有文件（`preexisting`），跳过后处理 | `download.rs::parse_merger_path` / `parse_already_downloaded_path` / `newest_media_since` / `run_download` |
| DL-14 | JS 运行时：解析与下载都把托管或配置的 deno 显式传给 yt-dlp（`--js-runtimes deno:<路径>`）；未配置时交给 yt-dlp 自行探测 PATH | `probe.rs::push_js_runtime`、`download.rs::build_args` |

## 4. 转码能力（TC）

| 编号 | 行为 | 实现位置 |
| --- | --- | --- |
| TC-01 | 添加本地文件/目录：文件多选（带扩展名过滤）、目录递归扫描；拖放路径按递归扫描；添加后立即解析 | `commands.rs::add_local` / `scan_dir`、`ui/index.html` |
| TC-02 | 批量转码逐条目独立执行，各自记日志与失败原因；失败条目可"重试"（重新解析后再转码） | `commands.rs::start_transcode` / `finish_transcode` |
| TC-03 | 编码器：`auto`（`ffmpeg -encoders` 探测到 `hevc_qsv` 用 QSV，否则 libx265）/ `libx265` / `hevc_nvenc` / `hevc_amf`。参数：libx265 `-crf 23 -preset medium`；NVENC `-rc vbr -cq 23 -preset p5`；AMF `-qp_i 23 -qp_p 23 -quality balanced`；QSV `-global_quality 23`（`low_power` 开时加 `-low_power 1`） | `transcode.rs::pick_encoder` / `detect_hw_encoders` |
| TC-04 | 手动旋转：按条目 `rot_angle` 生成 `transpose=1`（90°）/ `transpose=1,transpose=1`（180°）/ `transpose=2`（270°）；不做自动纠正 | `transcode.rs::build_vf` |
| TC-05 | 分辨率封顶：`scale='trunc(iw*s/2)*2':'trunc(ih*s/2)*2'`，其中 `s` 用 `min` **两两嵌套**表达（`min(min(1, MAXH/min(iw,ih)), MAXW/max(iw,ih))`）——上限按**短边/长边**（旋转不变量）判定：transpose 后 `iw`/`ih` 互换，直接写 `min(ih,MAXH)` 会把原视频长边当短边砍（1080P 转 90° 变 606×1080 的历史 bug）；且 ffmpeg 表达式求值器的 `min()`/`max()` **只接受两个参数**，三参数写法报 `Cannot parse expression for width`（实测）；宽高各自取偶，不放大 | `transcode.rs::build_vf` |
| TC-06 | 码率封顶：`brcap_kbps` 有值时加 `-maxrate <n>k -bufsize <2n>k` | `transcode.rs::build_args` |
| TC-07 | 音频增益：开启归一化且 `max_volume ∈ (-100,-0.5)dB` 时按 `min(-max_volume, max_gain_db)` 生成 `volume=XdB` 并重编码 AAC；否则音频 `copy`；作用于全部音轨（`-map 0:a?`） | `transcode.rs::build_args` |
| TC-08 | 封面：`keep_cover` 开时按探测到的**封面流绝对索引**映射——不旋转 `-c:v:1 copy` 保质量；**旋转时封面同过 transpose 滤镜链**并重编码 mjpeg（`-q:v:1 2` + `-disposition:v:1 attached_pic`，copy 的封面不会跟随旋转）；源为 MKV 附件型封面（探测不到 attached_pic）时回退 `-map 0:t?` + `-c:t copy`（附件不参与旋转） | `transcode.rs::build_args` |
| TC-09 | 输出显式映射主视频/音频/封面，不复制源流级旋转标签；MP4 + libx265 时加 `-tag:v:0 hvc1`（Apple 兼容） | `transcode.rs::build_args` |
| TC-10 | 容器 MP4 加 `-movflags +faststart`；MKV 不传该参数；容器由 `TranscodeParams.container` 决定（前端目前固定 mp4） | `transcode.rs::build_args` |
| TC-11 | 输出到默认输出目录；命名沿用 DL-10 模板（本地条目"标题+ID"用标题哈希作短指纹）；碰撞策略 `auto_inc`（`name (1).ext` 递增，上限 999）或 `skip`（已存在即报错跳过）；成功产物作为新条目（`TranscodeOut`）回到列表 | `transcode.rs::output_path` / `apply_filename_template`、`commands.rs::finish_transcode` |
| TC-12 | 输出文件名清洗 Windows 非法字符、去尾部点号、截断 120 字符 | `transcode.rs::sanitize_filename` |
| TC-13 | 硬件编码器探测（`ffmpeg -encoders` → QSV/NVENC/AMF），设置页对未检测到的选项标注"（未检测到）"但仍可选 | `transcode.rs::parse_encoders_output`、`commands.rs::probe_hw_encoders` |
| TC-14 | 硬编失败自动回退：非取消且当前不是 libx265 时，用 libx265 重试一次（进度与日志延续） | `transcode.rs::run_transcode` |
| TC-15 | 进度：`-progress pipe:1 -nostats`，按 `out_time_us` 相对探测时长换算百分比；取消终止进程树并删除半成品 | `transcode.rs::run_transcode_once` |

## 5. 合并能力（MG）

| 编号 | 行为 | 实现位置 |
| --- | --- | --- |
| MG-01 | 勾选 ≥2 条目 → 合并面板：顺序列表（上移/下移/移除）、容器（MP4/MKV）、编码器（auto/libx265/nvenc/amf）、输出文件名、音量归一化开关、参数差异预警 | `ui/index.html::openMerge` / `mergeWarn` |
| MG-02 | 同参直拼：各段 vcodec/height/fps/acodec/sample_rate 一致**且 extradata（SPS/PPS）非空且一致**时，用 concat demuxer 零重编码直拼；**extradata 未知一律判不同**（退回统一转码，避免 concat 静默花屏） | `merge.rs::same_parameters` / `concat_copy` |
| MG-03 | 异参统一：逐段转码（目标高度取各段最大 height，滤波 `scale=-2:<h>:force_original_aspect_ratio=decrease`，音频 aac 48kHz 双声道）后再 concat 直拼 | `merge.rs::transcode_segment` |
| MG-04 | 输出容器 MP4（默认）/ MKV；编码器跟随面板选择（`auto` 落到 libx265；libx265 输出 MP4 时加 `hvc1`）；MP4 加 `+faststart` | `merge.rs::encoder_args` / `concat_copy` |
| MG-05 | 合并后可选音量归一化：探测产物 `max_volume` → 视频 `copy`、音频 aac 增益至峰值 0dBFS（`general.max_gain_db` 封顶）；中间文件写在任务临时目录，成功后单次 rename 原子替换 | `merge.rs::post_normalize` |
| MG-06 | 输出文件名默认 `合并_<YYYYMMDD>`（本地时区日期），可编辑；碰撞策略同 TC-11；产物作为新条目（`MergeOut`）回到列表 | `commands.rs::default_merge_name` / `finish_merge`、`merge.rs::output_path` |
| MG-07 | 合并作为**一个作业**提交（锚点条目占一个并发额度），全部参与条目一起被标记"合并中"、结束后一起恢复原状态；取消标志共享，任一条目点"取消"即可取消整次合并 | `commands.rs::start_merge` / `run_merge_task` / `finish_merge` |

## 6. 设置与配置段

`config/config.json` 的全部字段与默认值（`crates/core/src/config.rs`）：

| 段 | 字段（默认值） |
| --- | --- |
| download | `max_h`=1080（后处理画质上限，短边）· `max_dl_h`=2160 · `fragments`=4 · `retries`=3 · `audio_only`=false · `playlist`=false · `embed_cover`=true · `filename_template`="纯标题" |
| transcode | `max_w`=1920 · `max_h`=1080 · `brcap_kbps`=5000 · `br_default_kbps`=8000 · `force_encoder_mode`="auto" · `low_power`=true · `keep_cover`=true（x265 CRF 固定 23，不落配置） |
| general | `default_output_dir`=null（空则用桌面）· `collision_policy`="auto_inc" · `normalize_audio`=true · `max_gain_db`=24.0 · `concurrency`=3 · `check_update`=true · `history_limit`=100（上限 200） |
| dependencies | `yt_dlp_path` / `ffmpeg_path` / `ffprobe_path` / `deno_path` = null（留空走 PATH，填写用指定路径，含 `tools\` 托管）· `potoken_enabled`=true |
| network | `proxy_url`=""（空 = 全部直连）· `site_proxy`={}（站点 → 是否走代理） |

缺失字段由 serde 默认值补齐；文件损坏时备份为 `config.json.corrupt-<时间戳>.json` 并回退默认值。
设置页分组：依赖（四个工具路径 + 下载/更新/链接/取消，下载 = 装托管副本并写回路径、更新 = 就地更新当前生效的那一份、链接 = 弹窗展示下载与构建页地址可复制，语义见 §7）、网络（代理地址 + 站点分流增删与勾选）、Cookie（站点列表 + 删除）、下载、转码、通用。

## 7. 目录与存储约定

所有运行时文件都在 **exe 同级**，不写注册表、不用 `%APPDATA%`。

| 目录 | 内容 |
| --- | --- |
| `config/` | `config.json`（设置）· `history.json`（列表/队列/条目日志）· `cookies/<host>.json` · `cache/thumbs/<条目id>.jpg` |
| `temp/` | 任务私有目录 `temp/<任务id>/`（导出的 Cookie 等）、`temp/merge_<uuid>/`（合并中间产物与 concat 列表）、`temp/tool_dl/`（工具链下载暂存） |
| `tools/` | 托管工具链：`yt-dlp.exe`、`ffmpeg.exe`、`ffprobe.exe`、`deno.exe`、`installed.json`（工具安装指纹） |

规则：
- 所有 JSON 原子写（临时文件 + **单次 rename** 覆盖目标）；写入失败保持内存态并告警。
- 中间产物只落 `temp/`，不写进用户输出目录；下载/转码/合并产物输出到默认输出目录（默认桌面）。
- 工具解析顺序：设置路径 → `tools\` → 系统 PATH，**任一级取不到就回退下一级**（`tools\` 为空是常态，PATH 里的工具照常可用；只有"显式填写却不存在"的路径直接报错，用户需要知道填错了）。托管 `tools\` 里按需下载，允许只托管部分工具。
- 依赖页「下载」固定装到 `<exe 同级>\tools\`（已有托管副本则直接返回，不重复下载），装完把该路径写回依赖设置，之后优先用这份托管副本。
- 下载源（Windows x86_64）：yt-dlp / deno 走官方 GitHub Release（有 `.sha256` 旁路文件）；ffmpeg/ffprobe 走 gyan.dev 的 release 别名包 `https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip`（303 跳转到当版 `packages/ffmpeg-<版本>-essentials_build.zip`，两工具同包各取所需；gyan.dev 不提供 `.sha256`，产物校验跳过）。
- 依赖页「更新」只更新**当前生效的那一份**：设置里填的路径就地覆盖，托管副本则更新 `tools\` 里那份；当前生效的是系统 PATH 里的（不归本程序管）→ 不下载，提示用户在系统里手动更新或改用「下载」装托管副本。
- 「更新」前先判断有没有新版本，已是最新则不下载、提示"已是最新，无需更新"：
  - yt-dlp / deno 有版本号 → 比 `releases/latest` 的 tag 与本地 `--version`（忽略 `v` 前缀）；
  - ffmpeg / ffprobe → 比 gyan.dev 的 `release-version` 文本 feed（内容即当前 release 版本号，如 `9.0.1`）与本地 `-version` 输出的版本段（gyan 构建版本号带构建后缀，取首个 `-` 前的版本段；BtbN 滚动构建 `N-…` 不以数字开头、原样保留，与 feed 必不等 → 判为需要更新）；本地或远端版本取不到时，回退比"上次安装时的远端产物 SHA-256"（`tools/installed.json`，键 = 配置键名，值 = 目标路径 + 指纹）；记录缺失、目标路径变过或远端指纹取不到，一律按"需要更新"处理，不误报"已是最新"。
- 下载/更新由系统 `curl` 下载：先 `curl -sIL` 取远端 `content-length` 作为总大小，下载中按已写入字节数换算百分比（每 1% 上报一次；取不到大小时只报阶段），阶段依次 `连接 → 下载 → 校验 → 解压 → 安装`；随后 SHA-256 校验（期望值取不到时跳过）→ zip 解压（zip 内目标条目按**后缀**匹配，如 `bin/ffmpeg.exe`——包顶层目录带版本号，完整路径会随版本漂移）→ `写目标同目录 <文件名>.tmp` + rename 原子覆盖（用户自填目录不可写时在该步报错）。下载中按钮为"取消 <阶段> <百分比>"，点击置取消标志并终止 `curl`。
- 依赖页每行有「链接」按钮：弹窗展示该工具的下载地址（程序「下载/更新」用的就是它）与构建/发布页地址，均可一键复制——下载慢时用户可手动下载后把 exe 路径填进设置。
- "清理临时文件"扫描 `temp/` 下条目并删除。

## 8. 数据模型

```
MediaItem
  id(uuid) · kind(url_task|local_file|transcode_out|merge_out) · title · path · url · site · host
  status · percent · error · speed · eta · file · log(VecDeque ≤300 行) · thumb
  rot_angle(0/90/180/270) · format_id · audio_only · sections(起止) · persist · updated_at(本地时区 `YYYY-MM-DD HH:MM:SS`)
  meta: MediaMeta

MediaMeta
  title · duration_secs · container · height · vcodec · vbitrate_kbps · fps
  acodec · abitrate_kbps · audio_tracks · audio_channels · sample_rate
  audio_volume{mean_volume_db, max_volume_db} · size_bytes · has_cover
  extradata(hex) · video_stream_index · cover_stream_index · rotate_tag
  formats(Vec<String> 标签) · download_formats(Vec<DownloadFormat>)

DownloadFormat
  format_id · label · height · ext · vcodec · acodec · filesize_bytes · fps · tbr_kbps · note · audio_only
```

画质/格式列 12 项渲染（核心层与前端同一口径）：
`容器 · 分辨率(≥2160→4K，≥1440→2K，其余 {短边}P) · 编码 · 视频码率(NMbps) · 帧率 · 音频编码 · 采样率(48kHz) · 音频码率(Nk) · 声道数(N声道) · 最大音量(N.NdB) · 时长(mm:ss) · 大小`
实现：`model.rs::MediaMeta::quality_line`、`ui/index.html::qualityLine`。

## 9. 状态机与并发调度

状态：`Probing`(解析中) · `Ready`(已就绪) · `Downloading` · `PostProcessing` · `Transcoding` · `Merging` · `Done` · `Failed` · `Canceled` · `NeedLogin`。
终态（仅可删除/重试）：`Done`、`Failed`、`Canceled`。

合法迁移白名单（`model.rs::transition`，其余一律拒绝）：

```
Probing        -> Ready | Failed | Canceled | NeedLogin
Ready          -> Downloading | Transcoding | Merging | Canceled
Downloading    -> PostProcessing | Failed | Canceled | NeedLogin
PostProcessing -> Done | Failed | Canceled
Transcoding    -> Done | Failed | Canceled
Merging        -> Done | Failed | Canceled
Done           -> Transcoding | Merging | Probing
Failed         -> Probing | NeedLogin
Canceled       -> Probing
NeedLogin      -> Probing
```

并发（`worker.rs::TaskQueue`，全局一份）：
- 额度 = `general.concurrency`（默认 3），下载/转码/合并**共享**；保存设置即生效。
- 解析不占额度；后处理在下载任务线程内串行执行。
- 取消：排队中直接出队并置 `Canceled`；运行中置取消标志 → 任务线程终止子进程树（Windows `taskkill /PID <pid> /T /F`）→ 清理本任务临时目录与残留 → 置 `Canceled`。
- 任务收尾统一顺序：写状态 → 清理取消标志（防注册表条目泄漏）→ 释放额度 → 启动下一个等待任务 → 持久化。
- 转码/合并结束后原条目恢复为"已就绪"（本地文件、转码产物）或"已完成"（下载产物、合并产物）。

## 10. CLI 与单实例

```
ytdlp-FFmpeg-GUI --url <URL> [--url <URL> ...] [--cookies <path>] [--dir <path>]
                 [--yt-dlp-path <path>] [--deno-path <path>]
```
- `--url`（可重复，等价 `-u`）；其余裸位置参数按 URL 处理；`--help`/`-h`/`--version`/`-v` 被忽略。
- 覆盖项只对本次调用生效：`--dir` 作为本次输出目录、`--cookies` 作为本次 Cookie 文件、`--yt-dlp-path`/`--deno-path` 进入工具解析器；均不写 config.json。
- 单实例：第二个实例把 argv 转发给已运行实例（`tauri-plugin-single-instance`），主实例**无条件**应用覆盖项（`--dir`/`--cookies`/工具路径，即使本次不带 URL），并把 URL 投入解析队列。

## 11. 关键实现约束（改代码前必读）

以下结论均由本机 ffmpeg 9.0.1 / yt-dlp 2026.08.19 实跑得出，代码已按此实现，**改动时不要破坏**：

1. **封面流必须用绝对索引映射**：`-map 0:t?` 在 MP4 上**选不中** attached_pic（输出只剩视频+音频）；`-map 0:m:attached_pic?` 是**非法说明符**（报 `Stream map '' matches no streams.`）。正确写法 `-map 0:<index>?`，索引取自 ffprobe 的 `index`。
2. **`-vf` 与第二条视频流的 `copy` 不能共存**：报 `Filtering and streamcopy cannot be used together.`。需要滤镜又要保留封面时，主视频走 `-filter_complex "[0:<idx>]<vf>[v]"` + `-map "[v]"`。
3. **视频类选项必须写 `-c:v:0` / `-tag:v:0`**：不带流后缀的 `-tag:v` 会落到 mjpeg 封面流上，MP4 写头直接失败（`Tag hvc1 incompatible with output codec id '7' (mp4v)`）；`-c:v` 同理会触发多 codec 选项告警。
4. **`extradata` 需要 `-show_data`**：只给 `-show_format -show_streams` 时 ffprobe 只输出 `extradata_size`，拿不到 SPS/PPS；而 concat demuxer 对 SPS 不一致的输入**不报错**（exit 0）却会从第 2 段起花屏，因此同参判据必须建立在真实 extradata 上，且"未知即判不一致"。
5. **`scale` 要显式偶数对齐**：`scale='min(iw,W)':'min(ih,H)':force_original_aspect_ratio=decrease` 保比正确但**不保证偶数**（1214x2160 → 607x1080），libx265 会报 `Picture width must be an integer multiple of the specified chroma subsampling` 且打不开编码器；必须加 `force_divisible_by=2`（或宽用 `-2`）。叠加旋转（TC-05）时不能用 `min(ih,H)`——transpose 后 `ih` 已与 `iw` 互换，上限必须用旋转不变量 `min(iw,ih)`/`max(iw,ih)` 表达。
6. **不要加 `--ignore-errors`**：yt-dlp 官方语义是"忽略下载与后处理错误、仍视为成功"，会让退出码判定失效、把失败当成功。
7. **产物定位不要"扫目录取最旧/全部"**：DASH 下载的最终文件只出现在 `[Merger] Merging formats into "…"` 行，`Destination:` 行指向随后被删除的中间文件；显式路径"存在即产物"，只有解析不到时才扫描目录且只认本次新增的最新一个。
8. **FAT/exFAT 时间戳粒度 2s**：显式产物路径不要再叠加 mtime 过滤，否则刚下载的文件可能被误判为"非本次产物"而假失败。
9. **MKV 下 `-movflags` 被静默忽略**（不报错），只有 MP4 需要 `+faststart`。
10. **`--js-runtimes deno:<path>` 必须显式传**：yt-dlp 默认只认 PATH 里的 deno，托管在 `tools\` 的 deno 不传等于没配。
11. **登录窗注入脚本要保活重建**：SPA 路由变化不触发页面加载事件，靠 `setInterval(800ms)` 自检重建按钮与提示条。
12. **JSON 原子写用单次 rename**，不要"先删后改名"（中间失败会丢整份文件）。
13. **已知限制**：未禁用 ffmpeg 的 autorotate。源文件自带 `rotate` 标记时，ffmpeg 会先按显示矩阵自动旋转，此时再叠加用户手动旋转会导致双重旋转。`MediaMeta.rotate_tag` 已记录源标记，后续可据此决定是否加 `-noautorotate` 并用它初始化 `rot_angle`。
14. **日期/时间戳一律走 `timefmt`**（epoch 秒 + 本地时区偏移；Windows 读注册表 `ActiveTimeBias`，含夏令时；其余平台按 UTC）。std 不提供本地时区，直接按 UTC 手算日期在东八区 0:00–8:00 会差一天（合并默认名、`日期-标题` 模板、`updated_at` 均受影响）。
15. **锁纪律**：`history` 等全局 `std::sync::Mutex` 不可重入——持锁期间不做文件 IO、不 `emit` 事件、不调用会再次加锁的函数（`log_item`/`update_item` 先出锁再调用），否则当场死锁冻结 UI。
16. **托管 `tools\` 目录只能当"回退候选"，不能占用显式配置位**：`ToolResolver::with_tools_dir` 只登记目录；`resolve` 顺序为 显式路径 → 托管目录 → 系统 PATH，托管文件不存在必须继续走 `find_in_path`。一旦把 `tools\<工具名>` 塞进"已配置路径"位，首次运行时空的 `tools\` 会让四个工具全部报"未找到"（而 PATH 里明明有），`--js-runtimes deno:<path>` 也会因此拿不到 PATH 里的 deno。
17. **三级命中位置（`ToolSource`）是「更新」的判据**：显式路径 / 托管副本可就地更新，命中 PATH 只能提示手动更新。所以 `resolve_with_source` 的每一级判定必须与解析结果严格对应，不要把"用户填的路径恰好也在 PATH 里"当成 PATH 来源。
18. **ffmpeg/ffprobe 的版本判定口径必须与 feed 一致**：远端版本来自 gyan.dev `release-version` 纯文本（如 `9.0.1`），本地版本从 `-version` 首行提取——gyan 构建 token 是 `9.0.1-full_build-www.gyan.dev`，必须取首个 `-` 前的版本段才能与 feed 相等；BtbN 滚动构建（`N-…`，不以数字开头）原样保留，与 feed 必不等 → 判为"需要更新"。gyan.dev 无 `.sha256` 旁路文件，SHA-256 指纹（`tools/installed.json`）只是版本取不到时的回退；判不准时必须落到"更新一次"，不能反过来误报"已是最新"。gyan 包顶层目录带版本号，zip 内目标条目只能按后缀（`bin/ffmpeg.exe`）匹配，不能写死完整路径。
19. **登录窗三处线程纪律**：① `WebviewWindowBuilder::new` 在 Windows 的同步命令/主线程里会死锁（官方文档明示），两个登录入口必须是 async 命令并把建窗放进 `spawn_blocking`；② close/done 魔法 URL 必须在 `on_navigation`（导航开始即触发、可取消）嗅探，不能用页面加载事件——`http://127.0.0.1/...` 上没有服务监听，加载必然失败，回调永远不来，"关闭"按钮随之失效；③ `on_navigation`/页面加载回调都在主线程执行，保存/关窗流程一律丢子线程；`with_webview` 里的 COM 等待必须带截止时间，且小于调用方兜底超时（见 `login_win.rs::wait_with_pump_timeout`）。

## 12. 未实现项（不在当前代码中）

以下为已知缺口，代码中**没有**对应实现，列出以避免误解。

| 项 | 现状 |
| --- | --- |
| URL 解析缓存（TTL + `cache.json` 索引） | 未实现；设置页"清理解析缓存"按钮只弹提示 |
| PO-Token 服务（YouTube 风控令牌） | `dependencies.potoken_enabled` 仅存于配置，未被使用 |
| `general.history_limit` / `general.check_update` / `download.max_dl_h` | 字段存在但未被读取 |
| 运行日志文件 `logs/`、界面状态 `ui.json` | 未实现（日志只在条目内存与 history.json 中） |
| 列显隐 / 列顺序 / 排序自定义 | 未实现 |
| 转码 9:16 居中裁切、质量/速度预设档、输出帧率上限、H.264 与仅音频预设 | 未实现 |
| 编码器"首文件协商、后续锁定"、QSV 硬解 + CPU 滤镜混合模式 | 未实现（每条目独立选择编码器） |
| 仅音频格式可选（mp3/m4a/opus） | 未实现（固定 mp3） |
| 批量转码 OK/FAIL/SKIP 汇总视图 | 未实现（逐条目日志） |
| 音视频合成（视频 + 音频 mux）、频道关注、aria2 加速、浏览器扩展桥接 | 未实现 |
| 非 Windows 平台 | 仅 Windows x64 |

## 13. 代码位置索引

| 模块 | 文件 |
| --- | --- |
| 数据模型与状态机 | `crates/core/src/model.rs` |
| 配置（原子写、损坏回退） | `crates/core/src/config.rs` |
| 列表/队列持久化 | `crates/core/src/history.rs` |
| 目录与原子写工具 | `crates/core/src/paths.rs` |
| 元数据解析（yt-dlp / ffprobe / volumedetect） | `crates/core/src/probe.rs` |
| 下载（参数、进度、后处理） | `crates/core/src/download.rs` |
| 转码（编码器、旋转/封顶/增益/封面） | `crates/core/src/transcode.rs` |
| 合并（同参直拼 / 异参统一 / 归一化） | `crates/core/src/merge.rs` |
| Cookie 存储与导出 | `crates/core/src/cookies.rs` |
| 外部进程、工具定位、进程树终止 | `crates/core/src/exec.rs` |
| 并发队列 | `crates/core/src/worker.rs` |
| CLI 解析 | `crates/core/src/cli.rs` |
| 本地时间格式化（时区偏移/日期戳/时间戳） | `crates/core/src/timefmt.rs` |
| 缩略图 | `crates/core/src/thumbs.rs` |
| 工具链托管下载 | `crates/core/src/tool_download.rs` |
| 命令桥接与后台任务 | `src-tauri/src/commands.rs` |
| 应用启动 / 单实例 / 命令注册 | `src-tauri/src/lib.rs` |
| 全局状态 | `src-tauri/src/state.rs` |
| 登录窗（含 Windows COM 抓 Cookie） | `src-tauri/src/login.rs`、`src-tauri/src/login_win.rs` |
| 前端单页 | `ui/index.html` |
