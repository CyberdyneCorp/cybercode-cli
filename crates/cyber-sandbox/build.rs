fn main() {
    for source in [
        "macos_plugin_exec.c",
        "linux_plugin_exec.c",
        "linux_filter.h",
        "linux_initial_exec.h",
    ] {
        println!("cargo:rerun-if-changed=native/{source}");
    }
    let mut build = cc::Build::new();
    build.flag("-Wall").flag("-Wextra").flag("-Werror");
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("macos") => {
            build
                .file("native/macos_plugin_exec.c")
                .flag("-Wno-error=deprecated-declarations");
        }
        Ok("linux") => {
            build.file("native/linux_plugin_exec.c");
            // Debian's native musl compiler omits Linux UAPI directories. Search
            // them after musl's standard headers, never shadowing libc headers.
            if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("musl")
                && std::env::var("HOST")
                    .ok()
                    .map(|host| host.split('-').next().unwrap_or("").to_owned())
                    == std::env::var("CARGO_CFG_TARGET_ARCH").ok()
            {
                let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
                for directory in [
                    format!("/usr/include/{arch}-linux-gnu"),
                    "/usr/include".into(),
                ] {
                    if std::path::Path::new(&directory).is_dir() {
                        build.flag("-idirafter").flag(&directory);
                    }
                }
            }
        }
        _ => return,
    }
    build.compile("cyber_plugin_exec");
}
