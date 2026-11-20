//! WGSL overrides of translated programs, hot (docs/graphics/hot-iteration.md).
//!
//! Once a frame ([`Nv2aShaders::maintain`]) the provider looks at the
//! hot-render hub's generation. When it moved, or programs were added,
//! every program is brought to what the hub says it should run: its
//! override (with parameters substituted) or its translation. A program
//! changes by re-registering its shader id with a new module, which drops
//! the renderer's cached pipelines; the next draw of anything builds its
//! pipeline again (from the compile workers, as at a cold start).
//!
//! An override is checked against the program's translation first
//! (`hot_render::iface::check`): an override that would not fit the
//! layout, vertex buffers or targets the renderer made for the program is
//! rejected and whatever ran before keeps running. Programs are published
//! to the hub under their pipeline-cache id, which names override files.

use super::*;
use hot_render::{DumpReport, Hub, Problem, ProgramInfo, program_text};

/// The provider's side of the hub.
pub(super) struct HotState {
    pub(super) hub: &'static Hub,
    /// The hub generation last brought in.
    seen: u64,
    /// Programs (translated or failed) when last brought in.
    programs: usize,
    /// Each registered program's cache id, worked out once.
    ids: FxHashMap<ShaderId, u64>,
    /// Programs running an override, and the hash of its source.
    current: FxHashMap<ShaderId, u64>,
}

impl HotState {
    pub(super) fn new(hub: &'static Hub) -> Self {
        Self {
            hub,
            seen: 0,
            programs: 0,
            ids: FxHashMap::default(),
            current: FxHashMap::default(),
        }
    }
}

/// Dump requests for a program no provider has are dropped after this
/// many frames.
const DUMP_TRIES: u32 = 8;

fn info(key: &ProgramKey, c: &Compiled) -> ProgramInfo {
    ProgramInfo {
        shader: c.id.0,
        vertex: match key.vertex {
            VertexSource::Microcode(_) => "microcode",
            VertexSource::Pretransformed(_) => "pretransformed",
        }
        .into(),
        textures: c.dims.iter().filter(|d| d.is_some()).count() as u8,
        discards: c.discards,
        warnings: c.warnings.len(),
    }
}

impl Nv2aShaders {
    /// Takes overrides, parameters and dumps from `hub` instead of the
    /// process's (`hot_render::global`): tests.
    pub fn with_hot(mut self, hub: &'static Hub) -> Self {
        self.hot = HotState::new(hub);
        self
    }

    /// Programs running an override now.
    pub fn overridden(&self) -> usize {
        self.hot.current.len()
    }

    /// Brings programs to what the hub says (module docs) and serves dumps.
    pub(super) fn hot_maintain(&mut self, ctx: &gfx::GfxContext, renderer: &mut gfx::Renderer) {
        let hub = self.hot.hub;
        if hub.dumps_pending() {
            self.serve_dumps();
        }
        let generation = hub.generation();
        if generation == self.hot.seen && self.programs.len() == self.hot.programs {
            return;
        }
        self.hot.seen = generation;
        self.hot.programs = self.programs.len();
        let active = hub.active();
        let hot = &mut self.hot;
        for (key, compiled) in &self.programs {
            let Ok(c) = compiled else { continue };
            let id = *hot.ids.entry(c.id).or_insert_with(|| {
                let id = program_id(key);
                hub.publish(id, info(key, c));
                id
            });
            let current = hot.current.get(&c.id).copied();
            if !active && current.is_none() {
                continue;
            }
            let want = if active { hub.source(id) } else { None };
            match (want, current) {
                (Some((_, h)), Some(cur)) if h == cur => hub.report(id, Ok(())),
                (Some((source, h)), _) => {
                    let checked =
                        translate(key).and_then(|t| hot_render::iface::check(&t.source, &source));
                    match checked {
                        Ok(()) => {
                            let bindings = self
                                .bindings
                                .get_or_insert_with(|| ProgramBindings::new(&ctx.device));
                            let m = module(&ctx.device, source);
                            renderer.register_shader(
                                c.id,
                                registered_program(bindings, &ctx.device, m, c.dims, key),
                            );
                            hot.current.insert(c.id, h);
                            hub.report(id, Ok(()));
                            eprintln!("nv2a: program {} runs its override", program_text(id));
                        }
                        Err(e) => {
                            eprintln!("nv2a: override of {} refused: {e}", program_text(id));
                            hub.report(id, Err(e));
                        }
                    }
                }
                (None, Some(_)) => {
                    // Back to the translation.
                    match translate(key) {
                        Ok(t) => {
                            let bindings = self
                                .bindings
                                .get_or_insert_with(|| ProgramBindings::new(&ctx.device));
                            let m = module(&ctx.device, t.source);
                            renderer.register_shader(
                                c.id,
                                registered_program(bindings, &ctx.device, m, c.dims, key),
                            );
                            hot.current.remove(&c.id);
                        }
                        Err(e) => eprintln!(
                            "nv2a: program {} no longer translates ({e}); its override stays",
                            program_text(id)
                        ),
                    }
                }
                (None, None) => {}
            }
        }
    }

    /// Writes translated WGSL for the dump requests this provider can serve.
    fn serve_dumps(&mut self) {
        let hub = self.hot.hub;
        for mut req in hub.take_dumps() {
            let mut report = DumpReport {
                dir: req.to.display().to_string(),
                ..DumpReport::default()
            };
            if let Err(e) = std::fs::create_dir_all(&req.to) {
                report.errors.push(Problem {
                    file: report.dir.clone(),
                    error: e.to_string(),
                });
                hub.dump_done(report);
                continue;
            }
            for (key, compiled) in &self.programs {
                let Ok(c) = compiled else { continue };
                let id = *self.hot.ids.entry(c.id).or_insert_with(|| program_id(key));
                if req.program.is_some_and(|p| p != id) {
                    continue;
                }
                let name = format!("{}.wgsl", program_text(id));
                let r = translate(key).and_then(|t| {
                    std::fs::write(req.to.join(&name), t.source).map_err(|e| e.to_string())
                });
                match r {
                    Ok(()) => report.written.push(name),
                    Err(error) => report.errors.push(Problem { file: name, error }),
                }
            }
            if report.written.is_empty() && report.errors.is_empty() && req.program.is_some() {
                // Another provider (the scaled view's) may have it.
                req.tries += 1;
                if req.tries < DUMP_TRIES {
                    hub.request_dump(req);
                    continue;
                }
                report.errors.push(Problem {
                    file: format!("{}.wgsl", program_text(req.program.unwrap_or(0))),
                    error: "no renderer has translated this program".into(),
                });
            }
            hub.dump_done(report);
        }
    }
}
