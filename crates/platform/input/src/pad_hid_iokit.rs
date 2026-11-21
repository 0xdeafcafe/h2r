//! The thin macOS half of [`super`]: an `IOHIDManager` matching Sony's
//! DualSense or DualShock 4 product IDs, opened with `kIOHIDOptionsTypeNone` so the device
//! is shared with GameController.framework and gilrs (which also open it
//! unseized), input reports collected through the manager's callback on this
//! thread's run loop, and output reports sent with `IOHIDDeviceSetReport`.
//! Everything here runs on the HID thread. Nothing in it is tested without
//! hardware; the logic it feeds is ([`super::PadLink`]).

use std::collections::VecDeque;
use std::ffi::c_void;
use std::io;
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use objc2_core_foundation::{
    CFArray, CFDictionary, CFIndex, CFNumber, CFRetained, CFRunLoop, CFRunLoopRunResult, CFString,
    kCFRunLoopDefaultMode,
};
use objc2_io_kit::{
    IOHIDDevice, IOHIDManager, IOHIDReportType, IOReturn, kIOHIDOptionsTypeNone,
    kIOHIDProductIDKey, kIOHIDTransportKey, kIOHIDVendorIDKey, kIOReturnSuccess,
};

use crate::dualsense::{SONY_VID, Transport};

/// What the callbacks write; boxed so its address stays put.
#[derive(Default)]
struct Inbox {
    device: Option<CFRetained<IOHIDDevice>>,
    reports: VecDeque<Vec<u8>>,
    removed: bool,
}

/// Input reports queued beyond this are dropped (only the latest mute
/// state matters).
const MAX_QUEUED: usize = 64;

unsafe extern "C-unwind" fn matched(
    ctx: *mut c_void,
    _: IOReturn,
    _: *mut c_void,
    device: NonNull<IOHIDDevice>,
) {
    // SAFETY: `ctx` is the `Inbox` the `Pad` owns, alive while scheduled.
    let inbox = unsafe { &mut *(ctx as *mut Inbox) };
    if inbox.device.is_none() {
        // SAFETY: the manager hands us a valid device; retain our own reference.
        inbox.device = Some(unsafe { CFRetained::retain(device) });
    }
}

unsafe extern "C-unwind" fn removed(
    ctx: *mut c_void,
    _: IOReturn,
    _: *mut c_void,
    device: NonNull<IOHIDDevice>,
) {
    // SAFETY: as in `matched`.
    let inbox = unsafe { &mut *(ctx as *mut Inbox) };
    if inbox
        .device
        .as_deref()
        .is_some_and(|d| std::ptr::eq(d, device.as_ptr()))
    {
        inbox.removed = true;
    }
}

unsafe extern "C-unwind" fn report(
    ctx: *mut c_void,
    _: IOReturn,
    _: *mut c_void,
    _: IOHIDReportType,
    id: u32,
    bytes: NonNull<u8>,
    len: CFIndex,
) {
    // SAFETY: as in `matched`; `bytes` holds `len` bytes for this call.
    let inbox = unsafe { &mut *(ctx as *mut Inbox) };
    let data = unsafe { std::slice::from_raw_parts(bytes.as_ptr(), len.max(0) as usize) };
    // macOS includes the report ID as the first byte when the device uses
    // IDs; make sure it is there either way.
    let mut r = Vec::with_capacity(data.len() + 1);
    if data.first() != Some(&(id as u8)) {
        r.push(id as u8);
    }
    r.extend_from_slice(data);
    if inbox.reports.len() == MAX_QUEUED {
        inbox.reports.pop_front();
    }
    inbox.reports.push_back(r);
}

struct Pad {
    manager: CFRetained<IOHIDManager>,
    run_loop: CFRetained<CFRunLoop>,
    inbox: Box<Inbox>,
    transport: Transport,
}

fn matcher(pid: u16) -> CFRetained<CFDictionary<CFString, CFNumber>> {
    let vendor = CFString::from_str(kIOHIDVendorIDKey.to_str().unwrap_or("VendorID"));
    let product = CFString::from_str(kIOHIDProductIDKey.to_str().unwrap_or("ProductID"));
    let (v, p) = (
        CFNumber::new_i32(i32::from(SONY_VID)),
        CFNumber::new_i32(i32::from(pid)),
    );
    CFDictionary::from_slices(&[&*vendor, &*product], &[&*v, &*p])
}

fn run_loop_for(wait: Duration) -> CFRunLoopRunResult {
    CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, wait.as_secs_f64(), true)
}

/// The first controller from the requested PlayStation family.
pub(super) fn open(product_ids: &[u16]) -> Option<Box<dyn super::HidPad>> {
    let manager = IOHIDManager::new(None, kIOHIDOptionsTypeNone);
    let matchers = CFArray::from_retained_objects(
        &product_ids
            .iter()
            .map(|&p| matcher(p))
            .collect::<Vec<_>>(),
    );
    let run_loop = CFRunLoop::current()?;
    let mode = unsafe { kCFRunLoopDefaultMode }?;
    let mut inbox = Box::<Inbox>::default();
    let ctx = &mut *inbox as *mut Inbox as *mut c_void;
    // SAFETY: the matchers are dictionaries; `ctx` outlives the
    // registration (the `Pad` owns it and unschedules before dropping it).
    unsafe {
        manager.set_device_matching_multiple(Some(matchers.as_opaque()));
        manager.register_device_matching_callback(Some(matched), ctx);
        manager.register_device_removal_callback(Some(removed), ctx);
        manager.register_input_report_callback(Some(report), ctx);
        manager.schedule_with_run_loop(&run_loop, mode);
    }
    let mut pad = Pad {
        manager,
        run_loop,
        inbox,
        transport: Transport::Usb,
    };
    // Not seized: the other backends keep reading the same pad.
    if pad.manager.open(kIOHIDOptionsTypeNone) != kIOReturnSuccess {
        return None;
    }
    // The matching callbacks for pads already plugged in run now.
    let start = Instant::now();
    while pad.inbox.device.is_none() {
        let left = Duration::from_millis(200).saturating_sub(start.elapsed());
        if left.is_zero() || run_loop_for(left) != CFRunLoopRunResult::HandledSource {
            break;
        }
    }
    let device = pad.inbox.device.clone()?;
    let transport = CFString::from_str(kIOHIDTransportKey.to_str().unwrap_or("Transport"));
    let bluetooth = device
        .property(&transport)
        .and_then(|v| v.downcast::<CFString>().ok())
        .is_some_and(|s| s.to_string().to_ascii_lowercase().contains("bluetooth"));
    pad.transport = if bluetooth {
        Transport::Bluetooth
    } else {
        Transport::Usb
    };
    Some(Box::new(pad))
}

impl super::HidPad for Pad {
    fn transport(&self) -> Transport {
        self.transport
    }

    fn read(&mut self, timeout: Duration) -> io::Result<Option<Vec<u8>>> {
        let start = Instant::now();
        loop {
            if let Some(r) = self.inbox.reports.pop_front() {
                return Ok(Some(r));
            }
            if self.inbox.removed {
                return Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "PlayStation controller removed",
                ));
            }
            let left = timeout.saturating_sub(start.elapsed());
            if left.is_zero() {
                return Ok(None);
            }
            match run_loop_for(left) {
                CFRunLoopRunResult::HandledSource => continue,
                CFRunLoopRunResult::TimedOut => return Ok(None),
                _ => return Err(io::Error::new(io::ErrorKind::NotConnected, "HID run loop ended")),
            }
        }
    }

    fn write(&mut self, r: &[u8]) -> io::Result<()> {
        let device = self
            .inbox
            .device
            .as_ref()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected))?;
        let ptr = NonNull::new(r.as_ptr() as *mut u8)
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        // SAFETY: `r` is valid for its length for the duration of the call.
        let ret = unsafe {
            device.set_report(
                IOHIDReportType::Output,
                CFIndex::from(r[0]),
                ptr,
                r.len() as CFIndex,
            )
        };
        if ret == kIOReturnSuccess {
            Ok(())
        } else {
            Err(io::Error::other(format!("IOHIDDeviceSetReport: {ret:#x}")))
        }
    }
}

impl Drop for Pad {
    fn drop(&mut self) {
        // SAFETY: same run loop and mode as scheduled; after this no callback
        // can reach the inbox.
        unsafe {
            if let Some(mode) = kCFRunLoopDefaultMode {
                self.manager.unschedule_from_run_loop(&self.run_loop, mode);
            }
        }
        self.manager.close(kIOHIDOptionsTypeNone);
    }
}
