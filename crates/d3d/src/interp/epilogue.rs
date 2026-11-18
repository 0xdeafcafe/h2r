//! Where a vertex program puts its clip position on the target, read from
//! the program itself.
//!
//! NV2A vertex programs write window coordinates: the program divides its
//! clip position by `w` and applies a viewport scale and offset it reads
//! from constants (docs/graphics/shaders.md, "Position fixup"). D3D's own
//! epilogue reads `c-38`/`c-37` (`c[58]`/`c[59]`, which `SET_VIEWPORT_*`
//! also write), but Halo 2's programs keep their own (`c[10]`/`c[11]`), so
//! the push-buffer registers say nothing about where a draw lands. E1
//! needs that mapping twice: the warp maps through it, and the world image
//! is overscanned by scaling it.
//!
//! [`Epilogue::of`] runs the program over a small symbolic domain: once
//! `oPos` holds the clip position (whatever computed it), it follows
//! `clip_i * k`, `1 / clip_w`, `clip_i / clip_w * k` and `... + o` with `k`
//! and `o` constant components, and accepts the program only when, at its
//! end, `oPos.x` and `oPos.y` are `clip / w * scale + offset`, `oPos.w` is
//! the clip `w`, and the two scale components are read nowhere else and
//! flow nowhere else (so scaling them moves the position and nothing
//! more). Anything else (a program that writes window coordinates its own
//! way, writes constants, or reads the scale for something else) is not
//! known: E1 holds such frames.

use shaders::vsh::{IluOp, MacOp, OutDest, OutReg, Program, ProgramKind, Source, SrcReg, Unit};

use crate::shader::CONSTANTS;

/// A constant component: register (hardware numbering) and component.
pub type Comp = (u8, u8);

/// Where a program's `oPos.xy` comes from: `clip.xy / clip.w * scale +
/// offset`, each a constant component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Epilogue {
    pub scale: [Comp; 2],
    pub offset: [Comp; 2],
}

impl Epilogue {
    /// The epilogue of the program in `microcode` (bare, four words per
    /// slot), or `None` when it is not one of the form above.
    pub fn of(microcode: &[u32]) -> Option<Epilogue> {
        let program = Program::from_words(ProgramKind::Normal, microcode).ok()?;
        analyse(&program)
    }

    /// The scale and offset these constants give: pixels =
    /// `ndc * scale + offset`.
    pub fn read(&self, c: &[[f32; 4]; CONSTANTS]) -> ([f64; 2], [f64; 2]) {
        let at = |(r, i): Comp| c[r as usize][i as usize] as f64;
        (
            [at(self.scale[0]), at(self.scale[1])],
            [at(self.offset[0]), at(self.offset[1])],
        )
    }

    /// Scales the program's viewport scale in `c` by `s`: the picture
    /// shrinks by `s` about the offset, and nothing else the program
    /// computes changes.
    pub fn overscan(&self, c: &mut [[f32; 4]; CONSTANTS], s: f64) {
        for (r, i) in self.scale {
            let v = &mut c[r as usize][i as usize];
            *v = (*v as f64 * s) as f32;
        }
    }
}

/// What a vertex program computes its clip position from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Position {
    /// The four rows `c[b..b + 4]`: `oPos.x` to `oPos.w` each a `dp4` or
    /// `dph` with `c[b + i]` (unswizzled, not relative) as one operand. That
    /// block is the matrix the draw's vertices go through to the screen:
    /// the only one that can be its camera.
    Rows(u8),
    /// No constant: the vertices as given (a pass-through, `mov oPos,
    /// v0`), already in screen or clip space. Nothing the camera does
    /// moves them.
    Fixed,
    /// Some other way (columns, a product of blocks, a program that cannot
    /// be read).
    Unknown,
}

/// What `microcode`'s clip position is computed from: `oPos` as each
/// component stands when it is first read back (`r12`: the epilogue
/// follows) or at the end, followed through temporaries and moves.
pub fn position(microcode: &[u32]) -> Position {
    let Ok(p) = Program::from_words(ProgramKind::Normal, microcode) else {
        return Position::Unknown;
    };
    // For each temporary (12 is `oPos`) and component: the constant row
    // it is the dot product with, if it is one, and whether any constant
    // reaches it.
    let mut rows: [[Option<u8>; 4]; 13] = [[None; 4]; 13];
    let mut constant: [[bool; 4]; 13] = [[false; 4]; 13];
    // `oPos` components read back: the clip position's, as they stand;
    // later writes are the epilogue's.
    let mut read = [false; 4];
    for inst in &p.instructions {
        let reaches = |s: &Source, constant: &[[bool; 4]; 13]| match s.reg {
            SrcReg::Const { .. } => true,
            SrcReg::Temp(t) => {
                (t as usize) >= 13
                    || s.swizzle
                        .iter()
                        .any(|&c| constant[t as usize][c as usize & 3])
            }
            _ => false,
        };
        let (ra, rb, rc) = inst.mac_reads();
        let mac_const = (ra && reaches(&inst.a, &constant))
            || (rb && reaches(&inst.b, &constant))
            || (rc && reaches(&inst.c, &constant));
        let ilu_const = reaches(&inst.c, &constant);
        for s in inst.sources() {
            if s.reg == SrcReg::Temp(12) {
                for &c in &s.swizzle {
                    read[c as usize & 3] = true;
                }
            }
        }
        let moved = |s: &Source, rows: &[[Option<u8>; 4]; 13]| -> [Option<u8>; 4] {
            match s.reg {
                SrcReg::Temp(t) if (t as usize) < 13 && !s.neg => {
                    s.swizzle.map(|c| rows[t as usize][c as usize & 3])
                }
                _ => [None; 4],
            }
        };
        let m: [Option<u8>; 4] = match inst.mac {
            MacOp::Dp4 | MacOp::Dph => [&inst.a, &inst.b]
                .into_iter()
                .find_map(|s| match s.reg {
                    SrcReg::Const {
                        index,
                        relative: false,
                    } if s.swizzle == [0, 1, 2, 3] && !s.neg => Some(index),
                    _ => None,
                })
                .map_or([None; 4], |r| [Some(r); 4]),
            MacOp::Mov => moved(&inst.a, &rows),
            _ => [None; 4],
        };
        let l = if inst.ilu == IluOp::Mov {
            moved(&inst.c, &rows)
        } else {
            [None; 4]
        };
        let mut write = |reg: usize, mask: shaders::vsh::Mask, v: [Option<u8>; 4], k: bool| {
            for (j, x) in v.into_iter().enumerate() {
                if mask.has(j) && !(reg == 12 && read[j]) {
                    rows[reg][j] = x;
                    constant[reg][j] = k;
                }
            }
        };
        if inst.mac_writes_temp() && (inst.mac_temp as usize) < 13 {
            write(inst.mac_temp as usize, inst.mac_mask, m, mac_const);
        }
        if inst.ilu_writes_temp() && (inst.ilu_temp as usize) < 13 {
            write(inst.ilu_temp as usize, inst.ilu_mask, l, ilu_const);
        }
        if let Some(o) = inst.out
            && o.dest == OutDest::Output(OutReg::Pos)
        {
            let (v, k) = if o.unit == Unit::Mac {
                (m, mac_const)
            } else {
                (l, ilu_const)
            };
            write(12, o.mask, v, k);
        }
    }
    if constant[12] == [false; 4] {
        return Position::Fixed;
    }
    match rows[12][0] {
        Some(b)
            if b as usize + 4 <= CONSTANTS && (0..4).all(|j| rows[12][j] == Some(b + j as u8)) =>
        {
            Position::Rows(b)
        }
        _ => Position::Unknown,
    }
}

/// A component's value, as far as the analysis follows it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Sym {
    Unknown,
    Const(Comp),
    /// The clip position's component `i`.
    Clip(u8),
    /// `1 / clip.w`.
    InvW,
    /// `clip_i * k`.
    Scaled(u8, Comp),
    /// `clip_i / w`.
    Ndc(u8),
    /// `clip_i / w * k`.
    NdcScaled(u8, Comp),
    /// `clip_i / w * k + o`.
    Window(u8, Comp, Comp),
}

impl Sym {
    /// The constant component this value is a product with, if any (what
    /// scaling that constant would change).
    fn factor(self) -> Option<Comp> {
        match self {
            Sym::Const(k) | Sym::Scaled(_, k) | Sym::NdcScaled(_, k) | Sym::Window(_, k, _) => {
                Some(k)
            }
            _ => None,
        }
    }
}

fn mul(a: Sym, b: Sym) -> Sym {
    use Sym::*;
    match (a, b) {
        (Clip(i), Const(k)) | (Const(k), Clip(i)) => Scaled(i, k),
        (Clip(i), InvW) | (InvW, Clip(i)) => Ndc(i),
        (Scaled(i, k), InvW) | (InvW, Scaled(i, k)) => NdcScaled(i, k),
        (Ndc(i), Const(k)) | (Const(k), Ndc(i)) => NdcScaled(i, k),
        _ => Unknown,
    }
}

fn add(a: Sym, b: Sym) -> Sym {
    match (a, b) {
        (Sym::NdcScaled(i, k), Sym::Const(o)) | (Sym::Const(o), Sym::NdcScaled(i, k)) => {
            Sym::Window(i, k, o)
        }
        _ => Unknown,
    }
}

use Sym::Unknown;

struct State {
    temps: [[Sym; 4]; 12],
    pos: [Sym; 4],
    /// `oPos` components written so far.
    written: [bool; 4],
    /// `oPos` components read back (`r12`) so far.
    read: [bool; 4],
    /// Constant components whose scaling would change something other
    /// than the position chain.
    tainted: Vec<Comp>,
    /// Lowest base of a relative (`c[a0.x + n]`) read.
    relative_base: Option<u8>,
}

impl State {
    fn taint(&mut self, v: Sym) {
        if let Some(k) = v.factor()
            && !self.tainted.contains(&k)
        {
            self.tainted.push(k);
        }
    }

    /// Component `j` of operand `s` (after swizzle and sign).
    fn get(&mut self, s: &Source, j: usize) -> Sym {
        let c = s.swizzle[j] as usize & 3;
        let v = match s.reg {
            SrcReg::Temp(12) => {
                if !self.written[c] {
                    Unknown
                } else {
                    self.read[c] = true;
                    self.pos[c]
                }
            }
            SrcReg::Temp(n) => self.temps.get(n as usize).map_or(Unknown, |t| t[c]),
            SrcReg::Const {
                index,
                relative: false,
            } => Sym::Const((index, c as u8)),
            SrcReg::Const {
                index,
                relative: true,
            } => {
                self.relative_base = Some(self.relative_base.map_or(index, |b| b.min(index)));
                Unknown
            }
            SrcReg::Input(_) | SrcReg::Unused => Unknown,
        };
        if s.neg {
            // A negated product is not followed; whatever it carried is
            // used.
            self.taint(v);
            Unknown
        } else {
            v
        }
    }

    /// Writes `v` to temporary `reg` (12 is `oPos`) under `mask`. `false`
    /// when the write breaks what the analysis needs.
    fn write_temp(&mut self, reg: u8, v: [Sym; 4], mask: shaders::vsh::Mask) -> bool {
        if reg == 12 {
            return self.write_pos(v, mask);
        }
        if let Some(t) = self.temps.get_mut(reg as usize) {
            for j in 0..4 {
                if mask.has(j) {
                    t[j] = v[j];
                }
            }
        }
        true
    }

    /// Writes `oPos`: a value the analysis does not follow becomes the
    /// clip position's component, unless that component was already read
    /// back as one (then there would be two clip positions).
    fn write_pos(&mut self, v: [Sym; 4], mask: shaders::vsh::Mask) -> bool {
        for (j, &x) in v.iter().enumerate() {
            if !mask.has(j) {
                continue;
            }
            self.pos[j] = match x {
                Unknown if self.read[j] && j != 2 => return false,
                Unknown => Sym::Clip(j as u8),
                s => s,
            };
            self.written[j] = true;
        }
        true
    }
}

fn analyse(p: &Program) -> Option<Epilogue> {
    let mut st = State {
        temps: [[Unknown; 4]; 12],
        pos: [Unknown; 4],
        written: [false; 4],
        read: [false; 4],
        tainted: Vec::new(),
        relative_base: None,
    };
    for inst in &p.instructions {
        // The components each unit's result is needed for.
        let mut mac_need = 0u8;
        if inst.mac_writes_temp() {
            mac_need |= inst.mac_mask.0 & 0xF;
        }
        let mut ilu_need = 0u8;
        if inst.ilu_writes_temp() {
            ilu_need |= inst.ilu_mask.0 & 0xF;
        }
        if let Some(o) = inst.out {
            match o.dest {
                // A program that writes constants changes them for later
                // vertices on the hardware: not followed.
                OutDest::Const(_) => return None,
                OutDest::Output(_) => match o.unit {
                    Unit::Mac => mac_need |= o.mask.0 & 0xF,
                    Unit::Ilu => ilu_need |= o.mask.0 & 0xF,
                },
            }
        }
        // MAC.
        let mut m = [Unknown; 4];
        let (ra, rb, rc) = inst.mac_reads();
        let componentwise = matches!(inst.mac, MacOp::Mov | MacOp::Mul | MacOp::Add | MacOp::Mad);
        if inst.mac != MacOp::Nop {
            if componentwise {
                for (j, mj) in m.iter_mut().enumerate() {
                    if mac_need & (8 >> j) == 0 {
                        continue;
                    }
                    let a = if ra { st.get(&inst.a, j) } else { Unknown };
                    let b = if rb { st.get(&inst.b, j) } else { Unknown };
                    let c = if rc { st.get(&inst.c, j) } else { Unknown };
                    let v = match inst.mac {
                        MacOp::Mov => a,
                        MacOp::Mul => mul(a, b),
                        MacOp::Add => add(a, c),
                        _ => add(mul(a, b), c),
                    };
                    // An operand's factor that does not carry into the
                    // result is used up here.
                    for x in [a, b, c] {
                        if x.factor().is_some() && x.factor() != v.factor() {
                            st.taint(x);
                        }
                    }
                    *mj = v;
                }
            } else {
                // Dot products and the rest read whole operands.
                for (used, s) in [(ra, inst.a), (rb, inst.b), (rc, inst.c)] {
                    if used {
                        for j in 0..4 {
                            let v = st.get(&s, j);
                            st.taint(v);
                        }
                    }
                }
            }
        }
        // ILU (reads C only).
        let mut l = [Unknown; 4];
        if inst.ilu != IluOp::Nop {
            if inst.ilu.is_scalar() {
                let x = st.get(&inst.c, 0);
                let v = match (inst.ilu, x) {
                    (IluOp::Rcp | IluOp::Rcc, Sym::Clip(3)) => Sym::InvW,
                    _ => {
                        st.taint(x);
                        Unknown
                    }
                };
                l = [v; 4];
            } else if inst.ilu == IluOp::Mov {
                for (j, lj) in l.iter_mut().enumerate() {
                    if ilu_need & (8 >> j) != 0 {
                        *lj = st.get(&inst.c, j);
                    }
                }
            } else {
                for j in 0..4 {
                    let v = st.get(&inst.c, j);
                    st.taint(v);
                }
            }
        }
        // Writes: temporaries first, then the output port (the same
        // order as the interpreter).
        if inst.mac_writes_temp() && !st.write_temp(inst.mac_temp, m, inst.mac_mask) {
            return None;
        }
        if inst.ilu_writes_temp() && !st.write_temp(inst.ilu_temp, l, inst.ilu_mask) {
            return None;
        }
        if let Some(o) = inst.out
            && let OutDest::Output(reg) = o.dest
        {
            let v = if o.unit == Unit::Mac { m } else { l };
            if reg == OutReg::Pos {
                if !st.write_pos(v, o.mask) {
                    return None;
                }
            } else {
                // Texture coordinates, colours, fog: what scaling a
                // factor here would change is not the position.
                for (j, x) in v.iter().enumerate() {
                    if o.mask.has(j) {
                        st.taint(*x);
                    }
                }
            }
        }
    }
    let (Sym::Window(0, kx, ox), Sym::Window(1, ky, oy), Sym::Clip(3)) =
        (st.pos[0], st.pos[1], st.pos[3])
    else {
        return None;
    };
    let fine = |k: Comp| {
        !st.tainted.contains(&k)
            && st.pos[2].factor() != Some(k)
            // A relative read could reach it only below its base.
            && st.relative_base.is_none_or(|b| b > k.0)
    };
    (kx != ky && fine(kx) && fine(ky)).then_some(Epilogue {
        scale: [kx, ky],
        offset: [ox, oy],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaders::vsh::encode::{Inst, Operand as O};

    const X: u8 = 8;
    const Y: u8 = 4;
    const Z: u8 = 2;
    const W: u8 = 1;

    fn clip() -> Vec<Inst> {
        (0..4)
            .map(|i| {
                Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(i)), None).to_out(
                    false,
                    OutReg::Pos,
                    8 >> i,
                )
            })
            .collect()
    }

    fn words(insts: &[Inst]) -> Vec<u32> {
        let mut insts = insts.to_vec();
        let last = insts.pop().unwrap().last();
        insts.push(last);
        insts.iter().flat_map(|i| i.words()).collect()
    }

    /// `mul oPos.xyz, r12, c[s] : rcc r1.w, r12.w` and `mad oPos.xyz,
    /// r12, r1.w, c[o]`.
    fn standard(s: u8, o: u8) -> [Inst; 2] {
        [
            Inst::mac(MacOp::Mul, Some(O::r(12)), Some(O::c(s)), None)
                .to_out(false, OutReg::Pos, X | Y | Z)
                .with_ilu(IluOp::Rcc, O::r(12).swz("w"))
                .ilu_to_temp(1, W),
            Inst::mac(
                MacOp::Mad,
                Some(O::r(12)),
                Some(O::r(1).swz("w")),
                Some(O::c(o)),
            )
            .to_out(false, OutReg::Pos, X | Y | Z),
        ]
    }

    #[test]
    fn d3ds_and_halos_epilogues_are_read() {
        for (s, o) in [(58, 59), (10, 11)] {
            let mut p = clip();
            p.extend(standard(s, o));
            assert_eq!(
                Epilogue::of(&words(&p)),
                Some(Epilogue {
                    scale: [(s, 0), (s, 1)],
                    offset: [(o, 0), (o, 1)],
                })
            );
        }
    }

    #[test]
    fn a_divide_first_epilogue_with_scale_and_offset_in_one_register_is_read() {
        // rcc r10.w, r12.w; mul oPos.xyz, r12, r10.w;
        // mad oPos.xy, r12.xy, c[40].xy, c[40].zw
        let mut p = clip();
        p.extend([
            Inst::ilu(IluOp::Rcc, O::r(12).swz("w")).ilu_to_temp(10, W),
            Inst::mac(MacOp::Mul, Some(O::r(12)), Some(O::r(10).swz("w")), None).to_out(
                false,
                OutReg::Pos,
                X | Y | Z,
            ),
            Inst::mac(
                MacOp::Mad,
                Some(O::r(12).swz("xyyy")),
                Some(O::c(40).swz("xyyy")),
                Some(O::c(40).swz("zwww")),
            )
            .to_out(false, OutReg::Pos, X | Y),
        ]);
        assert_eq!(
            Epilogue::of(&words(&p)),
            Some(Epilogue {
                scale: [(40, 0), (40, 1)],
                offset: [(40, 2), (40, 3)],
            })
        );
    }

    #[test]
    fn screen_space_lookups_from_the_clip_position_are_allowed() {
        // dp4 oT0.x, r12, c[8] reads the clip position, not the scale.
        let mut p = clip();
        p.push(
            Inst::mac(MacOp::Dp4, Some(O::r(12)), Some(O::c(8)), None).to_out(
                false,
                OutReg::T(0),
                X,
            ),
        );
        p.extend(standard(10, 11));
        assert!(Epilogue::of(&words(&p)).is_some());
    }

    #[test]
    fn programs_it_cannot_follow_are_not_read() {
        let mut cases: Vec<(&str, Vec<Inst>)> = Vec::new();
        // Window coordinates written as they are read.
        cases.push((
            "pass-through",
            vec![Inst::mac(MacOp::Mov, Some(O::v(0)), None, None).to_out(false, OutReg::Pos, 0xF)],
        ));
        // No divide by w.
        let mut p = clip();
        p.extend([
            Inst::mac(MacOp::Mul, Some(O::r(12)), Some(O::c(10)), None).to_out(
                false,
                OutReg::Pos,
                X | Y | Z,
            ),
            Inst::mac(MacOp::Add, Some(O::r(12)), None, Some(O::c(11))).to_out(
                false,
                OutReg::Pos,
                X | Y | Z,
            ),
        ]);
        cases.push(("no divide", p));
        // The scale also read for a texture coordinate.
        let mut p = clip();
        p.extend(standard(10, 11));
        p.push(Inst::mac(MacOp::Mov, Some(O::c(10)), None, None).to_out(
            false,
            OutReg::T(1),
            X | Y,
        ));
        cases.push(("scale read elsewhere", p));
        // The window position read back for a texture coordinate: scaling
        // would move the lookup too.
        let mut p = clip();
        p.extend(standard(10, 11));
        p.push(Inst::mac(MacOp::Mov, Some(O::r(12)), None, None).to_out(
            false,
            OutReg::T(1),
            X | Y,
        ));
        cases.push(("window position read", p));
        // A second clip position after the first was used.
        let mut p = clip();
        p.extend(standard(10, 11));
        p.push(
            Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(3)), None).to_out(false, OutReg::Pos, W),
        );
        cases.push(("w rewritten", p));
        // A relative read that could reach the scale.
        let mut p = clip();
        p.push(Inst::mac(MacOp::Mov, Some(O::c_rel(4)), None, None).to_out(
            false,
            OutReg::T(2),
            0xF,
        ));
        p.extend(standard(10, 11));
        cases.push(("relative below", p));
        for (name, p) in cases {
            assert_eq!(Epilogue::of(&words(&p)), None, "{name}");
        }
        // A relative read above the scale (skinning nodes) is fine.
        let mut p = clip();
        p.push(
            Inst::mac(MacOp::Mov, Some(O::c_rel(72)), None, None).to_out(false, OutReg::T(2), 0xF),
        );
        p.extend(standard(10, 11));
        assert!(Epilogue::of(&words(&p)).is_some());
        // Not microcode at all.
        assert_eq!(Epilogue::of(&[1, 2, 3]), None);
    }

    #[test]
    fn the_position_is_read_from_the_rows_it_comes_from() {
        // Straight to `oPos`, the epilogue after.
        let mut p = clip();
        p.extend(standard(10, 11));
        assert_eq!(position(&words(&p)), Position::Rows(0));
        // Through a temporary, as some of Halo 2's programs do: `xyz` into
        // `r10`, `w` into `r10` and `oPos` at once, then `mov oPos.xyz,
        // r10`.
        let mut p: Vec<Inst> = (0..3)
            .map(|i| {
                Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(20 + i)), None).to_temp(10, 8 >> i)
            })
            .collect();
        p.push(
            Inst::mac(MacOp::Dph, Some(O::v(0)), Some(O::c(23)), None)
                .to_temp(10, W)
                .to_out(false, OutReg::Pos, W),
        );
        p.push(Inst::mac(MacOp::Mov, Some(O::r(10)), None, None).to_out(
            false,
            OutReg::Pos,
            X | Y | Z,
        ));
        p.extend(standard(10, 11));
        assert_eq!(position(&words(&p)), Position::Rows(20));
        // Vertices passed through: no constant reaches them, with or
        // without an epilogue after.
        let pass = Inst::mac(MacOp::Mov, Some(O::v(0)), None, None).to_out(false, OutReg::Pos, 0xF);
        assert_eq!(
            position(&words(std::slice::from_ref(&pass))),
            Position::Fixed
        );
        let mut p = vec![pass];
        p.extend(standard(10, 11));
        assert_eq!(position(&words(&p)), Position::Fixed);
        // Anything else is not known.
        let scaled = Inst::mac(MacOp::Mul, Some(O::v(0)), Some(O::c(16)), None).to_out(
            false,
            OutReg::Pos,
            0xF,
        );
        assert_eq!(position(&words(&[scaled])), Position::Unknown);
        let mut two = clip();
        two[2] = Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(30)), None).to_out(
            false,
            OutReg::Pos,
            Z,
        );
        assert_eq!(
            position(&words(&two)),
            Position::Unknown,
            "rows of two blocks"
        );
        let mut swapped = clip();
        swapped.swap(0, 1);
        for (i, inst) in swapped.iter_mut().enumerate() {
            *inst = Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c([1, 0, 2, 3][i])), None).to_out(
                false,
                OutReg::Pos,
                8 >> i,
            );
        }
        assert_eq!(
            position(&words(&swapped)),
            Position::Unknown,
            "rows out of order"
        );
        let mut negated: Vec<Inst> = (0..4)
            .map(|i| Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(i)), None).to_temp(10, 8 >> i))
            .collect();
        negated.push(
            Inst::mac(MacOp::Mov, Some(O::r(10).negated()), None, None).to_out(
                false,
                OutReg::Pos,
                0xF,
            ),
        );
        assert_eq!(position(&words(&negated)), Position::Unknown, "negated");
    }

    #[test]
    fn the_overscan_moves_the_position_and_nothing_else() {
        let mut p = clip();
        p.push(
            Inst::mac(MacOp::Dp4, Some(O::r(12)), Some(O::c(8)), None).to_out(
                false,
                OutReg::T(0),
                X,
            ),
        );
        p.extend(standard(10, 11));
        let w = words(&p);
        let e = Epilogue::of(&w).unwrap();
        let program = Program::from_words(ProgramKind::Normal, &w).unwrap();
        let mut c = [[0.0f32; 4]; CONSTANTS];
        c[0] = [1.2, 0.1, 0.0, 0.5];
        c[1] = [0.0, 1.6, 0.2, -0.3];
        c[2] = [0.0, 0.0, -1.0, 0.9];
        c[3] = [0.0, 0.0, -1.0, 4.0];
        c[8] = [0.5, 0.25, 0.0, 0.5];
        c[10] = [320.0, -240.0, 1.0, 0.0];
        c[11] = [320.0, 240.0, 0.0, 0.0];
        assert_eq!(e.read(&c), ([320.0, -240.0], [320.0, 240.0]));
        let mut o = c;
        e.overscan(&mut o, 0.5);
        let mut inputs = [[0.0f32; 4]; 16];
        inputs[0] = [0.3, -0.7, 2.0, 1.0];
        let a = shaders::vsh::interp::run(&program, &inputs, &mut c.clone());
        let b = shaders::vsh::interp::run(&program, &inputs, &mut o.clone());
        assert_eq!(a.get(OutReg::T(0)), b.get(OutReg::T(0)));
        let (pa, pb) = (a.get(OutReg::Pos), b.get(OutReg::Pos));
        for (i, centre) in [(0, 320.0), (1, 240.0)] {
            assert!((pb[i] - (centre + 0.5 * (pa[i] - centre))).abs() < 1e-3);
        }
        assert_eq!((pa[2], pa[3]), (pb[2], pb[3]));
    }
}
