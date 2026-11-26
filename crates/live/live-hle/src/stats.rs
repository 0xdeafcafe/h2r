//! Legacy XOnline stats. Writes are accepted locally and captured; captures
//! own all nested values, and guest pointers never survive the call. Reads
//! of a playlist's leaderboard ask the server for the players' ranks
//! (docs/live/live-matchmaking.md §8); every other stat reads as absent.
use crate::{
    client::{Pending, Reply, Service, Session},
    state::{Error, Storage},
    status::*,
};
use live_proto::rank::{self, LeaderboardRow};
use runtime::{Guest, guest::Access};
use std::collections::{BTreeMap, VecDeque};

const SPEC_SIZE: u32 = 24;
const STAT_SIZE: u32 = 16;
const MAX_WRITE_SPECS: u32 = 20;
const MAX_READ_SPECS: u32 = 64;
const MAX_STATS: u32 = 64;
const MAX_STRING: u32 = 32;
const MAX_WRITES: usize = 256;
const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum StatValue {
    None,
    Long(i32),
    LongLong(i64),
    Double(f64),
    String(String),
}
#[derive(Debug, Clone, PartialEq)]
pub struct Stat {
    pub id: u16,
    pub value: StatValue,
}
#[derive(Debug, Clone, PartialEq)]
pub struct StatSpec {
    pub xuid: u64,
    pub user_flags: u32,
    pub leaderboard: u32,
    pub stats: Vec<Stat>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct StatWrite {
    pub sequence: u64,
    pub captured_at_unix_ms: i64,
    pub specs: Vec<StatSpec>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Write,
    WriteResult,
    Read,
    ReadResult,
}
#[derive(Clone)]
struct ReadQuery {
    xuid: u64,
    leaderboard: u32,
    ids: Vec<u16>,
}
struct ReadSpec {
    query: ReadQuery,
    stats_ptr: u32,
}
enum Task {
    Write,
    Read(Read),
}
struct Read {
    queries: Vec<ReadQuery>,
    /// The server's answer, while it has not come.
    pending: Option<Pending>,
    rows: Vec<LeaderboardRow>,
}
pub(crate) struct Context<'a> {
    pub storage: &'a mut Storage,
    pub service: Option<&'a Service>,
    pub session: Option<&'a Session>,
    /// The Live session this console is in (hex), for made-up bots'
    /// roster levels (docs/live/live-matchmaking.md §8.4).
    pub game_session: Option<String>,
}
#[derive(Default)]
pub(crate) struct Stats {
    tasks: BTreeMap<u32, Task>,
    writes: VecDeque<StatWrite>,
    bytes: usize,
    sequence: u64,
}
type Result<T> = std::result::Result<T, Error>;
// Parsing errors distinguish an invalid SDK value from a checked pointer fault.
enum ParseError {
    Fault(runtime::GuestFault),
    Status(u32),
}
impl From<runtime::GuestFault> for ParseError {
    fn from(e: runtime::GuestFault) -> Self {
        Self::Fault(e)
    }
}
type Parsed<T> = std::result::Result<T, ParseError>;
impl Stats {
    pub fn snapshot(&self) -> Vec<StatWrite> {
        self.writes.iter().cloned().collect()
    }
    pub fn drain(&mut self) -> Vec<StatWrite> {
        self.bytes = 0;
        self.writes.drain(..).collect()
    }
    pub fn clear_tasks(&mut self) {
        self.tasks.clear();
    }
    pub fn close(&mut self, h: u32) -> bool {
        self.tasks.remove(&h).is_some()
    }
    /// The task's status: 0 while the server has not answered a read. A
    /// read the server fails completes with every stat absent, as before
    /// reads reached it: the title reads absent as zero, level 1.
    pub fn poll(&mut self, h: u32) -> Option<u32> {
        if let Some(Task::Read(r)) = self.tasks.get_mut(&h)
            && let Some(reply) = r.pending.as_ref().and_then(Pending::take)
        {
            r.pending = None;
            if let Ok(Reply::Ranks(rows)) = reply {
                r.rows = rows;
            }
        }
        self.tasks.get(&h).map(|t| match t {
            Task::Read(Read {
                pending: Some(_), ..
            }) => 0,
            _ => TASK_SUCCESS,
        })
    }
    fn task(&mut self, m: &mut Guest, storage: &mut Storage, out: u32, task: Task) -> Result<u32> {
        let Some(h) = storage.alloc(m, 8) else {
            return Ok(OUT_OF_MEMORY);
        };
        m.set_u32(out, h)?;
        self.tasks.insert(h, task);
        Ok(0)
    }
    pub fn invoke(&mut self, op: Op, a: &[u32], m: &mut Guest, ctx: Context<'_>) -> Result<u32> {
        let result = self.invoke_parsed(op, a, m, ctx);
        match result {
            Ok(hr) => Ok(hr),
            Err(ParseError::Status(hr)) => Ok(hr),
            Err(ParseError::Fault(f)) => Err(Error::Fault(f)),
        }
    }
    fn invoke_parsed(&mut self, op: Op, a: &[u32], m: &mut Guest, ctx: Context<'_>) -> Parsed<u32> {
        let storage = ctx.storage;
        match op {
            Op::Write | Op::Read => {
                if a[2] != 0 {
                    return Err(ParseError::Status(INVALID_ARGUMENT));
                }
                m.check(a[3], 4, Access::Write, 1)?;
                if op == Op::Write {
                    let specs = write_specs(m, a[1], a[0])?;
                    let bytes = charge(&specs);
                    if self.writes.len() == MAX_WRITES || bytes > MAX_BYTES - self.bytes {
                        return Ok(OUT_OF_MEMORY);
                    }
                    let Some(sequence) = self.sequence.checked_add(1) else {
                        return Ok(OUT_OF_MEMORY);
                    };
                    let hr = self
                        .task(m, storage, a[3], Task::Write)
                        .map_err(task_error)?;
                    if hr != 0 {
                        return Ok(hr);
                    }
                    let write = StatWrite {
                        sequence,
                        captured_at_unix_ms: (time::OffsetDateTime::now_utc()
                            .unix_timestamp_nanos()
                            / 1_000_000) as i64,
                        specs,
                    };
                    if std::env::var_os("H2_BOOT_VERBOSE").is_some() {
                        eprintln!("live stats debug: {write:?}");
                    }
                    self.sequence = sequence;
                    self.bytes += bytes;
                    self.writes.push_back(write);
                    Ok(0)
                } else {
                    let queries: Vec<ReadQuery> = read_specs(m, a[1], a[0])?
                        .into_iter()
                        .map(|s| s.query)
                        .collect();
                    let pending = match ranks_request(&queries) {
                        Some((boards, xuids)) => match (ctx.service, ctx.session) {
                            (Some(service), Some(session)) => {
                                match service.ranks(
                                    session,
                                    boards,
                                    xuids,
                                    ctx.game_session.clone(),
                                ) {
                                    Ok(p) => Some(p),
                                    Err(hr) => return Ok(hr),
                                }
                            }
                            _ => None,
                        },
                        None => None,
                    };
                    self.task(
                        m,
                        storage,
                        a[3],
                        Task::Read(Read {
                            queries,
                            pending,
                            rows: Vec::new(),
                        }),
                    )
                    .map_err(task_error)
                }
            }
            Op::WriteResult => {
                if !matches!(self.tasks.get(&a[0]), Some(Task::Write)) {
                    return Ok(INVALID_ARGUMENT);
                }
                // Retail's no-attachment success: all optional results are zero.
                for p in &a[1..4] {
                    if *p != 0 {
                        m.check(*p, 4, Access::Write, 1)?;
                    }
                }
                for p in &a[1..4] {
                    if *p != 0 {
                        m.set_u32(*p, 0)?;
                    }
                }
                Ok(0)
            }
            Op::ReadResult => {
                let Some(Task::Read(read)) = self.tasks.get(&a[0]) else {
                    return Ok(INVALID_ARGUMENT);
                };
                if read.pending.is_some() {
                    return Ok(RESULTS_UNAVAILABLE);
                }
                let request = &read.queries;
                if a[1] as usize > request.len() {
                    return Ok(INVALID_ARGUMENT);
                }
                // The SDK receives caller-initialized specs/stat IDs, not blank
                // output structs. It fills types/values in those existing arrays.
                let output = read_specs(m, a[2], a[1])?;
                for (out, source) in output.iter().zip(request) {
                    if out.query.xuid != source.xuid
                        || out.query.leaderboard != source.leaderboard
                        || out.query.ids != source.ids
                    {
                        return Ok(RESULTS_UNAVAILABLE);
                    }
                    m.check(
                        out.stats_ptr,
                        out.query.ids.len() * STAT_SIZE as usize,
                        Access::Write,
                        1,
                    )?;
                }
                // A playlist leaderboard's stats are LONGs. NONE is the XDK's
                // absent-stat representation, which the title treats as zero.
                // Neither needs bytes from the optional extra buffer.
                for out in output {
                    let row = read.rows.iter().find(|r| {
                        r.xuid.0 == out.query.xuid && r.leaderboard == out.query.leaderboard
                    });
                    for (i, id) in out.query.ids.iter().enumerate() {
                        let mut stat = [0u8; 12];
                        if let Some(v) = row.and_then(|r| rank::live_stat(&r.standing, *id)) {
                            stat[..4].copy_from_slice(&1u32.to_le_bytes());
                            stat[4..8].copy_from_slice(&v.to_le_bytes());
                        }
                        m.write_bytes(out.stats_ptr + i as u32 * STAT_SIZE + 4, &stat, 1)?;
                    }
                }
                Ok(0)
            }
        }
    }
}
/// What to ask the server for a read: the leaderboards that could be a
/// playlist's (its playlist id, 1..=100) and ask for a rank stat, and every
/// player. `None` when no query could be one.
fn ranks_request(queries: &[ReadQuery]) -> Option<(Vec<u32>, Vec<u64>)> {
    let wanted = |q: &&ReadQuery| {
        q.xuid != 0
            && live_proto::hopper::layout::PLAYLIST_IDS.contains(&q.leaderboard)
            && q.ids.iter().any(|id| rank::LIVE_STATS.contains(id))
    };
    let mut boards: Vec<u32> = queries
        .iter()
        .filter(wanted)
        .map(|q| q.leaderboard)
        .collect();
    let mut xuids: Vec<u64> = queries.iter().filter(wanted).map(|q| q.xuid).collect();
    boards.sort_unstable();
    boards.dedup();
    xuids.sort_unstable();
    xuids.dedup();
    (!boards.is_empty()).then_some((boards, xuids))
}
fn task_error(e: Error) -> ParseError {
    match e {
        Error::Fault(f) => ParseError::Fault(f),
        Error::Unverified(_) => unreachable!("stats tasks only allocate guest storage"),
    }
}
fn header(m: &Guest, p: u32) -> Parsed<(u64, u32, u32, u32, u32)> {
    Ok((
        m.u64(p)?,
        m.u32(p + 8)?,
        m.u32(p + 12)?,
        m.u32(p + 16)?,
        m.u32(p + 20)?,
    ))
}
fn write_specs(m: &Guest, p: u32, count: u32) -> Parsed<Vec<StatSpec>> {
    if count > MAX_WRITE_SPECS {
        return Err(ParseError::Status(STAT_TOO_MANY_SPECS));
    }
    m.check(p, count as usize * SPEC_SIZE as usize, Access::Read, 1)?;
    let mut specs = Vec::with_capacity(count as usize);
    for i in 0..count {
        let (xuid, user_flags, leaderboard, n, ptr) = header(m, p + i * SPEC_SIZE)?;
        if xuid == 0 {
            return Err(ParseError::Status(STAT_PERMISSION_DENIED));
        }
        if n > MAX_STATS {
            return Err(ParseError::Status(STAT_TOO_MANY_STATS));
        }
        m.check(ptr, n as usize * STAT_SIZE as usize, Access::Read, 1)?;
        let mut stats = Vec::with_capacity(n as usize);
        for j in 0..n {
            let at = ptr + j * STAT_SIZE;
            let id = m.u16(at)?;
            let value = match m.u32(at + 4)? {
                0 => StatValue::None,
                1 => StatValue::Long(m.u32(at + 8)? as i32),
                2 => StatValue::LongLong(m.u64(at + 8)? as i64),
                3 => {
                    let value = f64::from_bits(m.u64(at + 8)?);
                    if !value.is_finite() {
                        return Err(ParseError::Status(STAT_BAD_REQUEST));
                    }
                    StatValue::Double(value)
                }
                4 => StatValue::String(string(m, m.u32(at + 8)?)?),
                _ => return Err(ParseError::Status(STAT_BAD_REQUEST)),
            };
            stats.push(Stat { id, value });
        }
        specs.push(StatSpec {
            xuid,
            user_flags,
            leaderboard,
            stats,
        });
    }
    Ok(specs)
}
fn string(m: &Guest, p: u32) -> Parsed<String> {
    let mut units = Vec::new();
    for i in 0..=MAX_STRING {
        let v = m.u16(p.wrapping_add(i * 2))?;
        if v == 0 {
            return String::from_utf16(&units).map_err(|_| ParseError::Status(STAT_BAD_REQUEST));
        }
        units.push(v);
    }
    Err(ParseError::Status(STAT_BAD_REQUEST))
}
fn read_specs(m: &Guest, p: u32, count: u32) -> Parsed<Vec<ReadSpec>> {
    if count > MAX_READ_SPECS {
        return Err(ParseError::Status(STAT_TOO_MANY_SPECS));
    }
    m.check(p, count as usize * SPEC_SIZE as usize, Access::Read, 1)?;
    let mut specs = Vec::with_capacity(count as usize);
    for i in 0..count {
        let (xuid, _, leaderboard, n, ptr) = header(m, p + i * SPEC_SIZE)?;
        if n > MAX_STATS {
            return Err(ParseError::Status(STAT_TOO_MANY_STATS));
        }
        m.check(ptr, n as usize * STAT_SIZE as usize, Access::Read, 1)?;
        let ids = (0..n)
            .map(|j| m.u16(ptr + j * STAT_SIZE))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        specs.push(ReadSpec {
            query: ReadQuery {
                xuid,
                leaderboard,
                ids,
            },
            stats_ptr: ptr,
        });
    }
    Ok(specs)
}
fn charge(specs: &[StatSpec]) -> usize {
    std::mem::size_of::<StatWrite>()
        + specs
            .iter()
            .map(|s| {
                std::mem::size_of::<StatSpec>()
                    + s.stats
                        .iter()
                        .map(|v| {
                            std::mem::size_of::<Stat>()
                                + match &v.value {
                                    StatValue::String(s) => s.len(),
                                    _ => 0,
                                }
                        })
                        .sum::<usize>()
            })
            .sum::<usize>()
}
