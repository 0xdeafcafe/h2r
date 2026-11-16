//! Saved variants: the file the game writes into an XAPI save directory,
//! its signature, the save's name, and the store of saves on the host disk.
//!
//! docs/variants.md, "Saved variants", has the evidence. In short:
//!
//! - A save is a directory `U:\<12 hex digits>\` named by a hash of the
//!   save name, with `SaveMeta.xbx` naming it (XAPI's format, docs/xapi.md).
//! - The save name is `"<type label>: <variant name>"`, the label compiled
//!   into the XBE per language ([`XbeNames`]).
//! - The payload is one file named for the content type: the 0x130-byte
//!   record and a 20-byte HMAC-SHA1 of it keyed with `XboxSignatureKey`.
//!   Our runtime leaves that key all zero ([`RUNTIME_KEY`]); **unverified**
//!   against a save the game wrote.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::record::{self, GameType, GameVariant, RECORD_SIZE};
use crate::sha1::hmac_sha1;

pub const SIGNATURE_SIZE: usize = 20;
pub const FILE_SIZE: usize = RECORD_SIZE + SIGNATURE_SIZE;
pub const SAVE_META: &str = "SaveMeta.xbx";
/// Halo 2's title id, the `UDATA` directory name.
pub const TITLE_ID: &str = "4D530064";
/// The longest save name the game builds, in UTF-16 units (`0x216da0`
/// caps its buffer at 0x80 units including the NUL).
pub const MAX_SAVE_NAME_UNITS: usize = 0x7f;
/// The separator between the type label and the variant name (`0x470054`).
pub const LABEL_SEPARATOR: &str = ": ";
/// `XboxSignatureKey` as our kernel leaves it: export 325 is never filled
/// (crates/kernel/STOPS.md), so it reads as zeros. Unverified against a
/// save written by the game under our runtime.
pub const RUNTIME_KEY: [u8; 16] = [0; 16];
/// Content types (`0x216b60` has one file name for each of 0..=10).
pub const CONTENT_TYPES: usize = 11;

/// The largest prime below 2^48, the modulus of XAPI's save-name hash.
const HASH_MODULUS: u64 = 0x0000_ffff_ffff_ffc5;

/// The save directory for a save name: 12 uppercase hex digits of
/// `h = (h * 0x10000 + unit) % HASH_MODULUS` over the UTF-16 units
/// (XAPI `0x3309b8`, docs/xapi.md).
pub fn directory_name(save_name: &str) -> String {
    let h = save_name
        .encode_utf16()
        .fold(0u64, |h, u| ((h << 16) + u64::from(u)) % HASH_MODULUS);
    format!("{h:012X}")
}

/// `SaveMeta.xbx` as XCreateSaveGame writes it with flags 0: UTF-16LE with
/// a byte-order mark and `Name=<name>` CR LF.
pub fn encode_meta(save_name: &str) -> Vec<u8> {
    std::iter::once(0xfeff)
        .chain("Name=".encode_utf16())
        .chain(save_name.encode_utf16())
        .chain("\r\n".encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// The `Name` of a `SaveMeta.xbx` (the reader skips the first two bytes and
/// reads `key=value` lines).
pub fn parse_meta_name(bytes: &[u8]) -> Option<String> {
    let units: Vec<u16> = bytes
        .get(2..)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let key: Vec<u16> = "Name=".encode_utf16().collect();
    units.split(|u| *u == u16::from(b'\n')).find_map(|line| {
        let line = line.strip_suffix(&[u16::from(b'\r')]).unwrap_or(line);
        line.strip_prefix(key.as_slice())
            .map(String::from_utf16_lossy)
    })
}

/// The save name the game gives a variant: `"<label>: <name>"`, cut to
/// [`MAX_SAVE_NAME_UNITS`].
pub fn save_name(label: &str, variant_name: &str) -> String {
    let units: Vec<u16> = format!("{label}{LABEL_SEPARATOR}{variant_name}")
        .encode_utf16()
        .take(MAX_SAVE_NAME_UNITS)
        .collect();
    String::from_utf16_lossy(&units)
}

/// The signature of a record under `key`.
pub fn sign(record: &[u8; RECORD_SIZE], key: &[u8; 16]) -> [u8; SIGNATURE_SIZE] {
    hmac_sha1(key, record)
}

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("a saved variant file is at least {FILE_SIZE:#x} bytes, got {0:#x}")]
    Short(usize),
    #[error("the game would reject this variant: {0}")]
    Rejected(String),
    #[error("{0} is not a game type the game can save")]
    GameType(GameType),
    #[error("naming a new save needs the XBE (the game's type labels and file names)")]
    NoNames,
    #[error("a save named {0:?} already exists")]
    Exists(String),
    #[error("{0} is not a saved variant")]
    NotAVariant(PathBuf),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// A decoded saved-variant file.
#[derive(Debug, Clone, PartialEq)]
pub struct VariantFile {
    pub variant: GameVariant,
    pub signature: [u8; SIGNATURE_SIZE],
    /// The signature matches under the key it was decoded with.
    pub signed: bool,
    /// Bytes after the signature (the game ignores them; kept on rewrite).
    pub trailing: Vec<u8>,
}

/// The file the game writes: record, then its signature.
pub fn encode_file(v: &GameVariant, key: &[u8; 16]) -> Vec<u8> {
    let r = v.to_bytes();
    let mut out = r.to_vec();
    out.extend_from_slice(&sign(&r, key));
    out
}

pub fn decode_file(b: &[u8], key: &[u8; 16]) -> Result<VariantFile, SaveError> {
    if b.len() < FILE_SIZE {
        return Err(SaveError::Short(b.len()));
    }
    let variant = GameVariant::from_bytes(&b[..RECORD_SIZE]).expect("length checked");
    let signature: [u8; SIGNATURE_SIZE] = b[RECORD_SIZE..FILE_SIZE].try_into().unwrap();
    Ok(VariantFile {
        signed: sign(variant.bytes(), key) == signature,
        variant,
        signature,
        trailing: b[FILE_SIZE..].to_vec(),
    })
}

// ---- names from the XBE ----

/// Builds an 11-entry table on its stack: `sub esp,2Ch` then
/// `mov [esp+4k], imm32` for each content type. Returns the file names.
const FILE_NAMES_FN: u32 = 0x0021_6b60;
/// The per-content-type label functions behind `0x217a80`, by content
/// type: each is `dec eax; cmp eax,7; ja default; jmp [eax*4+table]` with
/// `mov eax, string; ret` stubs, EAX the language index.
const LABEL_FNS: [u32; CONTENT_TYPES] = [
    0x0021_7620,
    0x0021_7680,
    0x0021_76e0,
    0x0021_7740,
    0x0021_77a0,
    0x0021_77f0,
    0x0021_7840,
    0x0021_78a0,
    0x0021_7900,
    0x0021_7960,
    0x0021_79c0,
];

/// The game's language index for the labels (`0x11ca80` over
/// XGetLanguage): English 0 ... Chinese 7; Portuguese uses English.
pub fn language_index(l: tags::Language) -> u32 {
    use tags::Language::*;
    match l {
        English | Portuguese => 0,
        Japanese => 1,
        German => 2,
        French => 3,
        Spanish => 4,
        Italian => 5,
        Korean => 6,
        Chinese => 7,
    }
}

/// The content types' file names and labels, read from the user's XBE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XbeNames {
    pub file_names: Vec<String>,
    pub labels: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum NamesError {
    #[error(transparent)]
    Xbe(#[from] xbe::Error),
    #[error("the code at {0:#x} is not what this XBE reader expects")]
    Shape(u32),
}

impl XbeNames {
    pub fn read(x: &xbe::Xbe, language: tags::Language) -> Result<Self, NamesError> {
        let lang = language_index(language);
        let mut file_names = vec![String::new(); CONTENT_TYPES];
        // `sub`, `test`, `mov eax`, then 11 stores of 7 or 8 bytes.
        let code = x.bytes_at(FILE_NAMES_FN, 0x68)?;
        if code[..3] != [0x83, 0xec, 0x2c] {
            return Err(NamesError::Shape(FILE_NAMES_FN));
        }
        let mut i = 3;
        while i + 7 <= code.len() {
            let (slot, imm_at) = match code[i..] {
                [0xc7, 0x04, 0x24, ..] => (0, i + 3),
                [0xc7, 0x44, 0x24, d, ..] => (usize::from(d) / 4, i + 4),
                _ => {
                    i += 1;
                    continue;
                }
            };
            let Some(imm) = code.get(imm_at..imm_at + 4) else {
                break;
            };
            let addr = u32::from_le_bytes(imm.try_into().unwrap());
            if let Some(n) = file_names.get_mut(slot) {
                *n = ascii_at(x, addr)?;
            }
            i = imm_at + 4;
        }
        if file_names.iter().any(String::is_empty) {
            return Err(NamesError::Shape(FILE_NAMES_FN));
        }
        let labels = LABEL_FNS
            .iter()
            .map(|f| label(x, *f, lang))
            .collect::<Result<_, _>>()?;
        Ok(XbeNames { file_names, labels })
    }

    pub fn file_name(&self, ty: GameType) -> Option<&str> {
        Some(self.file_names.get(ty.content_type()? as usize)?.as_str())
    }

    pub fn label(&self, ty: GameType) -> Option<&str> {
        Some(self.labels.get(ty.content_type()? as usize)?.as_str())
    }

    /// The game type whose file this is.
    pub fn game_type_of(&self, file: &str) -> Option<GameType> {
        let t = self
            .file_names
            .iter()
            .position(|n| n.eq_ignore_ascii_case(file))?;
        GameType::from_content_type(t as u32)
    }

    /// The variant name of a save name: the save name without a known
    /// label prefix (as the listing does, `0x216e50`).
    pub fn strip_label<'a>(&self, save_name: &'a str) -> &'a str {
        self.labels
            .iter()
            .find_map(|l| {
                save_name
                    .strip_prefix(l.as_str())?
                    .strip_prefix(LABEL_SEPARATOR)
            })
            .unwrap_or(save_name)
    }
}

fn ascii_at(x: &xbe::Xbe, addr: u32) -> Result<String, NamesError> {
    let b = x.bytes_at(addr, 32)?;
    let end = b
        .iter()
        .position(|c| *c == 0)
        .ok_or(NamesError::Shape(addr))?;
    Ok(String::from_utf8_lossy(&b[..end]).into_owned())
}

fn utf16_at(x: &xbe::Xbe, addr: u32) -> Result<String, NamesError> {
    let b = x.bytes_at(addr, 128)?;
    let units: Vec<u16> = b
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|u| *u != 0)
        .collect();
    Ok(String::from_utf16_lossy(&units))
}

/// `mov eax, imm32` at `at`.
fn mov_eax(x: &xbe::Xbe, at: u32) -> Result<u32, NamesError> {
    let b = x.bytes_at(at, 5)?;
    if b[0] != 0xb8 {
        return Err(NamesError::Shape(at));
    }
    Ok(u32::from_le_bytes(b[1..5].try_into().unwrap()))
}

fn label(x: &xbe::Xbe, f: u32, lang: u32) -> Result<String, NamesError> {
    let b = x.bytes_at(f, 13)?;
    // dec eax; cmp eax,7; ja rel8; jmp [eax*4+table]
    if b[0] != 0x48
        || b[1..4] != [0x83, 0xf8, 0x07]
        || b[4] != 0x77
        || b[6..9] != [0xff, 0x24, 0x85]
    {
        return Err(NamesError::Shape(f));
    }
    let default = f + 6 + u32::from(b[5]);
    let table = u32::from_le_bytes(b[9..13].try_into().unwrap());
    let stub = match lang.wrapping_sub(1) {
        k @ 0..=7 => x.read_u32(table + 4 * k)?,
        _ => default,
    };
    utf16_at(x, mov_eax(x, stub)?)
}

// ---- the store ----

/// One saved variant on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedVariant {
    /// The save directory.
    pub dir: PathBuf,
    /// The payload file's name.
    pub file: String,
    /// `SaveMeta.xbx`'s `Name`.
    pub save_name: String,
    pub contents: VariantFile,
}

/// The saved variants of one `UDATA\4D530064` directory.
#[derive(Debug, Clone)]
pub struct SaveStore {
    pub root: PathBuf,
    /// The signing key ([`RUNTIME_KEY`] for our runtime).
    pub key: [u8; 16],
    /// The game's names, needed to name new saves.
    pub names: Option<XbeNames>,
}

impl SaveStore {
    pub fn new(root: impl Into<PathBuf>, names: Option<XbeNames>) -> Self {
        SaveStore {
            root: root.into(),
            key: RUNTIME_KEY,
            names,
        }
    }

    /// `<data root>/disk/Partition1/UDATA/4D530064`, the data root being
    /// `~/Library/Application Support/h2recomp` (the VFS default,
    /// docs/storage.md).
    pub fn default_root() -> Option<PathBuf> {
        let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
        Some(
            Path::new(&home)
                .join("Library/Application Support/h2recomp/disk/Partition1/UDATA")
                .join(TITLE_ID),
        )
    }

    /// Every save directory holding a variant, sorted by save name. A
    /// directory counts when it has a `SaveMeta.xbx` with a `Name` and a
    /// payload file: one of the variant file names when the names are
    /// known, else any other file of at least [`FILE_SIZE`] bytes whose
    /// record names a playable game type.
    pub fn list(&self) -> io::Result<Vec<SavedVariant>> {
        let mut out = Vec::new();
        let rd = match fs::read_dir(&self.root) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e),
        };
        for e in rd {
            let e = e?;
            if !e.file_type()?.is_dir() {
                continue;
            }
            if let Ok(s) = self.read(&e.path()) {
                out.push(s);
            }
        }
        out.sort_by(|a, b| a.save_name.cmp(&b.save_name));
        Ok(out)
    }

    /// The saved variant in one save directory.
    pub fn read(&self, dir: &Path) -> Result<SavedVariant, SaveError> {
        let not = || SaveError::NotAVariant(dir.to_path_buf());
        let meta = fs::read(dir.join(SAVE_META)).map_err(|_| not())?;
        let save_name = parse_meta_name(&meta).ok_or_else(not)?;
        let mut files: Vec<_> = fs::read_dir(dir)?
            .filter_map(Result::ok)
            .filter(|f| f.file_type().is_ok_and(|t| t.is_file()))
            .map(|f| f.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.eq_ignore_ascii_case(SAVE_META))
            .collect();
        files.sort();
        for file in files {
            let known = match &self.names {
                Some(n) => n.game_type_of(&file).is_some(),
                None => true,
            };
            if !known {
                continue;
            }
            let bytes = fs::read(dir.join(&file))?;
            let Ok(contents) = decode_file(&bytes, &self.key) else {
                continue;
            };
            let ty = contents.variant.game_type();
            if ty.content_type().is_none() {
                continue;
            }
            if let Some(n) = &self.names
                && n.game_type_of(&file) != Some(ty)
            {
                continue;
            }
            return Ok(SavedVariant {
                dir: dir.to_path_buf(),
                file,
                save_name,
                contents,
            });
        }
        Err(not())
    }

    /// Writes `v` as a new save, named as the game names it. Fails if a
    /// save of that name exists (the game's CREATE_NEW).
    pub fn create(&self, v: &GameVariant) -> Result<SavedVariant, SaveError> {
        check_saveable(v)?;
        let names = self.names.as_ref().ok_or(SaveError::NoNames)?;
        let ty = v.game_type();
        let (label, file) = names
            .label(ty)
            .zip(names.file_name(ty))
            .ok_or(SaveError::GameType(ty))?;
        let save_name = save_name(label, &v.name());
        let dir = self.root.join(directory_name(&save_name));
        if dir.join(SAVE_META).exists() {
            return Err(SaveError::Exists(save_name));
        }
        fs::create_dir_all(&dir)?;
        let result = (|| {
            fs::write(dir.join(SAVE_META), encode_meta(&save_name))?;
            fs::write(dir.join(file), encode_file(v, &self.key))
        })();
        if let Err(e) = result {
            let _ = fs::remove_dir_all(&dir);
            return Err(e.into());
        }
        self.read(&dir)
    }

    /// Replaces a save's variant. The same name keeps the directory and
    /// rewrites the payload (keeping bytes after the signature); a new name
    /// or type writes a new save and removes the old one, as the save name
    /// (and so the directory) follows the variant name.
    pub fn update(&self, old: &SavedVariant, v: &GameVariant) -> Result<SavedVariant, SaveError> {
        check_saveable(v)?;
        let same_type = old.contents.variant.game_type() == v.game_type();
        let same_name = old.contents.variant.name() == v.name();
        if same_type && same_name {
            let mut bytes = encode_file(v, &self.key);
            bytes.extend_from_slice(&old.contents.trailing);
            fs::write(old.dir.join(&old.file), bytes)?;
            return self.read(&old.dir);
        }
        let new = self.create(v)?;
        if new.dir != old.dir {
            fs::remove_dir_all(&old.dir)?;
        }
        Ok(new)
    }

    pub fn delete(&self, s: &SavedVariant) -> Result<(), SaveError> {
        Ok(fs::remove_dir_all(&s.dir)?)
    }
}

/// The game cleans a record up before it writes it (`0x212ec0`); refuse
/// what the clean-up would change, since the game's load then fails.
fn check_saveable(v: &GameVariant) -> Result<(), SaveError> {
    let p = record::check(v);
    if p.is_empty() {
        Ok(())
    } else {
        Err(SaveError::Rejected(
            p.iter()
                .map(|p| format!("+{:#x} {}", p.offset, p.what))
                .collect::<Vec<_>>()
                .join("; "),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::SettingId;

    fn names() -> XbeNames {
        // Invented names: the real ones come from the user's XBE.
        XbeNames {
            file_names: (0..CONTENT_TYPES).map(|i| format!("type{i}")).collect(),
            labels: (0..CONTENT_TYPES).map(|i| format!("Label {i}")).collect(),
        }
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("variants-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn slayer(name: &str) -> GameVariant {
        let mut v = GameVariant::blank(GameType::Slayer);
        v.set_name(name).unwrap();
        v.set(SettingId(2), 25).unwrap();
        v
    }

    #[test]
    fn directory_name_is_the_xapi_hash() {
        assert_eq!(directory_name(""), "000000000000");
        assert_eq!(directory_name("A"), "000000000041");
        assert_eq!(directory_name("AB"), "000000410042");
        // Four units overflow 48 bits: the modulus applies.
        let h = [0x41u64, 0x42, 0x43, 0x44]
            .iter()
            .fold(0u64, |h, u| ((h << 16) + u) % HASH_MODULUS);
        assert_eq!(directory_name("ABCD"), format!("{h:012X}"));
        assert_ne!(h, 0x0041_0042_0043_0044 & 0xffff_ffff_ffff);
    }

    #[test]
    fn meta_round_trips() {
        let m = encode_meta("Label 1: Zombies");
        assert_eq!(&m[..2], &[0xff, 0xfe]);
        assert_eq!(
            &m[2..12],
            "Name="
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
                .as_slice()
        );
        assert!(m.ends_with(&[b'\r', 0, b'\n', 0]));
        assert_eq!(parse_meta_name(&m).as_deref(), Some("Label 1: Zombies"));
        assert_eq!(parse_meta_name(&[0xff, 0xfe]), None);
    }

    #[test]
    fn save_names_carry_the_label_and_are_bounded() {
        assert_eq!(save_name("King", "Sun King"), "King: Sun King");
        assert_eq!(
            save_name(&"L".repeat(120), "abcdefghij")
                .encode_utf16()
                .count(),
            MAX_SAVE_NAME_UNITS
        );
        let n = names();
        assert_eq!(n.strip_label("Label 2: Hill"), "Hill");
        assert_eq!(n.strip_label("Hill"), "Hill");
    }

    #[test]
    fn file_is_record_then_hmac() {
        let v = slayer("Duel");
        let f = encode_file(&v, &RUNTIME_KEY);
        assert_eq!(f.len(), FILE_SIZE);
        assert_eq!(&f[..RECORD_SIZE], v.bytes());
        assert_eq!(&f[RECORD_SIZE..], &hmac_sha1(&[0; 16], v.bytes()));
        let d = decode_file(&f, &RUNTIME_KEY).unwrap();
        assert!(d.signed && d.trailing.is_empty() && d.variant == v);
        // Another key, or a changed byte, fails the signature.
        assert!(!decode_file(&f, &[1; 16]).unwrap().signed);
        let mut bad = f.clone();
        bad[0x50] ^= 1;
        assert!(!decode_file(&bad, &RUNTIME_KEY).unwrap().signed);
        let mut long = f.clone();
        long.extend_from_slice(b"xyz");
        assert_eq!(decode_file(&long, &RUNTIME_KEY).unwrap().trailing, b"xyz");
        assert!(matches!(
            decode_file(&f[..FILE_SIZE - 1], &RUNTIME_KEY),
            Err(SaveError::Short(_))
        ));
    }

    #[test]
    fn store_creates_lists_updates_and_deletes() {
        let root = temp("store");
        let store = SaveStore::new(&root, Some(names()));
        let a = store.create(&slayer("Duel")).unwrap();
        assert_eq!(a.save_name, "Label 1: Duel");
        assert_eq!(a.file, "type1");
        assert_eq!(a.dir, root.join(directory_name("Label 1: Duel")));
        assert!(a.contents.signed);
        assert!(matches!(
            store.create(&slayer("Duel")),
            Err(SaveError::Exists(_))
        ));

        // Junk next to it is ignored.
        fs::create_dir_all(root.join("JUNK")).unwrap();
        fs::write(root.join("JUNK").join(SAVE_META), encode_meta("x")).unwrap();
        fs::write(root.join("JUNK").join("type1"), [0u8; 4]).unwrap();
        assert_eq!(store.list().unwrap(), vec![a.clone()]);

        // Same name: rewritten in place, trailing bytes kept.
        let mut path = a.dir.join(&a.file);
        let mut bytes = fs::read(&path).unwrap();
        bytes.push(7);
        fs::write(&path, bytes).unwrap();
        let a = store.read(&a.dir).unwrap();
        let mut v = a.contents.variant.clone();
        v.set(SettingId(2), 10).unwrap();
        let b = store.update(&a, &v).unwrap();
        assert_eq!(b.dir, a.dir);
        assert_eq!(b.contents.trailing, vec![7]);
        assert_eq!(b.contents.variant.get(SettingId(2)), Some(10));

        // Renamed: a new directory, the old one gone.
        v.set_name("Duel 2").unwrap();
        let c = store.update(&b, &v).unwrap();
        assert_ne!(c.dir, b.dir);
        assert!(!b.dir.exists());
        path = c.dir.join(SAVE_META);
        assert_eq!(
            parse_meta_name(&fs::read(path).unwrap()).as_deref(),
            Some("Label 1: Duel 2")
        );

        // A record the game would clean up is refused.
        let mut bad = v.clone();
        bad.set(SettingId(19), 3).unwrap();
        assert!(matches!(
            store.update(&c, &bad),
            Err(SaveError::Rejected(_))
        ));

        store.delete(&c).unwrap();
        assert!(store.list().unwrap().is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn without_names_saves_list_but_cannot_be_created() {
        let root = temp("nonames");
        let named = SaveStore::new(&root, Some(names()));
        named.create(&slayer("Duel")).unwrap();
        let plain = SaveStore::new(&root, None);
        let l = plain.list().unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].contents.variant.name(), "Duel");
        assert!(matches!(
            plain.create(&slayer("Other")),
            Err(SaveError::NoNames)
        ));
        assert!(
            SaveStore::new(root.join("missing"), None)
                .list()
                .unwrap()
                .is_empty()
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
