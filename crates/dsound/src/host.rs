//! The two things the shim needs from the rest of the recompiler.
//!
//! The runtime owns guest memory and the kernel agent owns events and guest
//! callbacks, so both are abstract here. Nothing in this crate links against
//! `runtime`.

/// A guest address range that could not be read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("guest memory fault at {addr:#010x} (+{len:#x})")]
pub struct MemFault {
    pub addr: u32,
    pub len: u32,
}

/// Flat guest memory, as the recompiled code sees it.
pub trait GuestMemory {
    fn read(&self, addr: u32, out: &mut [u8]) -> Result<(), MemFault>;
    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault>;
    /// Zeroed, `align`-aligned allocation for guest-visible objects and
    /// buffer memory (DirectSound allocates contiguous physical memory for
    /// buffers it creates with `dwBufferBytes != 0`). `None` when exhausted.
    fn alloc(&mut self, size: u32, align: u32) -> Option<u32>;
    fn free(&mut self, addr: u32);

    fn read_u32(&self, addr: u32) -> Result<u32, MemFault> {
        let mut b = [0; 4];
        self.read(addr, &mut b)?;
        Ok(u32::from_le_bytes(b))
    }
    fn write_u32(&mut self, addr: u32, v: u32) -> Result<(), MemFault> {
        self.write(addr, &v.to_le_bytes())
    }
    /// Reads `len` bytes. The span is validated first and the buffer grows
    /// chunk by chunk as the memory proves readable, so a hostile length
    /// faults instead of allocating up to 4 GiB.
    fn read_vec(&self, addr: u32, len: u32) -> Result<Vec<u8>, MemFault> {
        let fault = MemFault { addr, len };
        addr.checked_add(len).ok_or(fault)?;
        if len > MAX_GUEST_COPY {
            return Err(fault);
        }
        let mut v = Vec::new();
        let mut done = 0u32;
        while done < len {
            let n = (len - done).min(READ_CHUNK);
            v.resize((done + n) as usize, 0);
            self.read(addr + done, &mut v[done as usize..])
                .map_err(|_| fault)?;
            done += n;
        }
        Ok(v)
    }
}

/// Largest single guest copy the shim will make (the guest has 64 MiB).
pub const MAX_GUEST_COPY: u32 = 64 << 20;
const READ_CHUNK: u32 = 1 << 20;

/// `addr + off` as a guest address, faulting on wrap-around.
pub fn at(addr: u32, off: u32) -> Result<u32, MemFault> {
    addr.checked_add(off).ok_or(MemFault { addr, len: off })
}

/// A completion callback the game registered on a stream
/// (`DSSTREAMDESC.lpfnCallback`, `LPFNXMOCALLBACK`). The shim cannot run guest
/// code itself: the sink queues it and the kernel/runtime side invokes
/// `function(stream_context, packet_context, status)` (stdcall, three
/// dwords, return value unused) on a guest thread at a safe point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamCallback {
    pub function: u32,
    pub stream_context: u32,
    pub packet_context: u32,
    pub status: u32,
}

/// Completion notification, owned by the kernel agent.
pub trait EventSink {
    /// `SetEvent(handle)` for an `XMEDIAPACKET.hCompletionEvent`.
    fn signal_event(&mut self, handle: u32);
    /// Queue a guest completion callback (see [`StreamCallback`]).
    fn stream_callback(&mut self, cb: StreamCallback);
}

/// What every shim entry point receives.
pub trait Host: GuestMemory + EventSink {}
impl<T: GuestMemory + EventSink> Host for T {}
