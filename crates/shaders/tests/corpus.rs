//! Optional: every shader extracted from your own disc.
//!
//! Set `H2_SHADER_CORPUS` to a directory written by
//! `examples/extract_corpus.rs`: `*.xvu` (vrtx compiled-shader bytes) and
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
            "{}: no manifest.tsv ({e}); extract with examples/extract_corpus.rs",
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
