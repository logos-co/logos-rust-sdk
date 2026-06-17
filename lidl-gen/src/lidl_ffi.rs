//! FFI over logos-lidl's C ABI (`lidl/lidl_c.h`).
//!
//! `parse`/`serialize`/`validate` delegate to the canonical C++ frontend
//! instead of reimplementing the grammar in Rust — the AST crosses the
//! boundary as JSON (see `ast`). The C library is linked by `build.rs` from
//! `$LOGOS_LIDL_ROOT/lib`.

use crate::ast::ModuleDecl;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

extern "C" {
    fn lidl_parse_to_json(lidl: *const c_char, err: *mut *mut c_char) -> *mut c_char;
    fn lidl_serialize_from_json(json: *const c_char, err: *mut *mut c_char) -> *mut c_char;
    fn lidl_validate_json(json: *const c_char) -> *mut c_char;
    fn lidl_free_string(s: *mut c_char);
}

/// Take ownership of a malloc'd C string from the library, copy it into a Rust
/// `String`, and free the C allocation. Returns `None` for a null pointer.
fn take_cstring(ptr: *mut c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: ptr is a non-null, NUL-terminated string the C ABI handed us and
    // promised we own; we copy it out and hand it straight back to be freed.
    unsafe {
        let owned = CStr::from_ptr(ptr).to_string_lossy().into_owned();
        lidl_free_string(ptr);
        Some(owned)
    }
}

/// Parse `.lidl` text into an AST via the canonical C++ frontend.
pub fn parse(source: &str) -> Result<ModuleDecl, String> {
    let c_src = CString::new(source).map_err(|e| format!("lidl source has interior NUL: {e}"))?;
    let mut err: *mut c_char = std::ptr::null_mut();
    // SAFETY: c_src outlives the call; err is a valid out-pointer.
    let json_ptr = unsafe { lidl_parse_to_json(c_src.as_ptr(), &mut err) };
    if json_ptr.is_null() {
        return Err(take_cstring(err).unwrap_or_else(|| "logos-lidl: parse failed".into()));
    }
    take_cstring(err); // success: defensively drain any (unexpected) error string
    let json = take_cstring(json_ptr).ok_or("logos-lidl: parse returned null")?;
    serde_json::from_str(&json).map_err(|e| format!("decode AST JSON from logos-lidl: {e}"))
}

/// Serialize an AST back to `.lidl` text via the canonical C++ serializer.
pub fn serialize(module: &ModuleDecl) -> String {
    let json = serde_json::to_string(module).expect("serialize AST to JSON");
    let c_json = CString::new(json).expect("AST JSON has interior NUL");
    let mut err: *mut c_char = std::ptr::null_mut();
    // SAFETY: c_json outlives the call; err is a valid out-pointer.
    let lidl_ptr = unsafe { lidl_serialize_from_json(c_json.as_ptr(), &mut err) };
    if lidl_ptr.is_null() {
        let msg = take_cstring(err).unwrap_or_else(|| "unknown error".into());
        panic!("logos-lidl: serialize failed: {msg}");
    }
    take_cstring(err);
    take_cstring(lidl_ptr).expect("logos-lidl: serialize returned null")
}

/// Validate an AST. Returns `(errors, warnings)`.
pub fn validate(module: &ModuleDecl) -> (Vec<String>, Vec<String>) {
    let json = serde_json::to_string(module).expect("serialize AST to JSON");
    let c_json = CString::new(json).expect("AST JSON has interior NUL");
    // SAFETY: c_json outlives the call.
    let report_ptr = unsafe { lidl_validate_json(c_json.as_ptr()) };
    let Some(report) = take_cstring(report_ptr) else {
        return (vec!["logos-lidl: validate returned null".into()], vec![]);
    };
    #[derive(serde::Deserialize)]
    struct Report {
        #[serde(default)]
        errors: Vec<String>,
        #[serde(default)]
        warnings: Vec<String>,
    }
    match serde_json::from_str::<Report>(&report) {
        Ok(r) => (r.errors, r.warnings),
        Err(e) => (vec![format!("decode validation report: {e}")], vec![]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_the_c_abi() {
        let src = "module calc_module {\n  version \"1.0.0\"\n  depends []\n\
                   \x20 method add(a: int, b: int) -> int description \"Adds two ints\"\n\
                   \x20 method fetch() -> result\n\
                   \x20 event ready(count: uint)\n}\n";
        let m = parse(src).expect("parse");
        assert_eq!(m.name, "calc_module");
        assert_eq!(m.methods[0].description, "Adds two ints");
        assert!(m.methods[1].result_return); // `result` return-shape flag restored
        assert_eq!(m.events[0].name, "ready");

        // AST -> .lidl -> AST is stable through the canonical frontend.
        let text = serialize(&m);
        assert!(text.contains("description \"Adds two ints\""));
        assert_eq!(parse(&text).expect("reparse"), m);
    }
}
