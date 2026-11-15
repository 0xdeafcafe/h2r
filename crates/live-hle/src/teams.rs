//! Teams guest ABI over the owned, deterministic store in live-proto::team.
//! Task snapshots survive later mutations. The store survives local sign-out.
use crate::{
    client::{Pending, Reply, Service, Session, TeamRequest},
    outbox::Outbox,
    state::{Error, Storage},
    status::*,
};
use live_proto::{
    Xuid,
    team::{self, Action, Identity, Member, MemberProperties, Properties, Store, Team, TeamError},
};
use runtime::{Guest, GuestFault, guest::Access};
use std::collections::BTreeMap;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
pub const TEAM_PROPERTIES_SIZE: usize = 1670;
pub const TEAM_SIZE: usize = 1698;
pub const MEMBER_SIZE: usize = 146;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Create,
    Delete,
    ByUser,
    Results,
    Details,
    Members,
    MemberResults,
    MemberDetails,
    SetMember,
    Remove,
    Recruit,
    RecruitName,
    Answer,
}
impl Op {
    pub fn starts(self) -> bool {
        !matches!(
            self,
            Self::Results | Self::Details | Self::MemberResults | Self::MemberDetails
        )
    }
}
pub(crate) struct Context<'a> {
    pub storage: &'a mut Storage,
    pub outbox: &'a mut Outbox,
    pub actor: Option<Identity>,
    pub known: &'a [Identity],
    pub service: Option<&'a Service>,
    pub session: Option<&'a Session>,
}
#[derive(Clone)]
enum View {
    Teams(Vec<Team>),
    Members(Vec<(Member, bool)>),
    Done,
}
struct Task {
    status: u32,
    viewer: Xuid,
    view: View,
    remote: Option<(Pending, TeamRequest, u32)>,
}
#[derive(Default)]
pub(crate) struct Teams {
    pub store: Store,
    tasks: BTreeMap<u32, Task>,
}
enum Failure {
    Fault(GuestFault),
    Hr(u32),
}
impl From<GuestFault> for Failure {
    fn from(f: GuestFault) -> Self {
        Self::Fault(f)
    }
}
type Result<T> = std::result::Result<T, Failure>;
fn hr<T>(v: u32) -> Result<T> {
    Err(Failure::Hr(v))
}
impl Teams {
    pub fn clear_tasks(&mut self) {
        self.tasks.clear();
    }
    pub fn close(&mut self, h: u32, outbox: &mut Outbox) -> bool {
        if self.tasks.remove(&h).is_some() {
            outbox.close(h);
            true
        } else {
            false
        }
    }
    pub fn poll(&mut self, h: u32) -> Option<u32> {
        let task = self.tasks.get_mut(&h)?;
        let Some((pending, _, _)) = &task.remote else {
            return Some(task.status);
        };
        let Some(reply) = pending.take() else {
            return Some(0);
        };
        let (_, request, flags) = task.remote.take().unwrap();
        let teams = match reply {
            Ok(Reply::Teams(v)) => v,
            Err(e) => {
                task.status = e;
                return Some(e);
            }
            _ => {
                task.status = INVALID_ARGUMENT;
                return Some(task.status);
            }
        };
        match &request {
            TeamRequest::User(user) => {
                for cached in self.store.teams.values_mut() {
                    if !teams.iter().any(|t| t.id == cached.id) {
                        cached.members.remove(user);
                        cached.recruits.remove(user);
                    }
                }
                task.view = View::Teams(teams.clone());
            }
            TeamRequest::Team(_) => {
                let mut members = Vec::new();
                for t in &teams {
                    members.extend(t.members.values().cloned().map(|v| (v, false)));
                    if flags & 1 != 0 {
                        members.extend(t.recruits.values().cloned().map(|v| (v, true)));
                    }
                }
                task.view = View::Members(members);
            }
            TeamRequest::Change(action) => {
                if matches!(action, Action::Delete { .. }) {
                    self.store.teams.remove(&action.id());
                }
                task.view = View::Done;
            }
        }
        for team in teams {
            self.store.teams.insert(team.id, team);
        }
        task.status = TASK_SUCCESS;
        Some(TASK_SUCCESS)
    }
    pub fn invoke(
        &mut self,
        op: Op,
        a: &[u32],
        m: &mut Guest,
        ctx: Context<'_>,
    ) -> std::result::Result<u32, Error> {
        match self.checked(op, a, m, ctx) {
            Ok(v) => Ok(v),
            Err(Failure::Hr(v)) => Ok(v),
            Err(Failure::Fault(f)) => Err(Error::Fault(f)),
        }
    }
    fn checked(&mut self, op: Op, a: &[u32], m: &mut Guest, ctx: Context<'_>) -> Result<u32> {
        if !op.starts() {
            return self.results(op, a, m);
        }
        let actor = ctx.actor.clone().ok_or(Failure::Hr(NO_USER))?;
        let (event, out) = match op {
            Op::Create | Op::Delete | Op::ByUser => (a[4], a[5]),
            Op::Members | Op::Answer => (a[5], a[6]),
            Op::SetMember => (a[8], a[9]),
            Op::Remove | Op::RecruitName => (a[7], a[8]),
            Op::Recruit => (a[9], a[10]),
            _ => unreachable!(),
        };
        if event != 0 {
            return hr(INVALID_ARGUMENT);
        }
        m.check(out, 4, Access::Write, 1)?;
        if self.tasks.len() >= 128 {
            return hr(OUT_OF_MEMORY);
        }
        let now = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
        let action = match op {
            Op::ByUser => {
                let viewer = xuid(&a[1..4]);
                if ctx.service.is_some() {
                    return self.remote(m, ctx, out, viewer, TeamRequest::User(viewer), 0);
                }
                let view = View::Teams(self.store.of(viewer));
                return self.task(m, ctx.storage, out, viewer, Ok(view));
            }
            Op::Members => {
                if a[4] & !3 != 0 {
                    return hr(INVALID_ARGUMENT);
                }
                if ctx.service.is_some() {
                    return self.remote(
                        m,
                        ctx,
                        out,
                        actor.xuid,
                        TeamRequest::Team(xuid(&a[1..4])),
                        a[4],
                    );
                }
                let result = self.store.get(xuid(&a[1..4])).map(|t| {
                    let mut members: Vec<_> =
                        t.members.values().cloned().map(|v| (v, false)).collect();
                    if a[4] & 1 != 0 {
                        members.extend(t.recruits.values().cloned().map(|v| (v, true)));
                    }
                    View::Members(members)
                });
                return self.task(m, ctx.storage, out, actor.xuid, result);
            }
            Op::Create => Action::Create {
                id: new_id()?,
                properties: properties(m, a[1])?,
                member: member_properties(m, a[2])?,
                max_members: a[3],
            },
            Op::Delete => Action::Delete { id: xuid(&a[1..4]) },
            Op::SetMember => Action::MemberProperties {
                id: xuid(&a[1..4]),
                user: xuid(&a[4..7]),
                properties: member_properties(m, a[7])?,
            },
            Op::Remove => Action::Remove {
                id: xuid(&a[1..4]),
                user: xuid(&a[4..7]),
            },
            Op::Answer => {
                if a[4] > 2 {
                    return hr(INVALID_ARGUMENT);
                }
                Action::Answer {
                    id: xuid(&a[1..4]),
                    accept: a[4] == 1,
                }
            }
            Op::Recruit | Op::RecruitName => {
                let target = if op == Op::Recruit {
                    let uid = xuid(&a[4..7]);
                    ctx.known
                        .iter()
                        .find(|i| i.xuid == uid)
                        .cloned()
                        .or_else(|| {
                            ctx.service.map(|_| Identity {
                                xuid: uid,
                                gamertag: String::new(),
                            })
                        })
                } else {
                    let name = ansi(m, a[4])?;
                    ctx.known
                        .iter()
                        .find(|i| i.gamertag.eq_ignore_ascii_case(&name))
                        .cloned()
                        .or_else(|| {
                            ctx.service.map(|_| Identity {
                                xuid: Xuid(0),
                                gamertag: name,
                            })
                        })
                };
                let Some(target) = target else {
                    return self.task(
                        m,
                        ctx.storage,
                        out,
                        actor.xuid,
                        Err(TeamError::UserNotFound),
                    );
                };
                Action::Recruit {
                    id: xuid(&a[1..4]),
                    target,
                    properties: member_properties(m, if op == Op::Recruit { a[7] } else { a[5] })?,
                }
            }
            _ => unreachable!(),
        };
        if ctx.service.is_some() {
            if matches!(op, Op::Recruit | Op::RecruitName) {
                let handle = if op == Op::Recruit { a[8] } else { a[6] };
                if !ctx.outbox.accepts(handle, 4) {
                    return hr(INVALID_ARGUMENT);
                }
            }
            return self.remote(m, ctx, out, actor.xuid, TeamRequest::Change(action), 0);
        }
        let mut next = self.store.clone();
        let result = next.apply(&actor, &action, &now);
        if let Err(error) = result {
            return self.task(m, ctx.storage, out, actor.xuid, Err(error));
        }
        if next.teams.len() > 1024 {
            return hr(OUT_OF_MEMORY);
        }
        let h = if matches!(op, Op::Recruit | Op::RecruitName) {
            let send_op = if op == Op::Recruit {
                crate::outbox::Op::Recruit
            } else {
                crate::outbox::Op::RecruitByName
            };
            match ctx.outbox.invoke(send_op, a, m, ctx.storage, actor.xuid.0) {
                Ok(0) => m.u32(out)?,
                Ok(e) => return Ok(e),
                Err(Error::Fault(f)) => return Err(Failure::Fault(f)),
                Err(Error::Unverified(_)) => unreachable!(),
            }
        } else {
            let h = ctx.storage.alloc(m, 8).ok_or(Failure::Hr(OUT_OF_MEMORY))?;
            m.set_u32(out, h)?;
            h
        };
        self.store = next;
        self.tasks.insert(
            h,
            Task {
                status: TASK_SUCCESS,
                viewer: actor.xuid,
                view: View::Done,
                remote: None,
            },
        );
        if std::env::var_os("H2_BOOT_VERBOSE").is_some() {
            eprintln!("live team debug: actor={} action={action:?}", actor.xuid);
        }
        Ok(0)
    }
    fn task(
        &mut self,
        m: &mut Guest,
        storage: &mut Storage,
        out: u32,
        viewer: Xuid,
        result: std::result::Result<View, TeamError>,
    ) -> Result<u32> {
        let (status, view) = match result {
            Ok(v) => (TASK_SUCCESS, v),
            Err(e) => (e.hresult(), View::Done),
        };
        let h = storage.alloc(m, 8).ok_or(Failure::Hr(OUT_OF_MEMORY))?;
        m.set_u32(out, h)?;
        self.tasks.insert(
            h,
            Task {
                status,
                viewer,
                view,
                remote: None,
            },
        );
        Ok(0)
    }
    fn remote(
        &mut self,
        m: &mut Guest,
        ctx: Context<'_>,
        out: u32,
        viewer: Xuid,
        request: TeamRequest,
        flags: u32,
    ) -> Result<u32> {
        let actor = ctx.actor.as_ref().ok_or(Failure::Hr(NO_USER))?;
        let service = ctx.service.ok_or(Failure::Hr(NO_SESSION))?;
        let session = ctx.session.ok_or(Failure::Hr(NO_SESSION))?;
        let h = ctx.storage.alloc(m, 8).ok_or(Failure::Hr(OUT_OF_MEMORY))?;
        let pending = service
            .teams(session, actor.xuid.0, request.clone())
            .map_err(Failure::Hr)?;
        m.set_u32(out, h)?;
        self.tasks.insert(
            h,
            Task {
                status: 0,
                viewer,
                view: View::Done,
                remote: Some((pending, request, flags)),
            },
        );
        Ok(0)
    }
    fn results(&mut self, op: Op, a: &[u32], m: &mut Guest) -> Result<u32> {
        if self.poll(a[0]) == Some(0) {
            return hr(TASK_BUSY);
        }
        let Some(task) = self.tasks.get(&a[0]) else {
            return hr(INVALID_ARGUMENT);
        };
        if task.status & 0x80000000 != 0 {
            return Ok(task.status);
        }
        match (op, &task.view) {
            (Op::Results, View::Teams(teams)) => {
                ids(m, a[1], a[2], teams.iter().map(|t| t.id).collect())
            }
            (Op::MemberResults, View::Members(members)) => ids(
                m,
                a[1],
                a[2],
                members.iter().map(|(v, _)| v.identity.xuid).collect(),
            ),
            (Op::Details, View::Teams(teams)) => {
                let Some(team) = teams.iter().find(|t| t.id == xuid(&a[1..4])) else {
                    return hr(0x80004005);
                };
                let bytes = team_record(team, task.viewer);
                m.write_bytes(a[4], &bytes, 1)?;
                Ok(0)
            }
            (Op::MemberDetails, View::Members(members)) => {
                let Some((member, pending)) = members
                    .iter()
                    .find(|(v, _)| v.identity.xuid == xuid(&a[1..4]))
                else {
                    return hr(0x80004005);
                };
                let bytes = member_record(member, *pending);
                m.write_bytes(a[4], &bytes, 1)?;
                Ok(0)
            }
            _ => hr(INVALID_ARGUMENT),
        }
    }
}
fn ids(m: &mut Guest, count: u32, out: u32, ids: Vec<Xuid>) -> Result<u32> {
    m.check(count, 4, Access::Write, 1)?;
    m.check(out, ids.len() * 12, Access::Write, 1)?;
    let mut bytes = vec![0; ids.len() * 12];
    for (record, id) in bytes.as_chunks_mut::<12>().0.iter_mut().zip(&ids) {
        record[..8].copy_from_slice(&id.0.to_le_bytes());
    }
    m.write_bytes(out, &bytes, 1)?;
    m.set_u32(count, ids.len() as u32)?;
    Ok(0)
}
fn xuid(a: &[u32]) -> Xuid {
    Xuid(u64::from(a[0]) | u64::from(a[1]) << 32)
}
fn new_id() -> Result<Xuid> {
    let b = crate::network::random(8).map_err(|_| Failure::Hr(OUT_OF_MEMORY))?;
    Ok(Xuid(
        (u64::from_le_bytes(b.try_into().unwrap()) & 0x0000ffffffffffff) | 0x000a000000000000,
    ))
}
fn read(m: &Guest, p: u32, len: usize) -> Result<Vec<u8>> {
    let mut bytes = vec![0; len];
    m.read_bytes(p, &mut bytes, 1)?;
    Ok(bytes)
}
fn wide(bytes: &[u8]) -> Result<String> {
    let units: Vec<_> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|v| u16::from_le_bytes(*v))
        .collect();
    let n = units
        .iter()
        .position(|u| *u == 0)
        .ok_or(Failure::Hr(INVALID_ARGUMENT))?;
    String::from_utf16(&units[..n]).map_err(|_| Failure::Hr(INVALID_ARGUMENT))
}
fn properties(m: &Guest, p: u32) -> Result<Properties> {
    let b = read(m, p, TEAM_PROPERTIES_SIZE)?;
    let n = u16::from_le_bytes(b[1568..1570].try_into().unwrap()) as usize;
    if n > 100 {
        return hr(INVALID_ARGUMENT);
    }
    let result = Properties {
        name: wide(&b[..32])?,
        description: wide(&b[32..544])?,
        motto: wide(&b[544..1056])?,
        url: wide(&b[1056..1568])?,
        data: b[1570..1570 + n].to_vec(),
    };
    team::validate_properties(&result).map_err(|e| Failure::Hr(e.hresult()))?;
    Ok(result)
}
fn member_properties(m: &Guest, p: u32) -> Result<MemberProperties> {
    let b = read(m, p, 106)?;
    let n = u16::from_le_bytes(b[4..6].try_into().unwrap()) as usize;
    if n > 100 {
        return hr(INVALID_ARGUMENT);
    }
    let p = MemberProperties {
        privileges: u32::from_le_bytes(b[..4].try_into().unwrap()),
        data: b[6..6 + n].to_vec(),
    };
    team::validate_member(&p).map_err(|e| Failure::Hr(e.hresult()))?;
    Ok(p)
}
fn ansi(m: &Guest, p: u32) -> Result<String> {
    let mut bytes = Vec::new();
    for i in 0..16 {
        let b = m.u8(p.wrapping_add(i))?;
        if b == 0 {
            return String::from_utf8(bytes).map_err(|_| Failure::Hr(INVALID_ARGUMENT));
        }
        bytes.push(b);
    }
    hr(INVALID_ARGUMENT)
}
fn write_wide(out: &mut [u8], s: &str) {
    for (dst, u) in out.as_chunks_mut::<2>().0.iter_mut().zip(s.encode_utf16()) {
        dst.copy_from_slice(&u.to_le_bytes());
    }
}
fn properties_record(p: &Properties) -> Vec<u8> {
    let mut b = vec![0; TEAM_PROPERTIES_SIZE];
    write_wide(&mut b[..32], &p.name);
    write_wide(&mut b[32..544], &p.description);
    write_wide(&mut b[544..1056], &p.motto);
    write_wide(&mut b[1056..1568], &p.url);
    b[1568..1570].copy_from_slice(&(p.data.len() as u16).to_le_bytes());
    b[1570..1570 + p.data.len()].copy_from_slice(&p.data);
    b
}
fn member_properties_record(p: &MemberProperties) -> Vec<u8> {
    let mut b = vec![0; 106];
    b[..4].copy_from_slice(&p.privileges.to_le_bytes());
    b[4..6].copy_from_slice(&(p.data.len() as u16).to_le_bytes());
    b[6..6 + p.data.len()].copy_from_slice(&p.data);
    b
}
fn filetime(at: &str) -> u64 {
    OffsetDateTime::parse(at, &Rfc3339)
        .ok()
        .map(|v| (v.unix_timestamp_nanos() / 100 + 116444736000000000) as u64)
        .unwrap_or(0)
}
fn team_record(t: &Team, viewer: Xuid) -> Vec<u8> {
    let mut b = vec![0; TEAM_SIZE];
    b[..8].copy_from_slice(&t.id.0.to_le_bytes());
    b[12..1682].copy_from_slice(&properties_record(&t.properties));
    b[1682..1686].copy_from_slice(&u32::from(t.recruits.contains_key(&viewer)).to_le_bytes());
    b[1686..1694].copy_from_slice(&filetime(&t.created_at).to_le_bytes());
    b[1694..1698].copy_from_slice(&(t.members.len() as u32).to_le_bytes());
    b
}
fn member_record(m: &Member, pending: bool) -> Vec<u8> {
    let mut b = vec![0; MEMBER_SIZE];
    b[..8].copy_from_slice(&m.identity.xuid.0.to_le_bytes());
    let name = m.identity.gamertag.as_bytes();
    let n = name.len().min(15);
    b[12..12 + n].copy_from_slice(&name[..n]);
    b[28..134].copy_from_slice(&member_properties_record(&m.properties));
    b[134..138].copy_from_slice(&u32::from(pending).to_le_bytes());
    b[138..146].copy_from_slice(&filetime(&m.joined_at).to_le_bytes());
    b
}
