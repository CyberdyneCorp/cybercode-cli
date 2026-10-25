fn main() {
    println!("cargo:rerun-if-changed=native/macos.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new()
        .file("native/macos.m")
        .flag("-fobjc-arc")
        .flag("-fblocks")
        .compile("cyber_computer_native");
    for framework in [
        "Foundation",
        "AppKit",
        "CoreGraphics",
        "ApplicationServices",
        "ImageIO",
        "Carbon",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
