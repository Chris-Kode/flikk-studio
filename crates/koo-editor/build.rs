//! On Windows the Vulkan loader whisper.cpp links, `vulkan-1.dll`, is
//! loaded on first use, so the editor starts on a machine without it
//! (`src/vulkan_fallback.rs`).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = |key| std::env::var(key).unwrap_or_default();
    if target("CARGO_CFG_TARGET_OS") == "windows" && target("CARGO_CFG_TARGET_ENV") == "msvc" {
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:vulkan-1.dll");
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    }
}
