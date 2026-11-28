//! Replication admission at the guest session boundary. LAN discovery and
//! Live QoS use the same game description. Direct, invited and queued requests
//! converge on the host's session validator (docs/game/notes/creature-entity.md).

use guest_layout::{GuestMemory, MemFault, points};
use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy};
use live_proto::replication;
use runtime::{
    GuestFault,
    guest::{Access, FaultReason},
};

/// The feature that must be present before network sessions are allowed.
pub const FEATURE: &str = "replication-admission";

/// The title's existing incompatible-build reason in the joining context.
pub(crate) const JOIN_REASON_INCOMPATIBLE: u32 = 9;
/// The host's join-refuse message carries a four-bit reason. Value 15 is
/// reserved here for replication incompatibility.
const REFUSE_REPLICATION_VERSION: u32 = 15;
const JOIN_REASON: u32 = 0x104;

// The description's version word uses eight bits in Live QoS. System Link
// copies the word as-is. The retail description validators inspect the
// separate discriminator and build interval, which retain their meaning.
const DESCRIPTION_VERSION_PREFIX: u16 = 0xc0;
const JOIN_VERSION_PREFIX: u16 = 0xe000;
const VERSION_MASK: u16 = 0x3f;

const RETAIL_JOIN_HEADER: u16 = 2;
const MESSAGE_REQUEST: u32 = 0xc;
// Two alignment bytes between the bool at message +0x158 and DWORD at
// +0x15c. The decoder never reads them from the wire. Retail copies the
// entire inner request into the 0x1ac-byte deferred join record.
const REQUEST_VERSION: u32 = 0x14e;

/// Guest-side checks that protect membership on both transports.
pub struct ReplicationAdmission {
    version: u16,
}

impl ReplicationAdmission {
    /// Use the same local wire version for both the sender and receiver.
    pub const fn new(version: u16) -> Self {
        Self { version }
    }
}

fn decode_version(marker: u16, prefix: u16) -> Option<u16> {
    let version = marker & VERSION_MASK;
    (marker & !VERSION_MASK == prefix && version != 0).then_some(version)
}

fn advertise(m: &mut dyn GuestMemory, description: u32, version: u16) -> Result<(), MemFault> {
    m.set_u16(description, DESCRIPTION_VERSION_PREFIX | version)
}

fn check_description(
    m: &mut dyn GuestMemory,
    description: u32,
    context: u32,
    local: u16,
) -> Result<bool, MemFault> {
    let peer = decode_version(m.u16(description)?, DESCRIPTION_VERSION_PREFIX);
    if let Err(reason) = replication::require(local, peer) {
        m.set_u32(context.wrapping_add(JOIN_REASON), JOIN_REASON_INCOMPATIBLE)?;
        eprintln!("{FEATURE}: join description: {reason}");
        return Ok(false);
    }
    Ok(true)
}

fn write_request_version(
    m: &mut dyn GuestMemory,
    message: u32,
    version: u16,
) -> Result<(), MemFault> {
    m.set_u16(message, JOIN_VERSION_PREFIX | version)
}

/// Normalize message 8 for retail and attach the version to the request the
/// validator will see. The validator rejects the stock header as missing.
fn prepare_request(m: &mut dyn GuestMemory, message: u32, local: u16) -> Result<bool, MemFault> {
    let header = m.u16(message)?;
    if decode_version(header, JOIN_VERSION_PREFIX).is_none() && header != RETAIL_JOIN_HEADER {
        if let Err(reason) = replication::require(local, None) {
            eprintln!("{FEATURE}: join message header {header:#06x}: {reason}");
        }
        return Ok(false);
    }
    m.set_u16(
        message.wrapping_add(MESSAGE_REQUEST + REQUEST_VERSION),
        header,
    )?;
    m.set_u16(message, RETAIL_JOIN_HEADER)?;
    Ok(true)
}

fn host_refusal(m: &dyn GuestMemory, request: u32, local: u16) -> Result<Option<u32>, MemFault> {
    let marker = m.u16(request.wrapping_add(REQUEST_VERSION))?;
    if let Err(reason) = replication::require(local, decode_version(marker, JOIN_VERSION_PREFIX)) {
        eprintln!("{FEATURE}: host join check: {reason}");
        return Ok(Some(REFUSE_REPLICATION_VERSION));
    }
    Ok(None)
}

fn guest_fault(fault: MemFault) -> GuestFault {
    GuestFault {
        addr: fault.addr,
        len: fault.len as usize,
        access: if fault.write {
            Access::Write
        } else {
            Access::Read
        },
        reason: FaultReason::Unmapped,
    }
}

impl Feature for ReplicationAdmission {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            FEATURE,
            "Require matching replication versions before a session join",
        )
        .doc("docs/game/ai-networking.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let local = self.version;
        if local == 0 || local > VERSION_MASK {
            return Err(BuildError(format!("invalid replication version {local}")));
        }
        b.snapshot(SnapshotPolicy::Stateless);
        b.raw().after(
            &points::SESSION_DESCRIBE,
            "advertise replication version in LAN and QoS descriptions",
            move |c| {
                if c.cpu.eax & 0xff != 0 {
                    let mut m = c.ctx.memory();
                    // The original's ret 4 leaves its single argument here.
                    let description = m.u32(c.cpu.esp.wrapping_sub(4))?;
                    advertise(&mut *m, description, local).map_err(guest_fault)?;
                }
                Ok(())
            },
        );
        b.raw().before(
            &points::SESSION_CHECK_DESCRIPTION,
            "require a matching replication version on the joining console",
            move |c| {
                let pass = check_description(&mut *c.ctx.memory(), c.cpu.esi, c.cpu.edi, local)
                    .map_err(guest_fault)?;
                if pass {
                    Ok(Flow::Continue)
                } else {
                    c.ret(4).map(Flow::Return)
                }
            },
        );
        b.raw().before(
            &points::SESSION_WRITE_JOIN_REQUEST,
            "write replication version in the message 8 header",
            move |c| {
                let mut m = c.ctx.memory();
                let message = m.u32(c.cpu.esp.wrapping_add(12))?;
                write_request_version(&mut *m, message, local).map_err(guest_fault)?;
                Ok(Flow::Continue)
            },
        );
        b.raw().before(
            &points::SESSION_RECEIVE_JOIN_REQUEST,
            "attach the decoded version before retail copies the request",
            move |c| {
                let dispatch =
                    prepare_request(&mut *c.ctx.memory(), c.cpu.esi, local).map_err(guest_fault)?;
                if dispatch {
                    Ok(Flow::Continue)
                } else {
                    c.ret(4).map(Flow::Return)
                }
            },
        );
        b.raw()
            .before(
                &points::SESSION_JOIN_CHECK,
                "refuse incompatible direct, invited and queued requests",
                move |c| {
                    let refusal =
                        host_refusal(&*c.ctx.memory(), c.cpu.edi, local).map_err(guest_fault)?;
                    if let Some(reason) = refusal {
                        c.cpu.eax = reason;
                        c.ret(0).map(Flow::Return)
                    } else {
                        Ok(Flow::Continue)
                    }
                },
            )
            .first();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;
    use live_proto::replication::{CREATURE_FULL, CREATURE_PROBE, STOCK};

    const DESCRIPTION: u32 = 0x1000;
    const CONTEXT: u32 = 0x2000;
    const MESSAGE: u32 = 0x3000;
    const COPIED: u32 = 0x4000;
    const QUEUED: u32 = 0x5000;
    const TITLE_BUILD: u32 = 0x2651;
    const INNER_REQUEST_SIZE: usize = 0x1ac;

    #[test]
    fn description_carries_the_explicit_version_on_both_transports() {
        let mut m = FakeGuest::new();
        m.map(DESCRIPTION, 0x40).map(CONTEXT, 0x120);
        for local in [STOCK, CREATURE_PROBE, CREATURE_FULL] {
            for remote in [None, Some(STOCK), Some(CREATURE_PROBE), Some(CREATURE_FULL)] {
                m.put_u16(DESCRIPTION, 2)
                    .put_u32(DESCRIPTION + 8, TITLE_BUILD)
                    .put_u32(DESCRIPTION + 12, TITLE_BUILD)
                    .put_u32(CONTEXT + JOIN_REASON, 0);
                if let Some(remote) = remote {
                    advertise(&mut m, DESCRIPTION, remote).unwrap();
                    // The same explicit marker survives Live QoS's 8-bit field.
                    let encoded = m.u16(DESCRIPTION).unwrap();
                    let qos_version = u8::try_from(encoded).unwrap();
                    assert_eq!(
                        decode_version(u16::from(qos_version), DESCRIPTION_VERSION_PREFIX),
                        Some(remote)
                    );
                }
                let matched = remote == Some(local);
                assert_eq!(
                    check_description(&mut m, DESCRIPTION, CONTEXT, local).unwrap(),
                    matched
                );
                assert_eq!(m.u32(DESCRIPTION + 8).unwrap(), TITLE_BUILD);
                assert_eq!(m.u32(DESCRIPTION + 12).unwrap(), TITLE_BUILD);
                assert_eq!(
                    m.u32(CONTEXT + JOIN_REASON).unwrap(),
                    if matched { 0 } else { JOIN_REASON_INCOMPATIBLE },
                );
            }
        }
    }

    #[test]
    fn mixed_versions_are_refused_before_replication() {
        let mut m = FakeGuest::new();
        m.map(MESSAGE, MESSAGE_REQUEST + INNER_REQUEST_SIZE as u32)
            .map(COPIED, INNER_REQUEST_SIZE as u32)
            .map(QUEUED, 0x18 + INNER_REQUEST_SIZE as u32);

        for local in [STOCK, CREATURE_PROBE, CREATURE_FULL] {
            for remote in [None, Some(STOCK), Some(CREATURE_PROBE), Some(CREATURE_FULL)] {
                if let Some(remote) = remote {
                    write_request_version(&mut m, MESSAGE, remote).unwrap();
                } else {
                    m.put_u16(MESSAGE, RETAIL_JOIN_HEADER);
                }
                // A fresh receive stamps even the missing version over old scratch.
                let body = MESSAGE + MESSAGE_REQUEST;
                m.put_u16(body + REQUEST_VERSION, JOIN_VERSION_PREFIX | local);
                let mut expected = m.bytes(body, INNER_REQUEST_SIZE).unwrap();
                let marker = remote.map_or(RETAIL_JOIN_HEADER, |v| JOIN_VERSION_PREFIX | v);
                let offset = REQUEST_VERSION as usize;
                expected[offset..offset + 2].copy_from_slice(&marker.to_le_bytes());
                assert!(prepare_request(&mut m, MESSAGE, local).unwrap());
                assert_eq!(m.u16(MESSAGE).unwrap(), RETAIL_JOIN_HEADER);

                let request = m.bytes(body, INNER_REQUEST_SIZE).unwrap();
                assert_eq!(request, expected);
                // The direct/invited path copies the inner message and checks it
                // before claiming a peer. The deferred path checks its exact copy.
                m.put(COPIED, &request).put(QUEUED + 0x18, &request);
                let refusal = (remote != Some(local)).then_some(REFUSE_REPLICATION_VERSION);
                assert_eq!(host_refusal(&m, COPIED, local).unwrap(), refusal);
                assert_eq!(host_refusal(&m, QUEUED + 0x18, local).unwrap(), refusal);
            }
        }
    }

    #[test]
    fn unknown_message_headers_never_enter_the_retail_dispatch() {
        let mut m = FakeGuest::new();
        m.map(MESSAGE, MESSAGE_REQUEST + INNER_REQUEST_SIZE as u32);
        for header in [0, 1, 3, JOIN_VERSION_PREFIX, 0xffff] {
            m.put_u16(MESSAGE, header);
            assert!(!prepare_request(&mut m, MESSAGE, STOCK).unwrap());
            assert_eq!(m.u16(MESSAGE).unwrap(), header);
        }
    }

    #[test]
    fn guest_admission_points_have_named_signatures() {
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        r.add(&ReplicationAdmission::new(STOCK)).unwrap();
        let points = [
            &points::SESSION_DESCRIBE,
            &points::SESSION_CHECK_DESCRIPTION,
            &points::SESSION_WRITE_JOIN_REQUEST,
            &points::SESSION_RECEIVE_JOIN_REQUEST,
            &points::SESSION_JOIN_CHECK,
        ];
        let plan = r.build(&hooks::Base).unwrap().text();
        for point in points {
            assert!(!point.sig.is_opaque());
            assert!(plan.contains(point.name), "{plan}");
        }

        for absent in points {
            let retained: Vec<_> = points
                .into_iter()
                .filter(|point| point.name != absent.name)
                .map(|point| (point, point.base))
                .collect();
            let mut r = hooks::Registry::new(hooks::Switches::defaults());
            r.add(&ReplicationAdmission::new(STOCK)).unwrap();
            let incomplete = r.build(&hooks::Remap::new(&retained)).unwrap();
            assert!(!incomplete.is_on(FEATURE), "{} was optional", absent.name);
        }
    }
}
