//! Optional: every shader extracted from your own disc.
//!
//! Set `H2_SHADER_CORPUS` to a directory written by
//! `h2 extract-shaders`: `*.xvu` (vrtx compiled-shader bytes) and
//! `*.psdef` (spas implementation bytes 6..0xF6) files plus `manifest.tsv`.
//! Without the variable the test passes trivially, so no game data is needed
//! to build or test the crate. With it, the directory must hold exactly the
//! manifest's files with the manifest's hashes; an empty or partial
//! directory fails.
//!
//! With a GPU, vertex programs are also run on it and compared with the
//! interpreter on random inputs.

mod common;

use sha2::{Digest, Sha256};
use shaders::{psh, vsh};
use std::collections::BTreeMap;
use std::path::Path;

/// Checks `dir` against its manifest and returns the shader file paths.
fn manifest_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let manifest = std::fs::read_to_string(dir.join("manifest.tsv")).unwrap_or_else(|e| {
        panic!(
            "{}: no manifest.tsv ({e}); extract with `h2 extract-shaders --out DIR`",
            dir.display()
        )
    });
    let mut want = BTreeMap::new();
    for line in manifest.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        assert!(f.len() >= 2, "bad manifest line {line:?}");
        want.insert(f[0].to_string(), f[1].to_string());
    }
    let mut found = BTreeMap::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.ends_with(".xvu") || name.ends_with(".psdef") {
            let bytes = std::fs::read(e.path()).unwrap();
            let hash: String = Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            found.insert(name, hash);
        }
    }
    for (name, hash) in &want {
        match found.get(name) {
            None => panic!("{name} is in the manifest but missing"),
            Some(h) => assert_eq!(h, hash, "{name}: hash differs from the manifest"),
        }
    }
    for name in found.keys() {
        assert!(want.contains_key(name), "{name} is not in the manifest");
    }
    let count = |ext: &str| want.keys().filter(|k| k.ends_with(ext)).count();
    assert!(
        count(".xvu") > 0 && count(".psdef") > 0,
        "{}: corpus has {} vertex programs and {} pixel defs",
        dir.display(),
        count(".xvu"),
        count(".psdef")
    );
    want.keys().map(|k| dir.join(k)).collect()
}

#[test]
fn corpus_translates_validates_and_matches() {
    let Some(dir) = std::env::var_os("H2_SHADER_CORPUS") else {
        eprintln!("H2_SHADER_CORPUS not set; skipping");
        return;
    };
    let entries = manifest_files(Path::new(&dir));
    let gpu = common::gpu();
    let mut rng = common::Rng(0x0dd5_eed5);
    let (mut vs, mut ps, mut compared, mut nonfinite) = (0, 0, 0usize, 0usize);
    for path in entries {
        let name = path.display().to_string();
        let bytes = std::fs::read(&path).unwrap();
        if name.ends_with(".xvu") {
            vs += 1;
            let p = vsh::Program::parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
            let t = vsh::translate(&p, &Default::default()).unwrap();
            common::check(&t.module());
            let Some(gpu) = &gpu else { continue };
            let reads = p.inputs_read();
            let args: Vec<String> = (0..16)
                .filter(|n| reads & (1 << n) != 0)
                .map(|n| format!("hin[id.x * 16u + {n}u]"))
                .collect();
            let src = format!(
                "{}\n@group(1) @binding(8) var<storage, read> hin: array<vec4f>;\n@group(1) @binding(9) var<storage, read_write> hout: array<vec4f>;\n@compute @workgroup_size(1)\nfn harness(@builtin(global_invocation_id) id: vec3u) {{\n    let o = xvs_program({});\n    let b = id.x * 11u;\n    hout[b] = o.pos; hout[b + 1u] = o.d0; hout[b + 2u] = o.d1; hout[b + 3u] = o.fog; hout[b + 4u] = o.pts;\n    hout[b + 5u] = o.b0; hout[b + 6u] = o.b1; hout[b + 7u] = o.t0; hout[b + 8u] = o.t1; hout[b + 9u] = o.t2; hout[b + 10u] = o.t3;\n}}\n",
                t.module(),
                args.join(", ")
            );
            let mut u = vsh::VertexUniforms::default();
            for c in u.constants.iter_mut() {
                *c = std::array::from_fn(|_| rng.f(-1.0, 1.0));
            }
            let n = 16;
            let inputs: Vec<[[f32; 4]; 16]> = (0..n)
                .map(|_| std::array::from_fn(|_| std::array::from_fn(|_| rng.f(0.0, 1.0))))
                .collect();
            let flat: Vec<f32> = inputs.iter().flatten().flatten().copied().collect();
            let out = gpu.run(&src, "harness", 0, &u.to_bytes(), &flat, n * 44, n as u32);
            for (i, inp) in inputs.iter().enumerate() {
                let cpu = vsh::interp::run(&p, inp, &mut u.constants.clone());
                for r in 0..11 {
                    for k in 0..4 {
                        let (a, b) = (out[i * 44 + r * 4 + k], cpu.regs[r][k]);
                        if !b.is_finite() {
                            // Must match exactly (close() demands it) but
                            // counted apart; see gpu.rs on Metal fast math.
                            nonfinite += 1;
                        }
                        compared += 1;
                        assert!(
                            common::close(a, b, 1e-3),
                            "{name} vertex {i} reg {r}.{k}: gpu {a} cpu {b}"
                        );
                    }
                }
            }
        } else if name.ends_with(".psdef") {
            ps += 1;
            let d = psh::PixelShaderDef::from_bytes(&bytes).unwrap();
            let t = psh::translate(&d, &Default::default());
            assert!(t.warnings.is_empty(), "{name}: {:?}", t.warnings);
            common::check(&t.module());
        }
    }
    eprintln!(
        "corpus: {vs} vertex programs, {ps} pixel defs, {compared} GPU comparisons ({nonfinite} non-finite)"
    );
}

/// A scratch directory (synthetic files only; no game data).
fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("shaders-corpus-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Regression (review P2): pointing the test at an empty directory passed.
#[test]
#[should_panic(expected = "no manifest.tsv")]
fn empty_corpus_directory_fails() {
    manifest_files(&scratch("empty"));
}

#[test]
#[should_panic(expected = "missing")]
fn corpus_missing_a_manifest_file_fails() {
    let d = scratch("missing");
    std::fs::write(d.join("manifest.tsv"), "0000.xvu\tab\t1\tx\n").unwrap();
    manifest_files(&d);
}

#[test]
#[should_panic(expected = "0 pixel defs")]
fn corpus_without_pixel_defs_fails() {
    let d = scratch("nopixel");
    let bytes = [1u8, 2, 3];
    let hash: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    std::fs::write(d.join("a.xvu"), bytes).unwrap();
    std::fs::write(d.join("manifest.tsv"), format!("a.xvu\t{hash}\t1\tx\n")).unwrap();
    manifest_files(&d);
}

/// Whether the value instruction `at` writes through its output port
/// depends on an `rsq` (any radial distance needs a square root): a
/// backward slice over temporaries, whole registers, conservatively (a
/// partial write keeps the register live). Writes to `oPos` count as `r12`.
fn fog_slice_has_rsq(p: &vsh::Program, at: usize) -> bool {
    let temps = |srcs: &[&vsh::Source]| -> u16 {
        srcs.iter().fold(0u16, |m, s| match s.reg {
            vsh::SrcReg::Temp(r) => m | 1 << r,
            _ => m,
        })
    };
    let mac_sources = |i: &vsh::Instruction| {
        let (a, b, c) = i.mac_reads();
        let mut v = Vec::new();
        for (on, s) in [(a, &i.a), (b, &i.b), (c, &i.c)] {
            if on {
                v.push(s);
            }
        }
        temps(&v)
    };
    let ilu_sources = |i: &vsh::Instruction| {
        if i.ilu == vsh::IluOp::Nop {
            0
        } else {
            temps(&[&i.c])
        }
    };
    let i = &p.instructions[at];
    if i.out.is_some_and(|o| o.unit == vsh::Unit::Ilu) && i.ilu == vsh::IluOp::Rsq {
        return true;
    }
    let mut live = match i.out.map(|o| o.unit) {
        Some(vsh::Unit::Ilu) => ilu_sources(i),
        _ => mac_sources(i),
    };
    for i in p.instructions[..at].iter().rev() {
        let mut mac_dest = if i.mac_writes_temp() {
            1u16 << i.mac_temp
        } else {
            0
        };
        let mut ilu_dest = if i.ilu_writes_temp() {
            1u16 << i.ilu_temp
        } else {
            0
        };
        if let Some(o) = i.out
            && o.dest == vsh::OutDest::Output(vsh::OutReg::Pos)
        {
            match o.unit {
                vsh::Unit::Mac => mac_dest |= 1 << 12,
                vsh::Unit::Ilu => ilu_dest |= 1 << 12,
            }
        }
        if ilu_dest & live != 0 {
            if i.ilu == vsh::IluOp::Rsq {
                return true;
            }
            live |= ilu_sources(i);
        }
        if mac_dest & live != 0 {
            live |= mac_sources(i);
        }
    }
    false
}

/// The slice sees a radial distance behind a final multiply, and none
/// behind a plane distance (synthetic programs).
#[test]
fn fog_slice_finds_a_radial_distance() {
    use vsh::encode::{Inst, Operand, program};
    use vsh::{IluOp, MacOp, OutReg};
    // r0 = v0 - c[10]; r1.x = dp3(r0, r0); r1.x = rsq; r1.x = 1/rsq;
    // oFog = r1.x * c[11].x
    let radial = program(&[
        Inst::mac(
            MacOp::Add,
            Some(Operand::v(0)),
            None,
            Some(Operand::c(10).negated()),
        )
        .to_temp(0, 0xF),
        Inst::mac(MacOp::Dp3, Some(Operand::r(0)), Some(Operand::r(0)), None).to_temp(2, 8),
        Inst::ilu(IluOp::Rsq, Operand::r(2).swz("x")).ilu_to_temp(1, 8),
        Inst::ilu(IluOp::Rcp, Operand::r(1).swz("x")).ilu_to_temp(1, 8),
        Inst::mac(
            MacOp::Mul,
            Some(Operand::r(1).swz("x")),
            Some(Operand::c(11).swz("x")),
            None,
        )
        .to_out(false, OutReg::Fog, 8)
        .last(),
    ]);
    let p = vsh::Program::parse(&radial).unwrap();
    assert!(fog_slice_has_rsq(&p, 4));
    // A normalised normal elsewhere does not taint a plane distance.
    let planar = program(&[
        Inst::mac(MacOp::Dp3, Some(Operand::v(2)), Some(Operand::v(2)), None).to_temp(3, 8),
        Inst::ilu(IluOp::Rsq, Operand::r(3).swz("x")).ilu_to_temp(1, 8),
        Inst::mac(
            MacOp::Mul,
            Some(Operand::v(2)),
            Some(Operand::r(1).swz("x")),
            None,
        )
        .to_out(false, OutReg::D0, 0xF),
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_temp(4, 0xF),
        Inst::mac(MacOp::Dph, Some(Operand::r(4)), Some(Operand::c(35)), None)
            .to_out(false, OutReg::Fog, 8)
            .last(),
    ]);
    let p = vsh::Program::parse(&planar).unwrap();
    assert!(!fog_slice_has_rsq(&p, 4));
}

/// How the corpus produces and consumes fog: the form of every `oFog`
/// write (a plane distance `dph`/`dp4` with a constant, a scaled `w` from
/// `mad`/`mul`, a vertex attribute `mov`, or anything else, which would
/// include a radial `dp3`/`rsq` distance), whether any write depends on an
/// `rsq` ([`fog_slice_has_rsq`]), and which combiner slots read the fog
/// register. Prints the census and asserts that every write has one of the
/// three known shapes and none depends on an `rsq`.
#[test]
fn corpus_fog_census() {
    let Some(dir) = std::env::var_os("H2_SHADER_CORPUS") else {
        eprintln!("H2_SHADER_CORPUS not set; skipping");
        return;
    };
    let mut writes: BTreeMap<&str, usize> = BTreeMap::new();
    let mut radial = Vec::new();
    let (mut fog_programs, mut programs) = (0, 0);
    let mut reads: BTreeMap<String, usize> = BTreeMap::new();
    let (mut fog_defs, mut defs) = (0, 0);
    for path in manifest_files(Path::new(&dir)) {
        let bytes = std::fs::read(&path).unwrap();
        let name = path.display().to_string();
        if name.ends_with(".xvu") {
            programs += 1;
            let p = vsh::Program::parse(&bytes).unwrap();
            let mut any = false;
            for (at, i) in p.instructions.iter().enumerate() {
                let Some(out) = i.out else { continue };
                if out.dest != vsh::OutDest::Output(vsh::OutReg::Fog) {
                    continue;
                }
                any = true;
                if fog_slice_has_rsq(&p, at) {
                    radial.push(name.clone());
                }
                let from_const = |s: &vsh::Source| matches!(s.reg, vsh::SrcReg::Const { .. });
                let kind = match (out.unit, i.mac, i.ilu) {
                    (vsh::Unit::Mac, vsh::MacOp::Dph | vsh::MacOp::Dp4, _)
                        if from_const(&i.a) || from_const(&i.b) =>
                    {
                        "plane distance (dph/dp4 with a constant)"
                    }
                    (vsh::Unit::Mac, vsh::MacOp::Mad | vsh::MacOp::Mul, _)
                        if i.a.swizzle == [3; 4] =>
                    {
                        "scaled w (mad/mul of r.w)"
                    }
                    (vsh::Unit::Mac, vsh::MacOp::Mov, _)
                        if matches!(i.a.reg, vsh::SrcReg::Input(_)) =>
                    {
                        "vertex attribute (mov v)"
                    }
                    _ => "other",
                };
                *writes.entry(kind).or_insert(0) += 1;
            }
            fog_programs += usize::from(any);
        } else if name.ends_with(".psdef") {
            defs += 1;
            let d = psh::PixelShaderDef::from_bytes(&bytes).unwrap();
            let mut any = false;
            let mut note = |slot: String, byte: u32| {
                if byte & 0x0F == 3 {
                    let ch = if byte & 0x10 != 0 { "a" } else { "rgb" };
                    let inv = if byte >> 5 == 1 { "1-" } else { "" };
                    *reads.entry(format!("{slot}: {inv}fog.{ch}")).or_insert(0) += 1;
                    any = true;
                }
            };
            let stages = (d.combiner_count & 0xFF) as usize;
            for n in 0..stages.min(8) {
                for (k, l) in ["A", "B", "C", "D"].iter().enumerate() {
                    note(
                        format!("general rgb {l}"),
                        d.rgb_inputs[n] >> (24 - 8 * k) & 0xFF,
                    );
                    note(
                        format!("general alpha {l}"),
                        d.alpha_inputs[n] >> (24 - 8 * k) & 0xFF,
                    );
                }
            }
            for (k, l) in ["A", "B", "C", "D"].iter().enumerate() {
                note(
                    format!("final {l}"),
                    d.final_inputs_abcd >> (24 - 8 * k) & 0xFF,
                );
            }
            for (k, l) in ["E", "F", "G"].iter().enumerate() {
                note(
                    format!("final {l}"),
                    d.final_inputs_efg >> (24 - 8 * k) & 0xFF,
                );
            }
            fog_defs += usize::from(any);
        }
    }
    eprintln!("fog: {fog_programs} of {programs} vertex programs write oFog: {writes:?}");
    eprintln!("fog: {fog_defs} of {defs} pixel defs read the fog register: {reads:?}");
    eprintln!("fog: writes depending on an rsq: {radial:?}");
    assert_eq!(writes.get("other"), None, "a fog write of an unknown shape");
    assert!(
        radial.is_empty(),
        "fog writes depending on an rsq: {radial:?}"
    );
}
