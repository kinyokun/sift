fn main() {
    // Examples can act as the Nikon SDK helper (see `nikon.rs`): a copy of the
    // executable sits next to the SDK's `Contents/Frameworks`.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-examples=-Wl,-rpath,@executable_path/Contents/Frameworks");
    }
}
