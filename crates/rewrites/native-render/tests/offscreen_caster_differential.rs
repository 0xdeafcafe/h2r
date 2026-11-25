//! Compare the offscreen root's light with the sample returned by retail
//! 0x0003dc90. All object records and both samples are invented.
#![cfg(feature = "differential")]

use ir::interp::{Cpu, SparseMemory};
use lifted::{code::run_until, image::Image};
use scene_extract::layout::{camera, nearby_casters, object_nodes::TAG_INSTANCES, objects};
use scene_extract::testing::{self, BODIES, CACHE_DATA, Mem};

#[test]
fn offscreen_root_reads_the_sample_returned_by_the_guest() {
    let Some(image) = Image::get() else { return };
    let mut code = image.original();
    let mut seed = 0x9e37_79b9u32;
    for n in 0..96 {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let mut words = Mem::default();
        let slot = seed % 24;
        let handle = testing::object(&mut words, slot, 0, 1);
        let body = BODIES + 0x1000 * slot;
        let cache = CACHE_DATA + objects::CACHE_RECORD_SIZE * (slot + 7);
        testing::camera_record(&mut words, 0x1000);
        let cam = camera::read(&words, 0x1000).unwrap();
        words.f(body + objects::BODY_SPHERE, -1.0);
        words.w(TAG_INSTANCES, 0x2000);
        words.w(0x2008, 0x3000);
        words.w(0x3018, 2);
        words.w(body + 0x14, u32::MAX);
        words.w(body + 0xa8, 0);
        words.w(body + 4, if n % 3 == 0 { 1 << 24 } else { 0 });
        words.w(cache, if n % 3 == 2 { 0 } else { 1 << 24 });
        words.w(nearby_casters::DEFAULT_LIGHTING, 0x4000);
        for (sample, scale) in [(0x401c, 0.25), (cache + 0x54, 0.75)] {
            words.f(sample + 0xc, -4.0);
            words.f(sample + 0x10, scale);
            words.f(sample + 0x14, 0.0);
            words.f(sample + 0x1c, scale + (seed >> 8) as f32 / 134217728.0);
        }

        let got = nearby_casters::extract(&words, &cam);
        let mut ram = SparseMemory::new();
        for (&addr, &word) in &words.0 {
            ram.load(addr, &word.to_le_bytes());
        }
        let stack = 0x10000;
        let stop = lift::harness::HARNESS_EXIT;
        ram.load(stack, &stop.to_le_bytes());
        ram.load(stack + 4, &handle.to_le_bytes());
        let mut cpu = Cpu {
            eip: 0x0003_dc90,
            ..Default::default()
        };
        cpu.gpr[4] = stack;
        run_until(&mut code, &mut cpu, &mut ram, 1024, |c| c.eip == stop).unwrap();
        let sample = cpu.gpr[0];
        if sample == 0 {
            assert!(got.is_empty(), "seed {seed}");
        } else {
            assert_eq!(got.len(), 1, "seed {seed}");
            assert_eq!(got[0].handle, handle);
            assert_eq!(
                got[0].strength.to_bits(),
                ram.read_u32(sample + 0x1c).unwrap()
            );
            let dir =
                [0xc, 0x10, 0x14].map(|off| f32::from_bits(ram.read_u32(sample + off).unwrap()));
            let len = dir[0].hypot(dir[1]).hypot(dir[2]);
            assert_eq!(got[0].towards_light, dir.map(|v| v / len));
        }
    }
}
