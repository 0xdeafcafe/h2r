mod common;
use common::*;
use kernel::{
    Config, ConsoleConfig,
    console::{XC_AUDIO, XC_AUDIO_FLAG_AC3, XC_AUDIO_SURROUND},
    net::{LINK_100_MBPS, LINK_FULL_DUPLEX, LINK_UP},
    status,
};

const PHY_GET_LINK_STATE: u32 = 252;
const PHY_INITIALIZE: u32 = 253;

#[test]
fn offline_console_initializes_the_phy_and_reports_no_link() {
    let mut h = Harness::new();
    // Before initialization the PHY has not been probed.
    assert_eq!(h.success(PHY_GET_LINK_STATE, &[0]), 0);
    // The retail call shape: (FALSE, NULL), status checked as signed.
    assert_eq!(h.success(PHY_INITIALIZE, &[0, 0]) as i32, 0);
    for update in [0, 1] {
        assert_eq!(h.success(PHY_GET_LINK_STATE, &[update]), 0);
    }
    // A repeated initialization is harmless.
    assert_eq!(h.success(PHY_INITIALIZE, &[0, 0]), status::SUCCESS);
    assert_eq!(h.success(PHY_GET_LINK_STATE, &[1]), 0);
}

#[test]
fn unobserved_phy_initialize_shapes_stop() {
    let mut h = Harness::new();
    h.stopped(PHY_INITIALIZE, &[0, 0x1234]);
    // A stop leaves the PHY uninitialized.
    assert_eq!(h.success(PHY_GET_LINK_STATE, &[0]), 0);
}

#[test]
fn a_configured_link_is_reported_only_after_initialization_and_only_when_up() {
    let link = LINK_UP | LINK_100_MBPS | LINK_FULL_DUPLEX;
    let mut h = Harness::config(Config {
        console: ConsoleConfig {
            phy_link: link | 0xf0,
            ..ConsoleConfig::default()
        },
        ..Config::default()
    });
    assert_eq!(h.success(PHY_GET_LINK_STATE, &[0]), 0);
    assert_eq!(h.success(PHY_INITIALIZE, &[1, 0]), status::SUCCESS);
    // Bits the title never tests are not reported.
    assert_eq!(h.success(PHY_GET_LINK_STATE, &[0]), link);
    // Speed and duplex without link-up are meaningless: reported as no link.
    let mut h = Harness::config(Config {
        console: ConsoleConfig {
            phy_link: LINK_100_MBPS | LINK_FULL_DUPLEX,
            ..ConsoleConfig::default()
        },
        ..Config::default()
    });
    h.success(PHY_INITIALIZE, &[0, 0]);
    assert_eq!(h.success(PHY_GET_LINK_STATE, &[0]), 0);
}

#[test]
fn phy_link_bits_have_the_documented_literal_values() {
    // Literal masks, independent of the implementation's constants.
    assert_eq!(LINK_UP, 0x01);
    assert_eq!(LINK_100_MBPS, 0x04);
    assert_eq!(LINK_FULL_DUPLEX, 0x08);
    // (configured, reported once initialized)
    for (configured, reported) in [
        (0x00, 0x00),
        (0x01, 0x01),
        (0x05, 0x05),
        (0x09, 0x09),
        (0x0d, 0x0d),
        (0x03, 0x01), // bit 1 is not one the title tests
        (0xf1, 0x01),
        (0xff, 0x0d),
        (0x04, 0x00), // no link-up: no link
        (0x08, 0x00),
        (0x0c, 0x00),
        (0xfe, 0x00),
    ] {
        let mut h = Harness::config(Config {
            console: ConsoleConfig {
                phy_link: configured,
                ..ConsoleConfig::default()
            },
            ..Config::default()
        });
        h.success(PHY_INITIALIZE, &[0, 0]);
        assert_eq!(
            h.success(PHY_GET_LINK_STATE, &[0]),
            reported,
            "configured {configured:#x}"
        );
    }
}

#[test]
fn audio_setting_is_a_configured_dword() {
    let mut h = Harness::new();
    h.bytes(DATA + 16, &[0xcc; 8]);
    assert_eq!(
        h.success(24, &[XC_AUDIO, DATA, DATA + 16, 8, DATA + 4]),
        status::SUCCESS
    );
    assert_eq!(h.get(DATA), 4); // REG_DWORD
    assert_eq!(h.get(DATA + 4), 4);
    assert_eq!(h.get(DATA + 16), 0); // stereo, no passthrough
    assert_eq!(h.get(DATA + 20), 0);
    let mut h = Harness::config(Config {
        console: ConsoleConfig {
            audio_flags: XC_AUDIO_SURROUND | XC_AUDIO_FLAG_AC3,
            ..ConsoleConfig::default()
        },
        ..Config::default()
    });
    assert_eq!(
        h.success(24, &[XC_AUDIO, DATA, DATA + 16, 4, 0]),
        status::SUCCESS
    );
    assert_eq!(h.get(DATA + 16), XC_AUDIO_SURROUND | XC_AUDIO_FLAG_AC3);
}

#[test]
fn compare_memory_ulong_counts_leading_matching_dwords() {
    let mut h = Harness::new();
    for (i, v) in [7u32, 7, 7, 9, 7].into_iter().enumerate() {
        h.set(DATA + i as u32 * 4, v);
    }
    assert_eq!(h.success(269, &[DATA, 20, 7]), 12);
    assert_eq!(h.success(269, &[DATA, 11, 7]), 8); // trailing partial dword
    assert_eq!(h.success(269, &[DATA, 0, 7]), 0);
    assert_eq!(h.success(269, &[DATA, 20, 8]), 0);
    assert_eq!(h.success(269, &[DATA, 12, 7]), 12);
    h.stopped(269, &[DATA + 2, 8, 7]);
}

fn fields(h: &Harness, at: u32, f: [u16; 7]) {
    for (i, v) in f.into_iter().enumerate() {
        h.bytes(at + i as u32 * 2, &v.to_le_bytes());
    }
}
fn read_fields(h: &Harness) -> Vec<u16> {
    (0..8)
        .map(|i| h.runtime.memory().u16(DATA + 160 + i * 2).unwrap())
        .collect()
}
fn time(h: &Harness, at: u32) -> u64 {
    h.get(at) as u64 | (h.get(at + 4) as u64) << 32
}

#[test]
fn time_conversions_round_trip_and_match_known_instants() {
    let mut h = Harness::new();
    // 1601-01-01 00:00:00 is the NT epoch.
    fields(&h, DATA, [1601, 1, 1, 0, 0, 0, 0]);
    assert_eq!(h.success(304, &[DATA, DATA + 64]), 1);
    assert_eq!(time(&h, DATA + 64), 0);
    // 1970-01-01 is 11644473600 s after it.
    fields(&h, DATA, [1970, 1, 1, 0, 0, 0, 0]);
    assert_eq!(h.success(304, &[DATA, DATA + 64]), 1);
    assert_eq!(time(&h, DATA + 64), 116_444_736_000_000_000);
    // 2000-02-29 12:34:56.789 (leap day of a 400-year leap year).
    fields(&h, DATA, [2000, 2, 29, 12, 34, 56, 789]);
    assert_eq!(h.success(304, &[DATA, DATA + 64]), 1);
    let t = time(&h, DATA + 64);
    assert_eq!(
        t,
        125_911_584_000_000_000 + 59 * 864_000_000_000 + 45_296_789 * 10_000
    );
    h.runtime
        .memory()
        .write_bytes(DATA + 128, &t.to_le_bytes(), 1)
        .unwrap();
    h.success(305, &[DATA + 128, DATA + 160]);
    let got = read_fields(&h);
    assert_eq!(got, [2000, 2, 29, 12, 34, 56, 789, 2]); // a Tuesday
    // 1970-01-01 was a Thursday.
    h.runtime
        .memory()
        .write_bytes(DATA + 128, &116_444_736_000_000_000u64.to_le_bytes(), 1)
        .unwrap();
    h.success(305, &[DATA + 128, DATA + 160]);
    let got = read_fields(&h);
    assert_eq!(got, [1970, 1, 1, 0, 0, 0, 0, 4]);
}

#[test]
fn century_leap_rules_are_gregorian() {
    let mut h = Harness::new();
    for (year, leap) in [
        (1900, false),
        (2100, false),
        (2000, true),
        (2400, true),
        (2004, true),
    ] {
        fields(&h, DATA, [year, 2, 29, 0, 0, 0, 0]);
        h.bytes(DATA + 64, &[0xaa; 8]);
        assert_eq!(
            h.success(304, &[DATA, DATA + 64]),
            leap as u32,
            "{year}-02-29"
        );
        if !leap {
            assert_eq!(h.get(DATA + 64), 0xaaaa_aaaa);
            assert_eq!(h.get(DATA + 68), 0xaaaa_aaaa);
        }
    }
}

#[test]
fn invalid_time_fields_fail_without_writing() {
    let mut h = Harness::new();
    h.bytes(DATA + 64, &[0xaa; 8]);
    for bad in [
        [1600, 1, 1, 0, 0, 0, 0],
        [2001, 2, 29, 0, 0, 0, 0],
        [2001, 13, 1, 0, 0, 0, 0],
        [2001, 0, 1, 0, 0, 0, 0],
        [2001, 4, 31, 0, 0, 0, 0],
        [2001, 1, 1, 24, 0, 0, 0],
        [2001, 1, 1, 0, 60, 0, 0],
        [2001, 1, 1, 0, 0, 60, 0],
        [2001, 1, 1, 0, 0, 0, 1000],
        [2001, 1, 0, 0, 0, 0, 0],
        // Negative SHORTs (0xffff is -1) are not wrapped into range.
        [-1i16 as u16, 1, 1, 0, 0, 0, 0],
        [2001, -1i16 as u16, 1, 0, 0, 0, 0],
        [2001, 1, -1i16 as u16, 0, 0, 0, 0],
        [2001, 1, 1, -1i16 as u16, 0, 0, 0],
        [2001, 1, 1, 0, -1i16 as u16, 0, 0],
        [2001, 1, 1, 0, 0, -1i16 as u16, 0],
        [2001, 1, 1, 0, 0, 0, -1i16 as u16],
    ] {
        fields(&h, DATA, bad);
        assert_eq!(h.success(304, &[DATA, DATA + 64]), 0, "{bad:?}");
        assert_eq!(h.get(DATA + 64), 0xaaaa_aaaa, "{bad:?}");
        assert_eq!(h.get(DATA + 68), 0xaaaa_aaaa, "{bad:?}");
    }
}

/// Decoded-instruction check of the retail call sites behind the PHY
/// contract documented in net.rs. Reads the image at runtime; nothing from
/// it is stored here.
#[test]
#[ignore = "requires the user-supplied retail XBE"]
fn retail_phy_call_sites_have_the_documented_shapes() {
    use iced_x86::{Decoder, DecoderOptions, Mnemonic, OpKind, Register};
    let path = std::env::var("H2_XBE").unwrap_or_else(|_| "../../../disc/default.xbe".into());
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let xbe = xbe::Xbe::parse(&data).unwrap();
    let imports = xbe.kernel_imports().unwrap();
    let ordinal_of = |slot: u32| imports.iter().find(|i| i.slot_va == slot).unwrap().ordinal;
    let decode = |from: u32, to: u32| {
        let bytes = xbe.bytes_at(from, (to - from) as usize).unwrap();
        let mut d = Decoder::with_ip(32, bytes, from as u64, DecoderOptions::NONE);
        d.iter().collect::<Vec<_>>()
    };
    // (call site, ordinal, argument words pushed immediately before it)
    for (site, ordinal, words) in [
        (0x36c4a1u32, 253, 2),
        (0x36c4a8, 252, 1),
        (0x3d6a7d, 253, 2),
        (0x3d5f6b, 252, 1),
        (0x3dabc6, 252, 1),
    ] {
        // One-byte pushes, so the words decode exactly from site - words.
        let insns = decode(site - words, site + 6);
        let (pushes, call) = insns.split_at(words as usize);
        assert!(pushes.iter().all(|i| i.mnemonic() == Mnemonic::Push));
        let call = &call[0];
        assert_eq!(call.ip(), site as u64);
        assert_eq!(call.mnemonic(), Mnemonic::Call);
        assert_eq!(call.op0_kind(), OpKind::Memory);
        assert_eq!(ordinal_of(call.memory_displacement32()), ordinal);
    }
    // 0x3d6a83: the 253 result is compared and a signed-less branch follows.
    let insns = decode(0x3d6a83, 0x3d6a87);
    assert_eq!(insns[0].mnemonic(), Mnemonic::Cmp);
    assert_eq!(insns[1].mnemonic(), Mnemonic::Jl);
    // 0x3dabcc: the 252 result's bit 0 is tested directly.
    let t = &decode(0x3dabcc, 0x3dabce)[0];
    assert_eq!(t.mnemonic(), Mnemonic::Test);
    assert_eq!((t.op0_register(), t.immediate8()), (Register::AL, 1));
    // 0x3d5f88 / 0x3d5f96: bit 2 and bit 3 of the mask.
    let a = &decode(0x3d5f88, 0x3d5f8a)[0];
    assert_eq!((a.op0_register(), a.immediate8()), (Register::AL, 4));
    let b = &decode(0x3d5f96, 0x3d5f99)[0];
    assert_eq!((b.op0_register(), b.immediate8()), (Register::BL, 8));
}
