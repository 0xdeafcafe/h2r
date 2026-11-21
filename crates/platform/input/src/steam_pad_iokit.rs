//! The thin macOS half of [`super`]: one `IOHIDManager` matching every
//! Steam Controller product ID, opened with `kIOHIDOptionsTypeNone` (never
//! seizing a device), its callbacks queueing [`HidEvent`]s on this thread's
//! run loop, and reports sent with `IOHIDDeviceSetReport`. Each interface is
//! keyed by its `IOHIDDevice` address. Nothing here is tested without
//! hardware; the logic it feeds is ([`super::Hub`]).

use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::io;
use std::ptr::NonNull;
use std::sync::Arc;
use std::time::Duration;

use objc2_core_foundation::{
    CFArray, CFDictionary, CFIndex, CFNumber, CFRetained, CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext, CFString,
    kCFRunLoopDefaultMode,
};
use objc2_io_kit::{
    IOHIDDevice, IOHIDManager, IOHIDReportType, IOReturn, kIOHIDOptionsTypeNone,
    kIOHIDPrimaryUsageKey, kIOHIDPrimaryUsagePageKey, kIOHIDProductIDKey, kIOHIDVendorIDKey,
    kIOReturnSuccess,
};

use super::{HidDevice, HidEvent};
use crate::steam::{HidWrite, PRODUCTS, VALVE_VID};

/// Events queued beyond this are dropped from the front.
const MAX_QUEUED: usize = 256;

#[derive(Default)]
struct Inbox {
    devices: HashMap<u64, CFRetained<IOHIDDevice>>,
    events: VecDeque<HidEvent>,
}

impl Inbox {
    fn push(&mut self, e: HidEvent) {
        if self.events.len() == MAX_QUEUED {
            self.events.pop_front();
        }
        self.events.push_back(e);
    }
}

fn key(device: *const c_void) -> u64 {
    device as usize as u64
}

fn number(device: &IOHIDDevice, name: &std::ffi::CStr) -> u32 {
    let k = CFString::from_str(name.to_str().unwrap_or(""));
    device
        .property(&k)
        .and_then(|v| v.downcast::<CFNumber>().ok())
        .and_then(|n| n.as_i64())
        .map_or(0, |v| v as u32)
}

unsafe extern "C-unwind" fn matched(
    ctx: *mut c_void,
    _: IOReturn,
    _: *mut c_void,
    device: NonNull<IOHIDDevice>,
) {
    // SAFETY: `ctx` is the `Inbox` the `Host` owns, alive while scheduled.
    let inbox = unsafe { &mut *(ctx as *mut Inbox) };
    // SAFETY: the manager hands us a valid device; retain our own reference.
    let d = unsafe { CFRetained::retain(device) };
    let k = key(device.as_ptr() as *const c_void);
    let info = HidDevice {
        key: k,
        vid: number(&d, kIOHIDVendorIDKey) as u16,
        pid: number(&d, kIOHIDProductIDKey) as u16,
        usage_page: number(&d, kIOHIDPrimaryUsagePageKey),
        usage: number(&d, kIOHIDPrimaryUsageKey),
    };
    inbox.devices.insert(k, d);
    inbox.push(HidEvent::Added(info));
}

unsafe extern "C-unwind" fn removed(
    ctx: *mut c_void,
    _: IOReturn,
    _: *mut c_void,
    device: NonNull<IOHIDDevice>,
) {
    // SAFETY: as in `matched`.
    let inbox = unsafe { &mut *(ctx as *mut Inbox) };
    let k = key(device.as_ptr() as *const c_void);
    if inbox.devices.remove(&k).is_some() {
        inbox.push(HidEvent::Removed(k));
    }
}

unsafe extern "C-unwind" fn report(
    ctx: *mut c_void,
    _: IOReturn,
    sender: *mut c_void,
    _: IOHIDReportType,
    id: u32,
    bytes: NonNull<u8>,
    len: CFIndex,
) {
    // SAFETY: as in `matched`; `bytes` holds `len` bytes for this call.
    let inbox = unsafe { &mut *(ctx as *mut Inbox) };
    let data = unsafe { std::slice::from_raw_parts(bytes.as_ptr(), len.max(0) as usize) };
    // A device that numbers its reports gets its ID first, as hidapi
    // gives it; the 2015 controller's wired reports (ID 0) start with
    // their header.
    let mut r = Vec::with_capacity(data.len() + 1);
    if id != 0 && data.first() != Some(&(id as u8)) {
        r.push(id as u8);
    }
    r.extend_from_slice(data);
    inbox.push(HidEvent::Report(key(sender), r));
}

// A signalled source stays pending if shutdown races entry to run_in_mode.
struct Wake {
    run_loop: CFRetained<CFRunLoop>,
    source: CFRetained<CFRunLoopSource>,
}

// SAFETY: these retained objects are used across threads only to signal
// the source and wake its run loop. No HID callbacks or state cross threads.
unsafe impl Send for Wake {}
unsafe impl Sync for Wake {}

impl Wake {
    fn signal(&self) {
        self.source.signal();
        self.run_loop.wake_up();
    }
}

struct Host {
    manager: CFRetained<IOHIDManager>,
    wake: Arc<Wake>,
    inbox: Box<Inbox>,
}

fn matcher(pid: u16) -> CFRetained<CFDictionary<CFString, CFNumber>> {
    let vendor = CFString::from_str(kIOHIDVendorIDKey.to_str().unwrap_or("VendorID"));
    let product = CFString::from_str(kIOHIDProductIDKey.to_str().unwrap_or("ProductID"));
    let (v, p) = (
        CFNumber::new_i32(i32::from(VALVE_VID)),
        CFNumber::new_i32(i32::from(pid)),
    );
    CFDictionary::from_slices(&[&*vendor, &*product], &[&*v, &*p])
}

/// The host, or `None` if the manager will not open.
pub(super) fn open() -> Option<Box<dyn super::SteamHid>> {
    let manager = IOHIDManager::new(None, kIOHIDOptionsTypeNone);
    let matchers = CFArray::from_retained_objects(
        &PRODUCTS
            .iter()
            .map(|&(p, ..)| matcher(p))
            .collect::<Vec<_>>(),
    );
    let run_loop = CFRunLoop::current()?;
    let mode = unsafe { kCFRunLoopDefaultMode }?;
    unsafe extern "C-unwind" fn interrupted(_: *mut c_void) {}
    // A version-zero source has no data to retain and only a perform callback.
    let mut context = CFRunLoopSourceContext {
        version: 0,
        info: std::ptr::null_mut(),
        retain: None,
        release: None,
        copyDescription: None,
        equal: None,
        hash: None,
        schedule: None,
        cancel: None,
        perform: Some(interrupted),
    };
    // SAFETY: CF copies the version-zero context and its static function pointer.
    let source = unsafe { CFRunLoopSource::new(None, 0, &mut context) }?;
    run_loop.add_source(Some(&source), Some(mode));
    let mut inbox = Box::<Inbox>::default();
    let ctx = &mut *inbox as *mut Inbox as *mut c_void;
    // SAFETY: the matchers are dictionaries; `ctx` outlives the
    // registration (the `Host` owns it and unschedules before dropping it).
    unsafe {
        manager.set_device_matching_multiple(Some(matchers.as_opaque()));
        manager.register_device_matching_callback(Some(matched), ctx);
        manager.register_device_removal_callback(Some(removed), ctx);
        manager.register_input_report_callback(Some(report), ctx);
        manager.schedule_with_run_loop(&run_loop, mode);
    }
    let host = Host {
        manager,
        wake: Arc::new(Wake { run_loop, source }),
        inbox,
    };
    if host.manager.open(kIOHIDOptionsTypeNone) != kIOReturnSuccess {
        return None;
    }
    Some(Box::new(host))
}

impl super::SteamHid for Host {
    fn wait(&mut self, timeout: Duration) -> io::Result<Vec<HidEvent>> {
        if self.inbox.events.is_empty() {
            CFRunLoop::run_in_mode(
                unsafe { kCFRunLoopDefaultMode },
                timeout.as_secs_f64(),
                true,
            );
        }
        Ok(self.inbox.events.drain(..).collect())
    }

    fn wake_handle(&self) -> Option<Box<dyn Fn() + Send + Sync>> {
        let wake = self.wake.clone();
        Some(Box::new(move || wake.signal()))
    }

    /// hidapi's rule: report ID 0 means an unnumbered report, sent without
    /// the ID byte; any other ID is sent with it.
    fn write(&mut self, key: u64, w: &HidWrite) -> io::Result<()> {
        let device = self
            .inbox
            .devices
            .get(&key)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected))?;
        let id = *w.data.first().ok_or(io::ErrorKind::InvalidInput)?;
        let bytes = if id == 0 { &w.data[1..] } else { &w.data[..] };
        let ptr = NonNull::new(bytes.as_ptr() as *mut u8)
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let kind = if w.feature {
            IOHIDReportType::Feature
        } else {
            IOHIDReportType::Output
        };
        // SAFETY: `bytes` is valid for its length for the duration of the call.
        let ret =
            unsafe { device.set_report(kind, CFIndex::from(id), ptr, bytes.len() as CFIndex) };
        if ret == kIOReturnSuccess {
            Ok(())
        } else {
            Err(io::Error::other(format!("IOHIDDeviceSetReport: {ret:#x}")))
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        // SAFETY: same run loop and mode as scheduled; after this no callback
        // can reach the inbox.
        unsafe {
            if let Some(mode) = kCFRunLoopDefaultMode {
                self.manager.unschedule_from_run_loop(&self.wake.run_loop, mode);
                self.wake.run_loop.remove_source(Some(&self.wake.source), Some(mode));
            }
        }
        self.manager.close(kIOHIDOptionsTypeNone);
    }
}
