//! Reusing a copy must still read current guest bytes and retain draw snapshots.
mod common;
use common::*;
use d3d::{Backend, DrawOutcome, GuestMemory, ResolvedDraw, UploadPolicy};
use gfx::{Clear, D3DFormat, texture::DecodedTexture};

#[derive(Default)]
struct Copies {
    next: u32,
    draws: Vec<(usize, ResolvedDraw)>,
    fail: bool,
}
impl Backend for Copies {
    type Surface = u32;
    type Texture = u32;
    fn bc_supported(&self) -> bool {
        false
    }
    fn create_surface(&mut self, _: D3DFormat, _: u32, _: u32, _: bool) -> Result<u32, String> {
        self.next += 1;
        Ok(self.next)
    }
    fn destroy_surface(&mut self, _: u32) -> Result<(), String> {
        Ok(())
    }
    fn upload_texture(&mut self, _: &DecodedTexture) -> Result<u32, String> {
        Ok(1)
    }
    fn clear(&mut self, _: u32, _: Option<u32>, _: Clear) -> Result<(), String> {
        Ok(())
    }
    fn present(&mut self, _: u32, _: u32) -> Result<(), String> {
        Ok(())
    }
    fn draw(
        &mut self,
        _: u32,
        _: Option<u32>,
        draw: &ResolvedDraw,
        _: [Option<&u32>; 4],
    ) -> Result<DrawOutcome, String> {
        self.draws
            .push((draw.streams[0].data.as_ptr() as usize, draw.clone()));
        if std::mem::take(&mut self.fail) {
            Err("synthetic failure".into())
        } else {
            Ok(DrawOutcome::Drawn)
        }
    }
}

#[test]
fn stream_storage_is_reused_and_each_draw_reads_current_bytes() {
    let (mut d, mut m) = create(Copies::default(), UploadPolicy::Tracked);
    // The copy path's pool (the vertex cache, which keeps what the write
    // tracker reports clean, is tests/vertex_cache.rs).
    d.set_vertex_cache(None);
    set_states(&mut d, &mut m);
    quad_vb(&mut m, 0.0, 0.0, 8.0, 8.0, 0xffff_ffff);
    bind_quad(&mut d, &mut m);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let first = d.backend.draws[0].clone();
    // Reusing capacity must not cache the source bytes, even without dirty reports.
    m.write_u32(0x8000_0000 | VB_DATA, 0x3fc0_0000).unwrap();
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    let second = &d.backend.draws[1];
    assert_eq!(first.0, second.0, "payload capacity reused");
    assert_ne!(first.1.streams[0].data, second.1.streams[0].data);
    assert_eq!(second.1.streams[0].data[..4], 0x3fc0_0000u32.to_le_bytes());
    assert_eq!(
        d.backend.draws[0].1, first.1,
        "backend-owned snapshot stays intact"
    );
    // Both a failed backend call and a failed guest read retain reusable storage.
    d.backend.fail = true;
    assert!(d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).is_err());
    assert!(d.draw_vertices(&mut m, TRIANGLESTRIP, 204, 4).is_err());
    d.draw_vertices_up(&mut m, TRIANGLESTRIP, 3, 0x8000_0000 | VB_DATA, 20)
        .unwrap();
    let last = d.backend.draws.last().unwrap();
    assert_eq!(last.0, first.0);
    assert_eq!(
        last.1.streams[0].data.len(),
        60,
        "shorter draw has no stale tail"
    );
    assert_eq!(last.1.streams[0].stride, 20);
}

#[test]
fn indexed_windows_and_capacity_growth_keep_the_same_bytes() {
    let (mut d, mut m) = create(Copies::default(), UploadPolicy::Tracked);
    set_states(&mut d, &mut m);
    quad_vb(&mut m, 0.0, 0.0, 8.0, 8.0, 0xffff_ffff);
    bind_quad(&mut d, &mut m);
    let indices = MISC + 0x4000;
    m.write(indices, &[3, 0, 1, 0, 2, 0]).unwrap();
    d.draw_indexed_vertices(&mut m, TRIANGLESTRIP, 3, indices)
        .unwrap();
    let first = &d.backend.draws[0].1;
    let mut expected = vec![0; 60];
    m.read((0x8000_0000 | VB_DATA) + 20, &mut expected).unwrap();
    assert_eq!(first.indices.as_deref(), Some(&[2, 0, 1][..]));
    assert_eq!(first.streams[0].data, expected);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 64).unwrap();
    let grown = d.backend.draws.last().unwrap().0;
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 32).unwrap();
    let smaller = d.backend.draws.last().unwrap();
    assert_eq!(smaller.0, grown);
    assert_eq!(smaller.1.streams[0].data.len(), 640);
}
