//! Core Audio process tap (macOS 14.4+), wzór: github.com/insidegui/AudioCap i wersja Swift.
//! Tap globalny (stereo) z wykluczeniem własnego procesu → prywatne urządzenie zbiorcze →
//! IOProc na własnej kolejce. Zgody nie da się sprawdzić z góry: tap utworzony przed nią
//! nie oddaje żadnych buforów (wykrywa to `MeetingRecorder` i tworzy tap od nowa).
use anyhow::{anyhow, Result};
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_core_audio::*;
use objc2_core_audio_types::{AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp, kAudioFormatFlagIsNonInterleaved};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUUID};
use std::ffi::{c_void, CStr};
use std::ptr::NonNull;
use std::sync::Mutex;

use super::Sink;

pub fn availability() -> Result<(), String> {
    let (major, minor) = macos_version();
    if (major, minor) >= (14, 4) {
        Ok(())
    } else {
        Err(format!("Nagrywanie dźwięku aplikacji wymaga macOS 14.4 lub nowszego (masz {major}.{minor}) — nagrywany będzie tylko mikrofon."))
    }
}

fn macos_version() -> (u32, u32) {
    let out = std::process::Command::new("sw_vers").arg("-productVersion").output();
    let v = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    let mut it = v.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0))
}

fn check(status: i32, what: &str) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(anyhow!("Dźwięk systemowy: {what} (błąd {status})"))
    }
}

fn global(selector: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

unsafe fn get_property<T: Copy>(object: AudioObjectID, selector: u32, qualifier: Option<&[u8]>, mut value: T) -> Result<T, i32> {
    let mut address = global(selector);
    let mut size = std::mem::size_of::<T>() as u32;
    let (qsize, qptr) = qualifier.map_or((0, std::ptr::null()), |q| (q.len() as u32, q.as_ptr() as *const c_void));
    let status = AudioObjectGetPropertyData(
        object,
        NonNull::from(&mut address),
        qsize,
        qptr,
        NonNull::from(&mut size),
        NonNull::from(&mut value).cast(),
    );
    if status == 0 { Ok(value) } else { Err(status) }
}

fn own_process_object() -> Option<AudioObjectID> {
    let pid = std::process::id() as i32;
    let obj = unsafe { get_property::<AudioObjectID>(kAudioObjectSystemObject as u32, kAudioHardwarePropertyTranslatePIDToProcessObject, Some(&pid.to_ne_bytes()), 0) };
    obj.ok().filter(|o| *o != kAudioObjectUnknown)
}

/// UID wyjścia, na którym grają aplikacje (Meet, Zoom, Teams). Urządzenie „dźwięków systemowych”
/// bywa inne (np. wirtualne Background Music) i ma inną częstotliwość — tylko jako zapas.
fn default_output_uid() -> Result<String> {
    output_uid(kAudioHardwarePropertyDefaultOutputDevice).or_else(|_| output_uid(kAudioHardwarePropertyDefaultSystemOutputDevice))
}

fn output_uid(selector: u32) -> Result<String> {
    unsafe {
        let device = get_property::<AudioObjectID>(kAudioObjectSystemObject as u32, selector, None, 0)
            .map_err(|s| anyhow!("Dźwięk systemowy: brak domyślnego wyjścia (błąd {s})"))?;
        let uid = get_property::<*const NSString>(device, kAudioDevicePropertyDeviceUID, None, std::ptr::null())
            .map_err(|s| anyhow!("Dźwięk systemowy: brak UID wyjścia (błąd {s})"))?;
        if uid.is_null() {
            return Err(anyhow!("Dźwięk systemowy: brak UID wyjścia"));
        }
        // Właściwość zwraca CFString na +1 — przejmujemy własność.
        let uid: Retained<NSString> = Retained::from_raw(uid as *mut NSString).ok_or_else(|| anyhow!("UID"))?;
        Ok(uid.to_string())
    }
}

fn ns(key: &CStr) -> Retained<NSString> {
    NSString::from_str(key.to_str().expect("klucz Core Audio to ASCII"))
}

fn dict(pairs: &[(&CStr, Retained<AnyObject>)]) -> Retained<NSDictionary<NSString, AnyObject>> {
    let keys: Vec<Retained<NSString>> = pairs.iter().map(|(k, _)| ns(k)).collect();
    let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let values: Vec<&AnyObject> = pairs.iter().map(|(_, v)| &**v).collect();
    NSDictionary::from_slices(&key_refs, &values)
}

fn obj<T: objc2::Message>(r: Retained<T>) -> Retained<AnyObject> {
    // Każdy obiekt Foundation jest AnyObject — rzutowanie bez zmiany licznika referencji.
    unsafe { Retained::cast_unchecked(r) }
}

type IoBlock = RcBlock<dyn Fn(NonNull<AudioTimeStamp>, NonNull<AudioBufferList>, NonNull<AudioTimeStamp>, NonNull<AudioBufferList>, NonNull<AudioTimeStamp>)>;

pub struct Capture {
    tap: AudioObjectID,
    aggregate: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    _block: Option<IoBlock>,
    _queue: Option<DispatchRetained<DispatchQueue>>,
}

// Identyfikatory Core Audio i blok są bezpieczne do przeniesienia między wątkami.
unsafe impl Send for Capture {}

impl Capture {
    pub fn start(sink: Sink) -> Result<(Self, u32)> {
        let mut c = Capture { tap: kAudioObjectUnknown, aggregate: kAudioObjectUnknown, proc_id: None, _block: None, _queue: None };
        match unsafe { c.start_tap(sink) } {
            Ok(rate) => Ok((c, rate)),
            Err(e) => {
                c.teardown(); // nie zostawiaj w systemie tapu ani urządzenia zbiorczego z połowy startu
                Err(e)
            }
        }
    }

    unsafe fn start_tap(&mut self, sink: Sink) -> Result<u32> {
        let excluded: Vec<Retained<NSNumber>> = own_process_object().map(|o| NSNumber::new_u32(o)).into_iter().collect();
        let excluded = NSArray::from_retained_slice(&excluded);
        let description = CATapDescription::initStereoGlobalTapButExcludeProcesses(CATapDescription::alloc(), &excluded);
        let uuid = NSUUID::new();
        description.setUUID(&uuid);
        description.setName(&NSString::from_str("Dyktando X — nagrywanie spotkania"));
        description.setPrivate(true);

        let mut tap = kAudioObjectUnknown;
        check(AudioHardwareCreateProcessTap(Some(&description), &mut tap), "nie udało się utworzyć tapu")?;
        self.tap = tap;

        let asbd = get_property::<AudioStreamBasicDescription>(tap, kAudioTapPropertyFormat, None, std::mem::zeroed())
            .map_err(|s| anyhow!("Dźwięk systemowy: brak formatu tapu (błąd {s})"))?;
        let tap_rate = asbd.mSampleRate.round() as u32;
        let channels = asbd.mChannelsPerFrame.max(1) as usize;
        let non_interleaved = asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved != 0;
        if asbd.mBitsPerChannel != 32 {
            return Err(anyhow!("Dźwięk systemowy: nieobsługiwany format tapu ({} bitów)", asbd.mBitsPerChannel));
        }

        let output = default_output_uid()?;
        let sub_device = dict(&[(kAudioSubDeviceUIDKey, obj(NSString::from_str(&output)))]);
        let sub_tap = dict(&[
            (kAudioSubTapDriftCompensationKey, obj(NSNumber::new_bool(true))),
            (kAudioSubTapUIDKey, obj(uuid.UUIDString())),
        ]);
        let description = dict(&[
            (kAudioAggregateDeviceNameKey, obj(NSString::from_str("Dyktando X — tap spotkania"))),
            (kAudioAggregateDeviceUIDKey, obj(NSUUID::new().UUIDString())),
            (kAudioAggregateDeviceMainSubDeviceKey, obj(NSString::from_str(&output))),
            (kAudioAggregateDeviceIsPrivateKey, obj(NSNumber::new_bool(true))),
            (kAudioAggregateDeviceIsStackedKey, obj(NSNumber::new_bool(false))),
            (kAudioAggregateDeviceTapAutoStartKey, obj(NSNumber::new_bool(true))),
            (kAudioAggregateDeviceSubDeviceListKey, obj(NSArray::from_retained_slice(&[sub_device]))),
            (kAudioAggregateDeviceTapListKey, obj(NSArray::from_retained_slice(&[sub_tap]))),
        ]);
        let cf: &CFDictionary = &*(Retained::as_ptr(&description) as *const CFDictionary);
        let mut aggregate = kAudioObjectUnknown;
        check(AudioHardwareCreateAggregateDevice(cf, NonNull::from(&mut aggregate)), "nie udało się utworzyć urządzenia zbiorczego")?;
        self.aggregate = aggregate;
        // Bufory przychodzą w takcie urządzenia zbiorczego (tap jest do niego dopasowywany), nie
        // w formacie tapu: z wyjściem 16 kHz i tapem 48 kHz liczenie po tapie gubiło 2/3 dźwięku.
        let rate = match get_property::<f64>(aggregate, kAudioDevicePropertyNominalSampleRate, None, 0.0) {
            Ok(r) if r > 0.0 => r.round() as u32,
            _ => tap_rate,
        };

        let sink = Mutex::new(sink);
        let mut mono: Vec<f32> = Vec::new();
        let mono = Mutex::new(std::mem::take(&mut mono));
        let block: IoBlock = RcBlock::new(move |_now: NonNull<AudioTimeStamp>, input: NonNull<AudioBufferList>, _t: NonNull<AudioTimeStamp>, _o: NonNull<AudioBufferList>, _ot: NonNull<AudioTimeStamp>| {
            let list = input.as_ref();
            let buffers = std::slice::from_raw_parts(list.mBuffers.as_ptr(), list.mNumberBuffers as usize);
            let (Ok(mut out), Ok(mut sink)) = (mono.lock(), sink.lock()) else { return };
            out.clear();
            if non_interleaved {
                // Osobny bufor na kanał — uśredniamy (inaczej zostałby tylko lewy kanał).
                let frames = buffers.first().map_or(0, |b| b.mDataByteSize as usize / 4);
                out.resize(frames, 0.0);
                let n = buffers.len().max(1) as f32;
                for b in buffers {
                    if b.mData.is_null() { continue; }
                    let data = std::slice::from_raw_parts(b.mData as *const f32, (b.mDataByteSize as usize / 4).min(frames));
                    for (o, s) in out.iter_mut().zip(data) {
                        *o += s / n;
                    }
                }
            } else if let Some(b) = buffers.last() {
                // Tap to ostatni strumień wejściowy urządzenia zbiorczego (wyjście z mikrofonem,
                // np. słuchawki, dokłada własny strumień przed nim).
                if b.mData.is_null() { return; }
                let ch = (b.mNumberChannels as usize).max(1).min(channels.max(1));
                let data = std::slice::from_raw_parts(b.mData as *const f32, b.mDataByteSize as usize / 4);
                out.extend(data.chunks_exact(ch).map(|f| f.iter().sum::<f32>() / ch as f32));
            }
            sink(&out);
        });
        let queue = DispatchQueue::new("dyktando.meeting.systemtap", None);
        let mut proc_id: AudioDeviceIOProcID = None;
        check(
            AudioDeviceCreateIOProcIDWithBlock(NonNull::from(&mut proc_id), aggregate, Some(&queue), &*block as *const _ as *mut _),
            "nie udało się podpiąć odczytu",
        )?;
        self.proc_id = proc_id;
        self._block = Some(block);
        self._queue = Some(queue);
        check(AudioDeviceStart(aggregate, proc_id), "nie udało się wystartować")?;
        log::info!("Tap systemowy: tap {tap_rate} Hz, urządzenie zbiorcze {rate} Hz, {channels} kan., wyjście {output}");
        Ok(rate)
    }

    fn teardown(&mut self) {
        unsafe {
            if self.aggregate != kAudioObjectUnknown {
                if self.proc_id.is_some() {
                    AudioDeviceStop(self.aggregate, self.proc_id);
                    AudioDeviceDestroyIOProcID(self.aggregate, self.proc_id);
                }
                AudioHardwareDestroyAggregateDevice(self.aggregate);
            }
            if self.tap != kAudioObjectUnknown {
                AudioHardwareDestroyProcessTap(self.tap);
            }
        }
        self.proc_id = None;
        self.aggregate = kAudioObjectUnknown;
        self.tap = kAudioObjectUnknown;
        self._block = None;
        self._queue = None;
    }

    pub fn stop(mut self) {
        self.teardown();
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.teardown();
    }
}
