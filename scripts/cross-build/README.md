# 交叉编译（Linux 环境）

在 Linux 上交叉编译出 Windows MSVC 单 EXE 的备用通道。**CI 使用 Windows runner（见
`.github/workflows/`），本通道仅作本地备用；如需采用需先验证与 Windows runner 产物一致性。**

## 方案：cargo-xwin（MSVC 目标，推荐）

`cargo-xwin` 直接调用 MSVC 链接器，不需要 `mingw`，产物为 MSVC 单 EXE（与 CI 一致）。

```bash
# 1. 安装工具链（用户级，无需 sudo）
cargo install cargo-xwin
rustup target add x86_64-pc-windows-msvc

# 2. 仅核心层交叉检查（不依赖 Windows 系统库，可先验证语法/类型）
cargo check -p ytdlp-core --target x86_64-pc-windows-msvc

# 3. 完整交叉构建 Tauri 应用（首次会拉取 Windows SDK / 各平台依赖）
cd src-tauri
cargo xwin build --release --target x86_64-pc-windows-msvc
```

产物：`src-tauri/target/x86_64-pc-windows-msvc/release/ytdlp-FFmpeg-GUI.exe`

> Tauri v2 在 Windows target 上需要 WebView2 Runtime（Win10/11 已内置）与 Windows 资源编译；
> `cargo-xwin` 内置资源编译支持，若遇到 SDK 下载问题可设置环境变量 `XWIN_ARCH`。

## 备选：cargo-zigbuild（若 xwin 不可用）

```bash
cargo install cargo-zigbuild
cargo zigbuild --release --target x86_64-pc-windows-msvc
```

## 说明

- 绿色便携：产物为单 EXE，不写注册表；运行时在 exe 同级自动创建
  `config/ temp/ tools/ logs/`（需求文档 §3.7）。
- 交付形态：只交付一个 exe 文件，无安装包（不做 NSIS）；CI release 负责在发布时套一层 zip。
