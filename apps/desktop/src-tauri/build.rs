fn main() {
    // tauri-build 只对配置文件与 resources 发 rerun-if-changed，图标不在其中，
    // 缺这一行时改图标不会重跑构建脚本，exe 里会继续沿用旧的图标资源。
    println!("cargo:rerun-if-changed=icons/icon.ico");
    tauri_build::build()
}
