//! The boot path end to end through the real runtime Runner, a real VFS on
//! host directories and the default keyboard pad: Z: then N: as Halo mounts
//! them (`0x2d1c40 -> 0x2d5b77`, `0x12003 -> 0x2d5d0a`), then the input
//! calls Halo makes (`0x124aa0`, `0x124c00`, `0x124f31`).
//!
//! The cache database is the kernel's own `rawdisk::RawDisk` with its usual
//! seed; no bytes from the game are used.
use input::{
    InputManager,
    backend::keyboard::KeyboardMouse,
    bindings::Bindings,
    gamepad::{Feedback, Gamepad, Rumble},
};
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, abi, exec::HARNESS_EXIT,
    guest::Permissions,
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use vfs::{Access, CreateDisposition, Offset, OpenOptions};
use xapi::{
    CACHE_DATABASE_SIZE, Config, Events, GuestLayout, Result, VfsStorage, Xapi, register,
    register::addr, service_input,
};

const TITLE_ID: u32 = 0x4d53_0064;
const FS: u32 = 0x0050_0000;
const LAST_ERROR: u32 = 0x0050_2008;
const STACK_TOP: u32 = 0x0061_0000;
const SCRATCH: u32 = 0x0070_0000;
const CERTIFICATE: u32 = 0x0001_0184;
const COUNT: u32 = 0x0041_2000;
const MU_TYPE: u32 = 0x0040_87a8;

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = base.join(format!(
            "h2-xapi-host-boot-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The kernel's own raw disk, seeded as `Kernel::set_shared_vfs` seeds it:
/// this title on the utility partition (5), row 0.
fn seeded_disk() -> kernel::rawdisk::RawDisk {
    kernel::rawdisk::RawDisk::new(
        kernel::rawdisk::DISK_SIZE,
        &[kernel::rawdisk::CacheEntry {
            title_id: TITLE_ID,
            partition: 5,
        }],
    )
    .unwrap()
}

fn rw() -> OpenOptions {
    OpenOptions {
        access: Access::ALL,
        disposition: CreateDisposition::OpenIf,
        directory: Some(false),
        ..OpenOptions::default()
    }
}

fn put(vfs: &Mutex<vfs::Vfs>, name: &str, bytes: &[u8]) {
    let mut v = vfs.lock().unwrap();
    let h = v.open(name, rw()).unwrap().handle;
    v.write(h, Offset::Absolute(0), bytes).unwrap();
    v.close(h).unwrap();
}

fn exists(vfs: &Mutex<vfs::Vfs>, name: &str) -> bool {
    vfs.lock().unwrap().query_path(name).is_ok()
}

type Shared = Arc<Mutex<Xapi<VfsStorage>>>;

struct World {
    _temp: TempDir,
    vfs: Arc<Mutex<vfs::Vfs>>,
    keyboard: KeyboardMouse,
    xapi: Shared,
    runtime: Arc<Runtime>,
    ctx: ExecCtx,
    cpu: CpuAbi,
}

fn world() -> World {
    let temp = TempDir::new();
    let mut v = vfs::Vfs::with_storage(vfs::Config::new(&temp.0, TITLE_ID)).unwrap();
    v.mount(xapi::vfs_storage::RAW_DISK_DEVICE, seeded_disk())
        .unwrap();
    let vfs = Arc::new(Mutex::new(v));
    let (input, keyboard) = InputManager::with_keyboard_pad(Bindings::default()).unwrap();
    let xapi = Arc::new(Mutex::new(
        Xapi::new(
            Config::for_gamepad(addr::XDEVICE_TYPE_GAMEPAD),
            input,
            VfsStorage::new(Arc::clone(&vfs)),
        )
        .unwrap(),
    ));
    let mut d = Dispatch::default();
    register(&mut d, &xapi, GuestLayout::default()).unwrap();

    let mut g = Guest::new();
    g.map_zeroed(0x0001_0000, 0x1000, Permissions::RW).unwrap();
    g.set_u32(addr::XBE_CERTIFICATE_POINTER, CERTIFICATE)
        .unwrap();
    g.set_u32(CERTIFICATE + 8, TITLE_ID).unwrap();
    g.map_zeroed(0x002d_0000, 0x7000, Permissions::RWX).unwrap();
    // Input/MU code pages, the import table page (0x411000) and a page for
    // the kernel's HalDiskCachePartitionCount cell (= 3).
    g.map_zeroed(0x0040_8000, 0xb000, Permissions::RWX).unwrap();
    g.set_u32(addr::HAL_DISK_CACHE_PARTITION_COUNT_IMPORT, COUNT)
        .unwrap();
    g.set_u32(COUNT, 3).unwrap();
    g.map_zeroed(0x0046_1000, 0x1000, Permissions::RW).unwrap();
    g.set_u32(addr::CACHE_ROW, u32::MAX).unwrap();
    g.set_u32(addr::CACHE_DRIVE_ROW, 0x7777_7777).unwrap();
    g.map_zeroed(0x0048_0000, 0x1000, Permissions::RW).unwrap();
    g.map_zeroed(FS, 0x3000, Permissions::RW).unwrap();
    g.set_u32(FS + 4, FS + 0x1000).unwrap();
    g.set_u32(FS + 0x1000, FS + 0x2000).unwrap();
    g.map_zeroed(STACK_TOP - 0x1_0000, 0x1_0000, Permissions::RW)
        .unwrap();
    g.map_zeroed(SCRATCH, 0x1_0000, Permissions::RW).unwrap();
    let runtime = Runtime::new(g, d);
    let ctx = ExecCtx::new(Arc::clone(&runtime), 1);
    World {
        _temp: temp,
        vfs,
        keyboard,
        xapi,
        runtime,
        ctx,
        cpu: CpuAbi {
            fs_base: FS,
            ..CpuAbi::default()
        },
    }
}

/// No guest events exist here; record the handles.
#[derive(Default)]
struct Recorded(Vec<u32>);
impl Events for Recorded {
    fn signal(&mut self, guest_handle: u32) -> Result<()> {
        self.0.push(guest_handle);
        Ok(())
    }
}

impl World {
    /// stdcall `target(args)` to HARNESS_EXIT; checks the callee popped
    /// exactly the arguments; returns EAX.
    fn call(&mut self, target: u32, args: &[u32]) -> u32 {
        let esp = STACK_TOP - 0x100;
        {
            let mut m = self.runtime.memory();
            m.set_u32(esp, HARNESS_EXIT).unwrap();
            for (i, a) in args.iter().enumerate() {
                m.set_u32(esp + 4 + 4 * i as u32, *a).unwrap();
            }
        }
        self.cpu.esp = esp;
        self.cpu.eax = 0xdead_beef;
        let t = Runner::default().run(&mut self.cpu, &mut self.ctx, target);
        assert_eq!(t.kind, abi::STOP, "{target:#x}: {t:?}");
        assert!(
            matches!(self.ctx.event(t.event_id), Some(Event::HarnessExit)),
            "{target:#x}: {:?}",
            self.ctx.event(t.event_id)
        );
        assert_eq!(self.cpu.esp, esp + 4 + 4 * args.len() as u32, "{target:#x}");
        self.cpu.eax
    }
    fn u32(&self, a: u32) -> u32 {
        self.runtime.memory().u32(a).unwrap()
    }
    fn set(&self, a: u32, v: u32) {
        self.runtime.memory().set_u32(a, v).unwrap();
    }
    fn bytes<const N: usize>(&self, a: u32) -> [u8; N] {
        let mut b = [0; N];
        self.runtime.memory().read_bytes(a, &mut b, 1).unwrap();
        b
    }
    fn database(&self) -> [u8; CACHE_DATABASE_SIZE] {
        let mut v = self.vfs.lock().unwrap();
        let h = v
            .open(xapi::vfs_storage::RAW_DISK_DEVICE, OpenOptions::default())
            .unwrap()
            .handle;
        let mut s = [0; CACHE_DATABASE_SIZE];
        v.read(h, Offset::Absolute(0x800), &mut s).unwrap();
        v.close(h).unwrap();
        s
    }
    fn link(&self, letter: &str) -> Option<String> {
        self.vfs
            .lock()
            .unwrap()
            .namespace
            .query_symbolic_link(letter)
            .ok()
            .map(str::to_owned)
    }
}

#[test]
fn z_then_n_mount_on_separate_volumes_and_publish_their_rows() {
    let mut w = world();
    put(
        &w.vfs,
        "\\Device\\Harddisk0\\Partition5\\cache000.map",
        b"z cache",
    );
    put(
        &w.vfs,
        "\\Device\\Harddisk0\\Partition4\\cache002.map",
        b"stale",
    );
    w.set(LAST_ERROR, 1234);

    // N: before Z: fails like retail: FALSE, STATUS_UNSUCCESSFUL, N: row kept.
    assert_eq!(w.call(addr::XMOUNT_CACHE_DRIVE, &[]), 0);
    assert_eq!(w.u32(LAST_ERROR), 31);
    assert_eq!(w.u32(addr::CACHE_DRIVE_ROW), 0x7777_7777);
    assert_eq!(w.link("\\??\\N:"), None);

    // XMountUtilityDrive(FALSE), as XapiInitProcess calls it for Halo.
    assert_eq!(w.call(addr::XMOUNT_UTILITY_DRIVE, &[0]), 1);
    assert_eq!(w.u32(addr::CACHE_ROW), 0);
    assert!(exists(&w.vfs, "\\??\\Z:\\cache000.map"));

    w.set(LAST_ERROR, 1234);
    assert_eq!(w.call(addr::XMOUNT_CACHE_DRIVE, &[]), 1);
    assert_eq!(w.u32(LAST_ERROR), 1234, "success keeps the last error");
    assert_eq!(w.u32(addr::CACHE_DRIVE_ROW), 2);
    assert_eq!(w.u32(addr::CACHE_ROW), 0, "Z: row untouched");
    assert_eq!(
        w.link("\\??\\N:").unwrap().to_ascii_lowercase(),
        "\\device\\harddisk0\\partition4"
    );
    assert_eq!(
        w.link("\\??\\Z:").unwrap().to_ascii_lowercase(),
        "\\device\\harddisk0\\partition5"
    );
    assert!(!exists(&w.vfs, "\\??\\N:\\cache002.map"), "N: formatted");
    assert!(exists(&w.vfs, "\\??\\Z:\\cache000.map"), "Z: untouched");
    let db = w.database();
    let row = xapi::cache::row(&db, 2);
    assert_eq!((row.title_id, row.index, row.valid), (TITLE_ID, 1, 0));
    let z = xapi::cache::row(&db, 0);
    assert_eq!((z.title_id, z.index, z.valid), (TITLE_ID, 2, 1));

    // N: is writable, as the cache copier needs (cache002.map is mainmenu's).
    put(&w.vfs, "\\??\\N:\\cache002.map", b"menu");
    assert!(exists(
        &w.vfs,
        "\\Device\\Harddisk0\\Partition4\\cache002.map"
    ));

    // A file open on N: makes the format fail: FALSE, sharing violation,
    // N: row -1, and Z: still intact.
    let open = {
        let mut v = w.vfs.lock().unwrap();
        v.open("\\??\\N:\\cache002.map", rw()).unwrap().handle
    };
    assert_eq!(w.call(addr::XMOUNT_CACHE_DRIVE, &[]), 0);
    assert_eq!(w.u32(LAST_ERROR), 32);
    assert_eq!(w.u32(addr::CACHE_DRIVE_ROW), u32::MAX);
    assert_eq!(w.u32(addr::CACHE_ROW), 0);
    assert!(exists(&w.vfs, "\\??\\Z:\\cache000.map"));
    w.vfs.lock().unwrap().close(open).unwrap();

    // Without the raw disk's database the mount fails the same way.
    w.set(addr::CACHE_DRIVE_ROW, 5);
    {
        let mut v = w.vfs.lock().unwrap();
        let h = v
            .open(
                xapi::vfs_storage::RAW_DISK_DEVICE,
                OpenOptions {
                    access: Access::ALL,
                    ..OpenOptions::default()
                },
            )
            .unwrap()
            .handle;
        v.write(h, Offset::Absolute(0x800), &[0; 4]).unwrap();
        v.close(h).unwrap();
    }
    assert_eq!(w.call(addr::XMOUNT_CACHE_DRIVE, &[]), 0);
    assert_eq!(w.u32(LAST_ERROR), 1358);
    assert_eq!(w.u32(addr::CACHE_DRIVE_ROW), u32::MAX);
}

#[test]
fn utility_format_never_reaches_the_n_volume() {
    let mut w = world();
    assert_eq!(w.call(addr::XMOUNT_UTILITY_DRIVE, &[0]), 1);
    assert_eq!(w.call(addr::XMOUNT_CACHE_DRIVE, &[]), 1);
    put(&w.vfs, "\\??\\N:\\cache002.map", b"menu");
    put(&w.vfs, "\\??\\Z:\\cache000.map", b"z");
    assert_eq!(w.call(addr::XFORMAT_UTILITY_DRIVE, &[]), 1);
    assert!(!exists(&w.vfs, "\\??\\Z:\\cache000.map"));
    assert!(exists(&w.vfs, "\\??\\N:\\cache002.map"));
    // Z: cannot be remounted onto N:'s volume.
    let mut storage = VfsStorage::new(Arc::clone(&w.vfs));
    assert_eq!(
        xapi::Storage::mount_utility(&mut storage, 4, true),
        Err(xapi::Error::Win32(32))
    );
    assert_eq!(
        xapi::Storage::mount_cache(&mut storage, 5),
        Err(xapi::Error::Win32(32))
    );
    assert!(exists(&w.vfs, "\\??\\N:\\cache002.map"));
}

fn state(w: &World) -> (u32, Gamepad) {
    let b: [u8; 22] = w.bytes(SCRATCH + 0x200);
    (
        u32::from_le_bytes(b[..4].try_into().unwrap()),
        Gamepad::from_bytes(b[4..].try_into().unwrap()),
    )
}

#[test]
fn the_keyboard_pad_is_a_connected_controller_on_port_zero() {
    let mut w = world();
    let gamepad = addr::XDEVICE_TYPE_GAMEPAD;

    // XInitDevices(0, NULL) publishes Cur/Chg/Prev = 1/1/0.
    w.call(addr::XINIT_DEVICES, &[0, 0]);
    assert_eq!(
        (w.u32(gamepad), w.u32(gamepad + 4), w.u32(gamepad + 8)),
        (1, 1, 0)
    );
    // XGetDevices has no verified address; the adapter agrees with Cur.
    assert_eq!(w.xapi.lock().unwrap().get_devices(gamepad), 1);

    // 0x124aa0: gamepad changes report port 0 inserted once.
    assert_eq!(
        w.call(addr::XGET_DEVICE_CHANGES, &[gamepad, SCRATCH, SCRATCH + 4]),
        1
    );
    assert_eq!((w.u32(SCRATCH), w.u32(SCRATCH + 4)), (1, 0));
    assert_eq!(
        (w.u32(gamepad), w.u32(gamepad + 4), w.u32(gamepad + 8)),
        (1, 0, 1)
    );
    assert_eq!(
        w.call(addr::XGET_DEVICE_CHANGES, &[gamepad, SCRATCH, SCRATCH + 4]),
        0
    );
    // No memory units, ever.
    assert_eq!(
        w.call(addr::XGET_DEVICE_CHANGES, &[MU_TYPE, SCRATCH, SCRATCH + 4]),
        0
    );
    assert_eq!((w.u32(SCRATCH), w.u32(SCRATCH + 4)), (0, 0));
    assert_eq!(w.call(addr::XPEEK_DEVICES, &[MU_TYPE, 0, 0]), 0);
    assert_eq!(w.call(addr::XREAD_MU_META_DATA, &[0, 0, 0, 0, 0]), 1167);

    // XInputOpen(gamepad, 0, 0, NULL) as at 0x124b1f; ports 1..3 are empty.
    let handle = w.call(addr::XINPUT_OPEN, &[gamepad, 0, 0, 0]);
    assert_ne!(handle, 0);
    w.set(LAST_ERROR, 0);
    assert_eq!(w.call(addr::XINPUT_OPEN, &[gamepad, 1, 0, 0]), 0);
    assert_eq!(w.u32(LAST_ERROR), 1167);

    // Capabilities: SubType 1 (Halo's check at 0x124b3d) and both motors.
    assert_eq!(
        w.call(addr::XINPUT_GET_CAPABILITIES, &[handle, SCRATCH + 0x100]),
        0
    );
    let caps: [u8; 25] = w.bytes(SCRATCH + 0x100);
    assert_eq!(caps[0], 1);
    assert_eq!(&caps[21..25], &[0xff; 4]);

    // XInputGetState: 22 bytes, idle.
    assert_eq!(
        w.call(addr::XINPUT_GET_STATE, &[handle, SCRATCH + 0x200]),
        0
    );
    let (packet, pad) = state(&w);
    assert_eq!(pad, Gamepad::default());

    // The window presses Escape (START) and W; nothing changes until a pump.
    w.keyboard.key_down("Escape");
    w.keyboard.key_down("KeyW");
    assert_eq!(
        w.call(addr::XINPUT_GET_STATE, &[handle, SCRATCH + 0x200]),
        0
    );
    assert_eq!(state(&w), (packet, Gamepad::default()));
    let mut events = Recorded::default();
    service_input(&w.runtime, &w.xapi, &mut events).unwrap();
    assert_eq!(
        w.call(addr::XINPUT_GET_STATE, &[handle, SCRATCH + 0x200]),
        0
    );
    let (pressed_packet, pressed) = state(&w);
    assert!(pressed_packet > packet);
    assert!(pressed.buttons.contains(input::Buttons::START));
    assert_eq!(pressed.thumb_ly, 32767);
    w.keyboard.release_all();
    service_input(&w.runtime, &w.xapi, &mut events).unwrap();
    w.call(addr::XINPUT_GET_STATE, &[handle, SCRATCH + 0x200]);
    assert_eq!(state(&w).1, Gamepad::default());

    // Rumble as Halo's loop sends it (0x124f31): Header.hEvent 0, status
    // polled. Pending 997, then 0 after the next pump, motors forwarded.
    let feedback = SCRATCH + 0x300;
    let rumble = Rumble {
        left_motor: 0x4000,
        right_motor: 0x2000,
    };
    w.runtime
        .memory()
        .write_bytes(
            feedback,
            &Feedback {
                status: 0,
                h_event: 0,
                rumble,
            }
            .to_bytes(),
            1,
        )
        .unwrap();
    assert_eq!(w.call(addr::XINPUT_SET_STATE, &[handle, feedback]), 997);
    assert_eq!(w.u32(feedback), 997);
    service_input(&w.runtime, &w.xapi, &mut events).unwrap();
    assert_eq!(w.u32(feedback), 0);
    assert_eq!(w.keyboard.rumble(), rumble);
    assert!(events.0.is_empty(), "no event to signal");
    // With an event, the pump signals it after writing the status.
    w.set(feedback + 4, 0x88);
    assert_eq!(w.call(addr::XINPUT_SET_STATE, &[handle, feedback]), 997);
    service_input(&w.runtime, &w.xapi, &mut events).unwrap();
    assert_eq!(w.u32(feedback), 0);
    assert_eq!(events.0, [0x88]);

    // Unplug (pad disabled): the open handle keeps its last report and
    // returns 1167; the change is reported as a removal.
    w.keyboard.set_enabled(false);
    service_input(&w.runtime, &w.xapi, &mut events).unwrap();
    assert_eq!(
        w.call(addr::XINPUT_GET_STATE, &[handle, SCRATCH + 0x200]),
        1167
    );
    assert_eq!(
        w.call(addr::XGET_DEVICE_CHANGES, &[gamepad, SCRATCH, SCRATCH + 4]),
        1
    );
    assert_eq!((w.u32(SCRATCH), w.u32(SCRATCH + 4)), (0, 1));
    w.call(addr::XINPUT_CLOSE, &[handle]);
}

#[test]
fn aliased_letters_are_compared_by_the_volume_they_resolve_to() {
    let w = world();
    let alias = |v: &Mutex<vfs::Vfs>, name: &str, target: &str| {
        let mut v = v.lock().unwrap();
        let _ = v.namespace.delete_symbolic_link(name);
        v.namespace.create_symbolic_link(name, target).unwrap();
    };
    let mut storage = VfsStorage::new(Arc::clone(&w.vfs));
    {
        let mut v = w.vfs.lock().unwrap();
        let options = OpenOptions {
            directory: Some(true),
            ..rw()
        };
        let h = v
            .open("\\Device\\Harddisk0\\Partition5\\sub", options)
            .unwrap()
            .handle;
        v.close(h).unwrap();
    }
    put(
        &w.vfs,
        "\\Device\\Harddisk0\\Partition5\\cache000.map",
        b"z",
    );
    // Z: -> alias -> Partition5, and Z: -> a subdirectory of Partition5.
    for z_target in ["\\??\\UtilityAlias", "\\Device\\Harddisk0\\Partition5\\sub"] {
        alias(
            &w.vfs,
            "\\??\\UtilityAlias",
            "\\Device\\Harddisk0\\Partition5",
        );
        alias(&w.vfs, "\\??\\Z:", z_target);
        assert_eq!(
            xapi::Storage::mount_cache(&mut storage, 5),
            Err(xapi::Error::Win32(32)),
            "{z_target}"
        );
        assert!(exists(
            &w.vfs,
            "\\Device\\Harddisk0\\Partition5\\cache000.map"
        ));
        assert_eq!(w.link("\\??\\N:"), None);
    }
    // The same for the utility side: N: -> alias -> Partition4.
    alias(
        &w.vfs,
        "\\??\\CacheAlias",
        "\\Device\\Harddisk0\\Partition4",
    );
    alias(&w.vfs, "\\??\\N:", "\\??\\CacheAlias");
    put(
        &w.vfs,
        "\\Device\\Harddisk0\\Partition4\\cache002.map",
        b"menu",
    );
    assert_eq!(
        xapi::Storage::mount_utility(&mut storage, 4, true),
        Err(xapi::Error::Win32(32))
    );
    // XFormatUtilityDrive with Z: aliased onto N:'s volume.
    alias(&w.vfs, "\\??\\Z:", "\\??\\CacheAlias");
    assert_eq!(
        xapi::Storage::format_utility(&mut storage),
        Err(xapi::Error::Win32(32))
    );
    assert!(exists(
        &w.vfs,
        "\\Device\\Harddisk0\\Partition4\\cache002.map"
    ));
    // Z: through an alias still formats the volume it resolves to.
    alias(&w.vfs, "\\??\\Z:", "\\??\\UtilityAlias");
    xapi::Storage::format_utility(&mut storage).unwrap();
    assert!(!exists(
        &w.vfs,
        "\\Device\\Harddisk0\\Partition5\\cache000.map"
    ));
    assert!(exists(
        &w.vfs,
        "\\Device\\Harddisk0\\Partition4\\cache002.map"
    ));
}
