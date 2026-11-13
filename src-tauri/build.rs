fn main() {
    // Saving to the photo library (`src/photos.m`) exists on iOS only.
    println!("cargo:rerun-if-changed=src/photos.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        cc::Build::new().file("src/photos.m").flag("-fobjc-arc").compile("siftphotos");
        for framework in ["Photos", "Foundation"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
    tauri_build::build()
}
