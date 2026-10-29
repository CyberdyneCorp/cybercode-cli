fn main() {
    println!("cargo:rerun-if-changed=native/macos_plugin_exec.c");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new()
        .file("native/macos_plugin_exec.c")
        .flag("-Wall")
        .flag("-Wextra")
        .flag("-Werror")
        .flag("-Wno-error=deprecated-declarations")
        .compile("cyber_plugin_exec");
}
