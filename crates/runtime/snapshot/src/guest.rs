//! The runtime's sections: guest layout, physical RAM, the time-stamp
//! counter. The first subsystems saved, because every other section refers
//! to guest memory and must load after it.
use crate::Error;
use crate::codec::{Reader, Writer};
use crate::container::{Codec, Snapshot, SnapshotWriter, Tag};
use crate::pages::{self, Base, PageStats};
use runtime::guest::{ClaimRecord, GuestLayout, Mapping, Permissions, RAM_SIZE, ReservationRecord};
use runtime::{Guest, Runtime};

pub const LAYOUT: Tag = Tag::new("guestmap");
pub const RAM: Tag = Tag::new("ram");
pub const TSC: Tag = Tag::new("tsc");
const LAYOUT_VERSION: u32 = 1;
const RAM_VERSION: u32 = 1;
const TSC_VERSION: u32 = 1;

pub fn encode_layout(layout: &GuestLayout) -> Vec<u8> {
    let mut w = Writer::new();
    w.list(&layout.mappings, |w, m| {
        w.u32(m.virtual_start)
            .u32(m.len)
            .u32(m.physical_start)
            .u8(m.permissions.bits());
    })
    .u32(layout.next_physical)
    .u32(layout.next_id)
    .list(&layout.reservations, |w, r| {
        w.u32(r.id).u32(r.physical_start).u32(r.len);
        w.list(&r.aliases, |w, a| {
            w.u32(*a);
        });
        w.str(&r.owner);
    })
    .list(&layout.claims, |w, c| {
        w.u32(c.id).u32(c.virtual_start).u32(c.len).str(&c.owner);
    })
    .list(&layout.code, |w, (a, b)| {
        w.u64(*a).u64(*b);
    });
    w.into_bytes()
}

pub fn decode_layout(bytes: &[u8]) -> Result<GuestLayout, Error> {
    let mut r = Reader::new(bytes, "guestmap");
    let layout = GuestLayout {
        mappings: r.list(13, |r| {
            Ok(Mapping {
                virtual_start: r.u32()?,
                len: r.u32()?,
                physical_start: r.u32()?,
                permissions: Permissions::from_bits(r.u8()?)
                    .ok_or_else(|| Error::Corrupt("guestmap", "permissions".into()))?,
            })
        })?,
        next_physical: r.u32()?,
        next_id: r.u32()?,
        reservations: r.list(20, |r| {
            Ok(ReservationRecord {
                id: r.u32()?,
                physical_start: r.u32()?,
                len: r.u32()?,
                aliases: r.list(4, |r| r.u32())?,
                owner: r.str()?.to_owned(),
            })
        })?,
        claims: r.list(16, |r| {
            Ok(ClaimRecord {
                id: r.u32()?,
                virtual_start: r.u32()?,
                len: r.u32()?,
                owner: r.str()?.to_owned(),
            })
        })?,
        code: r.list(16, |r| Ok((r.u64()?, r.u64()?)))?,
    };
    r.finish()?;
    Ok(layout)
}

/// Add the guest's layout and RAM. `base` makes RAM a delta.
pub fn save_guest(
    out: &mut SnapshotWriter,
    guest: &Guest,
    base: Option<&Base<'_>>,
) -> Result<PageStats, Error> {
    let (ram, stats) = pages::encode(guest.physical_ram(), base)?;
    out.section(
        LAYOUT,
        LAYOUT_VERSION,
        Codec::Zstd,
        encode_layout(&guest.layout()),
    )?
    .section(RAM, RAM_VERSION, Codec::Stored, ram)?;
    Ok(stats)
}

/// Read the guest layout for either a decoded or a file-backed RAM restore.
pub fn load_layout(snapshot: &Snapshot<'_>) -> Result<GuestLayout, Error> {
    decode_layout(&snapshot.require(LAYOUT, LAYOUT_VERSION..=LAYOUT_VERSION)?)
}

/// Replace `guest`'s layout and RAM with the snapshot's. The guest must
/// have been booted like the saved one (`Guest::restore`); RAM is decoded
/// straight into it. A failed decode leaves the guest unusable.
pub fn load_guest(
    snapshot: &Snapshot<'_>,
    guest: &mut Guest,
    base: Option<&[u8]>,
) -> Result<PageStats, Error> {
    load_guest_ram(snapshot, guest, base, None)
}

/// Decode the physical pages of a standalone anchor without restoring its
/// device state, mappings or kernel into the running machine.
pub fn standalone_ram(snapshot: &Snapshot<'_>) -> Result<Vec<u8>, Error> {
    let ram = snapshot.require(RAM, RAM_VERSION..=RAM_VERSION)?;
    let mut out = vec![0; RAM_SIZE];
    pages::decode(&ram, &mut out, None)?;
    Ok(out)
}

/// Film seeks carry a named anchor in the private tick directory. Verify
/// both its identity and the checksums of the inherited pages.
pub fn load_guest_delta(
    snapshot: &Snapshot<'_>,
    guest: &mut Guest,
    base: &Base<'_>,
) -> Result<PageStats, Error> {
    load_guest_ram(snapshot, guest, Some(base.ram), Some(base.name))
}

fn load_guest_ram(
    snapshot: &Snapshot<'_>,
    guest: &mut Guest,
    base: Option<&[u8]>,
    expected_name: Option<&str>,
) -> Result<PageStats, Error> {
    let layout = load_layout(snapshot)?;
    let ram = snapshot.require(RAM, RAM_VERSION..=RAM_VERSION)?;
    if let Some(name) = expected_name {
        if pages::base_name(&ram)?.as_deref() != Some(name) {
            return Err(Error::Corrupt(
                "ram",
                "anchor name disagrees with the film index".into(),
            ));
        }
    }
    let mut stats = PageStats::default();
    guest
        .restore_with(&layout, |out| {
            debug_assert_eq!(out.len(), RAM_SIZE);
            stats = pages::decode(&ram, out, base)?;
            Ok::<(), Error>(())
        })
        .map_err(Error::Restore)??;
    Ok(stats)
}

pub fn save_tsc(out: &mut SnapshotWriter, runtime: &Runtime) -> Result<(), Error> {
    let mut w = Writer::new();
    w.u64(runtime.tsc_last());
    out.section(TSC, TSC_VERSION, Codec::Stored, w.into_bytes())?;
    Ok(())
}

pub fn load_tsc(snapshot: &Snapshot<'_>, runtime: &Runtime) -> Result<(), Error> {
    let bytes = snapshot.require(TSC, TSC_VERSION..=TSC_VERSION)?;
    let mut r = Reader::new(&bytes, "tsc");
    let last = r.u64()?;
    r.finish()?;
    runtime.restore_tsc_last(last);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_round_trips() {
        let layout = GuestLayout {
            mappings: vec![Mapping {
                virtual_start: 0x1_0000,
                len: 0x3000,
                physical_start: 0x4000,
                permissions: Permissions::RX,
            }],
            next_physical: 0x7000,
            next_id: 4,
            reservations: vec![ReservationRecord {
                id: 1,
                physical_start: 0x10_0000,
                len: 0x1000,
                aliases: vec![0x8010_0000, 0xc010_0000],
                owner: "arena".into(),
            }],
            claims: vec![ClaimRecord {
                id: 2,
                virtual_start: 0xa000_0000,
                len: 0x1000,
                owner: "hle".into(),
            }],
            code: vec![(0x1_0000, 1 << 32)],
        };
        assert_eq!(decode_layout(&encode_layout(&layout)).unwrap(), layout);
        let bytes = encode_layout(&layout);
        assert!(decode_layout(&bytes[..bytes.len() - 1]).is_err());
        let mut bad = bytes.clone();
        bad[16] = 9; // the first mapping's permission bits
        assert!(decode_layout(&bad).is_err());
    }
}
