//! The opt-in native save games: pure helpers, the adapter over the real VFS
//! on a private host directory, and the registered overrides through the real
//! runtime (with the game's allocator stood in for by a bump allocator).
use input::{InputManager, host::ConvertConfig};
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, abi, exec::HARNESS_EXIT,
    guest::Permissions,
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
};
use xapi::{
    Config, GuestLayout, GuestMemory, MemoryFault, Result, ThreadState, VfsStorage, Xapi,
    adapter::{Frame, Function, Outcome},
    register,
    register::{SAVE_OVERRIDES, addr},
    saves, unregister,
};

const TITLE_ID: u32 = 0x4d53_0064;

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = base.join(format!(
            "h2-xapi-saves-{}-{}",
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

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// Independent model of the retail hash (`0x3309b8`) in 128-bit arithmetic.
fn model_hash(name: &[u16]) -> String {
    let modulus: u128 = (1 << 48) - 59;
    let mut h: u128 = 0;
    for &unit in name {
        h = (h * 65536 + u128::from(unit)) % modulus;
    }
    format!("{h:012X}")
}

#[test]
fn directory_names_hash_the_utf16_name() {
    assert_eq!(saves::directory_name(&[]), "000000000000");
    assert_eq!(saves::directory_name(&utf16("A")), "000000000041");
    assert_eq!(saves::directory_name(&utf16("AB")), "000000410042");
    // Longer names wrap the 2^48 - 59 modulus.
    for name in ["Campaign Save 1", "\u{ffff}\u{ffff}\u{ffff}\u{ffff}x", "ab"] {
        let units = utf16(name);
        let got = saves::directory_name(&units);
        assert_eq!(got, model_hash(&units), "{name}");
        assert_eq!(got.len(), saves::DIRECTORY_NAME_LEN);
    }
    assert_ne!(
        saves::directory_name(&utf16("Campaign Save 1")),
        saves::directory_name(&utf16("Campaign Save 2"))
    );
}

#[test]
fn meta_is_utf16le_with_a_bom_and_optional_no_copy() {
    let name = utf16("Cairo Station");
    let plain = saves::encode_meta(&name, 0);
    assert_eq!(&plain[..2], &[0xff, 0xfe]);
    let text: Vec<u16> = plain[2..]
        .chunks(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .collect();
    assert_eq!(String::from_utf16(&text).unwrap(), "Name=Cairo Station\r\n");
    let locked = saves::encode_meta(&name, 1);
    let text: Vec<u16> = locked[2..]
        .chunks(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .collect();
    assert_eq!(
        String::from_utf16(&text).unwrap(),
        "Name=Cairo Station\r\nNoCopy=1\r\n"
    );
    // Other flag bits write nothing.
    assert_eq!(saves::encode_meta(&name, 2), plain);
    assert_eq!(saves::parse_meta(&locked, "Name"), Some(name));
    assert_eq!(saves::parse_meta(&locked, "NoCopy"), Some(utf16("1")));
    assert_eq!(saves::parse_meta(&locked, "Missing"), None);
    // "TitleName" is not "Name".
    let mut title = vec![0xff, 0xfe];
    title.extend("TitleName=x\r\n".encode_utf16().flat_map(u16::to_le_bytes));
    assert_eq!(saves::parse_meta(&title, "Name"), None);
    assert_eq!(saves::parse_meta(&[0xff], "Name"), None);
}

struct Memory(Vec<u8>);
impl GuestMemory for Memory {
    fn check(&self, address: u32, length: usize, write: bool) -> Result<()> {
        if address == 0 || address as usize + length > self.0.len() {
            return Err(xapi::Error::Memory(MemoryFault {
                address,
                length,
                write,
            }));
        }
        Ok(())
    }
    fn read(&self, address: u32, bytes: &mut [u8]) -> Result<()> {
        self.check(address, bytes.len(), false)?;
        bytes.copy_from_slice(&self.0[address as usize..][..bytes.len()]);
        Ok(())
    }
    fn write(&mut self, address: u32, bytes: &[u8]) -> Result<()> {
        self.check(address, bytes.len(), true)?;
        self.0[address as usize..][..bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
}

const STACK: u32 = 0x1000;
const ROOT: u32 = 0x2000;
const NAME: u32 = 0x2100;
const OUT: u32 = 0x2400;
const DATA: u32 = 0x3000;
const OBJECT: u32 = 0x4000;

struct Harness {
    _dir: TempDir,
    data_root: PathBuf,
    xapi: Xapi<VfsStorage>,
    memory: Memory,
    thread: ThreadState,
}

fn vfs_config(dir: &TempDir) -> vfs::Config {
    vfs::Config::new(&dir.0, TITLE_ID)
}

fn xapi_over(dir: &TempDir, save_games: bool) -> Xapi<VfsStorage> {
    let config = vfs_config(dir);
    let mut vfs = vfs::Vfs::with_storage(config.clone()).unwrap();
    vfs.create_standard_links(&config).unwrap();
    let mut xapi_config = Config::for_gamepad(0x100);
    xapi_config.save_games = save_games;
    Xapi::new(
        xapi_config,
        InputManager::new(ConvertConfig::default()).unwrap(),
        VfsStorage::new(Arc::new(Mutex::new(vfs))),
    )
    .unwrap()
}

impl Harness {
    fn new() -> Self {
        let dir = TempDir::new();
        let xapi = xapi_over(&dir, true);
        Self {
            data_root: dir.0.clone(),
            _dir: dir,
            xapi,
            memory: Memory(vec![0; 0x8000]),
            thread: ThreadState::default(),
        }
    }

    fn put_root(&mut self, root: &str) {
        let mut bytes = root.as_bytes().to_vec();
        bytes.push(0);
        self.memory.write(ROOT, &bytes).unwrap();
    }

    fn put_name(&mut self, name: &str) {
        self.put_name_at(NAME, name);
    }

    fn put_name_at(&mut self, at: u32, name: &str) {
        let mut bytes: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        bytes.extend([0, 0]);
        self.memory.write(at, &bytes).unwrap();
    }

    fn call(&mut self, function: Function, arguments: &[u32]) -> (u32, Outcome) {
        assert_eq!(arguments.len(), function.stack_words());
        self.memory
            .write(STACK, &0x1234_5678u32.to_le_bytes())
            .unwrap();
        for (i, a) in arguments.iter().enumerate() {
            self.memory
                .write(STACK + 4 + 4 * i as u32, &a.to_le_bytes())
                .unwrap();
        }
        let mut frame = Frame {
            eax: 0xdead_beef,
            esp: STACK,
        };
        let outcome = self
            .xapi
            .invoke(function, &mut frame, &mut self.memory, &mut self.thread)
            .unwrap();
        if outcome != Outcome::Original {
            assert_eq!(frame.esp, STACK + 4 + 4 * arguments.len() as u32);
        }
        (frame.eax, outcome)
    }

    fn create(&mut self, name: &str, disposition: u32, flags: u32) -> u32 {
        self.put_root("U:\\");
        self.put_name(name);
        self.call(
            Function::XCreateSaveGame,
            &[ROOT, NAME, disposition, flags, OUT, 0x100],
        )
        .0
    }

    fn delete(&mut self, name: &str) -> u32 {
        self.put_root("U:\\");
        self.put_name(name);
        self.call(Function::XDeleteSaveGame, &[ROOT, NAME]).0
    }

    fn save_dir(&self, name: &str) -> PathBuf {
        self.data_root
            .join("disk/Partition1/UDATA")
            .join(format!("{TITLE_ID:08X}"))
            .join(saves::directory_name(&utf16(name)))
    }

    fn out_path(&self) -> String {
        let mut bytes = vec![0; 0x100];
        self.memory.read(OUT, &mut bytes).unwrap();
        let end = bytes.iter().position(|&b| b == 0).unwrap();
        String::from_utf8(bytes[..end].to_vec()).unwrap()
    }

    fn found_name(&self) -> String {
        let mut bytes = vec![0u8; saves::FIND_DATA_NAME_UNITS * 2];
        self.memory
            .read(DATA + saves::FIND_DATA_NAME as u32, &mut bytes)
            .unwrap();
        let units: Vec<u16> = bytes
            .chunks(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .take_while(|&u| u != 0)
            .collect();
        String::from_utf16(&units).unwrap()
    }

    fn found_path(&self) -> String {
        let mut bytes = vec![0u8; saves::FIND_DATA_PATH_BYTES];
        self.memory
            .read(DATA + saves::FIND_DATA_PATH as u32, &mut bytes)
            .unwrap();
        let end = bytes.iter().position(|&b| b == 0).unwrap();
        String::from_utf8(bytes[..end].to_vec()).unwrap()
    }
}

#[test]
fn create_writes_the_directory_and_meta_under_the_title_udata() {
    let mut h = Harness::new();
    assert_eq!(h.create("Cairo Station", saves::CREATE_NEW, 0), 0);
    let dir = h.save_dir("Cairo Station");
    assert!(dir.is_dir(), "{dir:?}");
    assert_eq!(
        std::fs::read(dir.join("SaveMeta.xbx")).unwrap(),
        saves::encode_meta(&utf16("Cairo Station"), 0)
    );
    let hash = saves::directory_name(&utf16("Cairo Station"));
    assert_eq!(h.out_path(), format!("U:\\{hash}\\"));
    // Payload files are the game's, through the ordinary file path.
    assert!(!dir.join("SaveImage.xbx").exists());
}

#[test]
fn create_disposition_semantics() {
    let mut h = Harness::new();
    assert_eq!(h.create("Alpha", saves::CREATE_NEW, 1), 0);
    let dir = h.save_dir("Alpha");
    assert_eq!(
        std::fs::read(dir.join("SaveMeta.xbx")).unwrap(),
        saves::encode_meta(&utf16("Alpha"), 1)
    );
    // CREATE_NEW over an existing save: ERROR_ALREADY_EXISTS.
    assert_eq!(h.create("Alpha", saves::CREATE_NEW, 0), 183);
    // CREATE_ALWAYS (2) reuses it and does not rewrite the meta.
    assert_eq!(h.create("Alpha", 2, 0), 0);
    assert_eq!(
        std::fs::read(dir.join("SaveMeta.xbx")).unwrap(),
        saves::encode_meta(&utf16("Alpha"), 1)
    );
    assert_eq!(h.create("Alpha", saves::OPEN_EXISTING, 0), 0);
    // OPEN_EXISTING of a save that was never made fails with the path error.
    let missing = h.create("Beta", saves::OPEN_EXISTING, 0);
    assert!(matches!(missing, 2 | 3), "{missing}");
    assert!(!h.save_dir("Beta").exists());
}

#[test]
fn a_directory_holding_another_name_is_a_collision() {
    let mut h = Harness::new();
    assert_eq!(h.create("Alpha", saves::CREATE_NEW, 0), 0);
    // Foreign meta in Beta's directory, as a hash collision would leave.
    let beta = h.save_dir("Beta");
    std::fs::create_dir_all(&beta).unwrap();
    std::fs::write(
        beta.join("SaveMeta.xbx"),
        saves::encode_meta(&utf16("Other"), 0),
    )
    .unwrap();
    assert_eq!(
        h.create("Beta", saves::CREATE_NEW, 0),
        saves::ERROR_CANNOT_MAKE
    );
    assert_eq!(
        h.create("Beta", saves::OPEN_EXISTING, 0),
        saves::ERROR_NO_MATCH
    );
    assert_eq!(h.delete("Beta"), saves::ERROR_NO_MATCH);
    assert!(beta.join("SaveMeta.xbx").exists());
    // A directory without meta is not a save of any name.
    let empty = h.save_dir("Gamma");
    std::fs::create_dir_all(&empty).unwrap();
    assert_eq!(
        h.create("Gamma", saves::CREATE_NEW, 0),
        saves::ERROR_CANNOT_MAKE
    );
}

#[test]
fn create_reports_bad_arguments_and_short_output() {
    let mut h = Harness::new();
    // Not a drive path.
    h.put_root("Bogus");
    h.put_name("x");
    assert_eq!(
        h.call(Function::XCreateSaveGame, &[ROOT, NAME, 1, 0, OUT, 0x100])
            .0,
        saves::ERROR_PATH_NOT_FOUND
    );
    // A name over 127 units.
    h.put_name(&"n".repeat(128));
    h.put_root("U:\\");
    assert_eq!(
        h.call(Function::XCreateSaveGame, &[ROOT, NAME, 1, 0, OUT, 0x100])
            .0,
        87
    );
    // A null output pointer is allowed.
    h.put_name("Short");
    assert_eq!(
        h.call(Function::XCreateSaveGame, &[ROOT, NAME, 1, 0, 0, 0])
            .0,
        0
    );
}

/// The guest's bounded copy (`0x2d1404`) always NUL-terminates a nonzero
/// buffer; the backslash follows only when it and the NUL still fit.
#[test]
fn create_output_is_always_nul_terminated_at_every_boundary() {
    let mut h = Harness::new();
    h.put_root("U:\\");
    h.put_name("Short");
    assert_eq!(
        h.call(Function::XCreateSaveGame, &[ROOT, NAME, 1, 0, 0, 0])
            .0,
        0
    );
    let dir = format!("U:\\{}", saves::directory_name(&utf16("Short")));
    assert_eq!(dir.len(), 15);
    for (len, expected) in [
        (0usize, String::new()),
        (1, "\0".to_owned()),
        (2, format!("{}\0", &dir[..1])),
        (14, format!("{}\0", &dir[..13])),
        (15, format!("{}\0", &dir[..14])),
        (16, format!("{dir}\0")),
        (17, format!("{dir}\\\0")),
        (18, format!("{dir}\\\0")),
    ] {
        h.memory.write(OUT, &[0x55; 32]).unwrap();
        assert_eq!(
            h.call(
                Function::XCreateSaveGame,
                &[ROOT, NAME, 3, 0, OUT, len as u32]
            )
            .0,
            0,
            "{len}"
        );
        let mut got = [0u8; 32];
        h.memory.read(OUT, &mut got).unwrap();
        let written = expected.len();
        assert_eq!(&got[..written], expected.as_bytes(), "{len}");
        // Nothing past what the guest copy writes is touched.
        assert!(got[written..].iter().all(|&b| b == 0x55), "{len}");
    }
}

#[test]
fn delete_removes_the_save_and_everything_in_it() {
    let mut h = Harness::new();
    assert_eq!(h.create("Alpha", saves::CREATE_NEW, 0), 0);
    let dir = h.save_dir("Alpha");
    std::fs::create_dir(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/payload.bin"), [1, 2, 3]).unwrap();
    std::fs::write(dir.join("SaveImage.xbx"), [9; 16]).unwrap();
    assert_eq!(h.delete("Beta"), h.delete("Beta"));
    assert_ne!(h.delete("Beta"), 0);
    assert!(dir.exists());
    assert_eq!(h.delete("Alpha"), 0);
    assert!(!dir.exists());
    assert_ne!(h.delete("Alpha"), 0);
    // The name is free again.
    assert_eq!(h.create("Alpha", saves::CREATE_NEW, 0), 0);
}

#[test]
fn find_first_and_next_walk_the_saves_in_name_order() {
    let mut h = Harness::new();
    let mut names = vec!["Cairo Station", "Outskirts", "Metropolis"];
    for name in &names {
        assert_eq!(h.create(name, saves::CREATE_NEW, 0), 0);
    }
    // Entries the scanner rejects: a loose file and a directory with no meta.
    let root = h.save_dir("x").parent().unwrap().to_owned();
    std::fs::write(root.join("TitleMeta.xbx"), [0, 0]).unwrap();
    std::fs::create_dir(root.join("0000FACE0000")).unwrap();
    h.put_root("U:\\");
    let (_, outcome) = h.call(Function::XFindFirstSaveGame, &[ROOT, DATA]);
    let Outcome::NewSearch { pc } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(pc, 0x1234_5678);
    let search = h.xapi.take_pending_search().unwrap();
    assert!(h.xapi.take_pending_search().is_none());
    // Directory order is by hash name; the name comes from the meta.
    names.sort_by_key(|n| saves::directory_name(&utf16(n)));
    let mut seen = vec![h.found_name()];
    let first_path = h.found_path();
    assert_eq!(
        first_path,
        format!("U:\\{}\\", saves::directory_name(&utf16(&seen[0])))
    );
    let attributes = {
        let mut a = [0; 4];
        h.memory.read(DATA, &mut a).unwrap();
        u32::from_le_bytes(a)
    };
    assert_eq!(attributes & 0x10, 0x10);
    // Install the object as the allocator would.
    h.install_search(search);
    assert_eq!(h.memory_u32(OBJECT), saves::SEARCH_SIGNATURE);
    assert_eq!(h.memory_u32(OBJECT + 4), SEARCH_HANDLE);
    assert_eq!(h.memory_u32(OBJECT + 8), 3);
    for _ in 0..2 {
        h.memory.write(DATA, &[0; saves::FIND_DATA_SIZE]).unwrap();
        let (found, outcome) = h.call(Function::XFindNextSaveGame, &[OBJECT, DATA]);
        assert_eq!(
            (found, matches!(outcome, Outcome::Return { .. })),
            (1, true)
        );
        seen.push(h.found_name());
    }
    assert_eq!(seen, names);
    // The end: FALSE and ERROR_NO_MORE_FILES, repeatably.
    for _ in 0..2 {
        let (found, _) = h.call(Function::XFindNextSaveGame, &[OBJECT, DATA]);
        assert_eq!((found, h.thread.last_error), (0, 18));
        h.thread.last_error = 0;
    }
}

const SEARCH_HANDLE: u32 = 0x1c8;

impl Harness {
    /// Install a pending search as the host does: its block with a kernel
    /// handle in it, and the search adopted under that handle.
    fn install_search(&mut self, search: xapi::PendingSearch) {
        self.memory
            .write(OBJECT, &search.object(SEARCH_HANDLE))
            .unwrap();
        self.xapi.adopt_search(SEARCH_HANDLE, search);
    }

    fn memory_u32(&self, at: u32) -> u32 {
        let mut b = [0; 4];
        self.memory.read(at, &mut b).unwrap();
        u32::from_le_bytes(b)
    }
}

#[test]
fn find_first_failures_return_minus_one_with_the_error() {
    let mut h = Harness::new();
    h.put_root("U:\\");
    let (eax, outcome) = h.call(Function::XFindFirstSaveGame, &[ROOT, DATA]);
    assert_eq!((eax, h.thread.last_error), (u32::MAX, 18));
    assert!(matches!(outcome, Outcome::Return { .. }));
    assert!(h.xapi.take_pending_search().is_none());
    // A root that does not exist.
    h.put_root("Q:\\");
    let (eax, _) = h.call(Function::XFindFirstSaveGame, &[ROOT, DATA]);
    assert_eq!((eax, h.thread.last_error), (u32::MAX, 3));
    // A search handle that is not one.
    h.memory.write(OBJECT, &[0; 16]).unwrap();
    let (found, _) = h.call(Function::XFindNextSaveGame, &[OBJECT, DATA]);
    assert_eq!((found, h.thread.last_error), (0, 6));
}

#[test]
fn deleting_during_a_search_skips_the_removed_save() {
    let mut h = Harness::new();
    for name in ["A", "B", "C"] {
        assert_eq!(h.create(name, saves::CREATE_NEW, 0), 0);
    }
    h.put_root("U:\\");
    h.call(Function::XFindFirstSaveGame, &[ROOT, DATA]);
    let search = h.xapi.take_pending_search().unwrap();
    h.install_search(search);
    let first = h.found_name();
    for name in ["A", "B", "C"] {
        if name != first {
            assert_eq!(h.delete(name), 0);
        }
    }
    h.put_root("U:\\");
    let (found, _) = h.call(Function::XFindNextSaveGame, &[OBJECT, DATA]);
    assert_eq!(found, 0);
}

#[test]
fn the_family_is_generated_unless_enabled() {
    let dir = TempDir::new();
    let mut xapi = xapi_over(&dir, false);
    let mut memory = Memory(vec![0; 0x8000]);
    let mut thread = ThreadState::default();
    for function in [
        Function::XCreateSaveGame,
        Function::XDeleteSaveGame,
        Function::XFindFirstSaveGame,
        Function::XFindNextSaveGame,
        Function::XFindClose,
    ] {
        let mut frame = Frame {
            eax: 7,
            esp: u32::MAX,
        };
        assert_eq!(
            xapi.invoke(function, &mut frame, &mut memory, &mut thread),
            Ok(Outcome::Original)
        );
        assert_eq!(
            frame,
            Frame {
                eax: 7,
                esp: u32::MAX
            }
        );
    }
    // Enabled, XFindClose is still the generated one.
    let mut enabled = xapi_over(&dir, true);
    let mut frame = Frame {
        eax: 7,
        esp: u32::MAX,
    };
    assert_eq!(
        enabled.invoke(Function::XFindClose, &mut frame, &mut memory, &mut thread),
        Ok(Outcome::Original)
    );
}

#[test]
fn storage_without_saves_fails_with_not_supported() {
    struct Bare;
    impl xapi::Storage for Bare {
        fn mount_utility(&mut self, _: u8, _: bool) -> Result<()> {
            Ok(())
        }
        fn format_utility(&mut self) -> Result<()> {
            Ok(())
        }
    }
    let mut config = Config::for_gamepad(0x100);
    config.save_games = true;
    let mut xapi = Xapi::new(
        config,
        InputManager::new(ConvertConfig::default()).unwrap(),
        Bare,
    )
    .unwrap();
    let mut memory = Memory(vec![0; 0x8000]);
    memory.write(STACK, &1u32.to_le_bytes()).unwrap();
    memory.write(ROOT, b"U:\\\0").unwrap();
    memory.write(NAME, &[b'a', 0, 0, 0]).unwrap();
    for (i, a) in [ROOT, NAME, 1, 0, 0, 0].iter().enumerate() {
        memory
            .write(STACK + 4 + 4 * i as u32, &a.to_le_bytes())
            .unwrap();
    }
    let mut frame = Frame { eax: 0, esp: STACK };
    let mut thread = ThreadState::default();
    xapi.invoke(
        Function::XCreateSaveGame,
        &mut frame,
        &mut memory,
        &mut thread,
    )
    .unwrap();
    assert_eq!(frame.eax, 50);
}

// ---- through the runtime ----------------------------------------------

const FS: u32 = 0x0050_0000;
const LAST_ERROR: u32 = 0x0050_2008;
const STACK_TOP: u32 = 0x0061_0000;
const SCRATCH: u32 = 0x0070_0000;
const HEAP: u32 = 0x0080_0000;

struct World {
    _dir: TempDir,
    runtime: Arc<Runtime>,
    ctx: ExecCtx,
    cpu: CpuAbi,
    xapi: Arc<Mutex<Xapi<VfsStorage>>>,
    registration: xapi::Registration,
    /// (size, tag) of each request the stand-in allocator served.
    allocations: Arc<Mutex<Vec<(u32, u32)>>>,
    fail_allocation: Arc<AtomicU32>,
    /// (attributes, type, initial state) of each NtCreateEvent served.
    events: Arc<Mutex<Vec<(u32, u32, u32)>>>,
    fail_event: Arc<AtomicU32>,
}

const EVENT_HANDLE: u32 = 0x1a4;

fn world(save_games: bool) -> World {
    let dir = TempDir::new();
    let xapi = Arc::new(Mutex::new(xapi_over(&dir, save_games)));
    let mut dispatch = Dispatch::default();
    let registration = register(&mut dispatch, &xapi, GuestLayout::default()).unwrap();
    let allocations = Arc::new(Mutex::new(Vec::new()));
    let fail_allocation = Arc::new(AtomicU32::new(0));
    {
        let (allocations, fail, next) = (
            Arc::clone(&allocations),
            Arc::clone(&fail_allocation),
            AtomicU32::new(HEAP),
        );
        dispatch.register_override(addr::GAME_ALLOCATE, "test::alloc", move |cpu, ctx, _| {
            let guard = ctx.memory();
            let size = guard.u32(cpu.esp + 4)?;
            let tag = guard.u32(cpu.esp + 8)?;
            allocations.lock().unwrap().push((size, tag));
            cpu.eax = if fail.load(Ordering::Relaxed) != 0 {
                0
            } else {
                next.fetch_add(0x200, Ordering::Relaxed)
            };
            guard.ret(cpu, 8)
        });
    }
    let events = Arc::new(Mutex::new(Vec::new()));
    let fail_event = Arc::new(AtomicU32::new(0));
    {
        let (events, fail) = (Arc::clone(&events), Arc::clone(&fail_event));
        // The kernel's NtCreateEvent (ordinal 189, four words).
        dispatch.register_override(
            runtime::kernel::function_token(189),
            "test::NtCreateEvent",
            move |cpu, ctx, _| {
                let mut guard = ctx.memory();
                let out = guard.u32(cpu.esp + 4)?;
                events.lock().unwrap().push((
                    guard.u32(cpu.esp + 8)?,
                    guard.u32(cpu.esp + 12)?,
                    guard.u32(cpu.esp + 16)?,
                ));
                if fail.load(Ordering::Relaxed) != 0 {
                    cpu.eax = 0xc000_009a;
                } else {
                    guard.set_u32(out, EVENT_HANDLE)?;
                    cpu.eax = 0;
                }
                guard.ret(cpu, 16)
            },
        );
    }
    let mut g = Guest::new();
    g.map_zeroed(0x0001_0000, 0x3000, Permissions::RWX).unwrap();
    g.map_zeroed(0x0033_0000, 0x2000, Permissions::RWX).unwrap();
    g.map_zeroed(0x0048_0000, 0x1000, Permissions::RW).unwrap();
    g.map_zeroed(FS, 0x3000, Permissions::RW).unwrap();
    g.set_u32(FS + 4, FS + 0x1000).unwrap();
    g.set_u32(FS + 0x1000, FS + 0x2000).unwrap();
    g.set_u32(LAST_ERROR, 1234).unwrap();
    g.map_zeroed(STACK_TOP - 0x1_0000, 0x1_0000, Permissions::RW)
        .unwrap();
    g.map_zeroed(SCRATCH, 0x1_0000, Permissions::RW).unwrap();
    g.map_zeroed(HEAP, 0x1_0000, Permissions::RW).unwrap();
    let runtime = Runtime::new(g, dispatch);
    let ctx = ExecCtx::new(Arc::clone(&runtime), 1);
    World {
        _dir: dir,
        runtime,
        ctx,
        cpu: CpuAbi {
            fs_base: FS,
            ..CpuAbi::default()
        },
        xapi,
        registration,
        allocations,
        fail_allocation,
        events,
        fail_event,
    }
}

impl World {
    /// stdcall `target(args...)` returning to HARNESS_EXIT; gives EAX and
    /// requires the pinned cleanup of `4 * args.len()` bytes.
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
        assert_eq!(
            t.kind,
            abi::STOP,
            "{target:#x}: {t:?} {:?}",
            self.ctx.event(t.event_id)
        );
        assert!(
            matches!(self.ctx.event(t.event_id), Some(Event::HarnessExit)),
            "{target:#x}: {:?}",
            self.ctx.event(t.event_id)
        );
        assert_eq!(self.cpu.esp, esp + 4 + 4 * args.len() as u32);
        self.cpu.eax
    }

    fn put(&self, at: u32, bytes: &[u8]) {
        self.runtime.memory().write_bytes(at, bytes, 1).unwrap();
    }

    fn u32(&self, at: u32) -> u32 {
        self.runtime.memory().u32(at).unwrap()
    }

    fn put_wide(&self, at: u32, text: &str) {
        let mut bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        bytes.extend([0, 0]);
        self.put(at, &bytes);
    }
}

#[test]
fn registration_adds_the_four_save_overrides_only_when_enabled() {
    let off = world(false);
    for o in SAVE_OVERRIDES {
        assert!(
            !off.registration.addresses().contains(&o.addr),
            "{:#x}",
            o.addr
        );
    }
    assert_eq!(off.registration.installed.len(), register::OVERRIDES.len());

    let on = world(true);
    let mut got: Vec<u32> = on.registration.addresses();
    got.sort_unstable();
    for pinned in [0x0033_0df2, 0x0033_1055, 0x0033_1125, 0x0033_122f] {
        assert!(got.contains(&pinned), "{pinned:#x}");
    }
    // XFindClose (0x331276) stays generated.
    assert!(!got.contains(&0x0033_1276));
    assert_eq!(
        on.registration.installed.len(),
        register::OVERRIDES.len() + 4
    );

    // A second registration reports all twenty as occupied; unregister
    // disables them together.
    let mut dispatch = Dispatch::default();
    let x = Arc::clone(&on.xapi);
    let first = register(&mut dispatch, &x, GuestLayout::default()).unwrap();
    match register(&mut dispatch, &x, GuestLayout::default()) {
        Err(xapi::RegisterError::Occupied(c)) => assert_eq!(c.len(), 20),
        other => panic!("{other:?}"),
    }
    unregister(&mut dispatch, &first);
}

#[test]
fn save_calls_run_through_the_runner_with_the_pinned_cleanup() {
    let mut w = world(true);
    w.put(SCRATCH, b"U:\\\0");
    w.put_wide(SCRATCH + 0x100, "Cairo Station");
    // XCreateSaveGame: ret 18h.
    let status = w.call(
        addr::XCREATE_SAVE_GAME,
        &[SCRATCH, SCRATCH + 0x100, 1, 0, SCRATCH + 0x200, 0x100],
    );
    assert_eq!(status, 0);
    let hash = saves::directory_name(&utf16("Cairo Station"));
    let mut out = vec![0u8; 20];
    w.runtime
        .memory()
        .read_bytes(SCRATCH + 0x200, &mut out, 1)
        .unwrap();
    assert_eq!(&out[..17], format!("U:\\{hash}\\\0").as_bytes());
    // ret 8 for the other three.
    w.put_wide(SCRATCH + 0x300, "Nope");
    assert_ne!(
        w.call(addr::XDELETE_SAVE_GAME, &[SCRATCH, SCRATCH + 0x300]),
        0
    );
    let first = w.call(addr::XFIND_FIRST_SAVE_GAME, &[SCRATCH, SCRATCH + 0x1000]);
    // The stand-in allocator served exactly the original's request.
    assert_eq!(*w.allocations.lock().unwrap(), vec![(0x110, 0x2483_0000)]);
    assert_eq!(first, HEAP);
    assert_eq!(w.u32(first), saves::SEARCH_SIGNATURE);
    // The handle word is the kernel handle the stand-in NtCreateEvent made,
    // with the pinned arguments.
    assert_eq!(w.u32(first + 4), EVENT_HANDLE);
    assert_eq!(*w.events.lock().unwrap(), vec![(0, 0, 0)]);
    assert_eq!(w.u32(first + 8), 3);
    let mut name = vec![0u8; 28];
    w.runtime
        .memory()
        .read_bytes(
            SCRATCH + 0x1000 + saves::FIND_DATA_NAME as u32,
            &mut name,
            1,
        )
        .unwrap();
    assert_eq!(
        name,
        "Cairo Station\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        w.call(addr::XFIND_NEXT_SAVE_GAME, &[first, SCRATCH + 0x1000]),
        0
    );
    assert_eq!(w.u32(LAST_ERROR), 18);
    assert_eq!(
        w.call(addr::XDELETE_SAVE_GAME, &[SCRATCH, SCRATCH + 0x100]),
        0
    );
    // Nothing left: -1 and ERROR_NO_MORE_FILES, no allocation.
    assert_eq!(
        w.call(addr::XFIND_FIRST_SAVE_GAME, &[SCRATCH, SCRATCH + 0x1000]),
        u32::MAX
    );
    assert_eq!(w.allocations.lock().unwrap().len(), 1);
    assert_eq!(w.u32(LAST_ERROR), 18);
}

#[test]
fn a_failed_search_allocation_is_not_enough_memory() {
    let mut w = world(true);
    w.put(SCRATCH, b"U:\\\0");
    w.put_wide(SCRATCH + 0x100, "Only");
    assert_eq!(
        w.call(
            addr::XCREATE_SAVE_GAME,
            &[SCRATCH, SCRATCH + 0x100, 1, 0, 0, 0]
        ),
        0
    );
    w.fail_allocation.store(1, Ordering::Relaxed);
    assert_eq!(
        w.call(addr::XFIND_FIRST_SAVE_GAME, &[SCRATCH, SCRATCH + 0x1000]),
        u32::MAX
    );
    assert_eq!(w.u32(LAST_ERROR), 8);
}

#[test]
fn a_failed_search_handle_is_not_enough_memory() {
    let mut w = world(true);
    w.put(SCRATCH, b"U:\\\0");
    w.put_wide(SCRATCH + 0x100, "Only");
    assert_eq!(
        w.call(
            addr::XCREATE_SAVE_GAME,
            &[SCRATCH, SCRATCH + 0x100, 1, 0, 0, 0]
        ),
        0
    );
    w.fail_event.store(1, Ordering::Relaxed);
    assert_eq!(
        w.call(addr::XFIND_FIRST_SAVE_GAME, &[SCRATCH, SCRATCH + 0x1000]),
        u32::MAX
    );
    assert_eq!(w.u32(LAST_ERROR), 8);
}

#[test]
fn next_through_the_runner_follows_the_handle_in_the_block() {
    let mut w = world(true);
    w.put(SCRATCH, b"U:\\\0");
    for (i, name) in ["One", "Two"].iter().enumerate() {
        w.put_wide(SCRATCH + 0x100 + 0x40 * i as u32, name);
        assert_eq!(
            w.call(
                addr::XCREATE_SAVE_GAME,
                &[SCRATCH, SCRATCH + 0x100 + 0x40 * i as u32, 1, 0, 0, 0]
            ),
            0
        );
    }
    let block = w.call(addr::XFIND_FIRST_SAVE_GAME, &[SCRATCH, SCRATCH + 0x1000]);
    assert_eq!(
        w.call(addr::XFIND_NEXT_SAVE_GAME, &[block, SCRATCH + 0x1000]),
        1
    );
    // A block whose handle word names no open search is not a search.
    w.runtime.memory().set_u32(block + 4, 0x999).unwrap();
    assert_eq!(
        w.call(addr::XFIND_NEXT_SAVE_GAME, &[block, SCRATCH + 0x1000]),
        0
    );
    assert_eq!(w.u32(LAST_ERROR), 6);
}

// ---- mid-search changes, layout and failure injection -------------------

fn start_search(h: &mut Harness) -> String {
    h.put_root("U:\\");
    h.call(Function::XFindFirstSaveGame, &[ROOT, DATA]);
    let search = h.xapi.take_pending_search().unwrap();
    h.install_search(search);
    h.found_name()
}

fn next_name(h: &mut Harness) -> Option<String> {
    h.memory.write(DATA, &[0; saves::FIND_DATA_SIZE]).unwrap();
    let (found, _) = h.call(Function::XFindNextSaveGame, &[OBJECT, DATA]);
    (found == 1).then(|| h.found_name())
}

/// Names in the order the search yields them for `names`.
fn hash_order(names: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
    sorted.sort_by_key(|n| saves::directory_name(&utf16(n)));
    sorted
}

#[test]
fn deleting_an_already_returned_save_does_not_skip_the_next() {
    let mut h = Harness::new();
    let names = ["A", "B", "C", "D"];
    for name in names {
        assert_eq!(h.create(name, saves::CREATE_NEW, 0), 0);
    }
    let order = hash_order(&names);
    assert_eq!(start_search(&mut h), order[0]);
    assert_eq!(h.delete(&order[0]), 0);
    assert_eq!(next_name(&mut h).as_deref(), Some(order[1].as_str()));
    assert_eq!(next_name(&mut h).as_deref(), Some(order[2].as_str()));
    assert_eq!(next_name(&mut h).as_deref(), Some(order[3].as_str()));
    assert_eq!(next_name(&mut h), None);
}

#[test]
fn creating_a_save_before_the_cursor_does_not_repeat_one() {
    let mut h = Harness::new();
    // Three names in hash order: `earlier` sorts before `first`.
    let sorted = hash_order(&["S0", "S1", "S2"]);
    let (earlier, first, other) = (sorted[0].clone(), sorted[1].clone(), sorted[2].clone());
    assert_eq!(h.create(&first, saves::CREATE_NEW, 0), 0);
    assert_eq!(h.create(&other, saves::CREATE_NEW, 0), 0);
    assert_eq!(start_search(&mut h), first);
    assert_eq!(h.create(&earlier, saves::CREATE_NEW, 0), 0);
    assert_eq!(next_name(&mut h).as_deref(), Some(other.as_str()));
    assert_eq!(next_name(&mut h), None);
}

#[test]
fn find_data_has_the_abi_offsets_for_path_and_name() {
    let mut h = Harness::new();
    assert_eq!(h.create("Layout", saves::CREATE_NEW, 0), 0);
    h.put_root("U:\\");
    h.memory
        .write(DATA, &[0x77; saves::FIND_DATA_SIZE])
        .unwrap();
    h.call(Function::XFindFirstSaveGame, &[ROOT, DATA]);
    let mut raw = vec![0u8; 836];
    h.memory.read(DATA, &mut raw).unwrap();
    let path = format!("U:\\{}\\", saves::directory_name(&utf16("Layout")));
    assert_eq!(&raw[320..320 + path.len()], path.as_bytes());
    assert_eq!(raw[320 + path.len()], 0);
    let name: Vec<u8> = "Layout\0"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    assert_eq!(&raw[580..580 + name.len()], name.as_slice());
    assert_eq!(u32::from_le_bytes(raw[0..4].try_into().unwrap()), 0x10);
}

/// A [`VfsStorage`] that can fail chosen operations and records removals.
struct Faulty {
    inner: VfsStorage,
    touch_error: Option<u32>,
    create_file_error: Option<u32>,
    removed: Vec<String>,
}

impl xapi::Storage for Faulty {
    fn mount_utility(&mut self, partition: u8, format: bool) -> Result<()> {
        self.inner.mount_utility(partition, format)
    }
    fn format_utility(&mut self) -> Result<()> {
        self.inner.format_utility()
    }
    fn saves(&mut self) -> Option<&mut dyn xapi::SaveStore> {
        Some(self)
    }
}

impl xapi::SaveStore for Faulty {
    fn create_directory(&mut self, path: &str) -> Result<()> {
        self.inner.create_directory(path)
    }
    fn read_file(&mut self, path: &str, limit: usize) -> Result<Vec<u8>> {
        self.inner.read_file(path, limit)
    }
    fn create_file(&mut self, path: &str, contents: &[u8]) -> Result<()> {
        match self.create_file_error {
            Some(code) => Err(xapi::Error::Win32(code)),
            None => self.inner.create_file(path, contents),
        }
    }
    fn touch_directory(&mut self, path: &str) -> Result<()> {
        match self.touch_error {
            Some(code) => Err(xapi::Error::Win32(code)),
            None => self.inner.touch_directory(path),
        }
    }
    fn remove_tree(&mut self, path: &str) -> Result<()> {
        self.removed.push(path.to_owned());
        self.inner.remove_tree(path)
    }
    fn list_directories(&mut self, path: &str) -> Result<Vec<xapi::SaveDirectory>> {
        self.inner.list_directories(path)
    }
}

fn faulty(dir: &TempDir) -> (Xapi<Faulty>, Memory) {
    let real = xapi_over(dir, true);
    let mut config = Config::for_gamepad(0x100);
    config.save_games = true;
    let xapi = Xapi::new(
        config,
        InputManager::new(ConvertConfig::default()).unwrap(),
        Faulty {
            inner: real.storage,
            touch_error: None,
            create_file_error: None,
            removed: Vec::new(),
        },
    )
    .unwrap();
    let mut memory = Memory(vec![0; 0x8000]);
    memory.write(ROOT, b"U:\\\0").unwrap();
    memory.write(NAME, &[b'F', 0, 0, 0]).unwrap();
    (xapi, memory)
}

fn create_faulty(xapi: &mut Xapi<Faulty>, memory: &mut Memory, disposition: u32) -> u32 {
    memory.write(STACK, &1u32.to_le_bytes()).unwrap();
    for (i, a) in [ROOT, NAME, disposition, 0, OUT, 0x100].iter().enumerate() {
        memory
            .write(STACK + 4 + 4 * i as u32, &a.to_le_bytes())
            .unwrap();
    }
    let mut frame = Frame { eax: 0, esp: STACK };
    let mut thread = ThreadState::default();
    xapi.invoke(Function::XCreateSaveGame, &mut frame, memory, &mut thread)
        .unwrap();
    frame.eax
}

#[test]
fn a_timestamp_failure_does_not_change_the_disposition_result() {
    let dir = TempDir::new();
    let (mut xapi, mut memory) = faulty(&dir);
    assert_eq!(create_faulty(&mut xapi, &mut memory, saves::CREATE_NEW), 0);
    xapi.storage.touch_error = Some(5);
    // CREATE_NEW over the existing save is still 183, others still succeed.
    assert_eq!(
        create_faulty(&mut xapi, &mut memory, saves::CREATE_NEW),
        saves::ERROR_ALREADY_EXISTS
    );
    assert_eq!(create_faulty(&mut xapi, &mut memory, 2), 0);
    assert_eq!(
        create_faulty(&mut xapi, &mut memory, saves::OPEN_EXISTING),
        0
    );
}

#[test]
fn a_failed_meta_write_rolls_the_new_directory_back() {
    let dir = TempDir::new();
    let (mut xapi, mut memory) = faulty(&dir);
    xapi.storage.create_file_error = Some(112);
    assert_eq!(
        create_faulty(&mut xapi, &mut memory, saves::CREATE_NEW),
        112
    );
    let directory = format!("U:\\{}", saves::directory_name(&utf16("F")));
    assert_eq!(xapi.storage.removed, vec![directory]);
    let host = dir
        .0
        .join("disk/Partition1/UDATA")
        .join(format!("{TITLE_ID:08X}"))
        .join(saves::directory_name(&utf16("F")));
    assert!(!host.exists(), "{host:?}");
    // And the name is free again once the fault clears.
    xapi.storage.create_file_error = None;
    assert_eq!(create_faulty(&mut xapi, &mut memory, saves::CREATE_NEW), 0);
}
