//! A guest can keep an in-place texture after the Swap that made it.
//! The next seal can name another target first or only read the source.
use super::*;
use crate::backend::{DrawJudge, TargetInfo, Viewport};
use crate::scene_sink::{FrozenSample, ReplaySample};
use crate::shader::{ShaderInputs, VertexProcessing};
use crate::state::StateWords;
use frame_scene::carried::CameraUse;
use gfx::vertex::{PrimitiveType, VertexLayout, fvf};

fn packet() -> Arc<ResolvedDraw> {
    let raw = StateWords::defaults(false);
    let fvf = fvf::XYZRHW;
    Arc::new(ResolvedDraw {
        entry: "synthetic live texture reader",
        primitive: PrimitiveType::TriangleFan,
        vertex_count: 0,
        layout: VertexLayout::from_fvf(fvf).unwrap(),
        streams: Vec::new(),
        indices: None,
        state: raw.translate(&Default::default()),
        extras: raw.stage_extras(),
        raw: Arc::new(raw),
        target: TargetInfo {
            width: 1,
            height: 1,
            format: D3DFormat::A8R8G8B8,
            depth_format: None,
        },
        texture_info: [None; 4],
        shader: ShaderInputs {
            vertex: VertexProcessing::FixedFunction { fvf },
            provided_inputs: 1,
            vertex_defaults: [[0.0; 4]; 16],
            constants: Arc::new([[0.0; 4]; 192]),
            pixel_shader: None,
            combiner_state: Arc::new([0; 57]),
            final_combiner_latch: false,
            affine_projection: false,
        },
        viewport: Viewport {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            min_z: 0.0,
            max_z: 1.0,
        },
        clip: None,
        clip_rects: Vec::new(),
        hw: Default::default(),
        textures: [None; 4],
        glyph_cache: 0,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Live,
    WrongTexture,
    Destroyed,
}

fn exercise_source(adopt_producer: bool, source: Source) {
    let (mut gpu, _, shown) = device(true);
    let sample_source = gpu
        .create_surface(D3DFormat::A8R8G8B8, 1, 1, false)
        .unwrap();
    gpu.clear(sample_source, None, clear(81)).unwrap();
    let handle = gpu.surface_texture(sample_source).unwrap().unwrap();
    let proxy = handle.id;

    // Produce the live view through both the original and adopted paths.
    let aux = TargetRef::Aux(0);
    gpu.frame_owner.as_mut().unwrap().select = Box::new(move |n| {
        let mut replay = frame(n, 81);
        let at = replay.ops[0].0;
        replay.aux.push(replay.colour.unwrap());
        replay.ops = vec![
            (
                at,
                ReplayOp::Clear {
                    colour: aux,
                    depth: TargetRef::None,
                    clear: clear(81),
                },
            ),
            (
                at,
                ReplayOp::Sample {
                    of: aux,
                    id: 3,
                    how: ReplaySample::InPlace,
                },
            ),
        ];
        Some(Submission::Native(Candidate {
            frame: Arc::new(replay),
            before_original: None,
            eligible: Box::new(move |_, _| adopt_producer),
            draw: Box::new(move |gpu, targets| {
                let original = targets[&aux];
                gpu.clear(original, None, clear(81))?;
                let view = gpu.surface_texture(original)?.unwrap();
                gpu.present(targets[&TargetRef::SceneColour], 0)?;
                Ok(Outputs {
                    samples: HashMap::from([(3, view)]),
                    ..Default::default()
                })
            }),
        }))
    });
    gpu.present(shown, 0).unwrap();
    assert_eq!(
        gpu.call(move |w| Ok(w.live_sources.get(&proxy).copied()))
            .unwrap(),
        Some(sample_source)
    );

    if source == Source::Destroyed {
        gpu.destroy_surface(sample_source).unwrap();
        // A later creation must not revive a view into the retired surface.
        let _ = gpu
            .create_surface(D3DFormat::A8R8G8B8, 1, 1, false)
            .unwrap();
    }

    let draw = packet();
    let mut replay = frame(2, 19);
    let at = replay.ops[0].0;
    replay.aux.push(replay.colour.unwrap());
    replay.ops.push((
        at,
        ReplayOp::Bind {
            colour: TargetRef::SceneColour,
            depth: TargetRef::None,
        },
    ));
    let token = if source == Source::WrongTexture {
        proxy + 1
    } else {
        proxy
    };
    let (binding, _) = FrozenSample::pending(token);
    replay.ops.push((
        at,
        ReplayOp::Draw {
            camera: CameraUse::Screen,
            draw: Arc::clone(&draw),
            textures: [
                Some(ReplayTexture::Live {
                    target: aux,
                    binding,
                }),
                None,
                None,
                None,
            ],
        },
    ));
    let replay = Arc::new(replay);
    let used = Arc::new(AtomicUsize::new(0));
    let native = Arc::clone(&used);
    gpu.frame_owner.as_mut().unwrap().select = Box::new(move |_| {
        let native = Arc::clone(&native);
        Some(Submission::Native(Candidate {
            frame: Arc::clone(&replay),
            before_original: None,
            eligible: Box::new(|_, _| true),
            draw: Box::new(move |gpu, targets| {
                let shown = targets[&TargetRef::SceneColour];
                // The source appears in this seal only as a draw input.
                gpu.clear(shown, None, clear(19))?;
                gpu.copy_surface(targets[&aux], shown)?;
                gpu.present(shown, 0)?;
                native.fetch_add(1, Ordering::Relaxed);
                Ok(Outputs::default())
            }),
        }))
    });
    gpu.clear(shown, None, clear(19)).unwrap();
    gpu.draw_shared(shown, None, draw, [Some(&handle), None, None, None])
        .0
        .unwrap();
    drop(handle); // Release follows the draw but precedes Swap.
    gpu.present(shown, 0).unwrap();
    assert_eq!(value(&mut gpu, shown), 81);
    assert_eq!(
        used.load(Ordering::Relaxed),
        usize::from(source == Source::Live)
    );
    assert!(
        gpu.call(move |w| Ok(!w.live_sources.contains_key(&proxy)))
            .unwrap()
    );
}

#[test]
fn original_and_adopted_views_match_only_their_live_source_and_original_texture_proxy() {
    for adopt_producer in [false, true] {
        for source in [Source::Live, Source::WrongTexture, Source::Destroyed] {
            exercise_source(adopt_producer, source);
        }
    }
}

struct WrongJudge;

impl DrawJudge for WrongJudge {
    fn upload_view(&self, _: &DecodedTexture) -> bool {
        true
    }
    fn surface_view(&self, _: D3DFormat) -> bool {
        true
    }
    fn draw(&mut self, _: &ResolvedDraw, _: [Option<bool>; 4]) -> DrawOutcome {
        DrawOutcome::Skipped("synthetic prediction".into())
    }
}

#[test]
fn an_omitted_speculative_skip_runs_on_the_worker_and_preserves_its_error() {
    let (mut gpu, owned, target) = device(true);
    gpu.judge = Some(Box::new(WrongJudge));
    gpu.clear(target, None, clear(1)).unwrap();
    let (predicted, _) = gpu.draw_shared(target, None, packet(), [None; 4]);
    assert!(matches!(predicted.unwrap(), DrawOutcome::Skipped(_)));
    gpu.present(target, 0).unwrap();
    assert!(gpu.finish().unwrap_err().contains("wrong speculation"));
    assert_eq!(owned.load(Ordering::Relaxed), 0);
    assert_eq!(gpu.misses(), 1);
}
