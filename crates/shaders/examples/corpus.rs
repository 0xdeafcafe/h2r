//! Translate and validate a directory of extracted shaders.
//!
//! ```sh
//! cargo run -p shaders --example corpus -- <dir> [--dump <file>]
//! ```
//!
//! `<dir>` holds `*.xvu` files (a `vrtx` classification's compiled-shader
//! bytes: the `'x'` header plus microcode) and `*.psdef` files (the 0xF0
//! `D3DPIXELSHADERDEF` bytes, which Halo 2 keeps at offset 6 of each
//! `spas` postprocess implementation). Nothing here ships game data: point
//! it at a scratch directory you extracted yourself.
//!
//! With `--dump <file>` it prints that file's disassembly and WGSL instead.

use shaders::{psh, vsh};
use std::collections::BTreeMap;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = Path::new(args.get(1).expect("usage: corpus <dir> [--dump <file>]"));
    if args.get(2).map(String::as_str) == Some("--dump") {
        let path = dir.join(&args[3]);
        let bytes = std::fs::read(&path).unwrap();
        if args[3].ends_with(".xvu") {
            let p = vsh::Program::parse(&bytes).unwrap();
            print!("{}", vsh::disassemble(&p));
            println!(
                "{}",
                vsh::translate(&p, &Default::default()).unwrap().module()
            );
        } else {
            let d = psh::PixelShaderDef::from_bytes(&bytes).unwrap();
            print!("{}", psh::disassemble(&d));
            let t = psh::translate(&d, &Default::default());
            println!("{}", t.module());
            for w in t.warnings {
                println!("// warning: {w}");
            }
        }
        return;
    }
    if args.get(2).map(String::as_str) == Some("--disasm-all") {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        entries.sort();
        for path in entries {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let bytes = std::fs::read(&path).unwrap();
            let text = if name.ends_with(".xvu") {
                vsh::disassemble(&vsh::Program::parse(&bytes).unwrap())
            } else if name.ends_with(".psdef") {
                psh::disassemble(&psh::PixelShaderDef::from_bytes(&bytes).unwrap())
            } else {
                continue;
            };
            for line in text.lines() {
                println!("{name}\t{line}");
            }
        }
        return;
    }
    let report = shaders_corpus_report(dir);
    print!("{report}");
}

fn shaders_corpus_report(dir: &Path) -> String {
    let mut out = String::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut bump = |k: String| *counts.entry(k).or_default() += 1;
    let mut failures = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let bytes = std::fs::read(&path).unwrap();
        if name.ends_with(".xvu") {
            bump("vertex programs".into());
            let p = match vsh::Program::parse(&bytes) {
                Ok(p) => p,
                Err(e) => {
                    failures.push(format!("{name}: decode: {e}"));
                    continue;
                }
            };
            bump(format!("vertex kind {:?}", p.kind));
            for i in &p.instructions {
                bump(format!("mac {:?}", i.mac));
                bump(format!("ilu {:?}", i.ilu));
                if let Some(o) = i.out {
                    bump(format!("write {:?} mask {}", o.dest, o.mask));
                }
                if i.sources()
                    .any(|s| matches!(s.reg, vsh::SrcReg::Const { relative: true, .. }))
                {
                    bump("relative constant reads".into());
                }
            }
            let t = vsh::translate(&p, &Default::default()).unwrap();
            if let Err(e) = shaders::validate(&t.module()) {
                failures.push(format!("{name}: naga: {e}"));
            } else {
                bump("vertex programs valid".into());
            }
        } else if name.ends_with(".psdef") {
            bump("pixel defs".into());
            let d = psh::PixelShaderDef::from_bytes(&bytes).unwrap();
            let c = d.decode();
            bump(format!("combiner stages {}", c.stages.len()));
            bump(format!("final combiner {}", c.final_combiner.is_some()));
            for (i, m) in c.texture_modes.iter().enumerate() {
                bump(format!("tex mode {} (stage {i})", m.name()));
                if m.is_dot() {
                    bump(format!("dot map {:?} (stage {i})", c.dot_mapping[i]));
                }
            }
            for st in &c.stages {
                for p in [&st.rgb, &st.alpha] {
                    if p.output.mux {
                        bump("mux outputs".into());
                    }
                    if p.output.ab_dot || p.output.cd_dot {
                        bump("dot outputs".into());
                    }
                }
            }
            let t = psh::translate(&d, &Default::default());
            for w in &t.warnings {
                bump(format!("warning: {w}"));
            }
            if let Err(e) = shaders::validate(&t.module()) {
                failures.push(format!("{name}: naga: {e}"));
            } else {
                bump("pixel defs valid".into());
            }
        }
    }
    for (k, v) in &counts {
        out.push_str(&format!("{v:6}  {k}\n"));
    }
    for f in &failures {
        out.push_str(&format!("FAIL {f}\n"));
    }
    out
}
