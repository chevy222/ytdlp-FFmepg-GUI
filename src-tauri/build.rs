fn main() {
    // 图标经 tauri-build 编译为 Windows 资源（PE 资源节）嵌入 exe，但 tauri-build
    // 只对 tauri.conf.json / bundle.resources / 前端产物声明 rerun-if-changed，
    // **不包含图标**。而 cargo 的规则是：build script 只要声明了任意一个
    // rerun-if-changed，就不再按"整个包目录 mtime"做兜底监听 —— 于是替换图标后
    // build script 不会重跑，Windows 资源也不会重新生成。
    //
    // 在本地通常察觉不到（首次构建或 cargo clean 会重建），但在复用 target/ 的 CI
    // （Swatinem/rust-cache）上，旧的资源与产物被缓存恢复后会被原样复用，
    // 表现为 exe 图标永远停留在旧版本。这里显式声明，保证换图标必然触发重建。
    // 路径相对于本包根目录（src-tauri/）。
    println!("cargo:rerun-if-changed=icons/icon.ico");
    println!("cargo:rerun-if-changed=icons/icon.png");

    tauri_build::build()
}
