//! Native replacements for the XAPI save-game family (docs/xapi.md, "Opt-in
//! native save games"). Off unless `Config::save_games` is set.
//!
//! The retail routines are statically linked XDK code at the addresses in
//! `register::addr`. This module reproduces their observable behaviour over a
//! [`SaveStore`], so saves land in the title's `U:` area of the shared VFS
//! (host directory `<data root>/disk/Partition1/UDATA/<title id>`):
//!
//! - A save is a directory `<root><12 hex digits>` named by a hash of its
//!   UTF-16 name ([`directory_name`]), holding `SaveMeta.xbx`
//!   ([`encode_meta`]); the game puts its payload beside it.
//! - `XCreateSaveGame` (`0x330df2`) creates or opens that directory and
//!   returns a Win32 error (0 is success); `XDeleteSaveGame` (`0x331055`)
//!   removes it after checking the name; `XFindFirstSaveGame` (`0x331125`)
//!   and `XFindNextSaveGame` (`0x33122f`) list the directories whose
//!   `SaveMeta.xbx` yields a `Name`.
//! - `XFindClose` (`0x331276`) stays generated: the search object is a guest
//!   heap block the original `XFindClose` frees, see [`PendingSearch`].
use crate::{Error, GuestMemory, Result, Storage, Xapi, memory};

pub const SAVE_META: &str = "SaveMeta.xbx";
pub const SAVE_IMAGE: &str = "SaveImage.xbx";
pub const TITLE_META: &str = "TitleMeta.xbx";
pub const TITLE_IMAGE: &str = "TitleImage.xbx";

/// Largest prime below 2^48, the modulus of the name hash (`0x330a3a`).
pub const HASH_MODULUS: u64 = 0x0000_ffff_ffff_ffc5;
/// Hex digits in a save directory name.
pub const DIRECTORY_NAME_LEN: usize = 12;
/// Longest save name, in UTF-16 units, the metadata reader returns (the
/// original buffer is 0x80 units including the terminator).
pub const MAX_NAME_UNITS: usize = 127;
/// `root` plus a directory name must stay under this (`0x330c8b`, `0x330e22`).
pub const MAX_PATH_LEN: usize = 0xf7;

pub const ERROR_PATH_NOT_FOUND: u32 = 3;
pub const ERROR_INVALID_HANDLE: u32 = 6;
pub const ERROR_NO_MORE_FILES: u32 = 18;
pub const ERROR_CANNOT_MAKE: u32 = 0x52;
pub const ERROR_ALREADY_EXISTS: u32 = 183;
/// Returned when `SaveMeta.xbx` exists but its `Name` differs (`0x330d7a`).
pub const ERROR_NO_MATCH: u32 = 0x491;

/// `XCreateSaveGame` creation dispositions (the Win32 values).
pub const CREATE_NEW: u32 = 1;
pub const OPEN_EXISTING: u32 = 3;
/// `XCreateSaveGame` flag bit 0: write `NoCopy=1`.
pub const FLAG_NO_COPY: u32 = 1;

/// `XGAME_FIND_DATA`: a 320-byte `WIN32_FIND_DATAA` prefix, the save's ANSI
/// path at +320 (260 bytes) and its UTF-16 name at +580 (128 units).
pub const FIND_DATA_SIZE: usize = 836;
pub const FIND_DATA_ATTRIBUTES: usize = 0;
pub const FIND_DATA_CREATION_TIME: usize = 4;
pub const FIND_DATA_ACCESS_TIME: usize = 12;
pub const FIND_DATA_WRITE_TIME: usize = 20;
pub const FIND_DATA_FILE_NAME: usize = 44;
pub const FIND_DATA_PATH: usize = 320;
pub const FIND_DATA_NAME: usize = 580;
pub const FIND_DATA_PATH_BYTES: usize = 260;
pub const FIND_DATA_NAME_UNITS: usize = 128;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

/// The search object `XFindFirstSaveGame` returns: a 0x110-byte guest heap
/// block (tag `0x24830000`) the generated `XFindClose` frees.
pub const SEARCH_SIGNATURE: u32 = 0x5342_5645;
pub const SEARCH_SIZE: u32 = 0x110;
pub const SEARCH_HEAP_TAG: u32 = 0x2483_0000;
pub const SEARCH_OFFSET_HANDLE: u32 = 4;
pub const SEARCH_OFFSET_ROOT_LEN: u32 = 8;
pub const SEARCH_OFFSET_ROOT: u32 = 0xc;
/// Longest root a search keeps; the cursor lives in the block's last word.
pub const SEARCH_MAX_ROOT: usize = 0xf0;
pub const SEARCH_OFFSET_CURSOR: u32 = 0x10c;

/// The save directory name for `name`: 12 uppercase hex digits of
/// `h = (h * 0x10000 + unit) % HASH_MODULUS` over the UTF-16 units
/// (`0x3309b8`).
pub fn directory_name(name: &[u16]) -> String {
    let hash = name.iter().fold(0u64, |h, &unit| {
        ((h << 16) + u64::from(unit)) % HASH_MODULUS
    });
    format!("{hash:012X}")
}

/// `SaveMeta.xbx` as `XCreateSaveGame` writes it (`0x330f8f`): UTF-16LE with a
/// byte-order mark, `Name=<name>\r\n`, then `NoCopy=1\r\n` if `flags` bit 0.
pub fn encode_meta(name: &[u16], flags: u32) -> Vec<u8> {
    let mut units: Vec<u16> = vec![0xfeff];
    units.extend("Name=".encode_utf16());
    units.extend_from_slice(name);
    units.extend("\r\n".encode_utf16());
    if flags & FLAG_NO_COPY != 0 {
        units.extend("NoCopy=1\r\n".encode_utf16());
    }
    units.iter().flat_map(|u| u.to_le_bytes()).collect()
}

/// The value of `key` in a metadata file, at most `MAX_NAME_UNITS` units.
/// The original discards the first two bytes without checking them and
/// reads the rest as UTF-16LE `key=value` lines ended by `\r\n`. It finds
/// the key with a substring search; this takes the first line whose key
/// matches exactly, which is what every writer produces.
pub fn parse_meta(bytes: &[u8], key: &str) -> Option<Vec<u16>> {
    let body = bytes.get(2..)?;
    let units: Vec<u16> = body
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    let key: Vec<u16> = key.encode_utf16().collect();
    for line in units.split(|&u| u == u16::from(b'\n')) {
        let line = line.strip_suffix(&[u16::from(b'\r')]).unwrap_or(line);
        if let Some(value) = line
            .strip_prefix(key.as_slice())
            .and_then(|rest| rest.strip_prefix(&[u16::from(b'=')]))
        {
            return Some(value[..value.len().min(MAX_NAME_UNITS)].to_vec());
        }
    }
    None
}

/// One subdirectory of a save root, in the listing's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveDirectory {
    pub name: String,
    /// Windows FILETIME ticks.
    pub creation_time: u64,
    pub access_time: u64,
    pub write_time: u64,
}

/// The file operations the save family needs. Paths are guest DOS paths
/// (`U:\0123456789AB\SaveMeta.xbx`); errors are Win32 codes. The VFS
/// implementation is [`crate::VfsStorage`].
pub trait SaveStore {
    /// Fails with 183 if it exists, 3 if the parent is missing.
    fn create_directory(&mut self, path: &str) -> Result<()>;
    /// At most `limit` bytes of an existing file; 2 if missing.
    fn read_file(&mut self, path: &str, limit: usize) -> Result<Vec<u8>>;
    /// Create a new file with exactly these bytes; fails with 80 or 183 if it
    /// exists.
    fn create_file(&mut self, path: &str, contents: &[u8]) -> Result<()>;
    /// Set a directory's last-write time to now.
    fn touch_directory(&mut self, path: &str) -> Result<()>;
    /// Remove a directory and everything under it.
    fn remove_tree(&mut self, path: &str) -> Result<()>;
    /// The subdirectories of `path`, sorted case-insensitively by name.
    fn list_directories(&mut self, path: &str) -> Result<Vec<SaveDirectory>>;
}

/// What `XFindFirstSaveGame` leaves for the host after a hit. The block is
/// allocated in guest memory by the guest's own allocator (`register`), so the
/// generated `XFindClose` frees it; [`PendingSearch::object`] is its content.
/// Its [`SEARCH_OFFSET_HANDLE`] word is 0: `XFindClose` passes it to
/// CloseHandle, and 0 is never a live handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSearch {
    root: String,
    cursor: u32,
}

impl PendingSearch {
    pub fn object(&self) -> [u8; SEARCH_SIZE as usize] {
        let mut block = [0u8; SEARCH_SIZE as usize];
        block[..4].copy_from_slice(&SEARCH_SIGNATURE.to_le_bytes());
        let at = SEARCH_OFFSET_ROOT_LEN as usize;
        block[at..at + 4].copy_from_slice(&(self.root.len() as u32).to_le_bytes());
        let at = SEARCH_OFFSET_ROOT as usize;
        block[at..at + self.root.len()].copy_from_slice(self.root.as_bytes());
        let at = SEARCH_OFFSET_CURSOR as usize;
        block[at..at + 4].copy_from_slice(&self.cursor.to_le_bytes());
        block
    }
}

/// NUL-terminated UTF-16 string of at most `limit` units.
fn wide(memory: &impl GuestMemory, address: u32, limit: usize) -> Result<Vec<u16>> {
    if address == 0 {
        return Err(Error::INVALID_PARAMETER);
    }
    let mut units = Vec::new();
    for index in 0..=limit {
        let unit = u16::from_le_bytes(memory::read(memory, memory::offset(address, index * 2)?)?);
        if unit == 0 {
            return Ok(units);
        }
        units.push(unit);
    }
    Err(Error::INVALID_PARAMETER)
}

fn store<S: Storage>(storage: &mut S) -> Result<&mut dyn SaveStore> {
    storage.saves().ok_or(Error::NOT_SUPPORTED)
}

fn root_and_directory(root: &str, name: &[u16]) -> Result<String> {
    if root.is_empty()
        || name.len() > MAX_NAME_UNITS
        || root.len() + DIRECTORY_NAME_LEN >= MAX_PATH_LEN
    {
        return Err(Error::INVALID_PARAMETER);
    }
    Ok(format!("{root}{}", directory_name(name)))
}

/// `Ok` if `directory` holds a `SaveMeta.xbx` whose `Name` is `name`
/// (`0x330d16`): else the open error, or `ERROR_NO_MATCH`.
fn check_name(store: &mut dyn SaveStore, directory: &str, name: &[u16]) -> Result<()> {
    let bytes = store.read_file(&format!("{directory}\\{SAVE_META}"), 0x1000)?;
    match parse_meta(&bytes, "Name") {
        Some(found) if found == name => Ok(()),
        _ => Err(Error::Win32(ERROR_NO_MATCH)),
    }
}

/// One `XGAME_FIND_DATA` for `entry` under `root`, if its `SaveMeta.xbx`
/// names it (`0x330c15`).
fn scan(store: &mut dyn SaveStore, root: &str, entry: &SaveDirectory) -> Option<Vec<u8>> {
    if entry.name.len() + root.len() >= MAX_PATH_LEN || entry.name.len() >= 260 {
        return None;
    }
    let path = format!("{root}{}\\", entry.name);
    let bytes = store
        .read_file(&format!("{path}{SAVE_META}"), 0x1000)
        .ok()?;
    let name = parse_meta(&bytes, "Name")?;
    let mut data = vec![0u8; FIND_DATA_SIZE];
    data[FIND_DATA_ATTRIBUTES..][..4].copy_from_slice(&FILE_ATTRIBUTE_DIRECTORY.to_le_bytes());
    data[FIND_DATA_CREATION_TIME..][..8].copy_from_slice(&entry.creation_time.to_le_bytes());
    data[FIND_DATA_ACCESS_TIME..][..8].copy_from_slice(&entry.access_time.to_le_bytes());
    data[FIND_DATA_WRITE_TIME..][..8].copy_from_slice(&entry.write_time.to_le_bytes());
    data[FIND_DATA_FILE_NAME..][..entry.name.len()].copy_from_slice(entry.name.as_bytes());
    data[FIND_DATA_PATH..][..path.len()].copy_from_slice(path.as_bytes());
    for (index, unit) in name.iter().enumerate() {
        data[FIND_DATA_NAME + index * 2..][..2].copy_from_slice(&unit.to_le_bytes());
    }
    Some(data)
}

impl<S: Storage> Xapi<S> {
    /// `XCreateSaveGame(root, name, disposition, flags, path, path_len)`.
    pub(crate) fn create_save_game(
        &mut self,
        memory: &mut impl GuestMemory,
        arguments: [u32; 6],
    ) -> Result<()> {
        let [root, name, disposition, flags, out, out_len, ..] = arguments;
        let root = memory::ansi(memory, root, MAX_PATH_LEN)?;
        let name = wide(memory, name, MAX_NAME_UNITS)?;
        let directory = root_and_directory(&root, &name)?;
        let store = store(&mut self.storage)?;
        if disposition == OPEN_EXISTING {
            check_name(store, &directory, &name)?;
        } else {
            match store.create_directory(&directory) {
                Err(Error::Win32(ERROR_ALREADY_EXISTS)) => {
                    // An existing directory of another name is a hash
                    // collision or foreign data: not ours to reuse.
                    if check_name(store, &directory, &name).is_err() {
                        return Err(Error::Win32(ERROR_CANNOT_MAKE));
                    }
                    store.touch_directory(&directory)?;
                    if disposition == CREATE_NEW {
                        return Err(Error::Win32(ERROR_ALREADY_EXISTS));
                    }
                }
                Err(error) => return Err(error),
                Ok(()) => {
                    let meta = format!("{directory}\\{SAVE_META}");
                    if let Err(error) = store.create_file(&meta, &encode_meta(&name, flags)) {
                        // Roll the half-made save back (`0x331042`).
                        let _ = store.remove_tree(&directory);
                        return Err(error);
                    }
                }
            }
        }
        if out != 0 && out_len != 0 {
            let mut path = directory.into_bytes();
            let limit = out_len as usize;
            if path.len() + 1 < limit {
                path.extend_from_slice(b"\\\0");
            } else {
                path.truncate(limit);
            }
            memory.write(out, &path)?;
        }
        Ok(())
    }

    /// `XDeleteSaveGame(root, name)`.
    pub(crate) fn delete_save_game(
        &mut self,
        memory: &mut impl GuestMemory,
        root: u32,
        name: u32,
    ) -> Result<()> {
        let root = memory::ansi(memory, root, MAX_PATH_LEN)?;
        let name = wide(memory, name, MAX_NAME_UNITS)?;
        let directory = root_and_directory(&root, &name)?;
        let store = store(&mut self.storage)?;
        check_name(store, &directory, &name)?;
        store.remove_tree(&directory)
    }

    /// `XFindFirstSaveGame(root, data)`: on a hit fill `data` and leave the
    /// search for [`Xapi::take_pending_search`]; else error 18.
    pub(crate) fn find_first_save_game(
        &mut self,
        memory: &mut impl GuestMemory,
        root: u32,
        data: u32,
    ) -> Result<()> {
        let root = memory::ansi(memory, root, SEARCH_MAX_ROOT)?;
        let (cursor, block) = Self::next_save(store(&mut self.storage)?, &root, 0)?;
        memory.write(data, &block)?;
        self.pending_search = Some(PendingSearch { root, cursor });
        Ok(())
    }

    /// `XFindNextSaveGame(search, data)`.
    pub(crate) fn find_next_save_game(
        &mut self,
        memory: &mut impl GuestMemory,
        search: u32,
        data: u32,
    ) -> Result<()> {
        if memory::u32_at(memory, search)? != SEARCH_SIGNATURE {
            return Err(Error::Win32(ERROR_INVALID_HANDLE));
        }
        let cursor_at = memory::offset(search, SEARCH_OFFSET_CURSOR as usize)?;
        let cursor = memory::u32_at(memory, cursor_at)?;
        let root = memory::ansi(
            memory,
            memory::offset(search, SEARCH_OFFSET_ROOT as usize)?,
            SEARCH_MAX_ROOT,
        )?;
        let (next, block) = Self::next_save(store(&mut self.storage)?, &root, cursor)?;
        memory.write(data, &block)?;
        memory.write(cursor_at, &next.to_le_bytes())
    }

    /// The first valid save at or after listing index `from`, and the index
    /// after it. Error 18 if none.
    fn next_save(store: &mut dyn SaveStore, root: &str, from: u32) -> Result<(u32, Vec<u8>)> {
        let entries = store.list_directories(root)?;
        for (index, entry) in entries.iter().enumerate().skip(from as usize) {
            if let Some(data) = scan(store, root, entry) {
                return Ok((index as u32 + 1, data));
            }
        }
        Err(Error::Win32(ERROR_NO_MORE_FILES))
    }

    /// The search `XFindFirstSaveGame` just found, once. The host allocates
    /// its block and writes [`PendingSearch::object`] into it.
    pub fn take_pending_search(&mut self) -> Option<PendingSearch> {
        self.pending_search.take()
    }
}
