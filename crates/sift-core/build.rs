fn main() {
    // Examples can act as the Nikon SDK helper (see `nikon.rs`): a copy of the
    // executable sits next to the SDK's `Contents/Frameworks`.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-examples=-Wl,-rpath,@executable_path/Contents/Frameworks");
    }
    // Live Photos are written through AVFoundation (`src/live.m`).
    println!("cargo:rerun-if-changed=src/live.m");
    if std::env::var("CARGO_CFG_TARGET_VENDOR").as_deref() == Ok("apple") {
        cc::Build::new().file("src/live.m").flag("-fobjc-arc").compile("siftlive");
        for framework in ["AVFoundation", "CoreMedia", "CoreVideo", "ImageIO", "CoreGraphics", "Foundation"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
}
