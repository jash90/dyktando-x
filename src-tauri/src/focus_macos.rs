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
    fn AXValueGetValue(value: CFTypeRef, kind: u32, out: *mut c_void) -> u8;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, encoding: u32) -> CFStringRef;
    fn CFStringGetCString(s: CFStringRef, buf: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFStringGetLength(s: CFStringRef) -> isize;
    fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    fn CFArrayGetTypeID() -> usize;
    fn CFArrayGetCount(array: CFTypeRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
    fn CFStringGetCharacters(s: CFStringRef, range: CFRange, buf: *mut u16);
    static kCFBooleanTrue: CFTypeRef;
}

#[link(name = "proc")]
extern "C" {
    fn proc_pidpath(pid: i32, buf: *mut c_void, size: u32) -> i32;
}

const UTF8: u32 = 0x0800_0100;
const AX_ERROR_NO_VALUE: i32 = -25212;
const AX_VALUE_CF_RANGE: u32 = 4;
/// How much of the field around the cursor we read (UTF-16 units).
const FIELD_BEFORE: isize = 2000;
const FIELD_AFTER: isize = 500;
/// Limits of reading the focused window's texts (Electron trees can be huge and slow).
const WINDOW_NODES: usize = 1500;
const WINDOW_CHARS: usize = 20_000;
const WINDOW_TIME: std::time::Duration = std::time::Duration::from_millis(300);

#[repr(C)]
#[derive(Clone, Copy)]
struct CFRange {
    location: isize,
    length: isize,
}

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

/// `range` of a string attribute (UTF-16 units, clamped to its length).
unsafe fn string_range(s: CFStringRef, start: isize, end: isize) -> String {
    let len = CFStringGetLength(s);
    let (start, end) = (start.clamp(0, len), end.clamp(0, len));
    if end <= start {
        return String::new();
    }
    let mut buf = vec![0u16; (end - start) as usize];
    CFStringGetCharacters(s, CFRange { location: start, length: end - start }, buf.as_mut_ptr());
    String::from_utf16_lossy(&buf)
}

fn app_path(pid: i32) -> Option<String> {
    let mut buf = vec![0u8; 4096];
    let n = unsafe { proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
}

/// The focused application: its pid and element (with Electron's accessibility switched on).
/// `Err(None)` = an empty reply, `Err(Some(code))` = an AX error.
unsafe fn focused_app() -> Result<(i32, Owned), Option<i32>> {
    let system = Owned(AXUIElementCreateSystemWide());
    AXUIElementSetMessagingTimeout(system.0, 0.25);
    let app = match copy_attr(system.0, "AXFocusedApplication") {
        Ok(a) if !a.0.is_null() => a,
        Ok(_) => return Err(None),
        Err(e) => return Err(Some(e)),
    };
    let mut pid = 0;
    AXUIElementGetPid(app.0, &mut pid);
    let app_el = Owned(AXUIElementCreateApplication(pid));
    AXUIElementSetMessagingTimeout(app_el.0, 0.25);
    // Electron (Slack, VS Code, Discord…) builds the accessibility tree only on demand.
    let manual = cfstr("AXManualAccessibility");
    AXUIElementSetAttributeValue(app_el.0, manual.0, kCFBooleanTrue);
    Ok((pid, app_el))
}

/// The focused UI element of the focused application (errors as in `focused_app`).
unsafe fn focused_element(app_el: &Owned) -> Result<Owned, Option<i32>> {
    let focused = match copy_attr(app_el.0, "AXFocusedUIElement") {
        Ok(f) if !f.0.is_null() => f,
        Ok(_) => return Err(None),
        Err(e) => return Err(Some(e)),
    };
    AXUIElementSetMessagingTimeout(focused.0, 0.25);
    Ok(focused)
}

pub fn snapshot() -> super::Snapshot {
    let mut snap = super::Snapshot::default();
    let failed = |snap: &mut super::Snapshot, e: Option<i32>| {
        snap.nothing_focused = e == Some(AX_ERROR_NO_VALUE);
        snap.failed = e != Some(AX_ERROR_NO_VALUE);
    };
    unsafe {
        let (pid, app_el) = match focused_app() {
            Ok(x) => x,
            Err(e) => {
                failed(&mut snap, e);
                return snap;
            }
        };
        // We recognize Finder by the executable path (without AppKit).
        if app_path(pid).is_some_and(|p| p.contains("/Finder.app/")) {
            snap.app = Some("com.apple.finder".into());
        }
        let focused = match focused_element(&app_el) {
            Ok(f) => f,
            Err(e) => {
                failed(&mut snap, e);
                return snap;
            }
        };
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

/// The focused field's text around the cursor; None for password fields and elements without a
/// text value or a cursor.
pub fn field_text() -> Option<super::FieldText> {
    unsafe {
        let (_, app_el) = focused_app().ok()?;
        let focused = focused_element(&app_el).ok()?;
        let secure = |attr| string_attr(focused.0, attr).as_deref() == Some("AXSecureTextField");
        if secure("AXRole") || secure("AXSubrole") {
            return None;
        }
        let value = copy_attr(focused.0, "AXValue").ok()?;
        if value.0.is_null() || CFGetTypeID(value.0) != CFStringGetTypeID() {
            return None;
        }
        let range = copy_attr(focused.0, "AXSelectedTextRange").ok()?;
        let mut sel = CFRange { location: 0, length: 0 };
        if range.0.is_null() || AXValueGetValue(range.0, AX_VALUE_CF_RANGE, &mut sel as *mut CFRange as *mut c_void) == 0 {
            return None;
        }
        let (start, end) = (sel.location, sel.location + sel.length.max(0));
        Some(super::FieldText {
            before: string_range(value.0, start - FIELD_BEFORE, start),
            selected: string_range(value.0, start, end),
            after: string_range(value.0, end, end + FIELD_AFTER),
        })
    }
}

/// A string attribute of any length (the first `max` UTF-16 units).
unsafe fn long_string_attr(el: AXUIElementRef, name: &str, max: isize) -> Option<String> {
    let v = copy_attr(el, name).ok()?;
    (!v.0.is_null() && CFGetTypeID(v.0) == CFStringGetTypeID()).then(|| string_range(v.0, 0, max))
}

/// Texts of the focused window (values, titles, descriptions) in tree order, within time and size
/// limits; password fields skipped.
pub fn window_text() -> Option<String> {
    let deadline = std::time::Instant::now() + WINDOW_TIME;
    unsafe {
        let (_, app_el) = focused_app().ok()?;
        let window = copy_attr(app_el.0, "AXFocusedWindow").ok().filter(|w| !w.0.is_null())?;
        let mut queue = std::collections::VecDeque::from([window]);
        let (mut out, mut nodes) = (String::new(), 0);
        while let Some(el) = queue.pop_front() {
            nodes += 1;
            if nodes > WINDOW_NODES || out.len() > WINDOW_CHARS || std::time::Instant::now() > deadline {
                break;
            }
            AXUIElementSetMessagingTimeout(el.0, 0.1);
            // A password field is `AXTextField` with the subrole `AXSecureTextField`.
            let secure = |attr| string_attr(el.0, attr).as_deref() == Some("AXSecureTextField");
            if secure("AXRole") || secure("AXSubrole") {
                continue;
            }
            for attr in ["AXTitle", "AXValue", "AXDescription"] {
                if let Some(text) = long_string_attr(el.0, attr, 2000).filter(|t| !t.trim().is_empty()) {
                    out.push_str(text.trim());
                    out.push('\n');
                }
            }
            let Ok(children) = copy_attr(el.0, "AXChildren") else { continue };
            if children.0.is_null() || CFGetTypeID(children.0) != CFArrayGetTypeID() {
                continue;
            }
            for i in 0..CFArrayGetCount(children.0) {
                let child = CFArrayGetValueAtIndex(children.0, i);
                if !child.is_null() {
                    queue.push_back(Owned(CFRetain(child)));
                }
            }
        }
        Some(out)
    }
}
