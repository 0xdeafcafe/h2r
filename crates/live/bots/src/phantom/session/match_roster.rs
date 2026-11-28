//! The target session has a separate list of the parties admitted by matchmaking.
//! The retail hosting state (0x71982) compares its count and every 12-byte ID
//! against session membership before consulting the hopper's start wait.
use super::{MAX, R, at::*, mac_at, peer, player};
use guest_layout::memory::GuestMemory;

const PRESENT: u32 = 0x49fd;
const ROSTER: u32 = 0x4a00;
const REVISION: u32 = 0x4978;
// Structure of arrays filled by retail's party add (0x63550).
const PARTY_COUNT: usize = 0;
const NONCES: usize = 4;
const CLANS: usize = 0x84;
const TIMES: usize = 0x144;
const PLAYER_COUNT_AT: usize = 0x184;
const IDS: usize = 0x188;
const LEVELS: usize = 0x248;
const SKILLS: usize = 0x288;
const PARTIES: usize = 0x2c8;
const SIZE: usize = 0x308;

fn word(block: &[u8], at: usize) -> usize {
    u32::from_le_bytes(block[at..at + 4].try_into().unwrap()) as usize
}

fn put(block: &mut [u8], at: usize, value: usize) {
    block[at..at + 4].copy_from_slice(&(value as u32).to_le_bytes());
}

fn compact(block: &mut [u8], fields: &[(usize, usize)], index: usize, count: usize) {
    for &(base, width) in fields {
        block.copy_within(
            base + (index + 1) * width..base + count * width,
            base + index * width,
        );
        block[base + (count - 1) * width..base + count * width].fill(0);
    }
}

const PREFIX: [u8; 4] = [
    b'C',
    b'M',
    super::super::MACHINE_PREFIX[0],
    super::super::MACHINE_PREFIX[1],
];

/// Only the host's joinable matchmaking target reaches here. Real party
/// reservations can precede their players joining, so keep them verbatim.
pub(in crate::phantom) fn sync(m: &mut dyn GuestMemory, s: u32, filling: bool) -> R<()> {
    if m.u8(s + PRESENT)? == 0 {
        return Ok(());
    }
    let mut block = [0; SIZE];
    m.read(s + ROSTER, &mut block)?;
    let mut parties = word(&block, PARTY_COUNT);
    let mut players = word(&block, PLAYER_COUNT_AT);
    if !(1..=16).contains(&parties) || players > 16 {
        return Ok(());
    }
    let original = block;
    let mut bots = Vec::new();
    if filling {
        let mask = m.u32(s + PLAYER_MASK)?;
        let peers = m.u32(s + PEER_COUNT)?.min(MAX);
        for slot in (0..16).filter(|i| mask & (1 << i) != 0) {
            let a = player(s, slot);
            let owner = m.u32(a + P_PEER)?;
            if owner >= peers {
                continue;
            }
            let p = peer(s, owner);
            let mac = mac_at(m, p + PEER_MAC)?;
            if crate::phantom::handle_of(mac).is_none() {
                continue;
            }
            let mut nonce = [0; 8];
            nonce[..2].copy_from_slice(b"CM");
            nonce[2..].copy_from_slice(&mac);
            let mut id = [0; 12];
            m.read(a + P_ID, &mut id)?;
            bots.push((nonce, id));
            // The join message supplies this nonce separately from peer properties.
            let mut advertised = [0; 8];
            m.read(p + 0xf0, &mut advertised)?;
            if advertised != nonce {
                m.write(p + 0xf0, &nonce)?;
            }
        }
    }

    // Remove departed singleton parties and adjust the surviving party indices,
    // just as 0x633c0 does. Preserve the order of the retail parties and players.
    for party in (0..parties).rev() {
        let at = NONCES + party * 8;
        let nonce = &block[at..at + 8];
        if !nonce.starts_with(&PREFIX)
            || bots.iter().any(|(n, id)| {
                n.as_slice() == nonce
                    && (0..players).any(|i| {
                        word(&block, PARTIES + i * 4) == party
                            && &block[IDS + i * 12..IDS + (i + 1) * 12] == id.as_slice()
                    })
            })
        {
            continue;
        }
        for i in (0..players).rev() {
            let owner = word(&block, PARTIES + i * 4);
            if owner == party {
                compact(
                    &mut block,
                    &[(IDS, 12), (LEVELS, 4), (SKILLS, 4), (PARTIES, 4)],
                    i,
                    players,
                );
                players -= 1;
            } else if owner > party {
                put(&mut block, PARTIES + i * 4, owner - 1);
            }
        }
        compact(
            &mut block,
            &[(NONCES, 8), (CLANS, 12), (TIMES, 4)],
            party,
            parties,
        );
        parties -= 1;
    }
    for (nonce, id) in bots {
        if parties == 16
            || players == 16
            || block[IDS..IDS + players * 12]
                .chunks_exact(12)
                .any(|entry| entry == id.as_slice())
        {
            continue;
        }
        let at = NONCES + parties * 8;
        block[at..at + 8].copy_from_slice(&nonce);
        block[CLANS + parties * 12..CLANS + (parties + 1) * 12].fill(0);
        // All seats belong to this hosted match. Use the network clock
        // sampled by retail, or the host party's join time between samples.
        let joined = if m.u8(0x0051_0548).ok() == Some(1) {
            m.u32(0x0051_054c)? as usize
        } else {
            word(&block, TIMES)
        };
        put(&mut block, TIMES + parties * 4, joined);
        block[IDS + players * 12..IDS + (players + 1) * 12].copy_from_slice(&id);
        // No retail leaderboard samples for a synthetic peer.
        put(&mut block, LEVELS + players * 4, 0);
        put(&mut block, SKILLS + players * 4, u32::MAX as usize);
        put(&mut block, PARTIES + players * 4, parties);
        players += 1;
        parties += 1;
    }
    put(&mut block, PLAYER_COUNT_AT, players);
    put(&mut block, PARTY_COUNT, parties);
    if block != original {
        // 0x5c350 publishes the whole descriptor and advances the parameter revision.
        let revision = m.u32(s + REVISION)?;
        m.write(s + ROSTER, &block)?;
        m.set_u32(s + REVISION, revision.wrapping_add(1))?;
    }
    Ok(())
}
