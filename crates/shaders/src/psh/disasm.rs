//! Readable listing of a pixel shader definition.
//!
//! The syntax is our own (D3D's `xps.1.1` assembler cannot express every
//! encodable combination); it states each write as an equation, e.g.
//! `r0.rgb = (t0.rgb . c0.rgb) <<1` for a dot product with SHIFTLEFT_1.

use super::*;
use std::fmt::Write;

fn input_text(i: &Input, alpha_portion: bool) -> String {
    let ch = match (alpha_portion, i.alpha) {
        (false, false) => "rgb",
        (_, true) => "a",
        (true, false) => "b",
    };
    let r = format!("{}.{ch}", i.reg.name());
    if i.reg == Reg::Zero {
        return match i.map {
            InputMap::UnsignedIdentity | InputMap::SignedIdentity | InputMap::SignedNegate => {
                "0".into()
            }
            InputMap::UnsignedInvert => "1".into(),
            InputMap::ExpandNormal => "-1".into(),
            InputMap::ExpandNegate => "1".into(),
            InputMap::HalfBiasNormal => "-0.5".into(),
            InputMap::HalfBiasNegate => "0.5".into(),
        };
    }
    match i.map {
        InputMap::UnsignedIdentity => r,
        InputMap::UnsignedInvert => format!("(1-{r})"),
        InputMap::ExpandNormal => format!("bx2({r})"),
        InputMap::ExpandNegate => format!("-bx2({r})"),
        InputMap::HalfBiasNormal => format!("bias({r})"),
        InputMap::HalfBiasNegate => format!("-bias({r})"),
        InputMap::SignedIdentity => format!("signed({r})"),
        InputMap::SignedNegate => format!("-signed({r})"),
    }
}

fn map_text(m: OutMap) -> &'static str {
    match m {
        OutMap::Identity => "",
        OutMap::Bias => " bias",
        OutMap::Shl1 => " x2",
        OutMap::Shl1Bias => " bx2",
        OutMap::Shl2 => " x4",
        OutMap::Shl2Bias => " bx4",
        OutMap::Shr1 => " d2",
        OutMap::Shr1Bias => " bd2",
    }
}

fn portion(out: &mut String, label: &str, p: &Portion, alpha: bool) {
    let [a, b, c, d] = p.inputs.map(|i| input_text(&i, alpha));
    let o = &p.output;
    let ch = if alpha { "a" } else { "rgb" };
    let ab = if o.ab_dot {
        format!("{a} . {b}")
    } else {
        format!("{a} * {b}")
    };
    let cd = if o.cd_dot {
        format!("{c} . {d}")
    } else {
        format!("{c} * {d}")
    };
    let m = map_text(o.map);
    let mut any = false;
    for (reg, expr, bta) in [
        (o.ab, &ab, o.ab_blue_to_alpha),
        (o.cd, &cd, o.cd_blue_to_alpha),
    ] {
        if reg != Reg::Zero {
            let extra = if bta { " (blue to alpha)" } else { "" };
            let _ = writeln!(out, "  {label}: {}.{ch} = {expr}{m}{extra}", reg.name());
            any = true;
        }
    }
    if o.sum != Reg::Zero {
        let op = if o.mux { "mux" } else { "+" };
        let _ = writeln!(
            out,
            "  {label}: {}.{ch} = ({ab}) {op} ({cd}){m}",
            o.sum.name()
        );
        any = true;
    }
    if !any {
        let _ = writeln!(out, "  {label}: (no writes)");
    }
}

/// Disassembles a definition.
pub fn disassemble(def: &PixelShaderDef) -> String {
    let c = def.decode();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "combiners {} mux={} c0={} c1={}",
        c.stages.len(),
        if c.mux_msb { "msb" } else { "lsb" },
        if c.unique_c0 { "per-stage" } else { "shared" },
        if c.unique_c1 { "per-stage" } else { "shared" },
    );
    for (i, m) in c.texture_modes.iter().enumerate() {
        if *m == TexMode::None {
            continue;
        }
        let mut extra = String::new();
        if i > 0
            && c.dot_mapping[i] != DotMap::ZeroToOne
            && !matches!(c.dot_mapping[i], DotMap::Unused)
        {
            let _ = write!(extra, " map={:?}", c.dot_mapping[i]);
        }
        if i >= 2 {
            let _ = write!(extra, " input=t{}", c.input_texture[i]);
        }
        if *m == TexMode::ClipPlane {
            let _ = write!(extra, " compare={:04b}", c.compare_mode[i]);
        }
        let _ = writeln!(out, "tex t{i} {}{extra}", m.name());
    }
    for (n, st) in c.stages.iter().enumerate() {
        portion(&mut out, &format!("stage{n}.rgb"), &st.rgb, false);
        portion(&mut out, &format!("stage{n}.a  "), &st.alpha, true);
    }
    match &c.final_combiner {
        Some(f) => {
            let t = |i: &Input| input_text(i, false);
            let _ = writeln!(
                out,
                "final: rgb = {a}*{b} + (1-{a})*{c} + {d}; a = {g}; E={e} F={f}{cl}{v1}{r0}",
                a = t(&f.a),
                b = t(&f.b),
                c = t(&f.c),
                d = t(&f.d),
                g = input_text(&f.g, true),
                e = t(&f.e),
                f = t(&f.f),
                cl = if f.clamp_sum { " clamp_sum" } else { "" },
                v1 = if f.complement_v1 { " ~v1" } else { "" },
                r0 = if f.complement_r0 { " ~r0" } else { "" },
            );
        }
        None => out.push_str("final: (none) out = r0\n"),
    }
    out
}
