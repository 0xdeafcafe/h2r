//! Empty menu-facing service views. Counts/BOOLs are not HRESULTs. Explicit
//! data mutations remain outside this adapter; unsupported reads fail in ABI.
use crate::{
    state::{Error, Storage},
    status::*,
};
use runtime::{Guest, guest::Access};
use std::collections::{BTreeMap, VecDeque};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Notification,
    FriendsLatest,
    FriendsEnumerate,
    FriendsFinish,
    TitleName,
    NatType,
    SameTitle,
    SamePublisher,
    Teams,
    TeamResults,
    Members,
    MemberResults,
    TeamDetails,
    MemberDetails,
    Feedback,
    Query,
    QueryResults,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Friends,
    Teams,
    Members,
    Feedback,
    Query,
}
struct Task {
    kind: Kind,
    finished: bool,
}
#[derive(Default)]
pub(crate) struct Menu {
    tasks: BTreeMap<u32, Task>,
    notifications: BTreeMap<u32, VecDeque<u32>>,
    presence: BTreeMap<u32, BTreeMap<u32, Vec<[u8; 12]>>>,
}
type Result<T> = std::result::Result<T, Error>;
impl Menu {
    pub fn clear(&mut self) {
        self.tasks.clear();
        self.notifications.clear();
        self.presence.clear();
    }
    pub fn messages(&mut self, users: [u64; 4], ids: Vec<u32>) {
        self.notifications.clear();
        for (controller, user) in users.into_iter().enumerate() {
            if user != 0 {
                self.notifications
                    .insert(controller as u32, ids.iter().copied().collect());
            }
        }
    }
    pub fn remove_message(&mut self, user: u32, id: u32) {
        if let Some(q) = self.notifications.get_mut(&user) {
            q.retain(|m| *m != id);
        }
    }
    pub fn close(&mut self, h: u32) -> bool {
        self.presence.remove(&h);
        self.tasks.remove(&h).is_some()
    }
    pub fn presence_init(&mut self, h: u32) {
        self.presence.insert(h, BTreeMap::new());
    }
    pub fn presence_add(
        &mut self,
        m: &Guest,
        h: u32,
        group: u32,
        count: u32,
        p: u32,
    ) -> Result<u32> {
        let Some(groups) = self.presence.get_mut(&h) else {
            return Ok(INVALID_ARGUMENT);
        };
        if count > 1000 {
            return Ok(INVALID_ARGUMENT);
        }
        let mut bytes = vec![0; count as usize * 12];
        m.read_bytes(p, &mut bytes, 1)?;
        let entries = bytes.as_chunks::<12>().0.to_vec();
        if groups
            .iter()
            .filter(|(g, _)| **g != group)
            .map(|(_, e)| e.len())
            .sum::<usize>()
            + entries.len()
            > 1000
        {
            return Ok(BUFFER_TOO_SMALL);
        }
        groups.insert(group, entries);
        Ok(0)
    }
    pub fn presence_clear(&mut self, h: u32) -> u32 {
        match self.presence.get_mut(&h) {
            Some(g) => {
                g.clear();
                0
            }
            None => INVALID_ARGUMENT,
        }
    }
    pub fn presence_latest(&self, m: &mut Guest, a: &[u32]) -> Result<u32> {
        let Some(groups) = self.presence.get(&a[0]) else {
            return Ok(INVALID_ARGUMENT);
        };
        if a[2] > 1000 {
            return Ok(INVALID_ARGUMENT);
        }
        let mut bytes = vec![0; a[2] as usize * 61];
        if let Some(users) = groups.get(&a[1]) {
            for (dst, id) in bytes.as_chunks_mut::<61>().0.iter_mut().zip(users) {
                dst[..12].copy_from_slice(id);
            }
        }
        m.write_bytes(a[3], &bytes, 1)?;
        Ok(0)
    }
    pub fn poll(&self, h: u32) -> Option<u32> {
        self.tasks.get(&h).map(|t| match t.kind {
            Kind::Friends if !t.finished => TASK_RESULTS_AVAILABLE,
            Kind::Feedback => SERVICE_TEMPORARILY_UNAVAILABLE,
            Kind::Query => QUERY_DATASET_NOT_FOUND,
            _ => TASK_SUCCESS,
        })
    }
    fn task(
        &mut self,
        m: &mut Guest,
        storage: &mut Storage,
        event: u32,
        out: u32,
        kind: Kind,
    ) -> Result<u32> {
        if event != 0 {
            return Err(Error::Unverified("nonzero work event"));
        }
        m.check(out, 4, Access::Write, 1)?;
        let Some(h) = storage.alloc(m, 8) else {
            return Ok(OUT_OF_MEMORY);
        };
        m.set_u32(out, h)?;
        self.tasks.insert(
            h,
            Task {
                kind,
                finished: false,
            },
        );
        Ok(0)
    }
    pub fn invoke(
        &mut self,
        op: Op,
        a: &[u32],
        m: &mut Guest,
        storage: &mut Storage,
        initialized: bool,
        connected: bool,
    ) -> Result<u32> {
        use Op::*;
        match op {
            SameTitle => return Ok(u32::from(a[0] == 0x4d530064)),
            SamePublisher => return Ok(u32::from(a[0] >> 16 == 0x4d53)),
            NatType => return Ok(0), // Detection has not run; 0 is Halo's unknown case.
            Notification => {
                if !initialized {
                    return Ok(0);
                }
                m.check(a[1], 9, Access::Write, 1)?;
                m.check(a[2], 4, Access::Write, 1)?;
                let mut info = [0; 9];
                let mut flags = 0;
                let id = self
                    .notifications
                    .get_mut(&a[0])
                    .and_then(VecDeque::pop_front);
                if let Some(id) = id {
                    info[0] = 1;
                    info[1..5].copy_from_slice(&id.to_le_bytes());
                    info[5..9].copy_from_slice(&1u32.to_le_bytes());
                    if self.notifications.get(&a[0]).is_some_and(|q| !q.is_empty()) {
                        flags = 1;
                    }
                }
                m.write_bytes(a[1], &info, 1)?;
                m.set_u32(a[2], flags)?;
                return Ok(u32::from(id.is_some()));
            }
            FriendsLatest => {
                if initialized && a[1] <= 100 {
                    m.check(a[2], a[1] as usize * 88, Access::Write, 1)?;
                }
                return Ok(0);
            }
            _ => {}
        }
        if !initialized {
            return Ok(NOT_INITIALIZED);
        }
        match op {
            TitleName => {
                if a[2] > 4096 {
                    return Ok(INVALID_ARGUMENT);
                }
                if a[2] != 0 {
                    let name = if a[0] == 0x4d530064 { "Halo 2" } else { "" };
                    let units: Vec<u16> = name
                        .encode_utf16()
                        .take(a[2].saturating_sub(1) as usize)
                        .chain([0])
                        .collect();
                    m.write_bytes(
                        a[3],
                        &units
                            .into_iter()
                            .flat_map(u16::to_le_bytes)
                            .collect::<Vec<_>>(),
                        1,
                    )?;
                }
                Ok(0)
            }
            FriendsFinish => {
                let Some(t) = self
                    .tasks
                    .get_mut(&a[0])
                    .filter(|t| t.kind == Kind::Friends)
                else {
                    return Ok(INVALID_ARGUMENT);
                };
                t.finished = true;
                Ok(0)
            }
            TeamResults | MemberResults => {
                let kind = if op == TeamResults {
                    Kind::Teams
                } else {
                    Kind::Members
                };
                if self.tasks.get(&a[0]).is_none_or(|t| t.kind != kind) {
                    return Ok(INVALID_ARGUMENT);
                }
                m.set_u32(a[1], 0)?;
                Ok(0)
            }
            TeamDetails | MemberDetails => {
                let kind = if op == TeamDetails {
                    Kind::Teams
                } else {
                    Kind::Members
                };
                if self.tasks.get(&a[0]).is_none_or(|t| t.kind != kind) {
                    return Ok(INVALID_ARGUMENT);
                }
                Ok(TEAM_NOT_FOUND) // XONLINE_E_TEAMS_TEAM_NOT_FOUND
            }
            QueryResults => Ok(
                if self.tasks.get(&a[0]).is_some_and(|t| t.kind == Kind::Query) {
                    QUERY_DATASET_NOT_FOUND
                } else {
                    INVALID_ARGUMENT
                },
            ),
            FriendsEnumerate | Teams | Members | Feedback | Query => {
                if !connected {
                    return Ok(NO_SESSION);
                }
                let (event, out, kind) = match op {
                    FriendsEnumerate => {
                        if a[0] >= 4 {
                            return Ok(INVALID_ARGUMENT);
                        }
                        (a[1], a[2], Kind::Friends)
                    }
                    Teams => {
                        if a[0] >= 4 {
                            return Ok(INVALID_ARGUMENT);
                        }
                        (a[4], a[5], Kind::Teams)
                    }
                    Members => {
                        if a[0] >= 4 {
                            return Ok(INVALID_ARGUMENT);
                        }
                        (a[5], a[6], Kind::Members)
                    }
                    Feedback => {
                        if a[0] >= 4 {
                            return Ok(INVALID_ARGUMENT);
                        }
                        if a[5] != 0 {
                            m.check(a[5], 4, Access::Read, 1)?;
                        }
                        (a[6], a[7], Kind::Feedback)
                    }
                    Query => (a[8], a[9], Kind::Query),
                    _ => unreachable!(),
                };
                self.task(m, storage, event, out, kind)
            }
            _ => unreachable!(),
        }
    }
}
