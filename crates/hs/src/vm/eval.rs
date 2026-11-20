//! Evaluator bodies. Each is entered once per loop iteration of
//! `hs_thread_execute` while its frame is on top; `first` is the thread's
//! first-entry flag. An evaluator either starts one child evaluation (an atom
//! completes at once, a call pushes a frame) and returns, or returns its
//! value through [`Vm::ret`], which pops its frame. Locals are indexed in the
//! order the guest evaluator reserves them; the byte counts are its cursor
//! reservations.
use super::thread::{Dest, FINISHED, INSPECT, SAVED_SLEEP};
use super::value::{Cell, HsType};
use super::{
    Binding, EXTERNAL_GLOBALS, Engine, EngineCall, Native, ScenarioGlobals, VmError, VmResult,
    convert,
};
use crate::Datum;

/// The shared engine LCG (`imul 0x19660d; add 0x3c6ef35f`).
fn lcg(e: &mut dyn Engine) -> u32 {
    let s = e.random_seed();
    *s = s.wrapping_mul(0x0019_660d).wrapping_add(0x3c6e_f35f);
    *s
}

/// Authored 30-Hz ticks to game ticks: `cvtsi2ss`, `* (1/30)` (`0x0045dc6c`),
/// `* tick rate`, all single precision, then `fistp`.
fn scaled_ticks(e: &dyn Engine, ticks: i32) -> i32 {
    convert::fistp(ticks as f32 * (1.0f32 / 30.0) * f32::from(e.tick_rate()))
}

impl super::Vm<'_> {
    // Locals belong to the dispatched frame, which stays put when an
    // evaluation it starts pushes a child frame above it.
    fn reserve(&mut self, t: usize, bytes: u16) {
        let f = self.frame;
        self.th(t).frames[f].bytes = bytes;
    }
    fn local(&mut self, t: usize, i: usize) -> Cell {
        let f = self.frame;
        self.th(t).frames[f].local(i)
    }
    fn set(&mut self, t: usize, i: usize, c: Cell) {
        let f = self.frame;
        self.th(t).frames[f].set_local(i, c)
    }
    fn first_argument(&self, call: Datum) -> VmResult<Datum> {
        let n = self.node(call)?;
        Ok(self.node(Datum(n.data))?.next)
    }
    fn next(&self, d: Datum) -> VmResult<Datum> {
        Ok(self.node(d)?.next)
    }
    fn count(&self, d: Datum) -> VmResult<usize> {
        let mut n = 0;
        let mut d = d;
        while d != Datum::NULL {
            n += 1;
            d = self.next(d)?;
        }
        Ok(n)
    }

    /// Dispatch the top frame (`0x00209850` loop body).
    pub(super) fn step(&mut self, e: &mut dyn Engine, t: usize, first: bool) -> VmResult<()> {
        self.stats.dispatches += 1;
        self.frame = self.th(t).frames.len() - 1;
        let expr = self.th(t).top().expr;
        let n = self.node(expr)?;
        if n.script_call() {
            return self.script_call(e, t, n.opcode, first);
        }
        let index = n.opcode;
        if first {
            *self.stats.builtins.entry(index).or_default() += 1;
        }
        let f = self.catalog.function(index)?;
        match self.binding(index) {
            Binding::Null => Err(VmError::NullEvaluator { index }),
            Binding::Native(native) => self.native(e, t, native, index, expr, first),
            Binding::Override | Binding::Engine if f.flags & 2 != 0 => {
                Err(VmError::UnsupportedCustom { index })
            }
            Binding::Override | Binding::Engine => {
                let parameters = f.parameters.clone();
                let Some(args) = self.arguments(e, t, index, expr, &parameters, first)? else {
                    return Ok(());
                };
                let call = EngineCall {
                    index,
                    name: f.name.as_deref(),
                    evaluate: f.evaluate,
                    return_type: HsType(f.return_type),
                    parameters: &f.parameters,
                    arguments: &args,
                    thread: t as u16,
                };
                let result = match self.bindings.get(index) {
                    Some(body) => body(e, call),
                    None => e.call_with_globals(
                        call,
                        ScenarioGlobals {
                            cells: &mut self.globals[EXTERNAL_GLOBALS..],
                            definitions: &self.scenario.globals,
                        },
                    ),
                }
                .map_err(|error| VmError::Engine { index, error })?;
                self.ret(e, t, result)
            }
        }
    }

    /// `0x00209bf0`: a scenario script call evaluates the callee's root on
    /// this thread, into a frame local, then returns it cast from the
    /// script's return type.
    fn script_call(
        &mut self,
        e: &mut dyn Engine,
        t: usize,
        script: u16,
        first: bool,
    ) -> VmResult<()> {
        self.reserve(t, 4);
        if first {
            *self.stats.scripts.entry(script).or_default() += 1;
            let root = self
                .scenario
                .scripts
                .get(script as usize)
                .ok_or_else(|| VmError::Node("script index out of range".into()))?
                .root;
            return self.eval(e, t, root, Dest::Local(0));
        }
        let v = self.local(t, 0);
        self.ret(e, t, v)
    }

    /// `0x00209d50`: evaluate one argument per entry into `args[i]`, checking
    /// each argument node's type against `parameters[i]`; `Some` once all are
    /// in. Locals: the argument slots, a u16 index, the next argument datum.
    fn arguments(
        &mut self,
        e: &mut dyn Engine,
        t: usize,
        index: u16,
        expr: Datum,
        parameters: &[u16],
        first: bool,
    ) -> VmResult<Option<Vec<Cell>>> {
        let n = parameters.len();
        self.reserve(t, (n * 4 + 6) as u16);
        if first {
            self.set(t, n, Cell::ZERO);
            let a = self.first_argument(expr)?;
            self.set(t, n + 1, Cell(a.0));
        }
        let i = self.local(t, n).0 as usize;
        if i < n {
            let current = Datum(self.local(t, n + 1).0);
            let ty = (current != Datum::NULL)
                .then(|| self.node(current).map(|a| a.value_type))
                .transpose()?;
            if ty != Some(parameters[i]) {
                return Err(VmError::Argument { index, position: i });
            }
            self.eval(e, t, current, Dest::Local(i))?;
            let next = self.next(current)?;
            self.set(t, n + 1, Cell(next.0));
            self.set(t, n, Cell(i as u32 + 1));
            return Ok(None);
        }
        Ok(Some((0..n).map(|i| self.local(t, i)).collect()))
    }

    fn native(
        &mut self,
        e: &mut dyn Engine,
        t: usize,
        native: Native,
        index: u16,
        expr: Datum,
        first: bool,
    ) -> VmResult<()> {
        match native {
            Native::Begin => {
                // 0x00207c20. Locals: next child, result (0 when empty).
                self.reserve(t, 8);
                if first {
                    let a = self.first_argument(expr)?;
                    self.set(t, 0, Cell(a.0));
                    self.set(t, 1, Cell::ZERO);
                }
                let next = Datum(self.local(t, 0).0);
                if next != Datum::NULL {
                    self.eval(e, t, next, Dest::Local(1))?;
                    let after = self.next(next)?;
                    self.set(t, 0, Cell(after.0));
                    return Ok(());
                }
                let v = self.local(t, 1);
                self.ret(e, t, v)
            }
            Native::BeginRandom => {
                // 0x00207cf0. Locals: branch count, used-branch bitmap word,
                // result. Every entry steps the LCG and picks
                // (i + ((seed >> 16) * count >> 16)) % count, probing i = 0..
                // until an unused branch is found; all used returns the result.
                self.reserve(t, 10);
                if first {
                    let count = self.count(self.first_argument(expr)?)?;
                    if count > 32 {
                        return Err(VmError::BeginRandomOverflow { branches: count });
                    }
                    self.set(t, 0, Cell(count as u32));
                    self.set(t, 1, Cell::ZERO);
                }
                let count = self.local(t, 0).0 as i32;
                let seed = lcg(e);
                let offset = (((seed >> 16).wrapping_mul(count as u32)) >> 16) as i16 as i32;
                let mut bitmap = self.local(t, 1).0;
                for i in 0..count {
                    let pick = (i + offset) % count;
                    if bitmap & (1 << pick) == 0 {
                        let mut child = self.first_argument(expr)?;
                        for _ in 0..pick {
                            child = self.next(child)?;
                        }
                        self.eval(e, t, child, Dest::Local(2))?;
                        bitmap |= 1 << pick;
                        self.set(t, 1, Cell(bitmap));
                        return Ok(());
                    }
                }
                let v = self.local(t, 2);
                self.ret(e, t, v)
            }
            Native::If => {
                // 0x00207ef0. Locals: condition byte, chosen branch, result.
                // A false condition with no else branch returns 0.
                self.reserve(t, 12);
                let condition = self.first_argument(expr)?;
                if first {
                    self.set(t, 0, Cell::ZERO);
                    self.set(t, 1, Cell::NONE);
                    return self.eval(e, t, condition, Dest::Local(0));
                }
                if self.local(t, 1) != Cell::NONE {
                    let v = self.local(t, 2);
                    return self.ret(e, t, v);
                }
                let then = self.next(condition)?;
                let branch = if self.local(t, 0).boolean() {
                    then
                } else {
                    self.next(then)?
                };
                self.set(t, 1, Cell(branch.0));
                if branch == Datum::NULL {
                    return self.ret(e, t, Cell::ZERO);
                }
                self.eval(e, t, branch, Dest::Local(2))
            }
            Native::Set => {
                // 0x00208070. The value is evaluated straight into the
                // global cell; object-list globals adjust reference counts
                // around the store; the result is the refreshed global.
                self.reserve(t, 4);
                let target = self.first_argument(expr)?;
                let word = crate::expression::global_word(self.node(target)?.data)?;
                let ty = self.global_type(word)?;
                if first {
                    if ty == HsType::OBJECT_LIST {
                        let old = self.read_global(e, word)?;
                        if old != Cell::NONE {
                            e.object_list_reference(old.0, -1);
                        }
                    }
                    let slot = self.global_slot(word)?;
                    let value = self.next(target)?;
                    return self.eval(e, t, value, Dest::Global(slot));
                }
                self.write_back(e, word)?;
                if ty == HsType::OBJECT_LIST {
                    let new = self.read_global(e, word)?;
                    if new != Cell::NONE {
                        e.object_list_reference(new.0, 1);
                    }
                }
                let v = self.read_global(e, word)?;
                self.ret(e, t, v)
            }
            Native::AndOr => {
                // 0x002081e0. Locals: next operand, operand value, running
                // byte. `and` starts true, `or` false; each entry folds the
                // last operand in and continues only while the running value
                // still equals the start value (short circuit).
                self.reserve(t, 9);
                let is_and = index == 5;
                if first {
                    let a = self.first_argument(expr)?;
                    self.set(t, 0, Cell(a.0));
                    self.set(t, 2, Cell::from_bool(is_and));
                } else {
                    let (acc, v) = (self.local(t, 2).boolean(), self.local(t, 1).boolean());
                    let folded = if is_and { acc && v } else { acc || v };
                    self.set(t, 2, Cell::from_bool(folded));
                }
                let next = Datum(self.local(t, 0).0);
                if next != Datum::NULL && self.local(t, 2).boolean() == is_and {
                    self.eval(e, t, next, Dest::Local(1))?;
                    let after = self.next(next)?;
                    self.set(t, 0, Cell(after.0));
                    return Ok(());
                }
                let v = Cell::from_bool(self.local(t, 2).boolean());
                self.ret(e, t, v)
            }
            Native::Arithmetic => {
                // 0x00208310. Locals: operand count, next operand, operand
                // value, accumulator. The first operand seeds the
                // accumulator; later ones apply `index - 7`: + - * / min max.
                // SSE single precision. min keeps the accumulator unless it
                // compares greater; max takes the operand unless the
                // accumulator compares greater (so NaN operands win in max).
                self.reserve(t, 14);
                if first {
                    self.set(t, 0, Cell::ZERO);
                    let a = self.first_argument(expr)?;
                    self.set(t, 1, Cell(a.0));
                } else {
                    let (count, v, acc) = (
                        self.local(t, 0).0,
                        self.local(t, 2).real(),
                        self.local(t, 3).real(),
                    );
                    let r = if count == 0 {
                        v
                    } else {
                        match index.wrapping_sub(7) {
                            0 => acc + v,
                            1 => acc - v,
                            2 => acc * v,
                            3 => acc / v,
                            // `comiss acc, v`: min takes v on `ja`, max on `jbe`
                            // (which includes unordered).
                            4 if acc > v => v,
                            5 if acc.partial_cmp(&v) != Some(std::cmp::Ordering::Greater) => v,
                            _ => acc,
                        }
                    };
                    self.set(t, 3, Cell::from_real(r));
                    self.set(t, 0, Cell((count + 1) & 0xffff));
                }
                let next = Datum(self.local(t, 1).0);
                if next != Datum::NULL {
                    self.eval(e, t, next, Dest::Local(2))?;
                    let after = self.next(next)?;
                    self.set(t, 1, Cell(after.0));
                    return Ok(());
                }
                let v = self.local(t, 3);
                self.ret(e, t, v)
            }
            Native::Equal | Native::Compare => {
                // 0x002084c0 / 0x00208570: both operands are evaluated as the
                // *first operand's* node type. `=` compares that type's byte
                // size; comparisons convert real, long (to single!) or else
                // the low word, then compare as singles.
                let ty = self.node(self.first_argument(expr)?)?.value_type;
                let Some(a) = self.arguments(e, t, index, expr, &[ty, ty], first)? else {
                    return Ok(());
                };
                let result = if native == Native::Equal {
                    a[0].bytes_equal(a[1], HsType(ty)) != (index == 14)
                } else {
                    let (x, y) = match HsType(ty) {
                        HsType::REAL => (a[0].real(), a[1].real()),
                        HsType::LONG => (a[0].long() as f32, a[1].long() as f32),
                        _ => (f32::from(a[0].short()), f32::from(a[1].short())),
                    };
                    match index.wrapping_sub(15) {
                        0 => x > y,
                        1 => y > x,
                        2 => x >= y,
                        3 => y >= x,
                        _ => false,
                    }
                };
                self.ret(e, t, Cell::from_bool(result))
            }
            Native::Sleep => self.sleep(e, t, expr, first),
            Native::SleepForever => {
                // 0x00208750: dormant (-2) for the named script's thread, or
                // this thread. No locals: the argument is evaluated
                // (0x0020879b-0x002087a6 calls 0x002099f0) into a C-stack
                // slot, so it must be an atom, and may be a script-typed
                // global as well as a literal.
                let a = self.first_argument(expr)?;
                let target = if a != Datum::NULL {
                    let n = self.node(a)?;
                    if !n.atom() {
                        return Err(VmError::NonAtomScript);
                    }
                    let script = self.atom(e, a, &n)?;
                    self.script_thread(script.0 as u16)
                } else {
                    Some(t)
                };
                if let Some(target) = target {
                    self.th(target).wake = super::DORMANT;
                }
                self.ret(e, t, Cell::ZERO)
            }
            Native::SleepUntil => self.sleep_until(e, t, expr, first),
            Native::Wake => {
                // 0x00208c40: reads the script index straight from the
                // argument atom's data word; no evaluation.
                let a = self.first_argument(expr)?;
                let script = self.node(a)?.data as u16;
                if let Some(target) = self.script_thread(script) {
                    self.wake_thread(target)?;
                }
                self.ret(e, t, Cell::ZERO)
            }
            Native::Inspect => {
                // 0x00208e10: evaluate; console threads format the value.
                self.reserve(t, 4);
                let a = self.first_argument(expr)?;
                if first {
                    return self.eval(e, t, a, Dest::Local(0));
                }
                if self.th(t).flags & INSPECT != 0 {
                    let ty = HsType(self.node(a)?.value_type);
                    let v = self.local(t, 0);
                    e.inspect(ty, v);
                }
                self.ret(e, t, Cell::ZERO)
            }
            Native::ObjectCast => {
                // 0x00208f10: keep the object if its type bit is in the mask
                // at `0x004456d4[index - 23]`, else -1.
                self.reserve(t, 4);
                if first {
                    let a = self.first_argument(expr)?;
                    return self.eval(e, t, a, Dest::Local(0));
                }
                let mask = *self
                    .image
                    .cast_masks
                    .get(index.wrapping_sub(23) as usize)
                    .ok_or(VmError::CastMask { index })?;
                let o = self.local(t, 0);
                let keep = o != Cell::NONE
                    && e.object_type(o.0)
                        .is_some_and(|ty| ty < 16 && mask & (1 << ty) != 0);
                self.ret(e, t, if keep { o } else { Cell::NONE })
            }
            Native::Not | Native::Pin | Native::RandomRange | Native::RealRandomRange => {
                let parameters = self.catalog.function(index)?.parameters.clone();
                let Some(a) = self.arguments(e, t, index, expr, &parameters, first)? else {
                    return Ok(());
                };
                let arg = |i: usize| -> VmResult<Cell> {
                    a.get(i)
                        .copied()
                        .ok_or(VmError::Argument { index, position: i })
                };
                let v = match native {
                    // 0x002a0c40: `cmp byte,0; sete`.
                    Native::Not => Cell::from_bool(!arg(0)?.boolean()),
                    // 0x002a0c90: low if low > x, else high if x > high, else x.
                    Native::Pin => {
                        let (x, lo, hi) = (arg(0)?.real(), arg(1)?.real(), arg(2)?.real());
                        Cell::from_real(if lo > x {
                            lo
                        } else if x > hi {
                            hi
                        } else {
                            x
                        })
                    }
                    // 0x002a29d0: low + (((high - low) * (seed >> 16)) >> 16),
                    // 32-bit signed multiply, logical shift, 16-bit result.
                    Native::RandomRange => {
                        let (lo, hi) = (arg(0)?.short(), arg(1)?.short());
                        let r = lcg(e) >> 16;
                        let span = (i32::from(hi) - i32::from(lo)) as u32;
                        Cell::from_short(
                            (lo as u16).wrapping_add((span.wrapping_mul(r) >> 16) as u16) as i16,
                        )
                    }
                    // 0x002a2a50: low + (high - low) * ((seed >> 16) / 65535)
                    // on the x87, rounded to single on the store.
                    _ => {
                        let (lo, hi) = (f64::from(arg(0)?.real()), f64::from(arg(1)?.real()));
                        let r = f64::from(lcg(e) >> 16) * f64::from(1.0f32 / 65535.0);
                        Cell::from_real((r * (hi - lo) + lo) as f32)
                    }
                };
                self.ret(e, t, v)
            }
            Native::DiscardedUnevaluated => self.ret(e, t, Cell::ZERO),
            Native::DiscardedEvaluated => {
                let parameters = self.catalog.function(index)?.parameters.clone();
                let Some(a) = self.arguments(e, t, index, expr, &parameters, first)? else {
                    return Ok(());
                };
                if parameters == [HsType::STRING.0]
                    && let Some(text) = self.string(a[0])
                {
                    let text = text.to_vec();
                    e.print(&text);
                }
                self.ret(e, t, Cell::ZERO)
            }
        }
    }

    /// `0x002087e0`. Locals: duration (short), script (short, -1 for this
    /// thread), state. A negative duration, or one that scales to <= 0
    /// ticks, does not sleep (so `(sleep 0)` does not yield). Sleeping a
    /// finished thread does nothing; sleeping *another* thread saves its
    /// previous wake state once, for `wake` to restore.
    fn sleep(&mut self, e: &mut dyn Engine, t: usize, expr: Datum, first: bool) -> VmResult<()> {
        self.reserve(t, 10);
        let duration = self.first_argument(expr)?;
        if first {
            self.eval(e, t, duration, Dest::Local(0))?;
            self.set(t, 2, Cell::ZERO);
            return Ok(());
        }
        if self.local(t, 2).0 as u16 == 0 {
            self.set(t, 2, Cell(1));
            let script = self.next(duration)?;
            if script != Datum::NULL {
                return self.eval(e, t, script, Dest::Local(1));
            }
            self.set(t, 1, Cell::NONE);
        }
        let script = self.local(t, 1).0 as u16;
        let target = if script == 0xffff {
            Some(t)
        } else {
            self.script_thread(script)
        };
        let Some(target) = target else {
            return self.ret(e, t, Cell::ZERO);
        };
        let d = self.local(t, 0).short();
        if d >= 0 {
            let ticks = scaled_ticks(e, i32::from(d));
            if ticks > 0 && self.th(target).wake != FINISHED {
                let now = e.game_time();
                let th = self.th(target);
                if target != t && th.flags & SAVED_SLEEP == 0 {
                    th.flags |= SAVED_SLEEP;
                    th.saved_wake = th.wake;
                }
                th.wake = now.wrapping_add(ticks);
            }
        }
        self.ret(e, t, Cell::ZERO)
    }

    /// `0x002089b0`. Locals: condition byte, period (short, default 30),
    /// timeout (long, default -1), start tick, state. Once the optional
    /// arguments are in, each entry returns if the last condition value was
    /// true or the timeout has passed; otherwise it starts the condition's
    /// evaluation and sets wake = now + max(1, period), capped at the
    /// timeout. Starting the condition goes through `0x002099f0`: a call
    /// condition only pushes its frame, and (when sleeps yield) the thread
    /// loop stops before running it, so it is first evaluated one period
    /// later; a literal or global condition is sampled at once, but its value
    /// is only acted on at the next wake. Either way the wait never ends on
    /// the call tick. `wake` on a thread inside `sleep_until` abandons it.
    fn sleep_until(
        &mut self,
        e: &mut dyn Engine,
        t: usize,
        expr: Datum,
        first: bool,
    ) -> VmResult<()> {
        self.reserve(t, 18);
        let condition = self.first_argument(expr)?;
        let period = self.next(condition)?;
        if first {
            self.set(t, 0, Cell::ZERO);
            self.set(t, 3, Cell::from_long(e.game_time()));
            self.set(t, 4, Cell::ZERO);
            self.set(t, 1, Cell::from_short(30));
            self.set(t, 2, Cell::NONE);
            if period != Datum::NULL {
                return self.eval(e, t, period, Dest::Local(1));
            }
        }
        match self.local(t, 4).0 as u16 {
            0 => {
                self.set(t, 4, Cell(1));
                if period != Datum::NULL {
                    let timeout = self.next(period)?;
                    if timeout != Datum::NULL {
                        return self.eval(e, t, timeout, Dest::Local(2));
                    }
                }
            }
            1 => {}
            _ => return Ok(()),
        }
        let timeout = self.local(t, 2).long();
        let timeout_ticks = if timeout == -1 {
            -1
        } else {
            scaled_ticks(e, timeout)
        };
        let start = self.local(t, 3).long();
        let now = e.game_time();
        if self.local(t, 0).boolean()
            || (timeout_ticks != -1 && now >= start.wrapping_add(timeout_ticks))
        {
            return self.ret(e, t, Cell::ZERO);
        }
        self.eval(e, t, condition, Dest::Local(0))?;
        let period_ticks = scaled_ticks(e, i32::from(self.local(t, 1).short())).max(1);
        let mut wake = e.game_time().wrapping_add(period_ticks);
        if timeout_ticks != -1 {
            wake = wake.min(start.wrapping_add(timeout_ticks));
        }
        self.th(t).wake = wake;
        Ok(())
    }
}
