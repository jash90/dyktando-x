//! macOS 14.2+: procesy Core Audio z aktywnym wejściem (`kAudioProcessPropertyIsRunningInput`).
use objc2_core_audio::*;
use objc2_core_foundation::CFString;
use std::ffi::c_void;
use std::ptr::NonNull;

fn address(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress { mSelector: selector, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain }
}

pub fn processes_using_microphone() -> Vec<String> {
    unsafe {
        let system = kAudioObjectSystemObject as AudioObjectID;
        let mut addr = address(kAudioHardwarePropertyProcessObjectList);
        let mut size: u32 = 0;
        if AudioObjectGetPropertyDataSize(system, NonNull::from(&mut addr), 0, std::ptr::null(), NonNull::from(&mut size)) != 0 || size == 0 {
            return Vec::new();
        }
        let mut objects = vec![0 as AudioObjectID; size as usize / std::mem::size_of::<AudioObjectID>()];
        if AudioObjectGetPropertyData(system, NonNull::from(&mut addr), 0, std::ptr::null(), NonNull::from(&mut size), NonNull::new_unchecked(objects.as_mut_ptr() as *mut c_void)) != 0 {
            return Vec::new();
        }
        let own_pid = std::process::id() as i32;
        objects
            .into_iter()
            .filter_map(|obj| {
                let mut running: u32 = 0;
                let mut rsize = 4u32;
                let mut a = address(kAudioProcessPropertyIsRunningInput);
                if AudioObjectGetPropertyData(obj, NonNull::from(&mut a), 0, std::ptr::null(), NonNull::from(&mut rsize), NonNull::from(&mut running).cast()) != 0 || running == 0 {
                    return None;
                }
                let mut pid: i32 = 0;
                let mut psize = 4u32;
                let mut a = address(kAudioProcessPropertyPID);
                if AudioObjectGetPropertyData(obj, NonNull::from(&mut a), 0, std::ptr::null(), NonNull::from(&mut psize), NonNull::from(&mut pid).cast()) == 0 && pid == own_pid {
                    return None;
                }
                let mut bundle: *const CFString = std::ptr::null();
                let mut bsize = std::mem::size_of::<*const CFString>() as u32;
                let mut a = address(kAudioProcessPropertyBundleID);
                if AudioObjectGetPropertyData(obj, NonNull::from(&mut a), 0, std::ptr::null(), NonNull::from(&mut bsize), NonNull::from(&mut bundle).cast()) != 0 || bundle.is_null() {
                    return None;
                }
                // Właściwość zwraca CFString na +1.
                let s = objc2_core_foundation::CFRetained::from_raw(NonNull::new_unchecked(bundle as *mut CFString));
                Some(s.to_string())
            })
            .collect()
    }
}
