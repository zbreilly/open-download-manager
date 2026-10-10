//! Panics inside the engine must come back as `{"err":...}`, never unwind
//! across the C ABI (which would abort the host app).

use odm_ffi::{odm_call, odm_free, odm_shutdown, odm_start};
use std::ffi::{CStr, CString};

fn call(method: &str, args: &str) -> String {
    let m = CString::new(method).unwrap();
    let a = CString::new(args).unwrap();
    unsafe {
        let p = odm_call(m.as_ptr(), a.as_ptr());
        assert!(!p.is_null());
        let s = CStr::from_ptr(p).to_string_lossy().into_owned();
        odm_free(p);
        s
    }
}

#[test]
fn panics_do_not_cross_the_c_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let d = CString::new(dir.path().to_str().unwrap()).unwrap();
    // NULL callback is allowed
    assert_eq!(unsafe { odm_start(d.as_ptr(), None, std::ptr::null_mut()) }, 0);
    let r = call("__panic", "{}");
    assert!(r.contains("\"err\"") && r.contains("internal error"), "{r}");
    // the engine still answers afterwards
    let v = call("version", "{}");
    assert!(v.contains(env!("CARGO_PKG_VERSION")), "{v}");
    let u = call("no_such_method", "{}");
    assert!(u.contains("unknown method"), "{u}");
    odm_shutdown();
}
