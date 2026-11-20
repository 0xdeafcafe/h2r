//! Control requests are synchronous HS expressions. The game executes them
//! on global write-back. The per-actor bank is ordinary scenario state so
//! game saves and restores include both ownership and the completion tick.
use super::*;

const OP: &str = "h1_cmd_operation";
const ITEMS: &str = "h1_cmd_subjects";
const RESULT: &str = "h1_cmd_result";
const BANK: usize = 256;

pub(super) fn library(p: &mut Program, needs_pc: bool) {
    if needs_pc {
        for i in 0..BANK {
            p.global(&format!("h1_cmd_radius_{i}"), REAL, real(0.0));
        }
        p.global("h1_cmd_radius_value", REAL, real(0.0));
        for i in 0..BANK {
            p.global(&format!("h1_cmd_pc_{i}"), SHORT, short(0));
        }
        p.global("h1_cmd_pc_value", SHORT, short(0));
    }
    p.global(OP, SHORT, short(0));
    p.global(ITEMS, OBJECT_LIST, lit(OBJECT_LIST, NONE, "none"));
    p.global(RESULT, SHORT, short(0));
    for i in 0..BANK {
        for field in ["owner", "instance", "state", "fall"] {
            p.global(
                &format!("h1_cmd_{field}_{i}"),
                h2t::LONG,
                lit(h2t::LONG, NONE, "-1"),
            );
        }
    }
}

pub(super) fn radius() -> Expr {
    begin(vec![request(9, unit(), false), var("h1_cmd_radius_value")])
}
pub(super) fn set_radius(r: f32) -> Expr {
    begin(vec![
        set("h1_cmd_radius_value", real(r)),
        request(10, unit(), false),
    ])
}

pub(super) fn pc() -> Expr {
    request(7, unit(), true)
}
pub(super) fn go(index: usize) -> Expr {
    begin(vec![
        set("h1_cmd_pc_value", short(index as i16)),
        request(8, unit(), false),
    ])
}

fn request(op: i16, items: Expr, result: bool) -> Expr {
    let mut body = vec![
        set(ITEMS, items),
        set(OP, short(op)),
        set(ITEMS, lit(OBJECT_LIST, NONE, "none")),
    ];
    if result {
        body.push(var(RESULT));
    }
    begin(body)
}

pub(super) fn start(protect: bool) -> Expr {
    request(if protect { 6 } else { 1 }, unit(), false)
}
pub(super) fn finish() -> Expr {
    request(2, unit(), false)
}
pub(super) fn stimulus() -> Expr {
    cmp("=", SHORT, request(3, unit(), true), short(1))
}
pub(super) fn visit(list: Expr, query: bool) -> Expr {
    request(if query { 5 } else { 4 }, list, query)
}
