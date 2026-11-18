//! Whether an override can stand in for a translated program without the
//! renderer noticing: the pipeline layout, the vertex buffers and the
//! colour targets are made for the translated program, so an override
//! must ask for nothing they don't provide. wgpu treats a mismatch as a
//! validation error, which ends the run; this check turns it into a
//! rejected override instead.
//!
//! The rules, each against the translated program:
//! * every entry point it has (`vs_main`, `fs_main`, the count pass's
//!   depth entry when W-buffered) is there, with the same stage;
//! * every resource binding is one it has: same group and binding, same
//!   address space, and the same size (uniform structs) or the same image
//!   or sampler type;
//! * vertex inputs are locations it reads, of the same type;
//! * fragment outputs are locations it writes, of the same type;
//! * within the override, every location the fragment stage reads is one
//!   the vertex stage writes, of the same type.

use std::collections::BTreeMap;

use naga::{Binding, Module, ShaderStage, TypeInner};

/// Location → type, of an entry point's inputs or outputs.
type Locations = BTreeMap<u32, String>;

fn parse(what: &str, source: &str) -> Result<Module, String> {
    shaders::validate(source)
        .map(|(m, _)| m)
        .map_err(|e| format!("{what}: {e}"))
}

/// Checks `candidate` against `translated` (module docs). The error says
/// what differs.
pub fn check(translated: &str, candidate: &str) -> Result<(), String> {
    let t = parse("the translated program", translated)?;
    let c = parse("the override", candidate)?;
    for ep in &t.entry_points {
        let Some(o) = c.entry_points.iter().find(|o| o.name == ep.name) else {
            return Err(format!(
                "no entry point `{}` (the translated program has it)",
                ep.name
            ));
        };
        if o.stage != ep.stage {
            return Err(format!(
                "entry point `{}` is {:?}, the translated one {:?}",
                ep.name, o.stage, ep.stage
            ));
        }
    }
    let tb = bindings(&t);
    for (key, desc) in bindings(&c) {
        match tb.get(&key) {
            Some(d) if *d == desc => {}
            Some(d) => {
                return Err(format!(
                    "@group({}) @binding({}) is {desc}; the renderer binds {d}",
                    key.0, key.1
                ));
            }
            None => {
                return Err(format!(
                    "@group({}) @binding({}) is not bound for this program",
                    key.0, key.1
                ));
            }
        }
    }
    for ep in &c.entry_points {
        let Some(orig) = t.entry_points.iter().find(|o| o.name == ep.name) else {
            continue;
        };
        match ep.stage {
            ShaderStage::Vertex => {
                subset(
                    &inputs(&c, &ep.function),
                    &inputs(&t, &orig.function),
                    &format!("`{}` input", ep.name),
                    "the vertex buffers supply",
                )?;
            }
            ShaderStage::Fragment => {
                subset(
                    &outputs(&c, &ep.function),
                    &outputs(&t, &orig.function),
                    &format!("`{}` output", ep.name),
                    "the targets take",
                )?;
            }
            _ => {}
        }
    }
    // Inter-stage: what each fragment entry reads, every vertex entry writes.
    for f in c
        .entry_points
        .iter()
        .filter(|e| e.stage == ShaderStage::Fragment)
    {
        let reads = inputs(&c, &f.function);
        for v in c
            .entry_points
            .iter()
            .filter(|e| e.stage == ShaderStage::Vertex)
        {
            subset(
                &reads,
                &outputs(&c, &v.function),
                &format!("`{}` input", f.name),
                &format!("`{}` writes", v.name),
            )?;
        }
    }
    Ok(())
}

fn subset(have: &Locations, allowed: &Locations, what: &str, whose: &str) -> Result<(), String> {
    for (loc, ty) in have {
        match allowed.get(loc) {
            Some(t) if t == ty => {}
            Some(t) => {
                return Err(format!("{what} @location({loc}) is {ty}; {whose} {t}"));
            }
            None => return Err(format!("{what} @location({loc}): not what {whose}")),
        }
    }
    Ok(())
}

/// (group, binding) → address space and type.
fn bindings(m: &Module) -> BTreeMap<(u32, u32), String> {
    m.global_variables
        .iter()
        .filter_map(|(_, g)| {
            let b = g.binding.as_ref()?;
            Some((
                (b.group, b.binding),
                format!("{:?} {}", g.space, type_desc(m, g.ty)),
            ))
        })
        .collect()
}

/// A type as comparable text across modules: handles differ between
/// modules, so structs and arrays are compared by size.
fn type_desc(m: &Module, ty: naga::Handle<naga::Type>) -> String {
    let inner = &m.types[ty].inner;
    match inner {
        TypeInner::Struct { .. } | TypeInner::Array { .. } => {
            format!("{} bytes", inner.size(m.to_ctx()))
        }
        other => format!("{other:?}"),
    }
}

fn collect(
    m: &Module,
    ty: naga::Handle<naga::Type>,
    binding: Option<&Binding>,
    out: &mut Locations,
) {
    match binding {
        Some(Binding::Location { location, .. }) => {
            out.insert(*location, type_desc(m, ty));
        }
        Some(Binding::BuiltIn(_)) => {}
        None => {
            if let TypeInner::Struct { members, .. } = &m.types[ty].inner {
                for mem in members {
                    collect(m, mem.ty, mem.binding.as_ref(), out);
                }
            }
        }
    }
}

fn inputs(m: &Module, f: &naga::Function) -> Locations {
    let mut out = Locations::new();
    for a in &f.arguments {
        collect(m, a.ty, a.binding.as_ref(), &mut out);
    }
    out
}

fn outputs(m: &Module, f: &naga::Function) -> Locations {
    let mut out = Locations::new();
    if let Some(r) = &f.result {
        collect(m, r.ty, r.binding.as_ref(), &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A program shaped like a translated one (synthetic): uniforms at
    /// group 0, one texture at group 1, a varyings struct.
    const BASE: &str = r#"
struct V { @builtin(position) p: vec4f, @location(0) d0: vec4f, @location(4) t0: vec4f }
struct U { c: array<vec4f, 4> }
@group(0) @binding(1) var<uniform> pu: U;
@group(1) @binding(0) var tex0: texture_2d<f32>;
@group(1) @binding(1) var samp0: sampler;
@vertex fn vs_main(@location(0) pos: vec4f, @location(3) col: vec4f) -> V {
    var o: V; o.p = pos; o.d0 = col; o.t0 = vec4f(0.0); return o;
}
@fragment fn fs_main(v: V) -> @location(0) vec4f {
    return textureSample(tex0, samp0, v.t0.xy) * v.d0 * pu.c[0];
}
"#;

    #[test]
    fn the_translation_itself_passes() {
        check(BASE, BASE).unwrap();
    }

    #[test]
    fn a_body_change_passes() {
        let o = BASE.replace("* pu.c[0]", "* vec4f(1.0, 0.0, 1.0, 1.0)");
        check(BASE, &o).unwrap();
    }

    #[test]
    fn fewer_inputs_and_bindings_pass() {
        let o = r#"
struct V { @builtin(position) p: vec4f, @location(0) d0: vec4f }
@vertex fn vs_main(@location(0) pos: vec4f) -> V { var o: V; o.p = pos; o.d0 = vec4f(1.0); return o; }
@fragment fn fs_main(v: V) -> @location(0) vec4f { return v.d0; }
"#;
        check(BASE, o).unwrap();
    }

    #[test]
    fn a_missing_entry_point_is_refused() {
        let o = BASE.replace("fn fs_main", "fn fs_other");
        let e = check(BASE, &o).unwrap_err();
        assert!(e.contains("fs_main"), "{e}");
    }

    #[test]
    fn a_new_binding_is_refused() {
        let o = BASE.replace(
            "@group(1) @binding(1) var samp0: sampler;",
            "@group(1) @binding(1) var samp0: sampler;\n@group(1) @binding(2) var tex1: texture_2d<f32>;",
        );
        let e = check(BASE, &o).unwrap_err();
        assert!(e.contains("@binding(2)"), "{e}");
    }

    #[test]
    fn a_resized_uniform_is_refused() {
        let o = BASE.replace("array<vec4f, 4>", "array<vec4f, 8>");
        let e = check(BASE, &o).unwrap_err();
        assert!(e.contains("bytes"), "{e}");
    }

    #[test]
    fn a_new_vertex_input_is_refused() {
        let o = BASE.replace(
            "@location(3) col: vec4f",
            "@location(3) col: vec4f, @location(7) x: vec4f",
        );
        let e = check(BASE, &o).unwrap_err();
        assert!(e.contains("@location(7)"), "{e}");
    }

    #[test]
    fn a_retyped_vertex_input_is_refused() {
        let o = BASE
            .replace("@location(3) col: vec4f)", "@location(3) col: vec2f)")
            .replace("o.d0 = col;", "o.d0 = vec4f(col, 0.0, 1.0);");
        let e = check(BASE, &o).unwrap_err();
        assert!(e.contains("@location(3)"), "{e}");
    }

    #[test]
    fn unlinked_varyings_are_refused() {
        let o = r#"
struct V { @builtin(position) p: vec4f, @location(0) d0: vec4f }
struct F { @builtin(position) p: vec4f, @location(0) d0: vec4f, @location(5) t1: vec4f }
@vertex fn vs_main(@location(0) pos: vec4f) -> V { var o: V; o.p = pos; o.d0 = vec4f(1.0); return o; }
@fragment fn fs_main(v: F) -> @location(0) vec4f { return v.d0 + v.t1; }
"#;
        let e = check(BASE, o).unwrap_err();
        assert!(e.contains("@location(5)"), "{e}");
    }

    #[test]
    fn invalid_wgsl_is_refused_with_naga_s_message() {
        let e = check(BASE, "fn broken(").unwrap_err();
        assert!(e.starts_with("the override"), "{e}");
    }
}
