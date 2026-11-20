//! Checks the address constants against the real retail XBE. Ignored by
//! default (the image is not in the repository). Run with the path to a
//! `default.xbe`, read-only:
//!
//! `H2_XBE=/path/default.xbe cargo test -p bink-hle --test xbe_pins -- --ignored`
use bink_hle::addr::*;
use runtime::load_xbe;

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn constants_match_the_retail_image() {
    let Some(path) = std::env::var_os("H2_XBE") else {
        panic!("set H2_XBE to a retail default.xbe");
    };
    let image = load_xbe(&std::fs::read(path).unwrap()).unwrap();
    let mem = image.runtime.memory();
    let bytes = |va: u32, n: usize| {
        let mut v = vec![0; n];
        mem.read_bytes(va, &mut v, 1).unwrap();
        v
    };
    // Every rel32 call target inside the wrapper window, with its site.
    let window = bytes(0x0015_5e00, 0xd00);
    let mut calls = Vec::new();
    for i in 0..window.len() - 5 {
        if window[i] == 0xe8 {
            let rel = i32::from_le_bytes(window[i + 1..i + 5].try_into().unwrap());
            let site = 0x0015_5e00 + i as u32;
            calls.push((site, site.wrapping_add(5).wrapping_add(rel as u32)));
        }
    }
    for (name, f) in [
        ("BinkSetSoundSystem", BINK_SET_SOUND_SYSTEM),
        ("BinkOpen", BINK_OPEN),
        ("BinkWait", BINK_WAIT),
        ("BinkSetMixBins", BINK_SET_MIX_BINS),
        ("BinkSetMixBinVolumes", BINK_SET_MIX_BIN_VOLUMES),
        ("BinkSetMemory", BINK_SET_MEMORY),
        ("BinkCopyToBuffer", BINK_COPY_TO_BUFFER),
        ("BinkDoFrame", BINK_DO_FRAME),
        ("BinkNextFrame", BINK_NEXT_FRAME),
        ("BinkClose", BINK_CLOSE),
    ] {
        assert!(
            calls.iter().any(|c| c.1 == f),
            "{name} {f:#x} is not called from the wrapper"
        );
    }
    // BinkOpen checks the BIKi signature in its body.
    assert!(bytes(BINK_OPEN, 0x200).windows(4).any(|w| w == b"BIKi"));
    // MovieStart: `sub esp, 40h` prologue, and the four known callers.
    assert_eq!(bytes(MOVIE_START, 3), [0x83, 0xec, 0x40]);
    // The callers are outside the window; read them directly.
    for site in [0x0012_bff5u32, 0x0016_3949, 0x0022_3a63, 0x002a_b8e7] {
        let b = bytes(site, 5);
        assert_eq!(b[0], 0xe8, "call at {site:#x}");
        let t = site
            .wrapping_add(5)
            .wrapping_add(u32::from_le_bytes(b[1..5].try_into().unwrap()));
        assert_eq!(t, MOVIE_START, "call at {site:#x}");
    }
    // Wrapper state: the handle word is read by IsPlaying (`mov eax,[ST_BINK]`).
    assert_eq!(
        bytes(MOVIE_IS_PLAYING, 5),
        [&[0xa1u8][..], &ST_BINK.to_le_bytes()].concat()
    );
    // Active byte read by the same function; finished byte set by service.
    assert_eq!(
        &bytes(MOVIE_IS_PLAYING + 9, 5)[..],
        [&[0xa0u8][..], &ST_ACTIVE.to_le_bytes()].concat()
    );
    assert!(
        bytes(MOVIE_SERVICE, 0x100)
            .windows(4)
            .any(|w| w == ST_FINISHED.to_le_bytes())
    );
    assert!(
        bytes(MOVIE_START, 0x500)
            .windows(4)
            .any(|w| w == ST_FLAGS.to_le_bytes())
    );
}
