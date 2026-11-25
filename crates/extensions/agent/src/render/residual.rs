//! `render.residual` (docs/runtime/agent-api.md, "render.residual";
//! docs/graphics/retire-d3d.md, "New method: `render.residual`"): arms
//! `render_bridge::residual_census`'s window, and answers its last finished
//! one, ranked and cut to `top`.
//!
//! The census is filled by the D3D adapter and the producer observers;
//! boot's scene glue wires both and registers the function lookup
//! ([`set_functions`]). Without that the method is unavailable.

use std::collections::BTreeMap;
use std::sync::Mutex;

use agent_proto::{
    ResidualCaller, ResidualParams, ResidualProducer, ResidualReply, ResidualTotals,
    ResidualUnattributed, ResidualWindow, RpcError, code,
};
use render_bridge::residual_census::{self as census, Report, State, Work};

/// The analysed guest function holding an address, as `profile.fallbacks`
/// names it (boot: `recomp::fallback::function_at`).
pub type FunctionOf = fn(u32) -> Option<u32>;

/// Rows kept by default.
pub const TOP: usize = 50;
/// Most rows kept.
pub const TOP_MAX: usize = 500;

static FUNCTIONS: Mutex<Option<FunctionOf>> = Mutex::new(None);

/// Registers the function lookup, which also says the census is wired.
pub fn set_functions(f: FunctionOf) {
    *FUNCTIONS.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
}

fn functions() -> Option<FunctionOf> {
    *FUNCTIONS.lock().unwrap_or_else(|e| e.into_inner())
}

/// `render.residual`.
pub fn answer(p: &ResidualParams) -> Result<ResidualReply, RpcError> {
    let invalid = |m: &str| RpcError::new(code::INVALID_PARAMS, m);
    let top = match p.top {
        None => TOP,
        Some(n) if (1..=TOP_MAX).contains(&n) => n,
        Some(_) => return Err(invalid("top: 1 to 500")),
    };
    let frames = match p.arm {
        None => None,
        Some(a) if (1..=census::MAX_FRAMES).contains(&a.frames) => Some(a.frames),
        Some(_) => return Err(invalid("arm.frames: 1 to 3600")),
    };
    let function_of = functions().ok_or_else(|| {
        RpcError::new(
            code::UNAVAILABLE,
            "render.residual needs the D3D device's entry watch, which boot's scene glue \
             registers with the GPU renderer",
        )
    })?;
    let (state, last) = census::request(frames);
    Ok(reply(state, last.as_ref(), top, function_of))
}

fn ms(ns: u64) -> f64 {
    ns as f64 / 1e6
}

fn totals(w: &Work) -> ResidualTotals {
    ResidualTotals {
        draws: w.draws,
        claimed: w.claimed,
        residual: w.residual,
        entries: w.entries,
        push_words: w.push_words,
        state_words: w.state_words,
        clears: w.clears,
        binds: w.binds,
        native_guest_binds: w.native_guest_binds,
        guest_bind_refused_destroy: w.guest_bind_refused_destroy,
        native_rect_clears: w.native_rect_clears,
        clear_refused_rect_count: w.clear_refused_rect_count,
        copies: w.copies,
        uploads: w.uploads,
        locks: w.locks,
        queries: w.queries,
        presents: w.presents,
        d3d_ms: ms(w.d3d_ns),
    }
}

/// The answer for `state` and the last finished window: producers and
/// callers by `residual`, then `entries`, then name, at most `top` each.
pub fn reply(
    state: State,
    report: Option<&Report>,
    top: usize,
    function_of: FunctionOf,
) -> ResidualReply {
    let Some(r) = report else {
        return ResidualReply {
            state: state.name().into(),
            ..Default::default()
        };
    };
    let timing = r.window.timing;
    let producers = r.producers.as_ref().map(|by| {
        let mut rows: Vec<ResidualProducer> = by
            .iter()
            .map(|(p, w)| ResidualProducer {
                producer: p.name(),
                status: p.status().name().into(),
                slot: p.slot().into(),
                calls: w.calls,
                draws: w.work.draws,
                claimed: w.work.claimed,
                residual: w.work.residual,
                entries: w.work.entries,
                push_words: w.work.push_words,
                state_words: w.work.state_words,
                clears: w.work.clears,
                binds: w.work.binds,
                native_guest_binds: w.work.native_guest_binds,
                guest_bind_refused_destroy: w.work.guest_bind_refused_destroy,
                native_rect_clears: w.work.native_rect_clears,
                clear_refused_rect_count: w.work.clear_refused_rect_count,
                copies: w.work.copies,
                uploads: w.work.uploads,
                d3d_ms: ms(w.work.d3d_ns),
                guest_ms: timing.then(|| ms(w.guest_ns())),
            })
            .collect();
        rows.sort_by(|a, b| {
            (b.residual, b.entries, &a.producer).cmp(&(a.residual, a.entries, &b.producer))
        });
        rows.truncate(top);
        rows
    });
    let mut by_function: BTreeMap<String, ResidualCaller> = BTreeMap::new();
    for (&ret, s) in &r.sites {
        let function = format!("{:#x}", function_of(ret).unwrap_or(ret));
        let c = by_function
            .entry(function.clone())
            .or_insert_with(|| ResidualCaller {
                function,
                ..Default::default()
            });
        c.sites.push(format!("{ret:#x}"));
        for (&name, &n) in &s.entries {
            *c.entries.entry(name.to_owned()).or_default() += n;
        }
        c.draws += s.draws;
        c.inline_draws += s.inline_draws;
        c.residual += s.residual;
        c.d3d_ms += ms(s.d3d_ns);
    }
    let mut callers: Vec<ResidualCaller> = by_function.into_values().collect();
    callers.sort_by(|a, b| {
        (b.residual, b.calls(), &a.function).cmp(&(a.residual, a.calls(), &b.function))
    });
    callers.truncate(top);
    ResidualReply {
        state: state.name().into(),
        window: Some(ResidualWindow {
            frames: r.window.frames,
            game_frames: r.window.game_frames,
            presents: r.window.presents,
            ticks: r.window.ticks,
            map: r.window.map.clone(),
            timing,
        }),
        totals: Some(totals(&r.totals)),
        producers,
        callers: Some(callers),
        unattributed: Some(ResidualUnattributed {
            draws: r.unattributed.0,
            entries: r.unattributed.1,
        }),
        sites_full: r.sites_full,
        unbalanced: r.unbalanced,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_proto::ResidualArm;
    use render_bridge::residual_census::{Census, Producer, Tally, row};

    const DRAW: &str = "D3DDevice_DrawVertices";

    fn call(entry: &'static str, ret: u32, draws: u64, present: u64) -> census::Call {
        let own = Tally {
            draws,
            residual: draws,
            ..Tally::default()
        };
        census::Call {
            entry,
            ret,
            inline: Tally::default(),
            own,
            ns: 2_000_000,
            ran: true,
            present,
        }
    }

    /// A synthetic window: two producers, three return addresses in two
    /// functions, one draw a replacement claimed.
    fn report(observed: bool) -> Report {
        let hud = Producer::Fixed(row("hud").unwrap());
        let mut c = Census::new(observed, false);
        c.arm(1);
        c.call(&call("D3DDevice_Swap", 0x0001_3f40, 0, 41), None);
        c.frame_begin();
        c.enter(Producer::Layer { at: 0, layer: 3 });
        c.call(&call(DRAW, 0x0004_c976, 3, 41), None);
        c.call(&call(DRAW, 0x0004_c990, 1, 41), None);
        let claimed = census::Call {
            own: Tally {
                draws: 1,
                claimed: 1,
                ..Tally::default()
            },
            ..call("D3DDevice_DrawIndexedVertices", 0x0004_c990, 0, 41)
        };
        c.call(&claimed, None);
        c.leave(|p| matches!(p, Producer::Layer { .. }));
        c.enter(hud);
        c.call(&call(DRAW, 0x0019_1800, 2, 41), None);
        c.leave(|p| p == hud);
        c.frame_end();
        c.call(&call("D3DDevice_Swap", 0x0001_3f40, 0, 42), None);
        c.last().cloned().unwrap()
    }

    /// Return addresses below `0x0004_c980` are in one function, the rest
    /// in their own.
    fn function_of(ret: u32) -> Option<u32> {
        (0x0004_c000..0x0004_c980)
            .contains(&ret)
            .then_some(0x0004_c900)
            .or((ret >= 0x0004_c980).then_some(ret & !0xff))
    }

    #[test]
    fn callers_group_return_addresses_by_function_and_rank_by_residual() {
        let r = reply(State::Done, Some(&report(false)), TOP, function_of);
        assert_eq!(r.state, "done");
        let callers = r.callers.unwrap();
        let names: Vec<_> = callers.iter().map(|c| c.function.as_str()).collect();
        // Most residual first; the closing Swap's site is in no analysed
        // function, so it is named by itself.
        assert_eq!(names, ["0x4c900", "0x191800", "0x13f40"]);
        let c = callers.iter().find(|c| c.function == "0x4c900").unwrap();
        assert_eq!(c.sites, ["0x4c976", "0x4c990"]);
        assert_eq!((c.draws, c.residual, c.calls()), (5, 4, 3));
        assert_eq!(
            (c.entries[DRAW], c.entries["D3DDevice_DrawIndexedVertices"]),
            (2, 1)
        );
        let t = r.totals.unwrap();
        assert_eq!((t.draws, t.claimed, t.residual, t.presents), (7, 1, 6, 1));
        assert!(r.producers.is_none(), "no producers without the option");
        assert_eq!(r.window.unwrap().presents, [42, 42]);
    }

    #[test]
    fn producers_rank_by_residual_and_sum_to_the_totals() {
        let r = reply(State::Done, Some(&report(true)), TOP, function_of);
        let p = r.producers.unwrap();
        assert_eq!(p[0].producer, "layer.scene.3");
        assert_eq!(
            (
                p[0].residual,
                p[0].claimed,
                p[0].status.as_str(),
                p[0].slot.as_str()
            ),
            (4, 1, "partial", "opaque")
        );
        assert_eq!(p[1].producer, "hud");
        assert_eq!(p[0].guest_ms, None, "null without render-timing");
        let sum: u64 = p.iter().map(|p| p.residual).sum();
        assert_eq!(sum, r.totals.unwrap().residual);
        assert_eq!(r.unattributed.unwrap(), ResidualUnattributed::default());
        let cut = reply(State::Done, Some(&report(true)), 1, function_of);
        assert_eq!(
            (cut.producers.unwrap().len(), cut.callers.unwrap().len()),
            (1, 1)
        );
    }

    #[test]
    fn with_nothing_finished_the_answer_is_the_state_alone() {
        let r = reply(State::Armed, None, TOP, function_of);
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            serde_json::json!({"state": "armed"})
        );
    }

    #[test]
    fn out_of_range_params_are_refused() {
        let bad = |p: ResidualParams| answer(&p).unwrap_err().code;
        assert_eq!(
            bad(ResidualParams {
                top: Some(0),
                ..Default::default()
            }),
            code::INVALID_PARAMS
        );
        assert_eq!(
            bad(ResidualParams {
                top: Some(501),
                ..Default::default()
            }),
            code::INVALID_PARAMS
        );
        let arm = |frames| ResidualParams {
            arm: Some(ResidualArm { frames }),
            top: None,
        };
        assert_eq!(bad(arm(0)), code::INVALID_PARAMS);
        assert_eq!(bad(arm(3601)), code::INVALID_PARAMS);
    }
}
