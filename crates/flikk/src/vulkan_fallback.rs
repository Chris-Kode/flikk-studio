//! Whisper on the CPU where Windows has no Vulkan. whisper.cpp's Vulkan
//! backend links the Vulkan loader, `vulkan-1.dll`, which comes with a GPU
//! driver; `build.rs` delays loading it until Whisper first runs. When it
//! is missing, this hook stands in a `vkGetInstanceProcAddr` under which
//! Vulkan fails to start, and whisper.cpp falls back to the CPU as it does
//! on a machine without a GPU.

use std::ffi::{CStr, c_char, c_void};
use std::ptr::null;

/// MSVC's `DelayLoadInfo` (delayimp.h), up to the function looked up.
#[repr(C)]
struct DelayLoadInfo {
    size: u32,
    descriptor: *const c_void,
    slot: *mut c_void,
    dll: *const c_char,
    by_name: i32,
    /// The function's name when `by_name`, else its ordinal.
    function: *const c_char,
}

const FAIL_LOAD_LIB: u32 = 3;
const FAIL_GET_PROC: u32 = 4;
const VK_ERROR_INITIALIZATION_FAILED: i32 = -3;

type Hook = unsafe extern "system" fn(u32, *const DelayLoadInfo) -> *const c_void;

/// Called by the delay-load helper when a DLL, or a function in it, is
/// missing.
#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
static __pfnDliFailureHook2: Hook = on_failure;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
}

unsafe extern "system" fn on_failure(notify: u32, info: *const DelayLoadInfo) -> *const c_void {
    // SAFETY: the helper passes its own `DelayLoadInfo`, its names C strings.
    let info = unsafe { &*info };
    let dll = unsafe { CStr::from_ptr(info.dll) };
    if !dll.to_bytes().eq_ignore_ascii_case(b"vulkan-1.dll") {
        return null();
    }
    match notify {
        // The editor itself stands in for the DLL; its functions are then
        // missing, and come here.
        FAIL_LOAD_LIB => unsafe { GetModuleHandleW(null()) },
        FAIL_GET_PROC
            if info.by_name != 0
                && unsafe { CStr::from_ptr(info.function) } == c"vkGetInstanceProcAddr" =>
        {
            get_instance_proc_addr as *const c_void
        }
        _ => null(),
    }
}

/// Vulkan's entry point without Vulkan: it has only
/// `vkEnumerateInstanceVersion`, which fails. whisper.cpp asks for the
/// version first, before any other Vulkan function.
unsafe extern "system" fn get_instance_proc_addr(
    _instance: *mut c_void,
    name: *const c_char,
) -> *const c_void {
    // SAFETY: Vulkan names are C strings.
    if unsafe { CStr::from_ptr(name) } == c"vkEnumerateInstanceVersion" {
        enumerate_instance_version as *const c_void
    } else {
        null()
    }
}

extern "system" fn enumerate_instance_version(_version: *mut u32) -> i32 {
    VK_ERROR_INITIALIZATION_FAILED
}
