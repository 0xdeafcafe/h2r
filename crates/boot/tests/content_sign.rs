//! Halo 2's own content signature check, run on the fallback interpreter
//! with the kernel HLE behind it, accepts a ContentMeta.xbx signed by
//! `kernel::content::sign` (what `h2 import-mappack` writes) and rejects the
//! unsigned header a disc ships. docs/map-pack.md has the addresses.
//!
//! `H2_XBE=/path/default.xbe cargo test -p boot --test content_sign -- --ignored`

use iced_x86::{Decoder, DecoderOptions, Mnemonic};
use runtime::guest::Permissions;
use runtime::{CpuAbi, Event, ExecCtx, Runner};

/// XAPI: the header signature, `(handle, title, header, digest out)`. With
/// a null handle it covers the header bytes in memory, as the readers do
/// for a header of 0x88 bytes.
const SIGNATURE: u32 = 0x002d_2938;
/// What it runs: the title key (XcHMAC), XAPI's HMAC on XcSHA*, the thunks.
const CODE: [(u32, u32); 3] = [
    (0x002d_2919, 0x002d_2a1d),
    (0x002d_6459, 0x002d_6532),
    (0x002d_6f84, 0x002d_6fa2),
];
const BUF: u32 = 0x3000_0000;
const STACK: u32 = 0x3010_0000;
const TITLE: u32 = 0x4d53_0064;

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn the_guest_accepts_content_the_import_signs() {
    let path = std::env::var_os("H2_XBE").expect("set H2_XBE to a retail default.xbe");
    let bytes = std::fs::read(path).unwrap();
    let mut loaded =
        runtime::load_xbe_with(&bytes, &runtime::loader::LoadOptions::halo2()).unwrap();
    let _kernel = kernel::Kernel::for_image(&mut loaded).unwrap();
    let rt = loaded.runtime;
    boot::fallback::init(&bytes, &[], &[]).unwrap();
    boot::fallback::set_recovery(true);
    boot::fallback::install_unread_targets(&rt);
    // Every instruction is an entry, so calls, kernel returns and rets
    // all land back in the interpreter.
    let mut pcs = Vec::new();
    {
        let m = rt.memory();
        for (start, end) in CODE {
            let mut code = vec![0u8; (end - start) as usize];
            m.read_bytes(start, &mut code, 1).unwrap();
            let decoded = Decoder::with_ip(32, &code, start as u64, DecoderOptions::NONE);
            pcs.extend(
                decoded
                    .into_iter()
                    .filter(|i| i.mnemonic() != Mnemonic::Int3)
                    .map(|i| i.ip32()),
            );
        }
    }
    assert!(pcs.contains(&SIGNATURE));
    rt.edit_dispatch(|d| {
        for &pc in &pcs {
            d.register_override(
                pc,
                "test: content signature in the fallback",
                move |cpu, ctx, _| {
                    cpu.eip = pc;
                    Ok(boot::fallback::interpret(cpu, ctx))
                },
            );
        }
    });
    {
        let mut m = rt.memory();
        m.map_zeroed(BUF, 0x4000, Permissions::RW).unwrap();
        m.map_zeroed(STACK - 0x2000, 0x2000, Permissions::RW)
            .unwrap();
    }
    let mut ctx = ExecCtx::new(rt.clone(), 0);
    // The guest's digest for `meta`.
    let mut digest = |meta: &[u8], title: u32| -> [u8; 20] {
        let (header, out) = (BUF, BUF + 0x2000);
        let mut cpu = CpuAbi {
            esp: STACK - 0x100,
            eflags: 0x202,
            ebx: 0x1111,
            esi: 0x2222,
            edi: 0x3333,
            ebp: 0x4444,
            ..Default::default()
        };
        {
            let mut m = rt.memory();
            m.write_bytes(header, meta, 1).unwrap();
            m.write_bytes(out, &[0xee; 20], 1).unwrap();
            for (k, w) in [runtime::runner::HARNESS_EXIT, 0, title, header, out]
                .into_iter()
                .enumerate()
            {
                m.set_u32(cpu.esp + 4 * k as u32, w).unwrap();
            }
        }
        let esp = cpu.esp;
        let t = Runner::default().run(&mut cpu, &mut ctx, SIGNATURE);
        assert_eq!(ctx.event(t.event_id), Some(&Event::HarnessExit), "{t:?}");
        assert_eq!(cpu.eax, 1);
        assert_eq!(cpu.esp, esp + 4 + 16, "stdcall, four arguments");
        assert_eq!(
            (cpu.ebx, cpu.esi, cpu.edi, cpu.ebp),
            (0x1111, 0x2222, 0x3333, 0x4444)
        );
        let mut d = [0u8; 20];
        rt.memory().read_bytes(out, &mut d, 1).unwrap();
        d
    };

    // A synthetic ContentMeta: XCSF, header size 0x88, Halo 2's title and
    // offer, a body after the header.
    let mut meta = vec![0u8; 0x180];
    meta[0x14..0x18].copy_from_slice(b"XCSF");
    meta[0x18..0x1c].copy_from_slice(&0x88u32.to_le_bytes());
    meta[0x1c..0x20].copy_from_slice(&1u32.to_le_bytes());
    meta[0x24..0x28].copy_from_slice(&TITLE.to_le_bytes());
    meta[0x28..0x30].copy_from_slice(&(((TITLE as u64) << 32) | 1).to_le_bytes());
    for (i, b) in meta[0x30..].iter_mut().enumerate() {
        *b = (i * 7 + 3) as u8;
    }
    let unsigned = meta.clone();
    assert_ne!(
        digest(&unsigned, TITLE)[..],
        unsigned[..20],
        "an unsigned header fails"
    );

    kernel::content::sign(&mut meta, TITLE).unwrap();
    assert_eq!(
        digest(&meta, TITLE)[..],
        meta[..20],
        "the guest computes the import's signature"
    );
    assert_eq!(meta[0x20] & 1, 1, "and the readers' installed flag is set");
    // The guest's digest is a function of the signed span only.
    let mut body = meta.clone();
    body[0x100] ^= 0xff;
    assert_eq!(digest(&body, TITLE)[..], meta[..20]);
    let mut header = meta.clone();
    header[0x30] ^= 1;
    assert_ne!(digest(&header, TITLE)[..], meta[..20]);
    // A title of 0 means "the header's own".
    assert_eq!(digest(&meta, 0)[..], meta[..20]);
}
