use crate::{cstr_arg, error, guarded, packet_tunnel_runtime::runtime, strings::cstring_for};
use std::{
    ffi::{c_char, c_int},
    path::Path,
    ptr,
};

fn json_result(result: anyhow::Result<serde_json::Value>) -> *mut c_char {
    let value = match result {
        Ok(value) => serde_json::json!({"ok":true,"value":value}),
        Err(error) => serde_json::json!({"ok":false,"error":format!("{error:#}")}),
    };
    cstring_for(&value.to_string(), "packet tunnel response")
        .map(|v| v.into_raw())
        .unwrap_or(ptr::null_mut())
}

/// # Safety
/// directory 必须指向有效的 UTF-8 C 字符串，返回值由 easytier_ios_free_string 释放。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn easytier_ios_packet_tunnel_start(directory: *const c_char) -> *mut c_char {
    guarded(ptr::null_mut(), || {
        json_result((|| {
            let directory =
                unsafe { cstr_arg(directory, "runtime directory") }.map_err(anyhow::Error::msg)?;
            runtime().start(Path::new(directory))
        })())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn easytier_ios_packet_tunnel_snapshot(fd: c_int) -> *mut c_char {
    guarded(ptr::null_mut(), || json_result(runtime().snapshot(fd)))
}

/// # Safety
/// url 可以为空指针；否则必须为有效的 UTF-8 C 字符串。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn easytier_ios_packet_tunnel_web(url: *const c_char) -> *mut c_char {
    guarded(ptr::null_mut(), || {
        if url.is_null() {
            return json_result(Ok(runtime().web_status()));
        }
        json_result(
            unsafe { cstr_arg(url, "web URL") }
                .map_err(anyhow::Error::msg)
                .and_then(|url| runtime().configure_web(Some(url.to_owned()))),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn easytier_ios_packet_tunnel_stop() -> c_int {
    guarded(-1, || match runtime().stop() {
        Ok(()) => 0,
        Err(e) => {
            error::set_error(&format!("{e:#}"));
            -1
        }
    })
}
