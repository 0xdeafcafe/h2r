use super::*;
use crate::geometry::Exact;
use native_render::world::{Owner, PartInput, Placement, SectionInput, WorldBuilder};

const PAGE: LightmapKey = LightmapKey {
    page: 0,
    palette: Some(0),
};

/// A unit quad in the xy plane at height `z`, lit by [`PAGE`] when `lit`.
fn quad(material: u16, z: f32, lit: bool) -> SectionInput {
    SectionInput {
        positions: vec![[0.0, 0.0, z], [1.0, 0.0, z], [0.0, 1.0, z], [1.0, 1.0, z]],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
        lightmap_uvs: if lit { vec![[0.5; 2]; 4] } else { vec![] },
        normals: vec![[0.0, 0.0, 1.0]; 4],
        parts: vec![PartInput {
            material,
            transparent: false,
            triangles: vec![[0, 1, 2], [1, 3, 2]],
        }],
        ..Default::default()
    }
}

fn world(sections: &[(u16, bool)]) -> World {
    let mut b = WorldBuilder::new();
    for (i, &(material, lit)) in sections.iter().enumerate() {
        b.add_section(
            Owner::Cluster(i as _),
            &quad(material, 0.0, lit),
            &Placement::IDENTITY,
            lit.then_some(PAGE),
        );
    }
    b.finish()
}

fn near(a: &Mat4, b: &Mat4) -> bool {
    (0..4).all(|c| (0..4).all(|r| (a[c][r] - b[c][r]).abs() < 1e-4 * (1.0 + b[c][r].abs())))
}
type Mat4 = [[f32; 4]; 4];

#[test]
fn the_lighting_modes_have_ids_and_names() {
    for l in Lighting::ALL {
        assert_eq!(Lighting::from_id(l.id()), Some(l));
        assert!(!l.name().is_empty());
    }
    assert_eq!(Lighting::from_id("bright"), None);
    assert_eq!(Lighting::default(), Lighting::Game);
}

/// Game keeps both images, Flat drops the page, Lightmap only whitens
/// the base map, and Collision is Flat at a third.
#[test]
fn each_mode_swaps_the_images_it_uploads() {
    let base = Image::level(1, 1, vec![90, 60, 30, 200]).unwrap();
    let page = Image::solid([128, 128, 128, 255]);
    assert_eq!(Lighting::Game.base(&base), base);
    assert_eq!(Lighting::Game.page(&page), Some(page.clone()));
    assert_eq!(Lighting::Flat.base(&base), base);
    assert_eq!(Lighting::Flat.page(&page), None);
    assert_eq!(Lighting::LightmapOnly.base(&base), Image::solid([255; 4]));
    assert_eq!(Lighting::LightmapOnly.page(&page), Some(page.clone()));
    let dim = Lighting::Collision.base(&base);
    assert_eq!(dim.rgba, vec![30, 20, 10, 200], "alpha kept");
    assert_eq!(Lighting::Collision.page(&page), None);
    let chain = Image::from_levels(vec![(2, 2, vec![99; 16]), (1, 1, vec![6, 6, 6, 6])]).unwrap();
    assert_eq!(
        Lighting::Collision.base(&chain).mips,
        vec![vec![2, 2, 2, 6]]
    );
}

/// The renderer's camera built from the editor's has the editor's
/// view-projection, for an orbit and for an exact pose.
#[test]
fn the_render_camera_matches_the_editors() {
    let orbit = Camera {
        target: [3.0, -2.0, 1.0],
        yaw: 0.7,
        pitch: 0.4,
        distance: 25.0,
        ..Camera::default()
    };
    let exact = Camera {
        exact: Some(Exact {
            eye: [10.0, 4.0, 2.0],
            forward: [0.0, 1.0, 0.0],
            up: [0.0, 0.0, 1.0],
            near: 0.1,
            far: 900.0,
        }),
        ..orbit
    };
    for (cam, size) in [(orbit, [640, 480]), (exact, [300, 500])] {
        let r = render_camera(&cam, size);
        assert_eq!(r.position, cam.eye());
        assert_eq!((r.near, r.far), cam.near_far());
        let aspect = size[0] as f32 / size[1] as f32;
        assert!((r.aspect() - aspect).abs() < 1e-6);
        assert!(
            near(&r.view_projection(), &cam.view_proj(aspect)),
            "{:?}\n{:?}",
            r.view_projection(),
            cam.view_proj(aspect)
        );
    }
    let tiny = render_camera(&orbit, [0, 70_000]);
    assert_eq!(tiny.viewport, [0, 0, i16::MAX, 1]);
}

/// The depth the world pass writes is view depth over the far plane,
/// floored to D24's step.
#[test]
fn the_depth_rule_is_view_depth_over_far() {
    let cam = render_camera(&Camera::default(), [64, 64]);
    let ahead = |d: f32| {
        let mut p = cam.position;
        for k in 0..3 {
            p[k] += cam.forward[k] * d;
        }
        p
    };
    assert!((depth_of(&cam, ahead(cam.far * 0.25)) - 0.25).abs() < 1e-4);
    assert_eq!(depth_of(&cam, ahead(cam.far * 2.0)), 1.0);
    const { assert!(DEPTH_RULE.floor) };
    // Floored to D24's step, as the world's W-buffer depth is: at most
    // one step under the unfloored depth.
    let p = ahead(cam.far * 0.3);
    let (d, exact) = (depth_of(&cam, p), cam.depth_of(p) / cam.far);
    assert!(
        (-1e-7..=1.5 / DEPTH_RULE.full_scale).contains(&(exact - d)),
        "{d:e} {exact:e}"
    );
}

/// A draw with no base map, and a lit draw with no page, are left out;
/// the stats count what is left.
#[test]
fn draws_without_their_images_are_left_out() {
    let w = world(&[(0, true), (1, true), (2, false), (3, false)]);
    let base: HashMap<u16, Image> = [0u16, 1, 2]
        .into_iter()
        .map(|m| (m, Image::solid([m as u8; 4])))
        .collect();
    let load = NativeLoad::new(TagId(7), w.clone(), base.clone(), HashMap::new());
    assert_eq!(
        load.stats(Lighting::Game).draws,
        1,
        "only the unlit draw with a base map"
    );
    assert_eq!(load.stats(Lighting::Game).untextured, 3);
    assert_eq!(load.world().draws[0].material, 2);
    let pages = HashMap::from([(PAGE, Image::solid([128; 4]))]);
    let load = NativeLoad::new(TagId(7), w, base, pages);
    let s = load.stats(Lighting::Game);
    assert_eq!(
        (s.draws, s.untextured, s.materials, s.lightmap_pages),
        (3, 1, 3, 1)
    );
    assert_eq!(s.batches, 3);
}

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = gfx::instance();
    let Ok(adapter) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    else {
        eprintln!("no wgpu adapter; skipping");
        return None;
    };
    Some(pollster::block_on(adapter.request_device(&Default::default())).unwrap())
}

/// Draw `load` in `lighting` into a 64 by 64 viewport target through the
/// view the viewport's native passes write (`Shading::Gamma`) and read
/// the centre pixel back.
fn centre(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    load: &NativeLoad,
    lighting: Lighting,
    camera: &Camera,
) -> ([u8; 4], CullStats) {
    centre_of(device, queue, &mut NativeWorld::new(device, queue, load, lighting), camera)
}

/// [`centre`] of a world already uploaded (and perhaps repainted).
fn centre_of(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world: &mut NativeWorld,
    camera: &Camera,
) -> ([u8; 4], CullStats) {
    let _lighting = world.lighting();
    let size = wgpu::Extent3d {
        width: 64,
        height: 64,
        depth_or_array_layers: 1,
    };
    let target = |format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let color = target(
        crate::render::UI_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    );
    let depth = target(
        crate::render::DEPTH_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT,
    );
    world.render(
        device,
        queue,
        &color.create_view(&Default::default()),
        crate::render::UI_FORMAT,
        &depth.create_view(&Default::default()),
        crate::render::DEPTH_FORMAT,
        [64, 64],
        camera,
        Clear::all([0.0; 3]),
    );
    (read_centre(device, queue, &color), world.cull_stats(device, queue))
}

/// The centre pixel of a 64 by 64 colour texture.
fn read_centre(device: &wgpu::Device, queue: &wgpu::Queue, color: &wgpu::Texture) -> [u8; 4] {
    let size = wgpu::Extent3d {
        width: 64,
        height: 64,
        depth_or_array_layers: 1,
    };
    let row = 256u32;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * 64),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        color.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(64),
            },
        },
        size,
    );
    queue.submit([enc.finish()]);
    buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = buf.slice(..).get_mapped_range().unwrap();
    let at = (32 * row + 32 * 4) as usize;
    [data[at], data[at + 1], data[at + 2], data[at + 3]]
}

/// An invented lit quad draws through `GpuWorld` into the viewport's
/// own colour and depth formats, seen by the editor's camera, and each
/// Lighting mode changes the picture: Game is the base map times the
/// page, Flat the base map alone, Lightmap only the grey page, and
/// Collision a third of Flat. Skipped without a wgpu adapter.
#[test]
fn an_invented_world_draws_in_each_lighting_mode() {
    let Some((device, queue)) = device() else {
        return;
    };
    let base = HashMap::from([(0u16, Image::solid([200, 60, 20, 255]))]);
    let pages = HashMap::from([(PAGE, Image::solid([128, 128, 128, 255]))]);
    let load = NativeLoad::new(TagId(1), world(&[(0, true)]), base, pages);
    let camera = Camera {
        target: [0.5, 0.5, 0.0],
        yaw: 0.0,
        pitch: 1.3,
        distance: 3.0,
        ..Camera::default()
    };
    let mut seen = HashMap::new();
    for l in Lighting::ALL {
        let (px, stats) = centre(&device, &queue, &load, l, &camera);
        assert_eq!((stats.visible, stats.total), (1, 1), "{l:?}");
        seen.insert(l, px);
    }
    let [game, flat, light, dim] = Lighting::ALL.map(|l| seen[&l]);
    assert!(flat[0] > 150 && flat[0] > flat[2], "base map red: {flat:?}");
    assert!(
        game[0] < flat[0] && game[0] > game[2],
        "lit by the page: {game:?}"
    );
    assert!(
        light[0].abs_diff(light[2]) < 4 && light[0] > 60,
        "grey page: {light:?}"
    );
    assert!(
        dim[0] < flat[0] && dim[0] > dim[2],
        "a third of flat: {dim:?}"
    );

    // Facing away, nothing is drawn: the clear colour stays.
    let away = Camera {
        target: [50.0, 50.0, 50.0],
        pitch: -1.3,
        ..camera
    };
    let (px, stats) = centre(&device, &queue, &load, Lighting::Flat, &away);
    assert_eq!(stats.visible, 0);
    assert_eq!(&px[..3], &[0, 0, 0]);
}

/// The invented textured BSP opened as a map, and its raw resources.
fn textured() -> (Map<std::io::Cursor<Vec<u8>>>, ResourceFiles) {
    let bytes = crate::fixture::fixture_with_textured_bsp();
    let mut files = ResourceFiles::new();
    files.set_bytes(map::ResourceFile::Local, bytes.clone().into());
    (
        Map::from_reader(std::io::Cursor::new(bytes)).unwrap(),
        files,
    )
}

/// The whole map path: `load_world` reads the invented cluster, the
/// shader names its base map, and both bitmaps decode, the page
/// through the lightmap group's palette.
#[test]
fn the_invented_textured_bsp_loads_with_its_images() {
    use crate::fixture::{TEX_BASE_RGBA, TEX_FLOOR, id};
    let (mut map, mut files) = textured();
    let load = NativeLoad::read(&mut map, &mut files, 0).unwrap();
    assert_eq!(load.bsp, id(4));
    assert_eq!(load.lightmap_error, None);
    let s = load.stats(Lighting::Game);
    assert_eq!(
        (
            s.draws,
            s.batches,
            s.materials,
            s.lightmap_pages,
            s.untextured,
            s.failed_sections
        ),
        (1, 1, 1, 1, 0, 0)
    );
    let w = load.world();
    assert_eq!(w.draws[0].lightmap, Some(PAGE));
    assert_eq!(w.indices.len(), 6);
    let corners: Vec<[f32; 3]> = w.vertices.iter().map(|v| v.position).collect();
    assert!(
        corners.contains(&[TEX_FLOOR, TEX_FLOOR, 0.0]),
        "{corners:?}"
    );
    let base = &load.base[&0];
    assert_eq!((base.width, base.height), (4, 4));
    assert_eq!(base.rgba[..4], TEX_BASE_RGBA);
    let page = &load.pages[&PAGE];
    assert_eq!(page.rgba[..4], [0x80, 0x80, 0x80, 0xff]);
    assert!(
        NativeLoad::read(&mut map, &mut files, 1).is_err(),
        "no second BSP"
    );
}

/// Step 2: the invented BSP's layers are read and planned for each of
/// the five layers the game draws them in. Its shader has no template,
/// so its one material does not resolve: it is counted by cause, and no
/// layer draws. A world not read from a map has no layers.
#[test]
fn the_invented_bsps_layers_name_what_does_not_draw() {
    let (mut map, mut files) = textured();
    let load = NativeLoad::read(&mut map, &mut files, 0).unwrap();
    let layers = load.layers().expect("read from the map");
    let planned: Vec<u8> = layers.plans().keys().copied().collect();
    assert_eq!(planned, [1, 3, 5, 7, 8, 20], "layer 3 is planned too");
    for plan in layers.plans().values() {
        assert_eq!(plan.compiled, 0, "{plan:?}");
        assert!(plan.drawn.is_empty() && plan.parts.is_empty(), "{plan:?}");
    }
    assert_eq!(
        layers.unresolved().values().sum::<usize>(),
        1,
        "{:?}",
        layers.unresolved()
    );
    let bare = NativeLoad::new(load.bsp, World::default(), HashMap::new(), HashMap::new());
    assert!(bare.layers().is_none());
}

/// A BSP's base look is its translated pass, or `world.wgsl` with the
/// reason named: here a placement with no model rows for the rigid
/// program to read.
#[test]
fn a_base_look_that_cannot_be_translated_is_named() {
    let base = HashMap::from([(0u16, Image::solid([200; 4]))]);
    let load = NativeLoad::new(TagId(7), world(&[(0, false)]), base.clone(), HashMap::new());
    assert_eq!(load.base_fallback(), None);
    let mut b = WorldBuilder::new();
    b.add_section(
        Owner::Instance(4),
        &quad(0, 0.0, false),
        &Placement::IDENTITY,
        None,
    );
    let load = NativeLoad::new(TagId(7), b.finish(), base, HashMap::new());
    assert_eq!(load.base_fallback(), Some("placement 4 has no model rows"));
}

/// In the Game mode a part its material's layer-3 pass takes draws that
/// pass (a part with no base map too); every other drawn part draws its
/// base look. The other modes draw the base look alone.
#[test]
fn the_game_mode_draws_layer_3_where_it_compiled() {
    let w = world(&[(0, true), (1, false), (2, true)]);
    let base: HashMap<u16, Image> = [0u16, 1]
        .into_iter()
        .map(|m| (m, Image::solid([m as u8; 4])))
        .collect();
    let pages = HashMap::from([(PAGE, Image::solid([128; 4]))]);
    let mut load = NativeLoad::new(TagId(7), w.clone(), base, pages);
    let mut layer3 = layers_load::Layer3::default();
    for d in w.draws.iter().filter(|d| d.material != 1) {
        layer3.data.parts.push((
            (d.owner, d.part),
            (d.first_index, d.index_count, d.lightmap, d.material),
        ));
    }
    load.layers = Some(Arc::new(NativeLayers {
        view: LayersView::default(),
        world: w,
        images: StageImages::new(),
        layer3,
        plans: BTreeMap::new(),
    }));
    let keys = |layer3: Option<&layers_load::LayerData>| -> Vec<PassKey> {
        base_parts(&load, layer3)
            .into_iter()
            .map(|(_, k)| k)
            .collect()
    };
    // Material 2 has no base map: only layer 3 draws it, and it picks.
    let own: Vec<u16> = load
        .layer3_only(Lighting::Game)
        .iter()
        .map(|d| d.material)
        .collect();
    assert_eq!(own, [2]);
    assert_eq!(load.triangles(Lighting::Game).len(), 6, "three quads");
    // The other modes draw the base look alone: what is picked and
    // counted is what they show.
    assert!(load.layer3_only(Lighting::Flat).is_empty());
    assert_eq!(load.triangles(Lighting::Flat).len(), 4, "two quads");
    assert_eq!(load.stats(Lighting::Flat).draws, 2);
    assert_eq!(load.stats(Lighting::Game).draws, 3);
    let data = &load.layers().unwrap().layer3.data;
    assert_eq!(
        keys(Some(data)),
        [
            PassKey::Layer3(0),
            PassKey::Base {
                material: 1,
                lit: false
            },
            PassKey::Layer3(2),
        ]
    );
    assert_eq!(
        keys(None),
        [
            PassKey::Base {
                material: 0,
                lit: true
            },
            PassKey::Base {
                material: 1,
                lit: false
            },
        ]
    );
}

/// Layer 1 is refused whole and named; the other layers keep their plan.
#[test]
fn layer_1_is_refused_until_it_is_checked_against_the_game() {
    let plan = StaticLayer {
        compiled: 3,
        constants: Default::default(),
        drawn: [0u16, 2].into_iter().collect(),
        parts: vec![((Owner::Cluster(0), 0), (0, 6, None, 0))],
        refused: BTreeMap::from([("animated: x".to_owned(), 1)]),
        parts_refused: BTreeMap::new(),
    };
    let one = editor_plan(1, plan.clone());
    assert!(one.drawn.is_empty() && one.parts.is_empty());
    assert_eq!(one.compiled, 3);
    assert_eq!(
        one.refused,
        BTreeMap::from([
            ("animated: x".to_owned(), 1),
            (LAYER_1_UNCHECKED.to_owned(), 2)
        ])
    );
    assert_eq!(editor_plan(5, plan.clone()), plan);
    assert_eq!(
        editor_plan(1, StaticLayer::default()),
        StaticLayer::default()
    );
}

/// The layers draw in the game's order around the base look: 1 before
/// it, then 5, 8 and 20, then 7; each of the five once.
#[test]
fn the_layers_draw_in_the_games_order() {
    assert_eq!(LAYERS_BEFORE, [1]);
    assert_eq!(LAYERS_AFTER, [5, 8, 20, 7]);
    let mut all: Vec<u8> = LAYERS_BEFORE.iter().chain(&LAYERS_AFTER).copied().collect();
    all.sort_unstable();
    assert_eq!(all, native_render::layer_pass::LAYERS);
}

/// The invented layered BSP ([`crate::fixture::fixture_with_layered_bsp`])
/// opened as a map, and its raw resources.
fn layered() -> (Map<std::io::Cursor<Vec<u8>>>, ResourceFiles) {
    let bytes = crate::fixture::fixture_with_layered_bsp();
    let mut files = ResourceFiles::new();
    files.set_bytes(map::ResourceFile::Local, bytes.clone().into());
    (
        Map::from_reader(std::io::Cursor::new(bytes)).unwrap(),
        files,
    )
}

/// The layered floor's material resolves, its layer-5 pass compiles
/// with the camera's view constants, and the game-free plan draws it on
/// the floor's one part; no other layer has a pass of it.
#[test]
fn the_layered_floor_plans_its_pass() {
    use crate::fixture::LAYERED_LAYER;
    let (mut map, mut files) = layered();
    let load = NativeLoad::read(&mut map, &mut files, 0).unwrap();
    assert_eq!(
        load.stats(Lighting::Game).draws,
        1,
        "the base look is the textured map's"
    );
    let layers = load.layers().expect("read from the map");
    assert!(layers.unresolved().is_empty(), "{layers:?}");
    for (&layer, plan) in layers.plans() {
        if layer == LAYERED_LAYER {
            assert_eq!(plan.compiled, 1, "{layers:?}");
            assert!(plan.refused.is_empty(), "{layers:?}");
            assert_eq!(plan.drawn.iter().copied().collect::<Vec<_>>(), [0]);
            assert_eq!(plan.parts.len(), 1, "{layers:?}");
        } else {
            assert_eq!(plan.compiled, 0, "layer {layer}: {layers:?}");
            assert!(plan.drawn.is_empty(), "layer {layer}");
        }
    }
}

/// Draw `load` in `lighting` into a 64 by 64 viewport target cleared to
/// black: every pixel's colour and depth, row by row.
fn frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    load: &NativeLoad,
    lighting: Lighting,
    camera: &Camera,
) -> (Vec<[u8; 4]>, Vec<f32>) {
    let size = wgpu::Extent3d {
        width: 64,
        height: 64,
        depth_or_array_layers: 1,
    };
    let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
    let target = |format| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let color = target(crate::render::UI_FORMAT);
    let depth = target(crate::render::DEPTH_FORMAT);
    let mut world = NativeWorld::new(device, queue, load, lighting);
    world.render(
        device,
        queue,
        &color.create_view(&Default::default()),
        crate::render::UI_FORMAT,
        &depth.create_view(&Default::default()),
        crate::render::DEPTH_FORMAT,
        [64, 64],
        camera,
        Clear::all([0.0; 3]),
    );
    let read = |texture: &wgpu::Texture, aspect| {
        let row = 256u32;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * 64),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(64),
                },
            },
            size,
        );
        queue.submit([enc.finish()]);
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = buf.slice(..).get_mapped_range().unwrap();
        data.as_chunks::<4>().0.to_vec()
    };
    let pixels = read(&color, wgpu::TextureAspect::All);
    let depths = read(&depth, wgpu::TextureAspect::DepthOnly)
        .into_iter()
        .map(f32::from_le_bytes)
        .collect();
    (pixels, depths)
}

/// The layered floor's layer-5 pass draws over the base look on the GPU:
/// in Game every pixel of the floor the base look draws (Flat: the base
/// map, no layers) is the pass's colour, so the pass passed its
/// less-or-equal depth test against the base look's depth there, and
/// the depth it wrote is the base look's, bit for bit: the base look and
/// the pass share one position (`base_look::world_position`) and the
/// translated depth, so the layer does not flicker against it. The floor
/// is seen at a slant, so its depth varies across the view. Off the
/// floor nothing draws. Skipped without a wgpu adapter.
#[test]
fn the_layered_floor_draws_its_pass_over_the_base_look() {
    let Some((device, queue)) = device() else {
        return;
    };
    let (mut map, mut files) = layered();
    let load = NativeLoad::read(&mut map, &mut files, 0).unwrap();
    let half = crate::fixture::TEX_FLOOR / 2.0;
    let camera = Camera {
        target: [half, half, 0.0],
        yaw: 0.3,
        pitch: 0.9,
        distance: 6.0,
        ..Camera::default()
    };
    let (flat, flat_depth) = frame(&device, &queue, &load, Lighting::Flat, &camera);
    let (game, game_depth) = frame(&device, &queue, &load, Lighting::Game, &camera);
    let floor: Vec<usize> = (0..flat.len())
        .filter(|&i| flat[i][..3] != [0, 0, 0])
        .collect();
    assert!(
        floor.len() > 200,
        "the floor fills the view: {}",
        floor.len()
    );
    let green = |p: [u8; 4]| p[1] > 250 && p[0] < 5 && p[2] < 5;
    let lost: Vec<usize> = floor.iter().copied().filter(|&i| !green(game[i])).collect();
    assert!(
        lost.is_empty(),
        "the pass lost the depth test on {} of {} floor pixels, e.g. pixel {} {:?}, base look depth {:e} ({:#010x})",
        lost.len(),
        floor.len(),
        lost[0],
        game[lost[0]],
        flat_depth[lost[0]],
        flat_depth[lost[0]].to_bits(),
    );
    let differ: Vec<usize> = floor
        .iter()
        .copied()
        .filter(|&i| game_depth[i].to_bits() != flat_depth[i].to_bits())
        .collect();
    assert!(
        differ.is_empty(),
        "the pass's depth differs from the base look's on {} floor pixels, e.g. pixel {}: {:e} ({:#010x}) over {:e} ({:#010x})",
        differ.len(),
        differ[0],
        game_depth[differ[0]],
        game_depth[differ[0]].to_bits(),
        flat_depth[differ[0]],
        flat_depth[differ[0]].to_bits(),
    );
    for i in (0..flat.len()).filter(|i| !floor.contains(i)) {
        assert_eq!(game[i][..3], [0, 0, 0], "off the floor, pixel {i}");
    }
}

/// The invented map draws through `GpuWorld` into the viewport's
/// formats: the floor under the camera is the base map's red, darker
/// with its lightmap page than in Flat. Skipped without a wgpu adapter.
#[test]
fn the_invented_map_draws_through_the_native_world() {
    let Some((device, queue)) = device() else {
        return;
    };
    let (mut map, mut files) = textured();
    let load = NativeLoad::read(&mut map, &mut files, 0).unwrap();
    let half = crate::fixture::TEX_FLOOR / 2.0;
    let camera = Camera {
        target: [half, half, 0.0],
        yaw: 0.0,
        pitch: 1.3,
        distance: 6.0,
        ..Camera::default()
    };
    let (game, stats) = centre(&device, &queue, &load, Lighting::Game, &camera);
    assert_eq!((stats.visible, stats.total), (1, 1));
    let (flat, _) = centre(&device, &queue, &load, Lighting::Flat, &camera);
    assert!(
        flat[0] > flat[1] && flat[1] > flat[2],
        "the base map: {flat:?}"
    );
    assert!(
        game[0] < flat[0] && game[0] > game[2],
        "lit by the page: {game:?}"
    );
}

/// Step 1: the viewport draws the level through its native worlds, in
/// the Lighting mode it is set to, and without them draws nothing of the
/// level (this map has no scene and no grey world uploaded). Skipped
/// without a wgpu adapter.
#[test]
fn the_viewport_draws_the_level_through_the_native_world() {
    let Some((device, queue)) = device() else {
        return;
    };
    let (mut map, mut files) = textured();
    let (loads, errors) = read_all(&mut map, &mut files);
    assert!(errors.is_empty(), "{errors:?}");
    let half = crate::fixture::TEX_FLOOR / 2.0;
    let camera = Camera {
        target: [half, half, 0.0],
        yaw: 0.0,
        pitch: 1.3,
        distance: 6.0,
        ..Camera::default()
    };
    let mut vp = crate::render::Viewport::new(&device);
    let draw = |vp: &mut crate::render::Viewport| {
        vp.render(&device, &queue, &camera, [64, 64]);
        read_centre(&device, &queue, vp.color_texture().unwrap())
    };
    let empty = draw(&mut vp);
    vp.set_native(&device, &queue, &loads);
    assert!(vp.has_native());
    let game = draw(&mut vp);
    assert!(game[0] > game[2] && game != empty, "the floor: {game:?}");
    vp.set_lighting(&device, &queue, &loads, Lighting::LightmapOnly);
    assert_eq!(vp.lighting(), Lighting::LightmapOnly);
    let light = draw(&mut vp);
    assert!(light[0].abs_diff(light[2]) < 4, "grey page: {light:?}");
    let mut hidden = crate::layers::Layers::default();
    hidden.set(crate::layers::LayerId::Structure, false);
    vp.set_layers(&hidden);
    assert_eq!(draw(&mut vp), empty, "the Structure layer hides it");
    vp.set_layers(&crate::layers::Layers::default());
    vp.set_native(&device, &queue, &[]);
    assert!(!vp.has_native());
    assert_eq!(draw(&mut vp), empty);
}

/// The viewport keeps the native world's colours as the game writes
/// them: the base look is the game's `t0 · t3` in gamma space on the
/// stored bytes, so in Flat (white for the page) the floor is the base
/// map's bytes exactly. Drawn through the target's sRGB view they were
/// encoded a second time and came out pale (200, 60, 20 as about 229,
/// 133, 79). Skipped without a wgpu adapter.
#[test]
fn the_viewport_keeps_the_native_worlds_colours_as_stored() {
    let Some((device, queue)) = device() else {
        return;
    };
    let (mut map, mut files) = textured();
    let (loads, errors) = read_all(&mut map, &mut files);
    assert!(errors.is_empty(), "{errors:?}");
    let half = crate::fixture::TEX_FLOOR / 2.0;
    let camera = Camera {
        target: [half, half, 0.0],
        yaw: 0.0,
        pitch: 1.3,
        distance: 6.0,
        ..Camera::default()
    };
    let mut vp = crate::render::Viewport::new(&device);
    vp.set_native(&device, &queue, &loads);
    vp.set_lighting(&device, &queue, &loads, Lighting::Flat);
    vp.render(&device, &queue, &camera, [64, 64]);
    let px = read_centre(&device, &queue, vp.color_texture().unwrap());
    let want = crate::fixture::TEX_BASE_RGBA;
    assert!(
        (0..3).all(|c| px[c].abs_diff(want[c]) <= 3),
        "the floor {px:?}, the base map {want:?}"
    );
}

/// Step 2: the viewport's own passes write the native depth rule, so a
/// placement glyph above the native floor is drawn over it and one under
/// it is hidden by it. Skipped without a wgpu adapter.
#[test]
fn glyphs_test_depth_against_the_native_floor() {
    let Some((device, queue)) = device() else {
        return;
    };
    let (mut map, mut files) = textured();
    let (loads, _) = read_all(&mut map, &mut files);
    let half = crate::fixture::TEX_FLOOR / 2.0;
    let camera = Camera {
        target: [half, half, 0.0],
        yaw: 0.0,
        pitch: 1.3,
        distance: 6.0,
        ..Camera::default()
    };
    let mut vp = crate::render::Viewport::new(&device);
    vp.set_native(&device, &queue, &loads);
    let draw = |vp: &mut crate::render::Viewport, z: Option<f32>| {
        let markers: Vec<_> = z
            .map(|z| crate::geometry::Marker {
                block: crate::layout::BlockKind::Weapons,
                index: 0,
                position: [half, half, z],
                axes: crate::geometry::rotation([0.0; 3]),
                scale: 1.0,
                object: None,
            })
            .into_iter()
            .collect();
        vp.set_markers(&device, &crate::render::marker_vertices(&markers, &[]));
        vp.render(&device, &queue, &camera, [64, 64]);
        read_centre(&device, &queue, vp.color_texture().unwrap())
    };
    let floor = draw(&mut vp, None);
    let above = draw(&mut vp, Some(0.5));
    let below = draw(&mut vp, Some(-2.0));
    assert_ne!(above, floor, "the glyph over the floor is drawn");
    assert_eq!(below, floor, "the floor hides the glyph under it");
}

/// Step 5: picking against the native world's triangles meets the floor
/// where picking against the editor's own world triangles does, for rays
/// over it, beside it and from below; and the session opened on the map
/// picks the same placement with the native world loaded.
#[test]
fn picking_the_native_world_matches_the_editors_world() {
    use crate::geometry::{Ray, surface};
    let bytes = crate::fixture::fixture_with_textured_bsp();
    let dir = std::env::temp_dir().join(format!("native-pick-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("textured.map");
    std::fs::write(&path, &bytes).unwrap();
    let mut ed = crate::Session::new();
    ed.open(crate::save::MapSource::file(&path)).unwrap();
    let native = ed.native_world().expect("a map is open");
    assert_eq!(native.len(), 1, "{:?}", ed.native_errors());
    let ours = ed.world().0.to_vec();
    assert!(!ours.is_empty(), "the editor reads the floor too");
    let theirs = native[0].triangles(Lighting::Game);
    assert_eq!(theirs.len(), 2);
    let f = crate::fixture::TEX_FLOOR;
    for (origin, dir) in [
        ([f / 2.0, f / 2.0, 10.0], [0.0, 0.0, -1.0]),
        ([0.3, 3.1, 5.0], [0.1, -0.2, -1.0]),
        ([f + 1.0, f / 2.0, 10.0], [0.0, 0.0, -1.0]),
        ([1.0, 1.0, -10.0], [0.0, 0.0, 1.0]),
    ] {
        let ray = Ray { origin, dir };
        let (a, b) = (surface(&ours, &ray), surface(&theirs, &ray));
        match (a, b) {
            (Some(a), Some(b)) => {
                assert!((0..3).all(|k| (a[k] - b[k]).abs() < 1e-4), "{a:?} {b:?}")
            }
            (None, None) => {}
            other => panic!("{origin:?}: {other:?}"),
        }
    }
    let marker = ed.markers()[0];
    let ray = Ray {
        origin: [
            marker.position[0],
            marker.position[1],
            marker.position[2] + 20.0,
        ],
        dir: [0.0, 0.0, -1.0],
    };
    assert_eq!(
        crate::geometry::pick(ed.markers(), &ray),
        Some((marker.block, marker.index))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn close(a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() < 1e-4)
}

fn turned(scale: f32, ypr: [f32; 3], position: [f32; 3]) -> NodeMatrix {
    let [forward, left, up] = crate::geometry::rotation(ypr);
    NodeMatrix {
        scale,
        forward,
        left,
        up,
        position,
    }
}

/// Step 4: a node matrix's inverse undoes it, on either side.
#[test]
fn inverting_a_node_matrix_undoes_it() {
    let m = turned(2.5, [0.7, 0.3, -0.2], [3.0, -4.0, 1.5]);
    let back = invert(&m);
    for p in [[0.0; 3], [1.0, 2.0, 3.0], [-5.0, 0.25, 9.0]] {
        assert!(close(back.point(m.point(p)), p), "{p:?}");
        assert!(close(m.compose(&back).point(p), p), "{p:?}");
    }
}

/// An invented two-node model: a unit quad at height `z` bound to node 1,
/// whose inverse rest frame is not the identity, drawing material 0.
fn quad_model(z: f32) -> Model {
    let positions = vec![
        [-1.0, -1.0, z],
        [1.0, -1.0, z],
        [1.0, 1.0, z],
        [-1.0, 1.0, z],
    ];
    Model::build(&native_render::model::ModelInput {
        inverse_binds: vec![
            NodeMatrix::IDENTITY,
            turned(1.0, [0.4, 0.0, 0.0], [0.5, -0.25, 0.0]),
        ],
        sections: vec![native_render::model::ModelSectionInput {
            uvs: vec![[0.5, 0.5]; positions.len()],
            normals: vec![[0.0, 0.0, 1.0]; positions.len()],
            node_indices: vec![[1, 0, 0, 0]; positions.len()],
            node_weights: vec![[1.0, 0.0, 0.0, 0.0]; positions.len()],
            positions,
            node_map: Vec::new(),
            parts: vec![native_render::model::ModelPartInput {
                material: 0,
                transparent: false,
                triangles: vec![[0, 1, 2], [0, 2, 3]],
            }],
        }],
    })
}

fn placed_at(position: [f32; 3], object: Option<TagId>) -> Marker {
    Marker {
        block: crate::layout::BlockKind::Scenery,
        index: 0,
        position,
        axes: crate::geometry::rotation([0.9, 0.1, 0.0]),
        scale: 1.5,
        object,
    }
}

/// Step 4: in the rest pose every vertex lands where the scene puts the
/// placement's model-space point, and the placed sphere holds them all.
#[test]
fn the_rest_pose_places_vertices_as_the_scene_does() {
    let model = quad_model(0.5);
    let marker = placed_at([10.0, -3.0, 2.0], None);
    let placement = placement_matrix(&marker);
    let nodes = rest_nodes(&model, &placement);
    assert_eq!(nodes.len(), model.inverse_binds.len());
    let (centre, radius) = placed_sphere(&model, &placement);
    assert!(!model.vertices.is_empty());
    for v in &model.vertices {
        let at = model.skin(v, &nodes);
        let want = crate::scene::model_to_world(&marker, v.position);
        assert!(close(at, want), "{at:?} {want:?}");
        let d: f32 = (0..3)
            .map(|k| (at[k] - centre[k]).powi(2))
            .sum::<f32>()
            .sqrt();
        assert!(d <= radius + 1e-4, "{d} > {radius}");
    }
}

/// Models made for tests are found by their object tag; a model with no
/// inverse rest frames is placed by the placement alone.
#[test]
fn native_models_are_found_by_object_tag() {
    let tag = TagId(0x1234_0005);
    let models = NativeModels::from_models([(tag, quad_model(0.0), HashMap::new())]);
    assert_eq!(models.len(), 1);
    assert_eq!(models.of(tag).map(|m| m.object), Some(tag));
    assert!(models.of(TagId(7)).is_none());
    let bare = Model::default();
    let placement = placement_matrix(&placed_at([1.0, 2.0, 3.0], None));
    assert_eq!(rest_nodes(&bare, &placement), vec![placement]);
    assert_eq!(placed_sphere(&bare, &placement), ([1.0, 2.0, 3.0], 0.0));
}

/// Step 4: a placed model draws through the native renderer over the
/// native floor in its base map's colour; the Objects layer hides it, the
/// floor hides one placed under it, and a placement whose tag has no
/// model draws nothing. Skipped without a wgpu adapter.
#[test]
fn the_viewport_draws_placed_models_through_the_native_renderer() {
    let Some((device, queue)) = device() else {
        return;
    };
    let (mut map, mut files) = textured();
    let (loads, _) = read_all(&mut map, &mut files);
    let half = crate::fixture::TEX_FLOOR / 2.0;
    let camera = Camera {
        target: [half, half, 0.0],
        yaw: 0.0,
        pitch: 1.3,
        distance: 6.0,
        ..Camera::default()
    };
    let tag = TagId(0x1234_0005);
    let green = HashMap::from([(0u16, Image::solid([20, 220, 40, 255]))]);
    let models = std::sync::Arc::new(NativeModels::from_models([(tag, quad_model(0.5), green)]));
    let scene = crate::scene::Scene::default();
    let mut vp = crate::render::Viewport::new(&device);
    vp.set_native(&device, &queue, &loads);
    vp.set_native_models(&device, &queue, models);
    // No glyphs: the centre pixel is the model's or the floor's.
    let mut shown = crate::layers::Layers::default();
    shown.set(crate::layers::LayerId::Markers, false);
    vp.set_layers(&shown);
    let draw = |vp: &mut crate::render::Viewport, marker: Option<Marker>| {
        let markers: Vec<Marker> = marker.into_iter().collect();
        vp.set_placements(&device, &scene, &markers, &[]);
        vp.render(&device, &queue, &camera, [64, 64]);
        read_centre(&device, &queue, vp.color_texture().unwrap())
    };
    let upright = |z: f32, object| Marker {
        axes: crate::geometry::rotation([0.0; 3]),
        scale: 1.0,
        ..placed_at([half, half, z], object)
    };
    let floor = draw(&mut vp, None);
    let object = draw(&mut vp, Some(upright(0.0, Some(tag))));
    assert_eq!(vp.native_object_count(), 1);
    assert_eq!(vp.native_selected_count(), 0);
    assert!(
        object[1] > object[0] && object[1] > object[2],
        "the model's base colour: {object:?} over {floor:?}"
    );
    // Selected, it mixes towards the selection orange as the scene's
    // models do: redder and less blue than the green model.
    let selected = upright(0.0, Some(tag));
    vp.set_placements(
        &device,
        &scene,
        &[selected],
        &[(selected.block, selected.index)],
    );
    assert_eq!(vp.native_selected_count(), 1);
    vp.render(&device, &queue, &camera, [64, 64]);
    let tinted = read_centre(&device, &queue, vp.color_texture().unwrap());
    assert!(
        tinted[0] > object[0] + 40 && tinted[2] >= object[2],
        "selected: {tinted:?}, unselected: {object:?}"
    );
    let mut hidden = shown.clone();
    hidden.set(crate::layers::LayerId::Objects, false);
    vp.set_layers(&hidden);
    assert_eq!(draw(&mut vp, Some(upright(0.0, Some(tag)))), floor);
    vp.set_layers(&shown);
    assert_eq!(
        draw(&mut vp, Some(upright(-3.0, Some(tag)))),
        floor,
        "under the floor"
    );
    assert_eq!(draw(&mut vp, Some(upright(0.0, Some(TagId(9))))), floor);
    assert_eq!(vp.native_object_count(), 0);
    // A Lighting change uploads the models again and keeps them placed.
    draw(&mut vp, Some(upright(0.0, Some(tag))));
    vp.set_lighting(&device, &queue, &loads, Lighting::Flat);
    assert_eq!(vp.native_object_count(), 1);
}

/// The session reads a native model for, or names why it could not, every
/// object tag the map places.
#[test]
fn the_session_reads_the_placed_objects_models() {
    let bytes = crate::fixture::fixture_with_textured_bsp();
    let dir = std::env::temp_dir().join(format!("native-models-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("textured.map");
    std::fs::write(&path, &bytes).unwrap();
    let mut ed = crate::Session::new();
    ed.open(crate::save::MapSource::file(&path)).unwrap();
    let models = ed.native_models().expect("a map is open").clone();
    for object in ed.markers().iter().filter_map(|m| m.object) {
        let refused = models.errors.iter().any(|(t, _)| *t == object);
        assert!(models.of(object).is_some() || refused, "{object:?}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// The native selection highlight is the scene's: the same mix amount as
/// a selected scene instance's tint alpha.
#[test]
fn the_selection_highlight_matches_the_scenes_tint() {
    let marker = placed_at([0.0; 3], None);
    let tint = crate::render::Instance::of(&marker, true).tint;
    let alpha = f32::from(SELECTED_HIGHLIGHT[3]) / 255.0;
    assert!((alpha - tint[3]).abs() < 1.0 / 255.0, "{alpha} {}", tint[3]);
    assert_eq!(crate::render::Instance::of(&marker, false).tint[3], 0.0);
    let orange = [1.0, 0.78, 0.25];
    for k in 0..3 {
        let c = f32::from(SELECTED_HIGHLIGHT[k]) / 255.0;
        assert!((c - orange[k]).abs() < 1.0 / 255.0, "{k}: {c}");
    }
}

/// An item reference set after the map opened to a tag nothing placed at
/// open has its native model read at once, as gap 4 reads its collision:
/// the session hands out a new set holding it (here the fixture's spare
/// item collection, which has no entry and so no render model: it is
/// refused and named), and undoing keeps what was read.
#[test]
fn an_item_reference_set_after_open_has_its_model_read() {
    use crate::fixture::id;
    let bytes = crate::fixture::fixture_with_textured_bsp();
    let dir = std::env::temp_dir().join(format!("native-late-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("textured.map");
    std::fs::write(&path, &bytes).unwrap();
    let mut ed = crate::Session::new();
    ed.open(crate::save::MapSource::file(&path)).unwrap();
    assert!(
        !ed.native_world().unwrap().is_empty(),
        "{:?}",
        ed.native_errors()
    );
    // The spare item collection: nothing places it at open.
    let late = id(9);
    let before = ed.native_models().unwrap().clone();
    assert!(
        !before.has_read(TagId::NULL),
        "the tag metadata read: {:?}",
        before.errors
    );
    assert!(!before.has_read(late));
    ed.apply(crate::Command::set(
        crate::BlockKind::NetgameEquipment,
        0,
        "item",
        crate::Value::Tag(crate::TagRef {
            group: map::FourCC(*b"itmc"),
            id: late,
        }),
    ))
    .unwrap();
    let after = ed.native_models().unwrap().clone();
    assert!(!std::sync::Arc::ptr_eq(&before, &after), "a new set");
    assert!(after.has_read(late));
    assert!(
        after.errors.iter().any(|(t, _)| *t == late),
        "{:?}",
        after.errors
    );
    assert!(!before.has_read(late), "the old set is left as it was");
    assert!(ed.undo());
    assert!(ed.native_models().unwrap().has_read(late));
    std::fs::remove_dir_all(&dir).ok();
}

// ---- posed models (M1) --------------------------------------------------------

/// A pose that turns node 1 a quarter about z at frame 1 of 2, over a
/// two-node skeleton whose child sits at x = 1: `posed_nodes` composes the
/// turn into the child's world matrix and leaves the rest pose elsewhere.
#[test]
fn posed_nodes_compose_the_animation_over_the_defaults() {
    use tags::anim_sample::{NodeTransform, Pose};
    let skeleton = [
        SkeletonNode {
            parent: -1,
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: 1.0,
        },
        SkeletonNode {
            parent: 0,
            translation: [1.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: 1.0,
        },
    ];
    let mut pose = Pose {
        nodes: vec![NodeTransform::default(), NodeTransform::default()],
    };
    pose.nodes[1].rotation = [
        0.0,
        0.0,
        std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
    ];
    pose.nodes[1].has_rotation = true;
    let nodes = posed_nodes(&skeleton, &pose, &NodeMatrix::IDENTITY).unwrap();
    assert_eq!(nodes.len(), 2);
    // The root is where it rests.
    assert_eq!(nodes[0], NodeMatrix::IDENTITY);
    // The child is its rest frame after the quarter turn: x now points to
    // y, and the node still sits at x = 1.
    let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-5);
    assert!(close(nodes[1].forward, [0.0, 1.0, 0.0]), "{:?}", nodes[1]);
    assert!(close(nodes[1].position, [1.0, 0.0, 0.0]), "{:?}", nodes[1]);
    // A vertex at the child's x = 1 (its own origin) is unchanged; one
    // further out follows the turn, as the renderer skins it.
    assert!(close(nodes[1].point([0.0, 0.0, 0.0]), [1.0, 0.0, 0.0]));
    assert!(close(nodes[1].point([1.0, 0.0, 0.0]), [1.0, 1.0, 0.0]));
    // A pose for another node count is named, not papered over.
    let short = Pose {
        nodes: vec![NodeTransform::default()],
    };
    let e = posed_nodes(&skeleton, &short, &NodeMatrix::IDENTITY).unwrap_err();
    assert!(e.contains("node count"), "{e}");
}

/// A box chain halves each level to the nearest average, an odd edge
/// repeating its last column, and refuses bytes of another size.
#[test]
fn a_box_chain_averages_two_by_two_blocks() {
    // 3 by 2: the second column of the next level repeats column 2.
    let rgba: Vec<u8> = [[0u8, 0, 0, 255], [100, 0, 0, 255], [40, 8, 0, 255]]
        .into_iter()
        .chain([[200, 0, 0, 255], [100, 4, 0, 255], [80, 8, 0, 255]])
        .flatten()
        .collect();
    let image = box_chain(3, 2, &rgba, 2).unwrap();
    assert_eq!((image.width, image.height, image.mips.len()), (3, 2, 2));
    // level 1 is 1 by 1: columns 0 and 1 of both rows.
    assert_eq!(image.mips[0], vec![100, 1, 0, 255]);
    assert_eq!(image.mips[1], vec![100, 1, 0, 255]);
    assert!(box_chain(3, 2, &rgba[1..], 1).is_none());
    assert!(box_chain(0, 2, &[], 1).is_none());
}

/// A map's base bitmap is repainted in the world drawn from it: the
/// invented textured BSP read from a map names its base bitmap, a repaint
/// of the same size changes the centre pixel to the new colour, a repaint
/// of another size changes nothing, and Lightmap only keeps its white.
/// Skipped without a wgpu adapter.
#[test]
fn a_repainted_base_map_changes_the_drawn_world() {
    use crate::fixture::TEX_BASE_RGBA;
    let Some((device, queue)) = device() else {
        return;
    };
    let (mut map, mut files) = textured();
    let load = NativeLoad::read(&mut map, &mut files, 0).unwrap();
    let bitmap = load.base_bitmaps[&0];
    let camera = Camera {
        target: [2.0, 2.0, 0.0],
        yaw: 0.0,
        pitch: 1.3,
        distance: 3.0,
        ..Camera::default()
    };
    let mut world = NativeWorld::new(&device, &queue, &load, Lighting::Flat);
    assert!(world.draws_bitmap(bitmap));
    assert!(!world.draws_bitmap(TagId(0x77)));
    let (before, _) = centre_of(&device, &queue, &mut world, &camera);
    let blue: Vec<u8> = [10u8, 20, 240, 255].repeat(16);
    assert_eq!(world.repaint_base(&queue, bitmap, 4, 4, &blue), 1);
    let (after, _) = centre_of(&device, &queue, &mut world, &camera);
    assert_ne!(before, after, "the texture was written");
    assert!(after[2] > 180 && after[0] < 80, "blue: {after:?} (was {before:?}, base {TEX_BASE_RGBA:?})");
    // Another size is refused whole.
    assert_eq!(world.repaint_base(&queue, bitmap, 2, 2, &blue[..16]), 0);
    let (again, _) = centre_of(&device, &queue, &mut world, &camera);
    assert_eq!(after, again);
    // A bitmap no material took writes nothing.
    assert_eq!(world.repaint_base(&queue, TagId(0x77), 4, 4, &blue), 0);
    // Lightmap only draws white whatever the bitmap.
    let flat = NativeWorld::new(&device, &queue, &load, Lighting::LightmapOnly);
    assert_eq!(flat.repaint_base(&queue, bitmap, 4, 4, &blue), 0);
}

/// The invented coloured material ([`crate::fixture::fixture_with_shader_colour`])
/// with its pass in `layer`, opened as a map, and its raw resources.
fn coloured(layer: u8) -> (Map<std::io::Cursor<Vec<u8>>>, ResourceFiles) {
    let bytes = crate::fixture::fixture_with_shader_colour(layer);
    let mut files = ResourceFiles::new();
    files.set_bytes(map::ResourceFile::Local, bytes.clone().into());
    (
        Map::from_reader(std::io::Cursor::new(bytes)).unwrap(),
        files,
    )
}

/// The floor's shader drawn alone on each shape plans its layer-5 pass
/// and its base look, as the level does; in layer 1 the same pass is
/// refused by name and nothing of it is drawn.
#[test]
fn a_material_preview_plans_the_materials_own_passes() {
    use crate::fixture::{LAYERED_LAYER, id};
    use preview::{Shape, material, material_draws};
    for shape in [Shape::Sphere, Shape::Quad] {
        let (mut map, mut files) = coloured(LAYERED_LAYER);
        let p = material(&mut map, &mut files, id(6), shape).unwrap();
        assert_eq!(p.shape, shape);
        let d = material_draws(&p);
        assert_eq!(d.base, None, "{shape:?}: {d:?}");
        assert!(d.unresolved.is_empty(), "{d:?}");
        assert!(d.drawn.contains(&LAYERED_LAYER), "{shape:?}: {d:?}");
        assert!(
            d.drawn
                .iter()
                .all(|&l| l == LAYERED_LAYER || l == layers_load::LAYER_3),
            "{shape:?}: {d:?}"
        );
        assert!(
            d.refused.iter().all(|(layer, _)| *layer == layers_load::LAYER_3),
            "only layer 3 may stand aside for the base look: {d:?}"
        );
    }
    let (mut map, mut files) = coloured(1);
    let p = material(&mut map, &mut files, id(6), Shape::Sphere).unwrap();
    let d = material_draws(&p);
    assert!(!d.drawn.contains(&1), "{d:?}");
    let (_, why) = d.refused.iter().find(|(l, _)| *l == 1).expect("layer 1 named");
    assert!(why.contains(LAYER_1_UNCHECKED), "{why}");
}

/// The shape geometry: the sphere's points are on the unit sphere with
/// their normals, its triangles face outward (counter-clockwise from
/// outside, as the square's from above) and index its vertices.
#[test]
fn the_preview_shapes_face_outward() {
    use preview::{quad, sphere};
    let s = sphere();
    let n = s.positions.len();
    assert_eq!(s.normals.len(), n);
    assert_eq!((s.tangents.len(), s.binormals.len(), s.uvs.len()), (n, n, n));
    for p in &s.positions {
        let r = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
        assert!((r - 1.0).abs() < 1e-5, "{p:?}");
    }
    let facing = |sec: &SectionInput| {
        let mut out = 0usize;
        for t in &sec.parts[0].triangles {
            let [a, b, c] = (*t).map(|i| sec.positions[usize::from(i)]);
            let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let mid = [
                (a[0] + b[0] + c[0]) / 3.0,
                (a[1] + b[1] + c[1]) / 3.0,
                (a[2] + b[2] + c[2]) / 3.0,
            ];
            let outward = sec.normals[usize::from(t[0])];
            let dir = if sec.positions.len() == 4 { outward } else { mid };
            let dot = n[0] * dir[0] + n[1] * dir[1] + n[2] * dir[2];
            // A pole's triangles have no area: they face nowhere.
            if dot < -1e-7 {
                out += 1;
            }
            assert!(t.iter().all(|&i| usize::from(i) < sec.positions.len()));
        }
        out
    };
    assert_eq!(facing(&s), 0, "every sphere triangle faces out");
    assert_eq!(facing(&quad()), 0, "the square faces up");
}

/// On the GPU: the coloured material on a sphere draws its pass's
/// colour over the base look, and an edit of the colour parameter in
/// the map's bytes changes the picture to the new colour; in layer 1
/// the pass is absent and the base look alone is drawn. Skipped without
/// a wgpu adapter.
#[test]
fn a_material_preview_draws_its_colour_and_follows_an_edit() {
    use crate::fixture::{LAYERED_LAYER, SHADER_COLOURS, id, put};
    use preview::{Shape, material, render_material};
    let Some((device, queue)) = device() else {
        return;
    };
    let draw = |bytes: Vec<u8>| -> Vec<[u8; 4]> {
        let mut files = ResourceFiles::new();
        files.set_bytes(map::ResourceFile::Local, bytes.clone().into());
        let mut map = Map::from_reader(std::io::Cursor::new(bytes)).unwrap();
        let p = material(&mut map, &mut files, id(6), Shape::Sphere).unwrap();
        let tex = render_material(&device, &queue, &p, [64, 64], [0.0; 3]);
        read_pixels(&device, &queue, &tex)
    };
    let green = |p: &[u8; 4]| p[1] > 250 && p[0] < 5 && p[2] < 5;
    let red = |p: &[u8; 4]| p[0] > 250 && p[1] < 5 && p[2] < 5;
    let before = draw(crate::fixture::fixture_with_shader_colour(LAYERED_LAYER));
    let lit = before.iter().filter(|p| p[..3] != [0, 0, 0]).count();
    assert!(lit > 200, "the sphere fills the view: {lit}");
    // The pass draws at the base look's depth (the same position code):
    // all but a stray edge pixel is its colour.
    let most = |n: usize| n * 100 >= lit * 95;
    assert!(
        most(before.iter().filter(|p| green(p)).count()),
        "the sphere is the pass's green"
    );
    let mut edited = crate::fixture::fixture_with_shader_colour(LAYERED_LAYER);
    put(&mut edited, crate::fixture::META_FILE + SHADER_COLOURS, 0xffff_0000);
    let after = draw(edited);
    assert!(most(after.iter().filter(|p| red(p)).count()), "the edit shows");
    let refused = draw(crate::fixture::fixture_with_shader_colour(1));
    assert_eq!(refused.iter().filter(|p| green(p)).count(), 0, "layer 1 is absent");
    assert!(refused.iter().filter(|p| p[..3] != [0, 0, 0]).count() > 200, "the base look draws");
}

/// The floor's final C0 red lane follows a one-second ramp. Its green
/// and blue lanes still come from the authored colour.
#[test]
fn material_at_and_the_playing_clock_sample_the_same_pixels() {
    use crate::fixture::{self, FINAL_C0, LAYERED_LAYER, META, META_FILE, TEX_SHAD, id, put, putf};
    use preview::{MaterialSurface, Shape, material_at, render_material};

    let mut bytes = fixture::fixture_with_shader_colour(LAYERED_LAYER);
    // Free space after the vertex shader in the fixture's metadata.
    let table = 0x2c00;
    let post = META_FILE + TEX_SHAD + 0x60;
    for (field, count, relative) in [
        (0x1c, 1, 0x00), // instance LOD, mask and six layer entries
        (0x24, 6, 0x08),
        (0x2c, 1, 0x14), // one pass and its animation ranges
        (0x34, 1, 0x18),
        (0x3c, 1, 0x28), // time property, ref, range and binding
        (0x44, 1, 0x40),
        (0x4c, 1, 0x44),
        (0x54, 1, 0x48),
    ] {
        put(&mut bytes, post + field, count);
        put(&mut bytes, post + field + 4, META + (table + relative) as u32);
    }
    let at = META_FILE + table;
    put(&mut bytes, at, 1 << LAYERED_LAYER);
    for (offset, count) in [
        (4, 6u16),
        (0x08 + 2 * usize::from(LAYERED_LAYER), 1),
        (0x14, 1),
        (0x18 + 6, 1),
        (0x44, 1),
    ] {
        bytes[at + offset..at + offset + 2].copy_from_slice(&(count << 9).to_le_bytes());
    }
    putf(&mut bytes, at + 0x28 + 8, 1.0); // period
    put(&mut bytes, at + 0x28 + 12, 20);
    put(&mut bytes, at + 0x28 + 16, META + table as u32 + 0x50);
    putf(&mut bytes, at + 0x50 + 8, 1.0); // identity function, range [0, 1]
    bytes[at + 0x48..at + 0x4a].copy_from_slice(&FINAL_C0.to_le_bytes());

    let mut files = ResourceFiles::new();
    files.set_bytes(map::ResourceFile::Local, bytes.clone().into());
    let mut map = Map::from_reader(std::io::Cursor::new(bytes)).unwrap();
    let mut sample = |time| material_at(&mut map, &mut files, id(6), Shape::Sphere, time).unwrap();
    let start = sample(0.0);
    let half = sample(0.5);
    let plan = |p: &preview::MaterialPreview| {
        let layer = &p.load.layers().unwrap().plans()[&LAYERED_LAYER];
        assert!(layer.drawn.contains(&0), "{:?}", layer.refused);
        layer.constants[&0].clone()
    };
    assert_ne!(plan(&start), plan(&half));
    assert_eq!(plan(&start), plan(&sample(1.0)));

    let Some((device, queue)) = device() else {
        return;
    };
    let mut surface = MaterialSurface::new(&device, &queue, &start, [64, 64]);
    surface.draw(&device, &queue, &start, [0.0; 3]);
    let first = read_pixels(&device, &queue, surface.color());
    surface.draw_at(&device, &queue, &start, [0.0; 3], 0.5);
    let playing = read_pixels(&device, &queue, surface.color());
    assert_ne!(first, playing);
    let at_half = render_material(&device, &queue, &half, [64, 64], [0.0; 3]);
    assert_eq!(playing, read_pixels(&device, &queue, &at_half));
    surface.draw_at(&device, &queue, &start, [0.0; 3], 1.0);
    assert_eq!(first, read_pixels(&device, &queue, surface.color()));
}

/// Every pixel of a 64 by 64 colour texture.
fn read_pixels(device: &wgpu::Device, queue: &wgpu::Queue, color: &wgpu::Texture) -> Vec<[u8; 4]> {
    let row = 256u32;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * 64),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        color.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(64),
            },
        },
        wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([enc.finish()]);
    buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = buf.slice(..).get_mapped_range().unwrap();
    (0..64 * 64)
        .map(|i| {
            let at = (i / 64) * row as usize + (i % 64) * 4;
            [data[at], data[at + 1], data[at + 2], data[at + 3]]
        })
        .collect()
}
