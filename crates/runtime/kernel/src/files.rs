//! Guest ABI packing around the shared VFS. Every transfer completes before
//! the call returns, on synchronous and asynchronous (overlapped) file objects
//! alike; see README "VFS boundaries". Unsupported classes stop before
//! changing VFS state.
use crate::ord;
use crate::{Action, Core, abi::Args, status};
use runtime::{Guest, GuestFault, guest::Access};
use vfs::{Error, Offset, OpenOptions, Timestamp, Vfs};

/// Xbox/NT FILE_OPEN_FOR_FREE_SPACE_QUERY create option.
pub(crate) const FILE_OPEN_FOR_FREE_SPACE_QUERY: u32 = 0x0080_0000;
/// Access bits that could modify the opened object or its security, including
/// GENERIC_WRITE and GENERIC_ALL. A free-space query open must request none.
const MODIFYING_ACCESS: u32 = 0x0002
    | 0x0004
    | 0x0010
    | 0x0100
    | 0x0001_0000
    | 0x0004_0000
    | 0x0008_0000
    | 0x1000_0000
    | 0x4000_0000;

/// Bounded, fallibly allocated scratch for guest I/O: never more than one
/// `limits::IO_CHUNK` per call, whatever the guest's (possibly aliased) length.
fn chunk_buffer(length: usize) -> Option<Vec<u8>> {
    let size = length.min(crate::limits::IO_CHUNK);
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(size).ok()?;
    buffer.resize(size, 0);
    Some(buffer)
}
fn guest_offset(base: u32, offset: usize) -> Result<u32, GuestFault> {
    u32::try_from(offset)
        .ok()
        .and_then(|offset| base.checked_add(offset))
        .ok_or_else(|| crate::strings::overflow(base))
}

/// OBJECT_ATTRIBUTES.RootDirectory (HANDLE)-3, ObDosDevicesDirectory: the
/// name is relative to `\??\` (XAPI's DeleteFile, GetFileAttributes and
/// CreateFile pass drive paths such as `z:\cache000.map` this way).
pub(crate) const DOS_DEVICES_ROOT: u32 = 0xffff_fffd;
/// NtCreateFile/NtOpenFile options FILE_SYNCHRONOUS_IO_ALERT | _NONALERT.
const SYNCHRONOUS_IO: u32 = 0x30;
/// ByteOffset {FILE_USE_FILE_POINTER_POSITION, -1}.
const USE_FILE_POINTER_POSITION: i64 = -2;

pub(crate) struct File {
    mode: u32,
    access: vfs::Access,
    /// Opened with a FILE_SYNCHRONOUS_IO_* option. Otherwise the file object
    /// is asynchronous: transfers need an explicit ByteOffset and leave the
    /// file position alone, as NT only keeps CurrentByteOffset for
    /// synchronous file objects.
    synchronous: bool,
}

/// An OBJECT_ATTRIBUTES name: absolute, or relative to a guest file handle.
struct ObjectName {
    root: u32,
    path: String,
}
/// Decode OBJECT_ATTRIBUTES. A DosDevices-relative name becomes absolute
/// (`\??\` + name). `Err` carries the diagnostic stop for an unsupported form.
fn object_name(
    memory: &Guest,
    attributes: u32,
) -> Result<Result<ObjectName, &'static str>, GuestFault> {
    let fields = memory.read::<12>(attributes)?;
    let root = u32::from_le_bytes(fields[..4].try_into().unwrap());
    let descriptor = u32::from_le_bytes(fields[4..8].try_into().unwrap());
    let flags = u32::from_le_bytes(fields[8..12].try_into().unwrap());
    if flags & !0x40 != 0 {
        return Ok(Err("object attributes unsupported"));
    }
    let Some(path) = name(memory, descriptor)? else {
        return Ok(Err("unsupported file name"));
    };
    if root != DOS_DEVICES_ROOT {
        return Ok(Ok(ObjectName { root, path }));
    }
    if path.is_empty() || path.starts_with('\\') {
        return Ok(Err("rooted or empty name relative to \\?? unverified"));
    }
    Ok(Ok(ObjectName {
        root: 0,
        path: format!("\\??\\{path}"),
    }))
}

pub(crate) fn is_file_export(ordinal: u32) -> bool {
    matches!(
        ordinal,
        ord::IO_CREATE_SYMBOLIC_LINK
            | ord::IO_DELETE_SYMBOLIC_LINK
            | ord::NT_CREATE_FILE
            | ord::NT_FLUSH_BUFFERS_FILE
            | ord::NT_OPEN_FILE
            | ord::NT_OPEN_SYMBOLIC_LINK_OBJECT
            | ord::NT_QUERY_SYMBOLIC_LINK_OBJECT
            | ord::NT_QUERY_DIRECTORY_FILE
            | ord::NT_QUERY_FULL_ATTRIBUTES_FILE
            | ord::NT_QUERY_INFORMATION_FILE
            | ord::NT_QUERY_VOLUME_INFORMATION_FILE
            | ord::NT_READ_FILE
            | ord::NT_SET_INFORMATION_FILE
            | ord::NT_WRITE_FILE
    )
}

fn name(memory: &Guest, descriptor: u32) -> Result<Option<String>, GuestFault> {
    let bytes = memory.read::<8>(descriptor)?;
    let len = u16::from_le_bytes(bytes[..2].try_into().unwrap());
    let max = u16::from_le_bytes(bytes[2..4].try_into().unwrap());
    if len > max {
        return Ok(None);
    }
    let buffer = u32::from_le_bytes(bytes[4..].try_into().unwrap());
    let mut bytes = vec![0; len as usize];
    memory.read_bytes(buffer, &mut bytes, 1)?;
    if !bytes.is_ascii() || bytes.contains(&0) {
        return Ok(None);
    }
    Ok(Some(String::from_utf8(bytes).unwrap()))
}
fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(bytes: &mut [u8], at: usize, value: u64) {
    bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
}
fn basic(bytes: &mut [u8], at: usize, info: vfs::BasicInfo) {
    for (i, time) in [
        info.creation_time,
        info.last_access_time,
        info.last_write_time,
        info.change_time,
    ]
    .iter()
    .enumerate()
    {
        put64(bytes, at + i * 8, time.0);
    }
}
fn finish(memory: &mut Guest, iosb: u32, result: vfs::Result<u32>) -> Result<Action, GuestFault> {
    if result == Err(Error::NotSupported) {
        return Ok(Action::Stop("VFS operation unsupported"));
    }
    let (status, count) = match result {
        Ok(count) => (status::SUCCESS, count),
        Err(e) => (e.ntstatus(), 0),
    };
    finish_status(memory, iosb, status, count)
}
fn finish_status(
    memory: &mut Guest,
    iosb: u32,
    status: u32,
    count: u32,
) -> Result<Action, GuestFault> {
    let mut bytes = [0; 8];
    put32(&mut bytes, 0, status);
    put32(&mut bytes, 4, count);
    memory.write(iosb, bytes)?;
    Ok(Action::status(status))
}
// Once the target handle is published, close-source cleanup cannot turn a
// completed duplication into a reported failure with a live, hidden target.
fn publish_duplicate(
    memory: &mut Guest,
    output: u32,
    handle: u32,
    close_source: impl FnOnce() -> vfs::Result<()>,
) -> Result<Action, GuestFault> {
    memory.set_u32(output, handle)?;
    let _ = close_source();
    Ok(Action::status(status::SUCCESS))
}
fn offset(memory: &Guest, pointer: u32, write: bool) -> Result<vfs::Result<Offset>, GuestFault> {
    if pointer == 0 {
        return Ok(Ok(Offset::Current));
    }
    Ok(match memory.u64(pointer)? as i64 {
        -2 => Ok(Offset::Current),
        -1 if write => Ok(Offset::EndOfFile),
        v if v >= 0 => Ok(Offset::Absolute(v as u64)),
        _ => Err(Error::InvalidParameter),
    })
}

impl Core {
    /// Runs one guest file call, charging its host wall time (microseconds,
    /// rounded) to `Counter::FileIoUs` on whichever thread made it.
    pub(crate) fn file_call(&mut self, memory: &mut Guest, a: &Args) -> Result<Action, GuestFault> {
        if !timeline::phase::enabled() {
            return self.file_call_inner(memory, a);
        }
        let start = timeline::now_ns();
        let r = self.file_call_inner(memory, a);
        let ns = timeline::now_ns().saturating_sub(start);
        timeline::phase::count(timeline::phase::Counter::FileIoUs, (ns + 500) / 1000);
        r
    }
    fn file_call_inner(&mut self, memory: &mut Guest, a: &Args) -> Result<Action, GuestFault> {
        if memory.u8(self.kpcr + crate::layout::kpcr::IRQL)? != 0 {
            return Ok(Action::Stop("file operation above PASSIVE_LEVEL"));
        }
        let Some(storage) = self.storage.clone() else {
            return Ok(Action::Stop("no VFS configured"));
        };
        let mut storage = storage.lock().unwrap_or_else(|e| e.into_inner());
        self.file_call_with(memory, a, &mut storage)
    }
    pub(crate) fn file_handle(&self, handle: u32) -> vfs::Result<vfs::Handle> {
        self.file_handles
            .get(&handle)
            .copied()
            .ok_or(Error::InvalidHandle)
    }
    fn file_call_with(
        &mut self,
        memory: &mut Guest,
        a: &Args,
        storage: &mut Vfs,
    ) -> Result<Action, GuestFault> {
        if matches!(
            a.export.ordinal,
            ord::IO_CREATE_SYMBOLIC_LINK | ord::IO_DELETE_SYMBOLIC_LINK
        ) {
            let Some(link) = name(memory, a.get(0))? else {
                return Ok(Action::Stop("unsupported object string"));
            };
            let result = if a.export.ordinal == ord::IO_CREATE_SYMBOLIC_LINK {
                let Some(target) = name(memory, a.get(1))? else {
                    return Ok(Action::Stop("unsupported object string"));
                };
                storage.namespace.create_symbolic_link(&link, &target)
            } else {
                storage.namespace.delete_symbolic_link(&link)
            };
            return Ok(Action::status(
                result.map_or_else(Error::ntstatus, |_| status::SUCCESS),
            ));
        }
        if matches!(a.export.ordinal, ord::NT_CREATE_FILE | ord::NT_OPEN_FILE) {
            return self.file_open(memory, a, storage);
        }
        if a.export.ordinal == ord::NT_OPEN_SYMBOLIC_LINK_OBJECT {
            return self.link_open(memory, a, storage);
        }
        if a.export.ordinal == ord::NT_QUERY_SYMBOLIC_LINK_OBJECT {
            return self.link_query(memory, a);
        }
        if a.export.ordinal == ord::NT_QUERY_FULL_ATTRIBUTES_FILE {
            return self.full_attributes(memory, a, storage);
        }
        if a.export.ordinal == ord::NT_CLOSE {
            let native = self.file_handle(a.get(0)).unwrap();
            let object = self.handles.close(a.get(0)).unwrap();
            self.file_handles.remove(&a.get(0));
            let result = storage.close(native);
            if self.handles.references(object) == 0 {
                self.files.remove(&object);
                self.memory.free_pool(object);
            }
            return Ok(Action::status(result.map_or_else(Error::ntstatus, |_| 0)));
        }
        if a.export.ordinal == ord::NT_DUPLICATE_OBJECT {
            memory.check(a.get(1), 4, Access::Write, 1)?;
            if a.get(2) & !3 != 0 {
                return Ok(Action::status(status::INVALID_PARAMETER));
            }
            let old = self.file_handle(a.get(0)).unwrap();
            let native = match storage.duplicate(old) {
                Ok(h) => h,
                Err(e) => return Ok(Action::status(e.ntstatus())),
            };
            let object = self.handles.get(a.get(0)).unwrap();
            let handle = match self.handles.insert(object) {
                Ok(h) => h,
                Err(s) => {
                    let _ = storage.close(native);
                    return Ok(Action::status(s));
                }
            };
            self.file_handles.insert(handle, native);
            if a.get(2) & 1 != 0 {
                self.handles.close(a.get(0)).unwrap();
                self.file_handles.remove(&a.get(0));
                return publish_duplicate(memory, a.get(1), handle, || storage.close(old));
            }
            return publish_duplicate(memory, a.get(1), handle, || Ok(()));
        }
        let iosb = if matches!(
            a.export.ordinal,
            ord::NT_QUERY_DIRECTORY_FILE | ord::NT_READ_FILE | ord::NT_WRITE_FILE
        ) {
            a.get(4)
        } else {
            a.get(1)
        };
        if self.handles.get(a.get(0)).is_ok() && !self.file_handles.contains_key(&a.get(0)) {
            return Ok(Action::status(status::OBJECT_TYPE_MISMATCH));
        }
        let handle = match self.file_handle(a.get(0)) {
            Ok(h) => h,
            Err(e) => return Ok(Action::status(e.ntstatus())),
        };
        memory.check(iosb, 8, Access::Write, 1)?;
        match a.export.ordinal {
            ord::NT_FLUSH_BUFFERS_FILE => finish(memory, iosb, storage.flush(handle).map(|_| 0)),
            ord::NT_READ_FILE | ord::NT_WRITE_FILE => {
                // ApcRoutine needs user-APC delivery. An ApcContext without a
                // routine is a completion-port key, and Xbox has no
                // completion ports: XAPI passes its OVERLAPPED there.
                if a.get(2) != 0 {
                    return Ok(Action::Stop(
                        "file I/O APC completion requires APC delivery",
                    ));
                }
                // ApcContext (arg 3) is deliberately not read: with no
                // ApcRoutine it only keys a completion port, which this kernel
                // does not have, so dropping it is the complete behavior (the
                // IO_STATUS_BLOCK is the only completion record). A set low bit
                // on the Event handle is the NT convention for suppressing that
                // completion-port post. Whether the Xbox kernel honours it is
                // unverified, and it would otherwise fail as an invalid handle
                // below, so stop explicitly rather than guess.
                if a.get(1) & 1 != 0 {
                    return Ok(Action::Stop(
                        "file I/O event handle with its low bit set unverified",
                    ));
                }
                let event = if a.get(1) == 0 {
                    None
                } else {
                    match self.handles.get(a.get(1)) {
                        Ok(object) => match self.ensure_object(memory, object)? {
                            Some(crate::sync::Object::Event { .. }) => Some(object),
                            _ => return Ok(Action::status(status::OBJECT_TYPE_MISMATCH)),
                        },
                        Err(s) => return Ok(Action::status(s)),
                    }
                };
                let write = a.export.ordinal == ord::NT_WRITE_FILE;
                let buffer = a.get(5);
                let length = a.get(6) as usize;
                if !write {
                    timeline::phase::count(timeline::phase::Counter::ReadBytes, length as u64);
                }
                memory.check(
                    buffer,
                    length,
                    if write { Access::Read } else { Access::Write },
                    1,
                )?;
                let object = self.handles.get(a.get(0)).unwrap();
                let synchronous = self.files[&object].synchronous;
                if !synchronous {
                    // A non-synchronous file object has no position to use.
                    if a.get(7) == 0 {
                        return Ok(Action::status(status::INVALID_PARAMETER));
                    }
                    if memory.u64(a.get(7))? as i64 == USE_FILE_POINTER_POSITION {
                        return Ok(Action::Stop(
                            "file-pointer offset on an asynchronous file object unverified",
                        ));
                    }
                }
                let offset = match offset(memory, a.get(7), write)? {
                    Ok(o) => o,
                    Err(e) => return Ok(Action::status(e.ntstatus())),
                };
                let access = self.files[&object].access;
                if (!write && !access.contains(vfs::Access::READ))
                    || (write && access.0 & (vfs::Access::WRITE.0 | vfs::Access::APPEND.0) == 0)
                {
                    return Ok(Action::status(Error::AccessDenied.ntstatus()));
                }
                let Some(mut chunk) = chunk_buffer(length) else {
                    return finish_status(memory, iosb, status::INSUFFICIENT_RESOURCES, 0);
                };
                if let Some(event) = event {
                    // NtReadFile clears the caller's event before the transfer.
                    self.event_set(memory, event, false)?;
                }
                let position = if synchronous {
                    None
                } else {
                    match storage.query_information(handle) {
                        Ok(info) => Some(info.position),
                        Err(e) => return finish(memory, iosb, Err(e)),
                    }
                };
                // Transfer in bounded chunks. The first chunk uses the caller's
                // offset; later chunks continue from the position the previous
                // synchronous transfer left. A short chunk ends the transfer.
                // An end-of-file after some bytes is a successful short read.
                let mapped = if write {
                    None
                } else {
                    storage.try_map_read(handle, offset, length, |source| {
                        memory.map_file_read(buffer, source)
                    })
                };
                let result = if let Some(bytes) = mapped {
                    Ok(bytes)
                } else {
                    let mut done = 0usize;
                    let mut at = offset;
                    loop {
                        let n = (length - done).min(chunk.len());
                        let target = guest_offset(buffer, done)?;
                        let transferred = if write {
                            memory.read_bytes(target, &mut chunk[..n], 1)?;
                            storage.write(handle, at, &chunk[..n])
                        } else {
                            storage.read(handle, at, &mut chunk[..n])
                        };
                        match transferred {
                            Ok(count) => {
                                let count = count.min(n);
                                if !write {
                                    memory.write_bytes(target, &chunk[..count], 1)?;
                                    storage.note_guest_copy(count);
                                }
                                done += count;
                                at = Offset::Current;
                                if count < n || done == length {
                                    break Ok(done);
                                }
                            }
                            Err(Error::EndOfFile) if !write && done > 0 => break Ok(done),
                            Err(e) => break Err(e),
                        }
                    }
                };
                if let Some(position) = position {
                    let _ = storage.set_position(handle, position);
                }
                let action = finish(memory, iosb, result.map(|n| n as u32))?;
                // Completion signals the event. A request that fails at once
                // completes without it, as NT does for a synchronous error.
                if let (Some(event), Action::Complete(crate::abi::Value::U32(s))) = (event, &action)
                    && status::nt_success(*s)
                {
                    self.event_set(memory, event, true)?;
                }
                Ok(action)
            }
            ord::NT_QUERY_INFORMATION_FILE => {
                let class = a.get(4);
                if !matches!(class, 4 | 5 | 8 | 14 | 16 | 34) {
                    return Ok(Action::Stop("file information class unsupported"));
                }
                let object = self.handles.get(a.get(0)).unwrap();
                let file = &self.files[&object];
                // The retail kernel answers these queries whatever the handle's
                // granted access: XAPI queries basic information on its
                // GENERIC_WRITE (no FILE_READ_ATTRIBUTES) TitleMeta.xbx handle.
                // NT's per-class query access table is not applied.
                let info = match storage.query_information(handle) {
                    Ok(i) => i,
                    Err(e) => return finish(memory, iosb, Err(e)),
                };
                let mut bytes = vec![
                    0;
                    match class {
                        4 => 40,
                        5 => 24,
                        8 | 16 => 4,
                        14 => 8,
                        34 => 56,
                        _ => return Ok(Action::Stop("unrouted service case")),
                    }
                ];
                match class {
                    4 => {
                        basic(&mut bytes, 0, info.metadata.basic);
                        put32(&mut bytes, 32, info.metadata.basic.attributes);
                    }
                    5 => {
                        put64(&mut bytes, 0, info.metadata.allocation_size);
                        put64(&mut bytes, 8, info.metadata.end_of_file);
                        put32(&mut bytes, 16, 1);
                        bytes[20] = info.delete_pending as u8;
                        bytes[21] = info.metadata.is_directory() as u8;
                    }
                    8 => put32(&mut bytes, 0, info.access.0),
                    14 => put64(&mut bytes, 0, info.position),
                    16 => put32(&mut bytes, 0, file.mode),
                    // FILE_NETWORK_OPEN_INFORMATION: four times, allocation
                    // size, end of file, attributes, then 4 bytes of padding.
                    34 => {
                        basic(&mut bytes, 0, info.metadata.basic);
                        put64(&mut bytes, 32, info.metadata.allocation_size);
                        put64(&mut bytes, 40, info.metadata.end_of_file);
                        put32(&mut bytes, 48, info.metadata.basic.attributes);
                    }
                    _ => return Ok(Action::Stop("unrouted service case")),
                }
                self.file_query_output(memory, a, &bytes)
            }
            ord::NT_SET_INFORMATION_FILE => self.file_set_information(memory, a, storage, handle),
            ord::NT_QUERY_VOLUME_INFORMATION_FILE => {
                if !matches!(a.get(4), 3 | 7) {
                    return Ok(Action::Stop(
                        "volume information class needs more VFS metadata",
                    ));
                }
                let info = match storage.query_volume_information(handle) {
                    Ok(i) => i,
                    Err(e) => return finish(memory, iosb, Err(e)),
                };
                let mut bytes = vec![0; if a.get(4) == 3 { 24 } else { 32 }];
                put64(&mut bytes, 0, info.total_allocation_units);
                put64(&mut bytes, 8, info.available_allocation_units);
                let tail = if a.get(4) == 7 {
                    put64(&mut bytes, 16, info.available_allocation_units);
                    24
                } else {
                    16
                };
                put32(&mut bytes, tail, info.sectors_per_allocation_unit);
                put32(&mut bytes, tail + 4, info.bytes_per_sector);
                self.file_query_output(memory, a, &bytes)
            }
            ord::NT_QUERY_DIRECTORY_FILE => {
                if a.get(1) != 0 || a.get(2) != 0 || a.get(3) != 0 {
                    return Ok(Action::Stop("event/APC directory I/O unsupported"));
                }
                if !matches!(a.get(7), 1 | 12) {
                    return Ok(Action::Stop("directory information class unsupported"));
                }
                let pattern = if a.get(8) == 0 {
                    None
                } else {
                    let Some(p) = name(memory, a.get(8))? else {
                        return Ok(Action::Stop("unsupported search string"));
                    };
                    Some(p)
                };
                memory.check(a.get(5), a.get(6) as usize, Access::Write, 1)?;
                let entry = match storage.peek_directory(handle, pattern.as_deref(), a.boolean(9)) {
                    Ok(e) => e,
                    Err(e) => return finish(memory, iosb, Err(e)),
                };
                if !entry.name.is_ascii() {
                    return Ok(Action::Stop("non-ASCII directory record unsupported"));
                }
                let head = if a.get(7) == 1 { 64 } else { 12 };
                let mut bytes = vec![0; head + entry.name.len()];
                if a.get(7) == 1 {
                    basic(&mut bytes, 8, entry.metadata.basic);
                    put64(&mut bytes, 40, entry.metadata.end_of_file);
                    put64(&mut bytes, 48, entry.metadata.allocation_size);
                    put32(&mut bytes, 56, entry.metadata.basic.attributes);
                    put32(&mut bytes, 60, entry.name.len() as u32);
                } else {
                    put32(&mut bytes, 8, entry.name.len() as u32);
                }
                bytes[head..].copy_from_slice(entry.name.as_bytes());
                if bytes.len() > a.get(6) as usize {
                    return Ok(Action::Stop(
                        "directory truncation/overflow packing unverified; cursor retained",
                    ));
                }
                memory.write_bytes(a.get(5), &bytes, 1)?;
                match storage.consume_directory(handle) {
                    Ok(_) => finish(memory, iosb, Ok(bytes.len() as u32)),
                    Err(e) => finish(memory, iosb, Err(e)),
                }
            }
            _ => Ok(Action::Stop("declared export has no file handler")),
        }
    }
    fn file_query_output(
        &self,
        memory: &mut Guest,
        a: &Args,
        bytes: &[u8],
    ) -> Result<Action, GuestFault> {
        if bytes.len() > a.get(3) as usize {
            return Ok(Action::Stop(
                "file information short-buffer contract unverified",
            ));
        }
        memory.write_bytes(a.get(2), bytes, 1)?;
        finish(memory, a.get(1), Ok(bytes.len() as u32))
    }
    fn file_open(
        &mut self,
        memory: &mut Guest,
        a: &Args,
        storage: &mut Vfs,
    ) -> Result<Action, GuestFault> {
        memory.check(a.get(0), 4, Access::Write, 1)?;
        memory.check(a.get(3), 8, Access::Write, 1)?;
        let ObjectName { root, path } = match object_name(memory, a.get(2))? {
            Ok(n) => n,
            Err(reason) => return Ok(Action::Stop(reason)),
        };
        let (share, disposition, options, attributes) = if a.export.ordinal == ord::NT_CREATE_FILE {
            (a.get(6), a.get(7), a.get(8), a.get(5))
        } else {
            (a.get(4), 1, a.get(5), 0)
        };
        // FILE_SYNCHRONOUS_IO_ALERT (0x10) and _NONALERT (0x20) differ only in
        // whether a wait inside the I/O can be alerted. Every transfer here
        // completes on the host thread without a guest wait, so no alert or
        // APC can arrive and both behave identically. Both bits together is
        // rejected by VFS as STATUS_INVALID_PARAMETER. Neither bit opens an
        // asynchronous file object (XAPI's DeleteFile uses 0x4040).
        let synchronous = options & SYNCHRONOUS_IO != 0;
        // GetDiskFreeSpaceEx (retail 0x2d1620) opens the queried directory with
        // FILE_OPEN_FOR_FREE_SPACE_QUERY, then asks 218 for class 3. The handle
        // is an ordinary synchronous VFS handle: volume queries are answered
        // from its mount. Only the observed read-only FILE_OPEN shape is
        // admitted; the flag is not passed to VFS or reported by class 16.
        let options = if options & FILE_OPEN_FOR_FREE_SPACE_QUERY != 0 {
            if disposition != 1 || a.get(1) & MODIFYING_ACCESS != 0 {
                return Ok(Action::Stop(
                    "free-space query open with create or write semantics unverified",
                ));
            }
            options & !FILE_OPEN_FOR_FREE_SPACE_QUERY
        } else {
            options
        };
        // Read before I/O: generated XCreateSaveGame aliases AllocationSize
        // and the output IO_STATUS_BLOCK. A negative LARGE_INTEGER is invalid.
        let allocation = if a.export.ordinal == ord::NT_CREATE_FILE && a.get(4) != 0 {
            let size = memory.u64(a.get(4))?;
            if size > i64::MAX as u64 {
                return Ok(Action::status(status::INVALID_PARAMETER));
            }
            size
        } else {
            0
        };
        let open_options =
            match OpenOptions::from_nt(a.get(1), share, disposition, options, attributes) {
                Ok(o) => o,
                // A valid but unimplemented option is missing HLE, not a
                // filesystem result: keep the frame for a diagnostic stop.
                Err(Error::NotSupported) => {
                    return Ok(Action::Stop("file create/open option unsupported"));
                }
                Err(e) => return Ok(Action::status(e.ntstatus())),
            };
        if root != 0 && self.handles.get(root).is_ok() && !self.file_handles.contains_key(&root) {
            return Ok(Action::status(status::OBJECT_TYPE_MISMATCH));
        }
        let parent = if root == 0 {
            None
        } else {
            match self.file_handle(root) {
                Ok(h) => Some(h),
                Err(e) => return Ok(Action::status(e.ntstatus())),
            }
        };
        // Reserve the guest handle before VFS acts on the open: a disposition
        // such as FILE_OVERWRITE truncates during the open itself, so a full
        // handle table must fail before the file is touched.
        if let Err(s) = self.handles.check_capacity() {
            return Ok(Action::status(s));
        }
        let object = self.memory.allocate_pool(memory, 16, 0x656c6946);
        if object == 0 {
            return Ok(Action::status(status::NO_MEMORY));
        }
        let opened = match parent {
            Some(parent) => {
                storage.open_relative_allocated(parent, &path, open_options, allocation)
            }
            None => storage.open_allocated(&path, open_options, allocation),
        };
        match opened {
            Ok(opened) => {
                let handle = match self.handles.insert(object) {
                    Ok(h) => h,
                    Err(s) => {
                        let _ = storage.close(opened.handle);
                        self.memory.free_pool(object);
                        return Ok(Action::status(s));
                    }
                };
                self.file_handles.insert(handle, opened.handle);
                crate::level::note_open(&path);
                self.files.insert(
                    object,
                    File {
                        mode: options & 0x3e,
                        access: open_options.access,
                        synchronous,
                    },
                );
                memory.set_u32(a.get(0), handle)?;
                finish(memory, a.get(3), Ok(opened.information as u32))
            }
            Err(e) => {
                self.memory.free_pool(object);
                finish(memory, a.get(3), Err(e))
            }
        }
    }
    /// NtOpenSymbolicLinkObject: a handle to a namespace link object. The
    /// target is captured at open, as an NT link object's target is immutable.
    fn link_open(
        &mut self,
        memory: &mut Guest,
        a: &Args,
        storage: &mut Vfs,
    ) -> Result<Action, GuestFault> {
        memory.check(a.get(0), 4, Access::Write, 1)?;
        let link = match object_name(memory, a.get(1))? {
            Ok(ObjectName { root: 0, path }) => path,
            Ok(_) => return Ok(Action::Stop("symbolic-link object attributes unsupported")),
            Err(reason) => return Ok(Action::Stop(reason)),
        };
        let target = match storage.namespace.query_symbolic_link(&link) {
            Ok(target) => target.to_owned(),
            Err(e) => return Ok(Action::status(e.ntstatus())),
        };
        if target.len() > u16::MAX as usize - 1 {
            return Ok(Action::Stop("symbolic-link target too long"));
        }
        let object = self.memory.allocate_pool(memory, 16, 0x6b6e694c);
        if object == 0 {
            return Ok(Action::status(status::NO_MEMORY));
        }
        let handle = match self.handles.insert(object) {
            Ok(h) => h,
            Err(s) => {
                self.memory.free_pool(object);
                return Ok(Action::status(s));
            }
        };
        self.links.insert(object, target);
        memory.set_u32(a.get(0), handle)?;
        Ok(Action::status(status::SUCCESS))
    }
    /// NtQuerySymbolicLinkObject(Handle, STRING *Target, ULONG *ReturnedLength),
    /// following the NT object manager: the copy includes a terminating NUL,
    /// so it needs MaximumLength >= Length + 1; otherwise BUFFER_TOO_SMALL and
    /// nothing is copied. ReturnedLength receives Length + 1 either way.
    fn link_query(&mut self, memory: &mut Guest, a: &Args) -> Result<Action, GuestFault> {
        let object = match self.handles.get(a.get(0)) {
            Ok(object) => object,
            Err(s) => return Ok(Action::status(s)),
        };
        let Some(target) = self.links.get(&object) else {
            return Ok(Action::status(status::OBJECT_TYPE_MISMATCH));
        };
        let descriptor = memory.read::<8>(a.get(1))?;
        let maximum = u16::from_le_bytes([descriptor[2], descriptor[3]]) as usize;
        let buffer = u32::from_le_bytes(descriptor[4..8].try_into().unwrap());
        let needed = target.len() + 1;
        if a.get(2) != 0 {
            memory.check(a.get(2), 4, Access::Write, 1)?;
        }
        let result = if maximum >= needed {
            let mut bytes = target.as_bytes().to_vec();
            bytes.push(0);
            memory.write_bytes(buffer, &bytes, 1)?;
            memory.set_u16(a.get(1), target.len() as u16)?;
            status::SUCCESS
        } else {
            status::BUFFER_TOO_SMALL
        };
        if a.get(2) != 0 {
            memory.set_u32(a.get(2), needed as u32)?;
        }
        Ok(Action::status(result))
    }
    /// NtQueryFullAttributesFile(ObjectAttributes, FILE_NETWORK_OPEN_INFORMATION *):
    /// path metadata without opening a guest handle, so it neither needs
    /// access rights nor meets share modes. No IO_STATUS_BLOCK; the status is
    /// the result. A trailing backslash names a directory, as in NtOpenFile.
    fn full_attributes(
        &mut self,
        memory: &mut Guest,
        a: &Args,
        storage: &mut Vfs,
    ) -> Result<Action, GuestFault> {
        let out = a.get(1);
        memory.check(out, 56, Access::Write, 1)?;
        let ObjectName { root, path } = match object_name(memory, a.get(0))? {
            Ok(n) => n,
            Err(reason) => return Ok(Action::Stop(reason)),
        };
        let (path, directory) = match path.strip_suffix('\\') {
            Some(stripped) if !stripped.is_empty() => (stripped.to_owned(), true),
            _ => (path, false),
        };
        let result = if root == 0 {
            storage.query_path(&path)
        } else {
            if self.handles.get(root).is_ok() && !self.file_handles.contains_key(&root) {
                return Ok(Action::status(status::OBJECT_TYPE_MISMATCH));
            }
            let parent = match self.file_handle(root) {
                Ok(h) => h,
                Err(e) => return Ok(Action::status(e.ntstatus())),
            };
            // FILE_READ_ATTRIBUTES, share all: the query sees what an open
            // would, through the same relative-name rules.
            let options = OpenOptions::from_nt(0x80, 7, 1, 0, 0).expect("valid query open");
            match storage.open_relative(parent, &path, options) {
                Ok(opened) => {
                    let info = storage.query_information(opened.handle);
                    let _ = storage.close(opened.handle);
                    info.map(|i| i.metadata)
                }
                Err(e) => Err(e),
            }
        };
        let metadata = match result {
            Ok(m) if directory && !m.is_directory() => Err(Error::NotADirectory),
            other => other,
        };
        match metadata {
            Err(Error::NotSupported) => Ok(Action::Stop("VFS operation unsupported")),
            Err(e) => Ok(Action::status(e.ntstatus())),
            Ok(m) => {
                let mut bytes = [0u8; 56];
                basic(&mut bytes, 0, m.basic);
                put64(&mut bytes, 32, m.allocation_size);
                put64(&mut bytes, 40, m.end_of_file);
                put32(&mut bytes, 48, m.basic.attributes);
                memory.write_bytes(out, &bytes, 1)?;
                Ok(Action::status(status::SUCCESS))
            }
        }
    }
    fn file_set_information(
        &mut self,
        memory: &mut Guest,
        a: &Args,
        storage: &mut Vfs,
        handle: vfs::Handle,
    ) -> Result<Action, GuestFault> {
        let class = a.get(4);
        let needed = match class {
            4 => 40,
            13 => 1,
            14 | 19 | 20 => 8,
            _ => return Ok(Action::Stop("file set-information class unsupported")),
        };
        if a.get(3) < needed {
            return Ok(Action::status(Error::InvalidParameter.ntstatus()));
        }
        memory.check(a.get(2), needed as usize, Access::Read, 1)?;
        if class == 14 {
            let object = self.handles.get(a.get(0)).unwrap();
            if self.files[&object].access.0 & (vfs::Access::READ.0 | vfs::Access::WRITE.0) == 0 {
                return Ok(Action::status(Error::AccessDenied.ntstatus()));
            }
        }
        let result = match class {
            // FileAllocationInformation: below end of file it truncates the
            // file; otherwise it only reserves clusters, which VFS storage
            // has no notion of, so it succeeds with nothing to do. XAPI's
            // SetEndOfFile (0x2d2472) sets it equal to the new end of file.
            19 => {
                let size = memory.u64(a.get(2))?;
                if size > i64::MAX as u64 {
                    return Ok(Action::status(Error::InvalidParameter.ntstatus()));
                }
                let object = self.handles.get(a.get(0)).unwrap();
                if !self.files[&object].access.contains(vfs::Access::WRITE) {
                    return Ok(Action::status(Error::AccessDenied.ntstatus()));
                }
                let info = match storage.query_information(handle) {
                    Ok(i) => i.metadata,
                    Err(e) => return finish(memory, a.get(1), Err(e)),
                };
                if info.is_directory() {
                    return Ok(Action::Stop("directory allocation size unverified"));
                }
                if size < info.end_of_file {
                    storage.set_end_of_file(handle, size)
                } else {
                    Ok(())
                }
            }
            13 => storage.set_delete_pending(handle, memory.u8(a.get(2))? != 0),
            14 | 20 => {
                let value = memory.u64(a.get(2))?;
                if value > i64::MAX as u64 {
                    return Ok(Action::status(Error::InvalidParameter.ntstatus()));
                } else if class == 14 {
                    storage.set_position(handle, value)
                } else {
                    storage.set_end_of_file(handle, value)
                }
            }
            4 => {
                let mut info = match storage.query_information(handle) {
                    Ok(i) => i.metadata.basic,
                    Err(e) => return finish(memory, a.get(1), Err(e)),
                };
                let mut times = [
                    info.creation_time,
                    info.last_access_time,
                    info.last_write_time,
                    info.change_time,
                ];
                for (i, t) in times.iter_mut().enumerate() {
                    let value = memory.u64(a.get(2) + i as u32 * 8)?;
                    if value != 0 && value != u64::MAX {
                        *t = Timestamp(value);
                    }
                }
                [
                    info.creation_time,
                    info.last_access_time,
                    info.last_write_time,
                    info.change_time,
                ] = times;
                let attributes = memory.u32(a.get(2) + 32)?;
                if attributes != 0 {
                    info.attributes = attributes;
                }
                storage.set_basic_information(handle, info)
            }
            _ => return Ok(Action::Stop("unrouted service case")),
        };
        finish(memory, a.get(1), result.map(|_| 0))
    }
}

impl crate::snapshot::Persist for File {
    fn put(&self, w: &mut snapshot::codec::Writer) {
        let Self {
            mode,
            access,
            synchronous,
        } = self;
        w.u32(*mode).u32(access.0).bool(*synchronous);
    }
    fn get(r: &mut snapshot::codec::Reader<'_>) -> Result<Self, crate::snapshot::CodecError> {
        Ok(Self {
            mode: r.u32()?,
            access: vfs::Access(r.u32()?),
            synchronous: r.bool()?,
        })
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    #[test]
    fn duplicate_close_failure_cannot_report_failure_after_publishing_a_target() {
        let mut memory = Guest::new();
        memory
            .map_zeroed(0x10000, 4096, runtime::guest::Permissions::RW)
            .unwrap();
        let mut attempted = false;
        let action = publish_duplicate(&mut memory, 0x10000, 44, || {
            attempted = true;
            Err(Error::IoDeviceError)
        })
        .unwrap();
        assert!(attempted);
        assert_eq!(memory.u32(0x10000).unwrap(), 44);
        assert!(matches!(
            action,
            Action::Complete(crate::abi::Value::U32(status::SUCCESS))
        ));
    }
}
