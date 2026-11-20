//! The hopper tool's editing model, without any UI: a working copy of the
//! 16 hoppers in which every record remembers the slot its bytes come from,
//! the edits the playlist builder offers, the validation the Live server
//! applies, and [`apply`], which writes a working copy back into the image.
//!
//! This is the web builder's model (`web/src/admin/validate.ts`) applied
//! locally, the way the server's `apply_patch`
//! (`crates/live/live-server/src/admin.rs`) applies its PUT body: a record's
//! whole 0x37c-byte slot (its game variant and the blobs we cannot read)
//! moves with it, so reorders, copies and additions keep every byte. Two
//! differences, both to keep bytes exact: a hopper whose working copy is
//! unchanged is not touched at all, and within a changed hopper only the
//! changed parts (records, descriptor fields, text) are rewritten; the text
//! in particular goes to all nine language slots, so it is written only
//! when it changed.
//!
//! An entry can also be given another game variant ([`set_variant`]): the
//! picked 0x130-byte record replaces the tail of the entry's slot
//! (`+0x24c..+0x37c`), and every other byte of the slot still comes from
//! its source. [`slot_bytes`] is the one place an entry's slot is built,
//! for [`apply`] and for what the panel shows.

use live_proto::hopper::defaults;
use live_proto::hopper::{
    DWORDS, HOPPERS, HopperError, HopperFile, HopperText, MAX_DESCRIPTION, MAX_NAME, MAX_RECORDS,
    MAX_STRING, MAX_TITLE, RECORD, Record, VARIANT_AT, VARIANT_SIZE, Variant, set_slot_record,
    slot_record, slot_variant,
};
use variants::GameVariant;
use variants::record::{self as vrecord, Problem};

/// Session setup clamps a record's weight (`dwords[0]`) to 1..1000.
pub const MAX_WEIGHT: u32 = 1000;

/// A slot in the image the working copy came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotRef {
    pub hopper: usize,
    pub slot: usize,
}

/// One entry: a map (`id`) with a game variant, picked with probability
/// `dwords[0]` (its weight) over the hopper's total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditRecord {
    pub id: u32,
    pub string: String,
    pub flags: u32,
    pub dwords: [u32; 2],
    /// What the entry's game variant says about itself (name, engine,
    /// team play): its source slot's, or the picked one's.
    pub variant: Option<Variant>,
    /// Where the record's whole slot comes from; `None` for a zero-filled
    /// slot (only for adding to a hopper with no records at all).
    pub src: Option<SlotRef>,
    /// A game variant picked for this entry ([`set_variant`]): it replaces
    /// the source slot's last 0x130 bytes. `None` keeps the source's.
    pub picked: Option<Box<[u8; VARIANT_SIZE]>>,
}

impl EditRecord {
    pub fn weight(&self) -> u32 {
        self.dwords[0]
    }

    fn fields(&self) -> Record {
        Record {
            id: self.id,
            string: self.string.clone(),
            flags: self.flags,
            dwords: self.dwords,
        }
    }
}

/// The 0x37c bytes `r`'s slot gets when the working copy is written to
/// `base`: its source slot (zeros without one), the picked game variant
/// over the tail, then the editable fields, written the way
/// [`HopperFile::set_record`] writes them (the raw string only if it
/// changed).
pub fn slot_bytes(base: &HopperFile, r: &EditRecord) -> Result<Vec<u8>, HopperError> {
    let mut slot = match r.src {
        Some(src) => base.slot(src.hopper, src.slot)?.to_vec(),
        None => vec![0; RECORD],
    };
    if let Some(v) = &r.picked {
        slot[VARIANT_AT..].copy_from_slice(&v[..]);
    }
    set_slot_record(&mut slot, &r.fields())?;
    Ok(slot)
}

/// The game variant `r`'s slot will carry, decoded (name, game type and
/// every setting), or `None` if the slot cannot be built.
pub fn entry_variant(base: &HopperFile, r: &EditRecord) -> Option<GameVariant> {
    let slot = slot_bytes(base, r).ok()?;
    GameVariant::from_bytes(&slot[VARIANT_AT..]).ok()
}

/// Why [`set_variant`] refused a variant: the game's clean-up would
/// replace it (docs/editor/variants.md, "The clean-up").
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("the game would reject this variant: {}", describe(.0))]
pub struct Rejected(pub Vec<Problem>);

fn describe(problems: &[Problem]) -> String {
    problems
        .iter()
        .map(|p| format!("+{:#x}: {}", p.offset, p.what))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Links `v` to entry `r`: its record (`GameVariant::to_bytes`) becomes the
/// slot's last 0x130 bytes. Refuses a record the game's clean-up would
/// change, or one it keeps but cannot play (`variants::record::ours`).
/// The editable fields that lie inside the variant (the raw string, flags
/// and second dword) are re-read from the new bytes, so writing them back
/// leaves the variant exactly as picked; the map id and
/// weight stay.
pub fn set_variant(r: &mut EditRecord, v: &GameVariant) -> Result<(), Rejected> {
    let problems = vrecord::refused(v);
    if !problems.is_empty() {
        return Err(Rejected(problems));
    }
    let bytes = v.to_bytes();
    let mut slot = vec![0; RECORD];
    slot[VARIANT_AT..].copy_from_slice(&bytes);
    let inside = slot_record(&slot);
    r.string = inside.string;
    r.flags = inside.flags;
    r.dwords[1] = inside.dwords[1];
    r.variant = Some(slot_variant(&slot));
    r.picked = Some(Box::new(bytes));
    Ok(())
}

/// The variant an entry with a picked variant will carry, once its raw
/// fields are written over it (they lie inside the variant).
fn picked_after_fields(r: &EditRecord) -> Option<GameVariant> {
    let picked = r.picked.as_ref()?;
    let mut slot = vec![0; RECORD];
    slot[VARIANT_AT..].copy_from_slice(&picked[..]);
    set_slot_record(&mut slot, &r.fields()).ok()?;
    GameVariant::from_bytes(&slot[VARIANT_AT..]).ok()
}

/// One hopper's editable fields and its records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditHopper {
    pub index: usize,
    pub hopper_type: u32,
    pub status: u32,
    pub name: String,
    pub title: String,
    pub description: String,
    pub ranges: [u32; 3],
    pub dwords: Vec<u32>,
    pub records: Vec<EditRecord>,
    /// Make this hopper a playlist of this template's kind when written
    /// ([`create_playlist`]; a default playlist id,
    /// [`defaults::templates`]): its whole descriptor first, then the
    /// fields above.
    pub template: Option<u32>,
    /// Make this hopper an unused slot when written ([`clear_playlist`]).
    pub clear: bool,
}

/// An unused slot (type 0, which the game reads as no hopper) not being
/// made a playlist, or a playlist being cleared.
pub fn is_unused(h: &EditHopper) -> bool {
    h.clear || (h.hopper_type == 0 && h.template.is_none())
}

/// Makes `h` a playlist of the kind of the default playlist `template`:
/// its type, status 0, range fields and dwords as that template's
/// descriptor has them (the rest of the descriptor is written by
/// [`apply`]). An unused slot also takes the template's title,
/// description and first entry; a playlist keeps its text and entries.
/// False for an id that is not a template.
pub fn create_playlist(h: &mut EditHopper, template: u32) -> bool {
    let Some(t) = defaults::template(template) else {
        return false;
    };
    let mut scratch = HopperFile::blank();
    if scratch.create_from(0, &t).is_err() {
        return false;
    }
    let Ok(d) = scratch.hopper(0) else {
        return false;
    };
    let fresh = is_unused(h) || h.records.is_empty();
    if fresh {
        let Some(first) = t.entries().into_iter().next() else {
            return false;
        };
        let Some(key) = defaults::builtin_key(&first.variant) else {
            return false;
        };
        let Some(r) = builtin_record(first.map_id, key) else {
            return false;
        };
        h.title = clip(t.title, MAX_TITLE);
        h.description = clip(t.description, MAX_DESCRIPTION);
        h.records = vec![r];
    }
    h.template = Some(template);
    h.clear = false;
    h.hopper_type = d.hopper_type;
    h.status = 0;
    // The template's bytes from +0x51b read as no name; an old name would
    // be written over its validated table.
    h.name = String::new();
    h.ranges = d.ranges;
    h.dwords = d.dwords;
    true
}

/// Makes `h` an unused slot when written.
pub fn clear_playlist(h: &mut EditHopper) {
    h.clear = true;
    h.template = None;
    h.hopper_type = 0;
    h.records.clear();
}

/// An entry with built-in variant `key` ([`defaults::builtin_variants`])
/// on map `map_id`, weight 1, in a zero-filled slot.
pub fn builtin_record(map_id: u32, key: &str) -> Option<EditRecord> {
    let v = defaults::builtin_variant(key)?;
    let mut slot = vec![0; RECORD];
    slot[VARIANT_AT..].copy_from_slice(v.bytes());
    let inside = slot_record(&slot);
    Some(EditRecord {
        id: map_id,
        string: inside.string,
        flags: inside.flags,
        dwords: [1, inside.dwords[1]],
        variant: Some(slot_variant(&slot)),
        src: None,
        picked: Some(Box::new(*v.bytes())),
    })
}

/// The working copy of a file: all 16 hoppers, each record pointing at its
/// own slot.
pub fn load(file: &HopperFile) -> Vec<EditHopper> {
    (0..HOPPERS)
        .map(|i| {
            let h = file.hopper(i).expect("index in range");
            let text = file.text(i).expect("index in range");
            let count = (h.record_count as usize).min(MAX_RECORDS);
            let records = (0..count)
                .map(|j| {
                    let r = file.record(i, j).expect("slot in range");
                    EditRecord {
                        id: r.id,
                        string: r.string,
                        flags: r.flags,
                        dwords: r.dwords,
                        variant: file.variant(i, j).ok(),
                        src: Some(SlotRef { hopper: i, slot: j }),
                        picked: None,
                    }
                })
                .collect();
            EditHopper {
                index: i,
                hopper_type: h.hopper_type,
                status: h.status,
                name: h.name,
                title: text.title,
                description: text.description,
                ranges: h.ranges,
                dwords: h.dwords,
                records,
                template: None,
                clear: false,
            }
        })
        .collect()
}

/// What is wrong with this hopper (empty when it will save): the web
/// builder's `hopperErrors`, with the server's own checks.
pub fn hopper_errors(h: &EditHopper) -> Vec<String> {
    let mut errs = Vec::new();
    if h.clear {
        return errs;
    }
    if is_unused(h) {
        errs.push(
            "this slot is unused: make a playlist here (choose its kind) or undo the change"
                .to_string(),
        );
        return errs;
    }
    if !(1..=5).contains(&h.hopper_type) {
        errs.push("type must be 1 to 5".to_string());
    }
    if h.status > 2 {
        errs.push("status must be 0 (not loaded), 1 (ok) or 2 (error)".into());
    }
    if h.title.is_empty() {
        errs.push("the playlist needs a title".into());
    }
    if h.title.contains('\0') || h.description.contains('\0') {
        errs.push("the title and description cannot contain NUL".into());
    }
    if h.title.len() > MAX_TITLE {
        errs.push(format!("the title is longer than {MAX_TITLE} bytes"));
    }
    if h.description.len() > MAX_DESCRIPTION {
        errs.push(format!(
            "the description is longer than {MAX_DESCRIPTION} bytes"
        ));
    }
    if !h.name.is_ascii() {
        errs.push("the name must be ASCII".into());
    }
    if h.name.contains('\0') {
        errs.push("the name cannot contain NUL".into());
    }
    if h.name.len() > MAX_NAME {
        errs.push(format!("the name is longer than {MAX_NAME} characters"));
    }
    if !(1..=MAX_RECORDS).contains(&h.records.len()) {
        errs.push(format!(
            "1 to {MAX_RECORDS} entries, got {}",
            h.records.len()
        ));
    }
    if h.dwords.len() != DWORDS {
        errs.push(format!(
            "the dword array has {DWORDS} entries; leave it as loaded"
        ));
    }
    for (i, r) in h.records.iter().enumerate() {
        let n = i + 1;
        if !r.string.is_ascii() {
            errs.push(format!("entry {n}: the string must be ASCII"));
        }
        if r.string.contains('\0') {
            errs.push(format!("entry {n}: the string cannot contain NUL"));
        }
        if r.string.len() > MAX_STRING {
            errs.push(format!(
                "entry {n}: the string is longer than {MAX_STRING} characters"
            ));
        }
        // Only a picked variant is checked: a source slot's variant is
        // written back as it came.
        if r.picked.is_some() {
            match picked_after_fields(r) {
                Some(v) => {
                    let problems = vrecord::refused(&v);
                    if !problems.is_empty() {
                        errs.push(format!(
                            "entry {n}: the game would reject its game variant ({})",
                            describe(&problems)
                        ));
                    }
                }
                None => errs.push(format!("entry {n}: its game variant cannot be built")),
            }
        }
    }
    errs
}

/// Every hopper that differs from `baseline`, with its errors, labelled as
/// the playlist list numbers them. Unchanged hoppers are not checked: they
/// are written back exactly as they were (a blank image's empty hoppers
/// would otherwise block every save).
pub fn errors(baseline: &[EditHopper], edited: &[EditHopper]) -> Vec<String> {
    edited
        .iter()
        .filter(|h| baseline.get(h.index) != Some(*h))
        .flat_map(|h| {
            hopper_errors(h)
                .into_iter()
                .map(move |e| format!("Playlist {}: {e}", h.index + 1))
        })
        .collect()
}

/// Why [`apply`] refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ApplyError {
    #[error("{}", .0.join("; "))]
    Invalid(Vec<String>),
    #[error("the working copy has {0} hoppers, not 16")]
    Hoppers(usize),
    #[error(
        "playlist {hopper} entry {entry}: its source slot ({src:?}) is not one of the records loaded"
    )]
    Source {
        hopper: usize,
        entry: usize,
        src: SlotRef,
    },
    #[error(transparent)]
    Hopper(#[from] HopperError),
}

/// Writes `edited` (a working copy made by [`load`] from `base`, then
/// edited) into a copy of `base`. Hoppers equal to their loaded state keep
/// every byte; in a changed one, records are rebuilt from their source
/// slots, and descriptor fields and text are written only if they changed.
pub fn apply(base: &HopperFile, edited: &[EditHopper]) -> Result<HopperFile, ApplyError> {
    if edited.len() != HOPPERS || edited.iter().enumerate().any(|(i, h)| h.index != i) {
        return Err(ApplyError::Hoppers(edited.len()));
    }
    let baseline = load(base);
    let errs = errors(&baseline, edited);
    if !errs.is_empty() {
        return Err(ApplyError::Invalid(errs));
    }
    let mut out = base.clone();
    for (h, was) in edited.iter().zip(&baseline) {
        if h == was {
            continue;
        }
        let i = h.index;
        if h.clear {
            out.clear_hopper(i)?;
            continue;
        }
        if let Some(id) = h.template {
            let t = defaults::template(id).ok_or_else(|| {
                ApplyError::Invalid(vec![format!(
                    "Playlist {}: there is no playlist template {id}",
                    i + 1
                )])
            })?;
            out.create_from(i, &t)?;
        }
        if h.records != was.records {
            let mut slots = Vec::with_capacity(h.records.len());
            for (j, r) in h.records.iter().enumerate() {
                if let Some(src) = r.src
                    && !baseline
                        .get(src.hopper)
                        .is_some_and(|b| src.slot < b.records.len())
                {
                    return Err(ApplyError::Source {
                        hopper: i + 1,
                        entry: j + 1,
                        src,
                    });
                }
                slots.push(slot_bytes(base, r)?);
            }
            for (j, bytes) in slots.iter().enumerate() {
                out.set_slot(i, j, bytes)?;
            }
            out.set_record_count(i, h.records.len() as u32)?;
        }
        let fields_changed = h.hopper_type != was.hopper_type
            || h.status != was.status
            || h.name != was.name
            || h.ranges != was.ranges
            || h.dwords != was.dwords
            || h.records.len() != was.records.len();
        if fields_changed {
            let mut d = out.hopper(i)?;
            d.hopper_type = h.hopper_type;
            d.status = h.status;
            d.name = h.name.clone();
            d.ranges = h.ranges;
            d.dwords = h.dwords.clone();
            d.record_count = h.records.len() as u32;
            out.set_hopper(i, &d)?;
        }
        // A template wrote its own text, so the playlist's goes back on.
        if h.template.is_some() || h.title != was.title || h.description != was.description {
            out.set_text(
                i,
                &HopperText {
                    title: h.title.clone(),
                    description: h.description.clone(),
                },
            )?;
        }
    }
    Ok(out)
}

// ---------- edits ----------

/// Swaps entry `i` with its neighbour `i + by` (by = -1 or 1).
pub fn move_record(h: &mut EditHopper, i: usize, by: isize) -> bool {
    let Some(j) = i.checked_add_signed(by) else {
        return false;
    };
    if i >= h.records.len() || j >= h.records.len() {
        return false;
    }
    h.records.swap(i, j);
    true
}

/// Inserts a copy of entry `i` (the same source slot) after it.
pub fn duplicate_record(h: &mut EditHopper, i: usize) -> bool {
    if h.records.len() >= MAX_RECORDS {
        return false;
    }
    let Some(r) = h.records.get(i).cloned() else {
        return false;
    };
    h.records.insert(i + 1, r);
    true
}

/// Removes entry `i`; a hopper keeps at least one.
pub fn remove_record(h: &mut EditHopper, i: usize) -> bool {
    if h.records.len() <= 1 || i >= h.records.len() {
        return false;
    }
    h.records.remove(i);
    true
}

/// Adds an entry at the end: a copy of the last one (its game variant
/// included), or a zero-filled slot with weight 1 when the hopper has none.
pub fn add_record(h: &mut EditHopper) -> bool {
    if h.records.len() >= MAX_RECORDS {
        return false;
    }
    let r = h.records.last().cloned().unwrap_or(EditRecord {
        id: 0,
        string: String::new(),
        flags: 0,
        dwords: [1, 0],
        variant: None,
        src: None,
        picked: None,
    });
    h.records.push(r);
    true
}

/// Moves entry `from` to position `to` (drag and drop), shifting the rest.
pub fn move_record_to(h: &mut EditHopper, from: usize, to: usize) -> bool {
    let n = h.records.len();
    if from >= n || to >= n || from == to {
        return false;
    }
    let r = h.records.remove(from);
    h.records.insert(to, r);
    true
}

/// Each entry's chance of being picked, as session setup computes it: the
/// weight clamped to 1..1000 over the total.
pub fn chances(h: &EditHopper) -> Vec<f64> {
    let w: Vec<f64> = h
        .records
        .iter()
        .map(|r| f64::from(r.weight().clamp(1, MAX_WEIGHT)))
        .collect();
    let total: f64 = w.iter().sum();
    w.iter().map(|x| x / total).collect()
}

/// At most `max` UTF-8 bytes, cut on a character boundary.
pub fn clip(s: &str, max: usize) -> String {
    let mut end = 0;
    for (i, c) in s.char_indices() {
        if i + c.len_utf8() > max {
            break;
        }
        end = i + c.len_utf8();
    }
    s[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{good_variant, sample};
    use live_proto::hopper::INFLATED_SIZE;

    #[test]
    fn an_unedited_copy_writes_back_every_byte() {
        let f = sample();
        let model = load(&f);
        let out = apply(&f, &model).unwrap();
        assert_eq!(out.image(), f.image());
        assert_eq!(out.image().len(), INFLATED_SIZE);
    }

    #[test]
    fn load_reads_fields_text_and_variants() {
        let f = sample();
        let m = load(&f);
        assert_eq!(m.len(), 16);
        assert_eq!(m[1].title, "List 1");
        assert_eq!(m[1].records.len(), 3);
        assert_eq!(m[1].records[2].id, 12);
        assert_eq!(m[1].records[2].weight(), 3);
        let v = m[1].records[1].variant.as_ref().unwrap();
        assert_eq!((v.name.as_str(), v.engine, v.team), ("V11", 1, true));
        assert_eq!(m[1].records[1].src, Some(SlotRef { hopper: 1, slot: 1 }));
    }

    /// The byte ranges of hopper `i`'s descriptor and data block.
    fn ranges_of(i: usize) -> [std::ops::Range<usize>; 2] {
        let d = i * 0x614;
        let b = 0x6140 + i * 0x15cb8;
        [d..d + 0x614, b..b + 0x15cb8]
    }

    fn outside(a: &[u8], b: &[u8], keep: &[std::ops::Range<usize>]) {
        for (k, (x, y)) in a.iter().zip(b).enumerate() {
            if !keep.iter().any(|r| r.contains(&k)) {
                assert_eq!(x, y, "byte {k:#x} changed outside the edited hopper");
            }
        }
    }

    #[test]
    fn a_weight_edit_changes_only_that_dword() {
        let f = sample();
        let mut m = load(&f);
        m[2].records[1].dwords[0] = 500;
        let out = apply(&f, &m).unwrap();
        let at = 0x6140 + 2 * 0x15cb8 + 0x48 + RECORD + 0x248;
        let mut want = f.image().to_vec();
        want[at..at + 4].copy_from_slice(&500u32.to_le_bytes());
        assert_eq!(out.image(), &want[..]);
    }

    #[test]
    fn reorder_moves_whole_slots_and_keeps_other_hoppers() {
        let f = sample();
        let mut m = load(&f);
        assert!(move_record(&mut m[1], 0, 1));
        assert!(!move_record(&mut m[1], 0, -1));
        assert!(!move_record(&mut m[1], 2, 1));
        let out = apply(&f, &m).unwrap();
        assert_eq!(out.slot(1, 0).unwrap(), f.slot(1, 1).unwrap());
        assert_eq!(out.slot(1, 1).unwrap(), f.slot(1, 0).unwrap());
        assert_eq!(out.slot(1, 2).unwrap(), f.slot(1, 2).unwrap());
        // Nothing else in the file moved, the descriptor included.
        let [_, block] = ranges_of(1);
        outside(f.image(), out.image(), &[block]);
        // Reloading gives the reordered list back.
        let again = load(&out);
        assert_eq!(again[1].records[0].variant.as_ref().unwrap().name, "V11");
        assert_eq!(again[1].records[0].id, 11);
    }

    #[test]
    fn duplicate_remove_and_add_keep_variants() {
        let f = sample();
        let mut m = load(&f);
        assert!(duplicate_record(&mut m[0], 0));
        assert!(remove_record(&mut m[0], 2));
        assert!(add_record(&mut m[0]));
        let out = apply(&f, &m).unwrap();
        let back = load(&out);
        let names: Vec<_> = back[0]
            .records
            .iter()
            .map(|r| r.variant.clone().unwrap().name)
            .collect();
        assert_eq!(names, ["V00", "V00", "V00"]);
        assert_eq!(out.slot(0, 1).unwrap(), f.slot(0, 0).unwrap());
        assert_eq!(out.block_record_count(0).unwrap(), 3);
        assert_eq!(out.hopper(0).unwrap().record_count, 3);
        // Slots past the new count keep their old bytes.
        assert_eq!(out.slot(0, 5).unwrap(), f.slot(0, 5).unwrap());
        let [desc, block] = ranges_of(0);
        outside(f.image(), out.image(), &[desc, block]);
    }

    #[test]
    fn copying_across_playlists_carries_the_slot() {
        let f = sample();
        let mut m = load(&f);
        let from = m[2].records[3].clone();
        m[0].records.push(from);
        let out = apply(&f, &m).unwrap();
        assert_eq!(out.slot(0, 2).unwrap(), f.slot(2, 3).unwrap());
        assert_eq!(out.slot(2, 3).unwrap(), f.slot(2, 3).unwrap());
    }

    #[test]
    fn text_is_written_only_when_it_changes() {
        let f = sample();
        let mut m = load(&f);
        m[0].records[0].dwords[0] = 9;
        let out = apply(&f, &m).unwrap();
        assert_eq!(&out.image()[0x3c..0x41], b"Liste", "other languages kept");
        m[0].title = "Snipers".into();
        let out = apply(&f, &m).unwrap();
        assert_eq!(out.text(0).unwrap().title, "Snipers");
        assert_eq!(&out.image()[0x3c..0x44], b"Snipers\0");
    }

    #[test]
    fn a_blank_hopper_can_get_its_first_entry() {
        let f = HopperFile::blank();
        let mut m = load(&f);
        assert!(m[5].records.is_empty());
        assert!(add_record(&mut m[5]));
        m[5].hopper_type = 2;
        m[5].title = "New".into();
        m[5].records[0].id = 7;
        let out = apply(&f, &m).unwrap();
        let back = load(&out);
        assert_eq!(back[5].records.len(), 1);
        assert_eq!(back[5].records[0].id, 7);
        assert_eq!(back[5].records[0].weight(), 1);
        assert_eq!(back[5].hopper_type, 2);
        // The other fifteen blank hoppers do not block the save.
        assert!(back[4].records.is_empty());
    }

    #[test]
    fn validation_matches_the_server() {
        let f = sample();
        let base = load(&f);
        let mut m = base.clone();
        m[0].title = "Sixteen chars!!!".into();
        m[0].description = "d".repeat(128);
        m[0].hopper_type = 6;
        m[0].status = 3;
        m[0].name = "café".into();
        m[0].records[0].string = "s".repeat(MAX_STRING + 1);
        m[1].title.clear();
        let errs = errors(&base, &m);
        for want in [
            "Playlist 1: type must be 1 to 5",
            "Playlist 1: status must be 0 (not loaded), 1 (ok) or 2 (error)",
            "Playlist 1: the title is longer than 15 bytes",
            "Playlist 1: the description is longer than 127 bytes",
            "Playlist 1: the name must be ASCII",
            "Playlist 1: entry 1: the string is longer than 71 characters",
            "Playlist 2: the playlist needs a title",
        ] {
            assert!(errs.iter().any(|e| e == want), "{want} in {errs:?}");
        }
        assert!(matches!(apply(&f, &m), Err(ApplyError::Invalid(_))));
        // Removing down to one is allowed, never to zero.
        let mut m = base.clone();
        assert!(remove_record(&mut m[0], 0));
        assert!(!remove_record(&mut m[0], 0));
        m[0].records.clear();
        assert!(errors(&base, &m)[0].contains("1 to 100 entries, got 0"));
        // A full hopper takes no more.
        let mut m = base;
        while add_record(&mut m[0]) {}
        assert_eq!(m[0].records.len(), MAX_RECORDS);
        assert!(!duplicate_record(&mut m[0], 0));
    }

    #[test]
    fn a_bad_source_slot_is_refused() {
        let f = sample();
        let mut m = load(&f);
        m[0].records[0].src = Some(SlotRef { hopper: 3, slot: 5 });
        assert!(matches!(apply(&f, &m), Err(ApplyError::Source { .. })));
        let mut m = load(&f);
        m.pop();
        assert_eq!(apply(&f, &m).unwrap_err(), ApplyError::Hoppers(15));
    }

    #[test]
    fn moving_to_a_position_and_chances() {
        let f = sample();
        let mut m = load(&f);
        assert!(move_record_to(&mut m[2], 0, 3));
        let ids: Vec<_> = m[2].records.iter().map(|r| r.id).collect();
        assert_eq!(ids, [21, 22, 23, 20]);
        let c = chances(&m[2]);
        // Weights 2, 3, 4, 1.
        assert!((c[0] - 0.2).abs() < 1e-9 && (c[3] - 0.1).abs() < 1e-9);
        m[2].records[0].dwords[0] = 0; // clamped to 1
        assert!((chances(&m[2])[0] - 1.0 / 9.0).abs() < 1e-9);
    }

    fn slot_at(i: usize, j: usize) -> usize {
        0x6140 + i * 0x15cb8 + 0x48 + j * RECORD
    }

    #[test]
    fn a_picked_variant_replaces_only_the_slot_tail() {
        let f = sample();
        let mut m = load(&f);
        let v = good_variant("Team Pick");
        set_variant(&mut m[1].records[2], &v).unwrap();
        let r = &m[1].records[2];
        assert_eq!((r.id, r.weight()), (12, 3), "map and weight stay");
        assert_eq!(r.variant.as_ref().unwrap().name, "Team Pick");
        assert!(r.variant.as_ref().unwrap().team);
        assert_eq!(r.dwords[1], 2, "the second dword is read from the variant");
        let out = apply(&f, &m).unwrap();
        let at = slot_at(1, 2);
        let mut want = f.image().to_vec();
        want[at + VARIANT_AT..at + RECORD].copy_from_slice(v.bytes());
        assert_eq!(
            out.image(),
            &want[..],
            "only the variant's 0x130 bytes change"
        );
        // Reloading reads it back, decoded.
        let back = load(&out);
        let got = entry_variant(&out, &back[1].records[2]).unwrap();
        assert_eq!(got, v);
        assert_eq!(got.game_type(), variants::GameType::Slayer);
    }

    #[test]
    fn a_hopper_slot_carries_the_extension_block() {
        let f = sample();
        let mut m = load(&f);
        let mut v = good_variant("Moon Pick");
        let e = variants::ext::Ext {
            gravity: 50,
            unlimited_grenades: true,
            ..Default::default()
        };
        v.set_ext(&e).unwrap();
        set_variant(&mut m[1].records[2], &v).unwrap();
        // The editable fields end before the block (the raw string,
        // flags and second dword are at variant +0x00, +0x48, +0x58).
        m[1].records[2].dwords[0] = 7;
        let out = apply(&f, &m).unwrap();
        let back = load(&out);
        let got = entry_variant(&out, &back[1].records[2]).unwrap();
        assert_eq!(got.ext(), e);
        assert_eq!(got.ext_block(), v.ext_block());
    }

    #[test]
    fn entry_variant_decodes_the_source_or_the_pick() {
        let f = sample();
        let mut m = load(&f);
        let before = entry_variant(&f, &m[0].records[1]).unwrap();
        assert_eq!(before.name(), "V01");
        assert_eq!(before.game_type(), variants::GameType::Ctf);
        assert_eq!(&before.to_bytes()[..], &f.slot(0, 1).unwrap()[VARIANT_AT..]);
        let v = good_variant("Swapped");
        set_variant(&mut m[0].records[1], &v).unwrap();
        assert_eq!(entry_variant(&f, &m[0].records[1]).unwrap(), v);
    }

    #[test]
    fn a_variant_the_game_rejects_is_refused() {
        let f = sample();
        let mut m = load(&f);
        let was = m[0].records[0].clone();
        let mut b = good_variant("Bad").to_bytes();
        b[0x44] = 5; // engine 5 (Race): the clean-up resets it
        let bad = GameVariant::from_bytes(&b).unwrap();
        let err = set_variant(&mut m[0].records[0], &bad).unwrap_err();
        assert!(err.0.iter().any(|p| p.offset == 0x44), "{err}");
        assert_eq!(m[0].records[0], was, "nothing changed");
        assert_eq!(apply(&f, &m).unwrap().image(), f.image());
    }

    #[test]
    fn raw_edits_that_break_a_picked_variant_block_the_save() {
        let f = sample();
        let mut m = load(&f);
        set_variant(&mut m[2].records[0], &good_variant("Raw")).unwrap();
        // The flags dword is the variant's +0x48; bit 15 is out of range.
        m[2].records[0].flags |= 1 << 15;
        let errs = errors(&load(&f), &m);
        assert!(
            errs.iter()
                .any(|e| e.starts_with("Playlist 3: entry 1: the game would reject")),
            "{errs:?}"
        );
        assert!(matches!(apply(&f, &m), Err(ApplyError::Invalid(_))));
        // In range, the raw edit lands inside the picked variant.
        m[2].records[0].flags = 1 | 2;
        let out = apply(&f, &m).unwrap();
        let got = entry_variant(&out, &load(&out)[2].records[0]).unwrap();
        assert_eq!(got.name(), "Raw");
        assert_eq!(&got.to_bytes()[0x48..0x4c], &3u32.to_le_bytes());
    }

    #[test]
    fn source_variants_are_not_judged() {
        // The salted sample's variants break the clean-up's rules, but they
        // came from the file: edits elsewhere still save.
        let f = sample();
        let mut m = load(&f);
        assert!(!vrecord::check(&entry_variant(&f, &m[0].records[0]).unwrap()).is_empty());
        m[0].records[0].dwords[0] = 50;
        apply(&f, &m).unwrap();
    }

    #[test]
    fn picks_travel_with_copies_and_moves() {
        let f = sample();
        let mut m = load(&f);
        let v = good_variant("Mover");
        set_variant(&mut m[0].records[0], &v).unwrap();
        assert!(duplicate_record(&mut m[0], 0));
        assert!(move_record_to(&mut m[0], 0, 2));
        let out = apply(&f, &m).unwrap();
        let back = load(&out);
        let names: Vec<_> = back[0]
            .records
            .iter()
            .map(|r| entry_variant(&out, r).unwrap().name())
            .collect();
        assert_eq!(names, ["Mover", "V01", "Mover"]);
        // Outside the variant, each slot is still its source's.
        assert_eq!(
            out.slot(0, 2).unwrap()[..VARIANT_AT],
            f.slot(0, 0).unwrap()[..VARIANT_AT]
        );
        assert_eq!(out.slot(0, 1).unwrap(), f.slot(0, 1).unwrap());
    }

    #[test]
    fn a_new_entry_in_a_blank_hopper_takes_a_variant() {
        let f = HopperFile::blank();
        let mut m = load(&f);
        assert!(add_record(&mut m[6]));
        m[6].hopper_type = 2;
        m[6].title = "Fresh".into();
        let v = good_variant("First");
        set_variant(&mut m[6].records[0], &v).unwrap();
        let out = apply(&f, &m).unwrap();
        let slot = out.slot(6, 0).unwrap();
        assert_eq!(&slot[VARIANT_AT..], v.bytes());
        assert!(slot[..VARIANT_AT].iter().enumerate().all(|(k, b)| {
            // Only the weight (+0x248) is set before the variant.
            *b == 0 || (0x248..0x24c).contains(&k)
        }));
    }

    #[test]
    fn an_unused_slot_becomes_a_playlist_the_game_loads() {
        use live_proto::hopper::check_descriptor;
        let f = HopperFile::default_set();
        let base = load(&f);
        let i = defaults::playlists().len();
        assert!(is_unused(&base[i]));
        // Untouched unused slots never block a save; a title alone in one
        // says what to do.
        let mut m = base.clone();
        m[i].title = "Modtacular".into();
        let errs = errors(&base, &m);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].starts_with("Playlist 12: this slot is unused"));
        // From a template: one entry, the template's rules.
        let mut m = base.clone();
        assert!(!create_playlist(&mut m[i], 99));
        assert!(create_playlist(&mut m[i], 2));
        assert_eq!(m[i].records.len(), 1);
        assert_eq!(m[i].hopper_type, 4);
        m[i].title = "Modtacular".into();
        assert_eq!(errors(&base, &m), Vec::<String>::new());
        let out = apply(&f, &m).unwrap();
        let d = out.descriptor(i).unwrap();
        assert_eq!(check_descriptor(&d), vec![]);
        assert_eq!(d.playlist_id, 12);
        assert_eq!(out.text(i).unwrap().title, "Modtacular");
        assert_eq!(out.problems(&defaults::retail_map_ids()), vec![]);
        let e = out.entry(i, 0).unwrap();
        assert_eq!(e.variant, defaults::variants::team_slayer());
        // Clearing it again gives back the file it came from.
        let mut again = load(&out);
        clear_playlist(&mut again[i]);
        assert!(is_unused(&again[i]));
        let back = apply(&out, &again).unwrap();
        assert!(back.image() == f.image());
    }

    #[test]
    fn another_kind_keeps_a_playlists_text_and_entries() {
        use live_proto::hopper::check_descriptor;
        let f = HopperFile::default_set();
        let mut m = load(&f);
        let entries = m[0].records.clone();
        assert!(create_playlist(&mut m[0], 7));
        assert_eq!(m[0].title, "Rumble Pit");
        assert_eq!(m[0].records, entries);
        let out = apply(&f, &m).unwrap();
        let d = out.descriptor(0).unwrap();
        assert_eq!(check_descriptor(&d), vec![]);
        assert_eq!(d.hopper_type, 1, "Free for All is unranked");
        assert_eq!(out.text(0).unwrap().title, "Rumble Pit");
        assert_eq!(d.playlist_id, 1, "the id stays the playlist's own");
    }

    #[test]
    fn clip_cuts_on_characters() {
        assert_eq!(clip("Team Snipers Pro", 15), "Team Snipers Pr");
        assert_eq!(clip("ééé", 5), "éé");
        assert_eq!(clip("ab", 15), "ab");
    }
}
