//! macOS: the focused element via Accessibility (direct FFI to ApplicationServices).
use std::ffi::{c_void, CString};
use std::os::raw::c_char;

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type AXUIElementRef = *const c_void;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(el: AXUIElementRef, attr: CFStringRef, value: *mut CFTypeRef) -> i32;
    fn AXUIElementSetAttributeValue(el: AXUIElementRef, attr: CFStringRef, value: CFTypeRef) -> i32;
    fn AXUIElementIsAttributeSettable(el: AXUIElementRef, attr: CFStringRef, settable: *mut u8) -> i32;
    fn AXUIElementSetMessagingTimeout(el: AXUIElementRef, seconds: f32) -> i32;
    fn AXUIElementGetPid(el: AXUIElementRef, pid: *mut i32) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, encoding: u32) -> CFStringRef;
    fn CFStringGetCString(s: CFStringRef, buf: *mut c_char, size: isize, encoding: u32) -> u8;
    static kCFBooleanTrue: CFTypeRef;
}

#[link(name = "proc")]
extern "C" {
    fn proc_pidpath(pid: i32, buf: *mut c_void, size: u32) -> i32;
}

const UTF8: u32 = 0x0800_0100;
const AX_ERROR_NO_VALUE: i32 = -25212;

struct Owned(CFTypeRef);
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}

fn cfstr(s: &str) -> Owned {
    let c = CString::new(s).expect("no NUL");
    Owned(unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) })
}

unsafe fn copy_attr(el: AXUIElementRef, name: &str) -> Result<Owned, i32> {
    let attr = cfstr(name);
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(el, attr.0, &mut value);
    if err == 0 { Ok(Owned(value)) } else { Err(err) }
}

unsafe fn string_attr(el: AXUIElementRef, name: &str) -> Option<String> {
    let v = copy_attr(el, name).ok()?;
    if v.0.is_null() || CFGetTypeID(v.0) != CFStringGetTypeID() {
        return None;
    }
    let mut buf = vec![0 as c_char; 256];
    (CFStringGetCString(v.0, buf.as_mut_ptr(), buf.len() as isize, UTF8) != 0)
        .then(|| std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
}

fn app_path(pid: i32) -> Option<String> {
    let mut buf = vec![0u8; 4096];
    let n = unsafe { proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

pub fn snapshot() -> super::Snapshot {
    let mut snap = super::Snapshot::default();
    unsafe {
        let system = Owned(AXUIElementCreateSystemWide());
        AXUIElementSetMessagingTimeout(system.0, 0.25);
        let app = match copy_attr(system.0, "AXFocusedApplication") {
            Ok(a) if !a.0.is_null() => a,
            Ok(_) => {
                snap.failed = true;
                return snap;
            }
            Err(e) => {
                snap.nothing_focused = e == AX_ERROR_NO_VALUE;
                snap.failed = e != AX_ERROR_NO_VALUE;
                return snap;
            }
        };
        let mut pid = 0;
        AXUIElementGetPid(app.0, &mut pid);
        // We recognize Finder by the executable path (without AppKit).
        if app_path(pid).is_some_and(|p| p.contains("/Finder.app/")) {
            snap.app = Some("com.apple.finder".into());
        }
        let app_el = Owned(AXUIElementCreateApplication(pid));
        AXUIElementSetMessagingTimeout(app_el.0, 0.25);
        // Electron (Slack, VS Code, Discord…) builds the accessibility tree only on demand.
        let manual = cfstr("AXManualAccessibility");
        AXUIElementSetAttributeValue(app_el.0, manual.0, kCFBooleanTrue);
        let focused = match copy_attr(app_el.0, "AXFocusedUIElement") {
            Ok(f) if !f.0.is_null() => f,
            Ok(_) => {
                snap.failed = true;
                return snap;
            }
            Err(e) => {
                snap.nothing_focused = e == AX_ERROR_NO_VALUE;
                snap.failed = e != AX_ERROR_NO_VALUE;
                return snap;
            }
        };
        AXUIElementSetMessagingTimeout(focused.0, 0.25);
        snap.role = string_attr(focused.0, "AXRole");
        let value = cfstr("AXValue");
        let mut settable = 0u8;
        if AXUIElementIsAttributeSettable(focused.0, value.0, &mut settable) == 0 {
            snap.value_settable = settable != 0;
        }
        snap.has_text_range = copy_attr(focused.0, "AXSelectedTextRange").is_ok();
        snap.editable_ancestor = copy_attr(focused.0, "AXEditableAncestor").is_ok();
    }
    snap
}
