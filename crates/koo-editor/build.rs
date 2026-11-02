//! On Windows the Vulkan loader whisper.cpp links, `vulkan-1.dll`, is
//! loaded on first use, so the editor starts on a machine without it
//! (`src/vulkan_fallback.rs`). On Linux ONNX Runtime's WebGPU links Dawn,
//! which cargo puts beside the binary.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = |key| std::env::var(key).unwrap_or_default();
    if target("CARGO_CFG_TARGET_OS") == "windows" && target("CARGO_CFG_TARGET_ENV") == "msvc" {
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:vulkan-1.dll");
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    }
    if target("CARGO_CFG_TARGET_OS") == "linux" && target("CARGO_CFG_TARGET_ARCH") == "x86_64" {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    }
}
