//! Causal field predictors over the v7 quantization grids.
use super::*;
const HIT_LIST: usize = LIST_COUNT;
const LISTS_WITH_HITS: usize = LIST_COUNT + 1;
const CHANNELS: usize = 24;
const WIDTH: usize = 128;
pub(super) const META: usize = LISTS_WITH_HITS * CHANNELS * WIDTH;

#[derive(Default, Clone, Copy)]
struct FieldHistory {
    previous: [i64; 3],
    slope: [i64; 3],
    present: bool,
    shared: bool,
}
impl FieldHistory {
    fn update(&mut self, v: [i64; 3]) {
        self.slope = if self.present {
            std::array::from_fn(|i| v[i] - self.previous[i])
        } else {
            [0; 3]
        };
        self.present = true;
        self.previous = v;
    }
}

type Header = (String, Option<u32>, Option<u64>, bool);
#[derive(Default)]
pub(super) struct History {
    rows: [Vec<Vec<FieldHistory>>; LISTS_WITH_HITS],
    lengths: [usize; LISTS_WITH_HITS],
    cold: [Vec<Entity>; LISTS_WITH_HITS],
    pub(super) held_lists: u8,
    pub(super) strings: Vec<Val>,
    ids: HashMap<Val, usize>,
    pub(super) meta: Header,
    pub(super) tick: u64,
    pub(super) seq: u64,
}
pub(super) trait Symbols {
    const WRITING: bool;
    fn bit(&mut self, ctx: usize, want: bool) -> bool;
    fn uint(&mut self, ctx: usize, want: u64) -> u64;
    fn signed(&mut self, ctx: usize, want: i64) -> i64 {
        unzigzag(self.uint(ctx, zigzag(want)))
    }
    fn literal(&mut self, want: u8) -> u8;
}
#[derive(Default)]
struct References {
    positions: Vec<Val>,
    first_aim: Option<([f64; 3], u64)>,
}
struct RowContext {
    direction: Option<[u32; 2]>,
    travel: Option<[i64; 3]>,
    hit: bool,
}
fn direction_base(first: Option<([f64; 3], u64)>, yaw: Option<&Val>) -> Option<[u32; 2]> {
    let (v, base) = first?;
    let Val::Int(y) = yaw? else { return None };
    crate::archive::rotated(v, dyaw(*y) - dyaw(base)).ok()
}
fn travel_base(vel: Option<&FieldHistory>, dt: u64, regular: bool) -> Option<[i64; 3]> {
    let vel = vel.filter(|v| v.present && dt <= PREDICT_TICKS)?;
    Some(std::array::from_fn(|i| {
        let dv = if regular { vel.slope[i] } else { 0 };
        let d = (2 * vel.previous[i] + dv) * 4 * dt as i64;
        (d + 15 * d.signum()) / 30
    }))
}

impl History {
    fn machine_updates<T: Symbols>(
        &mut self,
        r: &mut T,
        src: &[MachinePosition],
    ) -> Result<Vec<MachinePosition>> {
        if !r.bit(META + 37, !src.is_empty()) {
            return Ok(Vec::new());
        }
        let n = bounded(
            r.uint(META + 96, src.len().saturating_sub(1) as u64),
            MAX_MACHINE_POSITIONS - 1,
        )? + 1;
        let mut delta = Vec::with_capacity(if T::WRITING { 0 } else { n });
        let mut next = 0u64;
        for j in 0..n {
            let gap = src.get(j).map_or(0, |m| u64::from(m.i) - next);
            let idx = next + bounded(r.uint(META + 128, gap), 32767)? as u64;
            if idx > 32767 {
                return Err(PackError::Bad("machine index"));
            }
            let p = r.literal(src.get(j).map_or(0, |m| m.p));
            next = idx + 1;
            if !T::WRITING {
                delta.push(MachinePosition { i: idx as u16, p });
            }
        }
        Ok(delta)
    }
    pub(super) fn read_frame<T: Symbols>(&mut self, r: &mut T, dt: u64, seq: u64) -> Result<Frame> {
        if r.bit(META, false) {
            self.meta = read_meta(&blob(r, META + 1, &[], MAX_STRING + 24)?)?;
        }
        let mut refs = References::default();
        let mut lists = std::array::from_fn(|_| Vec::new());
        for (j, list) in lists.iter_mut().enumerate() {
            *list = self.fields(r, j, None, dt, &mut refs)?;
        }
        let hits = self.fields(r, HIT_LIST, None, dt, &mut refs)?;
        let travel = self.machine_updates(r, &[])?;
        self.tick = self
            .tick
            .checked_add(dt)
            .ok_or(PackError::Bad("tick overflow"))?;
        self.seq = seq + 1;
        let (game, bsp, cut, cinematic) = &self.meta;
        let mut f = frame_of(
            game,
            self.tick,
            bsp.map(u64::from),
            *cut,
            *cinematic,
            &lists,
            &hits,
        )?;
        f.machine_positions = travel;
        Ok(f)
    }

    pub(super) fn write_frame<T: Symbols>(
        &mut self,
        r: &mut T,
        frame: &Frame,
        src: &Lists,
        dt: u64,
    ) -> Result<()> {
        let meta = (&frame.game_id, frame.bsp, frame.cut, frame.cinematic);
        if r.bit(
            META,
            meta != (&self.meta.0, self.meta.1, self.meta.2, self.meta.3),
        ) {
            let data = write_meta(frame)?;
            blob(r, META + 1, &data, MAX_STRING + 24)?;
            self.meta = (frame.game_id.clone(), frame.bsp, frame.cut, frame.cinematic);
        }
        let mut refs = References::default();
        for (list, rows) in src
            .lists
            .iter()
            .chain(std::iter::once(&src.hits))
            .enumerate()
        {
            self.fields(r, list, Some(rows), dt, &mut refs)?;
        }
        self.machine_updates(r, &frame.machine_positions)?;
        Ok(())
    }

    fn fields<T: Symbols>(
        &mut self,
        r: &mut T,
        list: usize,
        src: Option<&[Entity]>,
        dt: u64,
        refs: &mut References,
    ) -> Result<Vec<Entity>> {
        let (schema, max) = if list == HIT_LIST {
            (HIT, MAX_FRAME_HITS)
        } else {
            LISTS[list]
        };
        let base = list * CHANNELS * WIDTH;
        let static_list = matches!(list, 3 | 4 | 6);
        if static_list && r.bit(base + 23 * WIDTH + 40, self.held_lists & (1 << list) != 0) {
            return Ok(if T::WRITING {
                Vec::new()
            } else {
                self.cold[list].clone()
            });
        }
        let want = src.map_or(0, <[Entity]>::len) as i64;
        let n = bounded(
            (self.lengths[list] as i64
                + r.signed(base + 23 * WIDTH, want - self.lengths[list] as i64)) as u64,
            max,
        )?;
        self.lengths[list] = n;
        let rows = &mut self.rows[list];
        rows.resize_with(n, || vec![FieldHistory::default(); schema.len()]);
        let velocity = schema.iter().position(|f| matches!(f.kind, Kind::Vel));
        let is_unit = list == 0 || list == 2;
        let aim = if list == 0 { 6 } else { 4 };
        // A single decision covers all slowly changing fields in this row.
        // The ordinary field grammar handles every exception and list replacement.
        let full_mask = (1u32 << schema.len()) - 1;
        let hot_mask = match list {
            0 => {
                (1 << 2)
                    | (1 << 3)
                    | (1 << 6)
                    | (1 << 8)
                    | (1 << 13)
                    | (1 << 14)
                    | (1 << 16)
                    | (1 << 20)
            }
            1 => (1 << 2) | (1 << 3) | (1 << 4) | (1 << 7) | (1 << 8),
            _ => full_mask,
        };
        let grouped = hot_mask != full_mask;
        let cache = &mut self.cold[list];
        if grouped {
            cache.resize_with(n, Vec::new);
        }
        refs.first_aim = None;
        let mut out = Vec::with_capacity(if T::WRITING { 0 } else { n });
        for (i, hist) in rows.iter_mut().enumerate() {
            let mut e: Entity = Vec::with_capacity(if T::WRITING { 0 } else { schema.len() });
            let source = src.map(|v| &v[i]);
            let mut ctx = RowContext {
                direction: None,
                travel: travel_base(velocity.map(|vi| &hist[vi]), dt, is_unit),
                hit: list == HIT_LIST,
            };
            let held = grouped
                && r.bit(
                    base + 23 * WIDTH + 41,
                    source.is_some_and(|s| {
                        let prior = &cache[i];
                        if prior.is_empty() {
                            return false;
                        }
                        let mut bits = full_mask ^ hot_mask;
                        while bits != 0 {
                            let j = bits.trailing_zeros() as usize;
                            bits &= bits - 1;
                            if s[j] != prior[j] {
                                return false;
                            }
                        }
                        true
                    }),
                );
            if !T::WRITING && held {
                if cache[i].len() != schema.len() {
                    return Err(PackError::Bad("unseeded row"));
                }
                e.clone_from(&cache[i]);
            }
            let mut mask = if held { hot_mask } else { full_mask };
            while mask != 0 {
                let j = mask.trailing_zeros() as usize;
                mask &= mask - 1;
                let field = &schema[j];
                if is_unit && j == aim {
                    ctx.direction = direction_base(refs.first_aim, source.unwrap_or(&e).get(3));
                } else {
                    ctx.direction = None;
                }
                let expect = source.map(|s| &s[j]);
                let value = channel(
                    r,
                    base + j * WIDTH,
                    field,
                    &mut hist[j],
                    expect,
                    &ctx,
                    &refs.positions,
                    &mut self.strings,
                    &mut self.ids,
                )?;
                if !T::WRITING {
                    if held {
                        e[j] = value;
                    } else {
                        e.push(value);
                    }
                }
            }
            let actual = source.unwrap_or(&e);
            if grouped && !held {
                cache[i].clone_from(actual);
            }
            if list == 0 {
                refs.positions.push(actual[2].clone());
            }
            if i == 0
                && is_unit
                && let (Val::Dir(q), Val::Int(y)) = (&actual[aim], &actual[3])
            {
                refs.first_aim = Some((unoct(*q), *y));
            }
            if !T::WRITING {
                out.push(e);
            }
        }
        if static_list {
            src.unwrap_or(&out).clone_into(cache);
        }
        Ok(out)
    }
}
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn channel<T: Symbols>(
    r: &mut T,
    ctx: usize,
    field: &Field,
    h: &mut FieldHistory,
    src: Option<&Val>,
    row: &RowContext,
    players: &[Val],
    strings: &mut Vec<Val>,
    ids: &mut HashMap<Val, usize>,
) -> Result<Val> {
    let was = h.present;
    if field.opt && !r.bit(ctx + usize::from(was), src.is_some_and(|v| *v != Val::None)) {
        *h = FieldHistory::default();
        return Ok(Val::None);
    }
    let lane = ctx + 8;
    if matches!(
        field.kind,
        Kind::Id | Kind::Str | Kind::Actions | Kind::Agos
    ) {
        let (id, v) = dictionary(r, lane, field.kind, src, h.previous[0], strings, ids)?;
        h.update([id as i64, 0, 0]);
        return Ok(v);
    }
    if matches!(field.kind, Kind::Bool) {
        let v = r.bit(
            ctx + 2 + usize::from(h.previous[0] != 0),
            matches!(src, Some(Val::Bool(true))),
        );
        h.update([i64::from(v), 0, 0]);
        return Ok(Val::Bool(v));
    }
    let mut value = match src {
        Some(Val::Int(n)) => [*n as i64, 0, 0],
        Some(Val::Pos(q) | Val::Vel(q)) => *q,
        Some(Val::Dir(q)) => [i64::from(q[0]), i64::from(q[1]), 0],
        _ => [0; 3],
    };
    let count = match field.kind {
        Kind::Pos | Kind::Vel => 3,
        Kind::Dir => 2,
        _ => 1,
    };
    if row.hit && matches!(field.kind, Kind::Pos) && !players.is_empty() {
        let ordinal = src.and_then(|p| players.iter().position(|v| v == p));
        if r.bit(ctx + 4, ordinal.is_some()) {
            let index = bounded(r.uint(lane, ordinal.unwrap_or(0) as u64), players.len() - 1)?;
            return Ok(players[index].clone());
        }
    }
    let mut shift = [0; 3];
    if let Some(base) = row.direction {
        let share = (0..2).all(|k| (value[k] - i64::from(base[k])).abs() <= 16);
        let share = r.bit(ctx + 5 + usize::from(h.shared), share);
        if share != h.shared {
            *h = FieldHistory::default();
            h.shared = share;
        }
        if share {
            shift = [i64::from(base[0]), i64::from(base[1]), 0];
        }
    } else if h.shared {
        *h = FieldHistory::default();
    }
    for k in 0..count {
        let prev = h.previous[k];
        let pred = match field.kind {
            Kind::Pos => prev + row.travel.map_or(h.slope[k], |d| d[k]),
            Kind::Vel | Kind::Dir | Kind::Angle | Kind::AnimPhase => prev + h.slope[k],
            _ => prev,
        };
        let mut delta = value[k] - shift[k] - pred;
        if matches!(field.kind, Kind::Angle) {
            delta = (delta + 8192).rem_euclid(16384) - 8192;
        }
        value[k] = pred + r.signed(lane + 32 * k, delta);
        if matches!(field.kind, Kind::Angle) {
            value[k] = value[k].rem_euclid(16384);
        }
    }
    if !T::WRITING {
        validate_number(field.kind, value, shift)?;
    }
    h.update(value);
    Ok(match field.kind {
        Kind::Pos => Val::Pos(value),
        Kind::Vel => Val::Vel(value),
        Kind::Dir => Val::Dir([(value[0] + shift[0]) as u32, (value[1] + shift[1]) as u32]),
        _ => Val::Int(value[0] as u64),
    })
}
