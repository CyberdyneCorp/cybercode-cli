#![allow(unsafe_code)]
//! The only Rust FFI boundary; caller strings stay ephemeral and C never logs them.
use std::ffi::{CStr, CString, c_char, c_void};

use serde_json::{Value, json};

use super::{Backend, Capture, Health, Native, NativeTarget, Selector};
use crate::{Action, Error, control::InputPermit};

unsafe extern "C" {
    fn cyber_computer_health(prompt: bool, output: *mut *mut c_char) -> i32;
    fn cyber_computer_resolve(selector: *const c_char, output: *mut *mut c_char) -> i32;
    fn cyber_computer_capture(
        target: *const c_char,
        metadata: *mut *mut c_char,
        bytes: *mut *mut u8,
        length: *mut usize,
    ) -> i32;
    fn cyber_computer_input(
        target: *const c_char,
        action: *const c_char,
        cancelled: extern "C" fn(*const c_void) -> bool,
        context: *const c_void,
    ) -> i32;
    fn cyber_computer_free(pointer: *mut c_void);
}

fn error(code: i32) -> Error {
    match code {
        1 => Error::UnsupportedPlatform,
        2 => Error::Consent("Screen Recording or Accessibility permission is missing"),
        3 => Error::StaleFrame,
        4 => Error::Input("native operation was cancelled"),
        5 => Error::Limit("native capture exceeds its byte limit"),
        _ => Error::Native("native operation failed or timed out"),
    }
}

fn encoded(value: &impl serde::Serialize) -> Result<CString, Error> {
    CString::new(serde_json::to_vec(value).map_err(|_| Error::Input("invalid native request"))?)
        .map_err(|_| Error::Input("invalid native request"))
}

fn result<T: serde::de::DeserializeOwned>(code: i32, pointer: *mut c_char) -> Result<T, Error> {
    if code != 0 {
        if !pointer.is_null() {
            unsafe {
                cyber_computer_free(pointer.cast());
            }
        }
        return Err(error(code));
    }
    if pointer.is_null() {
        return Err(Error::Native("native result was empty"));
    }
    let parsed = unsafe { serde_json::from_slice(CStr::from_ptr(pointer).to_bytes()) };
    unsafe {
        cyber_computer_free(pointer.cast());
    }
    parsed.map_err(|_| Error::Native("invalid native result"))
}

extern "C" fn cancelled(context: *const c_void) -> bool {
    // The native function is synchronous; its borrow cannot outlive this call.
    unsafe {
        context
            .cast::<InputPermit>()
            .as_ref()
            .is_none_or(InputPermit::cancelled)
    }
}

impl Backend for Native {
    fn health(&self, prompt: bool) -> Result<Health, Error> {
        let mut output = std::ptr::null_mut();
        let mut health: Health = result(
            unsafe { cyber_computer_health(prompt, &mut output) },
            output,
        )?;
        health.binary = std::env::current_exe()
            .ok()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        Ok(health)
    }
    fn resolve(&self, selector: &Selector) -> Result<NativeTarget, Error> {
        selector.validate()?;
        let request = encoded(selector)?;
        let mut output = std::ptr::null_mut();
        let target: NativeTarget = result(
            unsafe { cyber_computer_resolve(request.as_ptr(), &mut output) },
            output,
        )?;
        target.geometry()?;
        Ok(target)
    }
    fn capture(&self, target: &NativeTarget) -> Result<Capture, Error> {
        let request = encoded(target)?;
        let mut output = std::ptr::null_mut();
        let mut bytes = std::ptr::null_mut();
        let mut length = 0;
        let code = unsafe {
            cyber_computer_capture(request.as_ptr(), &mut output, &mut bytes, &mut length)
        };
        let metadata = result(code, output);
        if bytes.is_null() {
            return Err(metadata.err().unwrap_or(Error::Native("empty capture")));
        }
        if length > 8 * 1024 * 1024 {
            unsafe {
                cyber_computer_free(bytes.cast());
            }
            return Err(Error::Limit("capture exceeds 8 MiB"));
        }
        let png = unsafe { std::slice::from_raw_parts(bytes, length).to_vec() };
        unsafe {
            cyber_computer_free(bytes.cast());
        }
        let capture = Capture {
            target: metadata?,
            png,
        };
        capture.validate()?;
        Ok(capture)
    }
    fn input(&self, target: &NativeTarget, permit: &InputPermit) -> Result<(), Error> {
        if permit.cancelled() {
            return Err(Error::Input("native operation was cancelled"));
        }
        if target.target() != *permit.target() || target.geometry()? != permit.frame().geometry {
            return Err(Error::StaleFrame);
        }
        let request = encoded(target)?;
        let action = encoded(&action(permit)?)?;
        let code = unsafe {
            cyber_computer_input(
                request.as_ptr(),
                action.as_ptr(),
                cancelled,
                (permit as *const InputPermit).cast(),
            )
        };
        if code == 0 { Ok(()) } else { Err(error(code)) }
    }
}

fn action(permit: &InputPermit) -> Result<Value, Error> {
    let geometry = permit.frame().geometry;
    let point = |point| {
        geometry
            .desktop(point)
            .map(|point| json!({"x":point.x,"y":point.y}))
    };
    Ok(match permit.action() {
        Action::Move { point: value } => json!({"action":"move","point":point(*value)?}),
        Action::Click {
            point: value,
            button,
            clicks,
        } => json!({"action":"click","point":point(*value)?,"button":button,"clicks":clicks}),
        Action::Drag {
            from,
            to,
            duration_ms,
        } => {
            json!({"action":"drag","from":point(*from)?,"to":point(*to)?,"duration_ms":duration_ms})
        }
        Action::Scroll {
            point: value,
            dx,
            dy,
        } => json!({"action":"scroll","point":point(*value)?,"dx":dx,"dy":dy}),
        Action::Type { text } => json!({"action":"type","text":text}),
        Action::Key { chord } => json!({"action":"key","chord":chord}),
        Action::Wait { milliseconds } => json!({"action":"wait","milliseconds":milliseconds}),
    })
}
