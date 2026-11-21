use super::*;
use crate::geometry::Exact;
use native_render::world::{Owner, PartInput, Placement, SectionInput, WorldBuilder};

const PAGE: LightmapKey = LightmapKey {
    page: 0,
    palette: 0,
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

/// The depth the world pass writes is view depth over the far plane.
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
    assert!(!DEPTH_RULE.floor);
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
    assert_eq!(load.stats().draws, 1, "only the unlit draw with a base map");
    assert_eq!(load.stats().untextured, 3);
    assert_eq!(load.world().draws[0].material, 2);
    let pages = HashMap::from([(PAGE, Image::solid([128; 4]))]);
    let load = NativeLoad::new(TagId(7), w, base, pages);
    let s = load.stats();
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

/// Draw `load` in `lighting` into a 64 by 64 viewport target and read
/// the centre pixel back.
fn centre(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    load: &NativeLoad,
    lighting: Lighting,
    camera: &Camera,
) -> ([u8; 4], CullStats) {
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
        crate::render::COLOR_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    );
    let depth = target(
        crate::render::DEPTH_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT,
    );
    let mut world = NativeWorld::new(device, queue, load, lighting);
    assert_eq!(world.lighting(), lighting);
    let stats = world.render(
        device,
        queue,
        &color.create_view(&Default::default()),
        crate::render::COLOR_FORMAT,
        &depth.create_view(&Default::default()),
        crate::render::DEPTH_FORMAT,
        [64, 64],
        camera,
        [0.0; 3],
    );
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
    ([data[at], data[at + 1], data[at + 2], data[at + 3]], stats)
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
