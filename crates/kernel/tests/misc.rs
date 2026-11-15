mod common;
use common::*;
use kernel::{clock::Clock, handles::HandleTable, status};
use runtime::Event;

#[test]
fn fastcall_atomics_wrap_and_compare_under_shared_alias_identity() {
    let mut h = Harness::new();
    let physical = h
        .runtime
        .memory()
        .mappings()
        .iter()
        .find(|m| m.virtual_start == DATA)
        .unwrap()
        .physical_start;
    h.runtime
        .memory()
        .map(0x20000000, 4096, physical, runtime::guest::Permissions::RW)
        .unwrap();
    h.set(DATA, u32::MAX);
    assert_eq!(h.success(53, &[0x20000000]), 0);
    assert_eq!(h.get(DATA), 0);
    assert_eq!(h.success(52, &[DATA]), u32::MAX);
    assert_eq!(h.success(54, &[DATA, 99]), u32::MAX);
    assert_eq!(h.get(DATA), 99);
    assert_eq!(h.success(55, &[DATA, 3]), 99);
    assert_eq!(h.get(DATA), 102);
    assert_eq!(h.success(51, &[DATA, 7, 102]), 102);
    assert_eq!(h.get(DATA), 7);
    assert_eq!(h.success(51, &[DATA, 8, 102]), 7);
    assert_eq!(h.get(DATA), 7);
    let before = h.get(DATA);
    let t = h.call(51, &[0xdead0000, 8, 7]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(h.get(DATA), before);
    assert_eq!(h.cpu.esp, STACK_TOP - 0x100);
}

#[test]
fn av_pointer_round_trip_is_guest_storage() {
    let mut h = Harness::new();
    assert_eq!(h.success(1, &[]), 0);
    assert_eq!(h.success(4, &[DATA]), 0xfeedface);
    assert_eq!(h.success(1, &[]), DATA);
}

#[test]
fn deterministic_clock_replays_wraps_ticks_and_returns_edx_eax() {
    let mut config = kernel::Config {
        epoch_100ns: 123456789,
        ..kernel::Config::default()
    };
    config.data_exports.insert(156, DATA);
    let mut h = Harness::config(config);
    let ticks = (u32::MAX as u64 + 2) * 10_000;
    h.kernel.advance_time(&h.runtime, ticks).unwrap();
    assert_eq!(h.get(DATA), 1);
    h.success(126, &[]);
    assert_eq!((h.cpu.edx as u64) << 32 | h.cpu.eax as u64, ticks);
    h.success(127, &[]);
    assert_eq!(
        (h.cpu.edx as u64) << 32 | h.cpu.eax as u64,
        Clock::FREQUENCY
    );
    h.success(128, &[DATA + 8]);
    assert_eq!(h.runtime.memory().u64(DATA + 8).unwrap(), ticks + 123456789);
    let snapshot = h.kernel.clock();
    assert_eq!(
        Clock::replay(123456789, snapshot.advances()).unwrap(),
        snapshot
    );
    assert!(Clock::replay(0, &[9, 8]).is_err());
    assert!(Clock::new(u64::MAX).advance(1).is_err());
    let c = Clock::new(500);
    assert_eq!(c.deadline(-10).unwrap(), 10);
    assert_eq!(c.deadline(510).unwrap(), 10);
    assert_eq!(c.deadline(499).unwrap(), 0);
}

#[test]
fn handles_retain_identity_across_duplication_and_reject_stale_values() {
    let mut handles = HandleTable::default();
    let a = handles.insert(0x12340000).unwrap();
    let b = handles.duplicate(a).unwrap();
    assert_ne!(a, b);
    assert_eq!(handles.get(b), Ok(0x12340000));
    assert_eq!(handles.close(a), Ok(0x12340000));
    assert_eq!(handles.get(a), Err(status::INVALID_HANDLE));
    assert!(handles.insert(42).unwrap() > b);
    assert!(handles.duplicate(a).is_err());
    assert_eq!(handles.len(), 2);
    assert!(!status::nt_success(status::INVALID_HANDLE));
    assert!(status::nt_success(status::TIMEOUT));
}

#[test]
fn debug_print_formats_guest_varargs_and_stops_before_partial_logging() {
    let mut h = Harness::new();
    h.bytes(DATA, b"[%s] %+06d %#x %08X %p %% %.3s %c\n\0");
    h.bytes(DATA + 0x100, b"halo\0");
    assert_eq!(
        h.success(
            8,
            &[
                DATA,
                DATA + 0x100,
                (-3i32) as u32,
                42,
                7,
                0x1234,
                DATA + 0x100,
                b'!' as u32
            ]
        ),
        0
    );
    assert_eq!(
        h.kernel.log(),
        vec!["[halo] -00003 0x2a 00000007 00001234 % hal !\n"]
    );
    h.bytes(DATA, b"prefix %f\0");
    h.stopped(8, &[DATA, 0]);
    assert_eq!(h.kernel.log().len(), 1);
    h.bytes(DATA, b"%s\0");
    let t = h.call(8, &[DATA, 0xdead0000]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(h.kernel.log().len(), 1);
    h.bytes(DATA, b"%*.*s %ld %#o\0");
    h.bytes(DATA + 0x100, b"abcde\0");
    h.success(8, &[DATA, (-5i32) as u32, 2, DATA + 0x100, 42, 8]);
    assert_eq!(h.kernel.log()[1], "ab    42 010");
}

#[test]
fn counted_string_exports_initialize_convert_compare_and_free() {
    let mut h = Harness::new();
    h.bytes(DATA + 0x100, b"Halo\0");
    h.success(289, &[DATA, DATA + 0x100]);
    assert_eq!(h.get(DATA), 0x00050004);
    assert_eq!(h.get(DATA + 4), DATA + 0x100);
    h.success(289, &[DATA + 8, 0]);
    assert_eq!(h.get(DATA + 8), 0);
    assert_eq!(h.get(DATA + 12), 0);
    h.bytes(DATA + 0x110, b"halo\0");
    h.success(289, &[DATA + 8, DATA + 0x110]);
    assert_eq!(h.success(279, &[DATA, DATA + 8, 0]), 0);
    assert_eq!(h.success(279, &[DATA, DATA + 8, 1]), 1);
    assert_eq!(h.success(260, &[DATA + 16, DATA, 1]), 0);
    let unicode = h.get(DATA + 20);
    assert_eq!(h.get(DATA + 16), 0x000a0008);
    assert_eq!(
        h.runtime.memory().read::<10>(unicode).unwrap(),
        [b'H', 0, b'a', 0, b'l', 0, b'o', 0, 0, 0]
    );
    h.success(290, &[DATA + 24, unicode]);
    assert_eq!(h.get(DATA + 24), 0x000a0008);
    assert_eq!(h.success(308, &[DATA + 32, DATA + 16, 1]), 0);
    assert_eq!(h.get(DATA + 32), 0x00050004);
    assert_eq!(h.success(279, &[DATA, DATA + 32, 0]), 1);
    h.success(286, &[DATA + 32]);
    h.success(287, &[DATA + 16]);
    assert_eq!(h.get(DATA + 32), 0);
    assert_eq!(h.get(DATA + 36), 0);
    h.stopped(286, &[DATA]); // Descriptor points at caller storage, not an allocated string.
    h.set(DATA + 40, 0x00010000);
    h.set(DATA + 44, DATA + 0x200);
    h.stopped(260, &[DATA + 40, DATA, 0]);
    h.bytes(DATA + 0x100, &[0xe9, 0]);
    h.success(289, &[DATA, DATA + 0x100]);
    h.stopped(260, &[DATA + 16, DATA, 1]);
    let t = h.call(289, &[DATA, 0xdead0000]);
    assert!(matches!(
        h.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
}

#[test]
fn irql_exports_preserve_low_byte_fastcall_and_validate_transitions() {
    let mut h = Harness::new();
    assert_eq!(h.success(103, &[]), 0);
    assert_eq!(h.success(160, &[0x1002]), 0);
    assert_eq!(h.success(103, &[]), 2);
    assert_eq!(h.success(129, &[]), 2);
    h.stopped(160, &[1]);
    assert_eq!(h.success(103, &[]), 2);
    h.stopped(161, &[3]);
    assert_eq!(h.success(103, &[]), 2);
    assert_eq!(h.success(161, &[0]), 0xfeedface);
    assert_eq!(h.success(129, &[]), 0);
    h.success(161, &[0]);
}

#[test]
fn dbgprint_log_and_stop_history_have_lifetime_budgets() {
    let mut h = Harness::new();
    h.bytes(DATA + 0x100, b"line %d\0");
    let total = kernel::limits::MAX_LOG_MESSAGES + 10;
    for i in 0..total as u32 {
        assert_eq!(h.success(8, &[DATA + 0x100, i]), 0);
    }
    let log = h.kernel.log_bytes();
    assert!(log.len() <= kernel::limits::MAX_LOG_MESSAGES);
    assert!(log.iter().map(Vec::len).sum::<usize>() <= kernel::limits::MAX_LOG_BYTES);
    assert_eq!(h.kernel.dropped_log_messages() as usize + log.len(), total);
    assert_eq!(
        log.last().unwrap(),
        format!("line {}", total - 1).as_bytes()
    );

    let stops = kernel::limits::MAX_STOP_REASONS + 5;
    for _ in 0..stops {
        h.stopped(171, &[DATA]);
    }
    let reasons = h.kernel.stop_reasons();
    assert!(reasons.len() <= kernel::limits::MAX_STOP_REASONS);
    assert_eq!(
        h.kernel.dropped_stop_reasons() as usize + reasons.len(),
        stops
    );
}

#[test]
fn handle_table_has_a_live_handle_budget() {
    let mut table = HandleTable::default();
    for _ in 0..kernel::limits::MAX_HANDLES {
        table.insert(0x1000).unwrap();
    }
    assert_eq!(table.insert(0x1000), Err(status::INSUFFICIENT_RESOURCES));
    assert_eq!(table.duplicate(4), Err(status::INSUFFICIENT_RESOURCES));
    table.close(4).unwrap();
    assert!(table.insert(0x1000).is_ok());
}

/// XcSHAInit/Update/Final through the guest ABI: the context is the caller's
/// 116-byte buffer, updates may be split anywhere, and Final writes 20 bytes.
#[test]
fn xc_sha_hashes_split_guest_updates_into_a_fips_digest() {
    let mut h = Harness::new();
    const CTX: u32 = DATA + 0x100;
    const INPUT: u32 = DATA + 0x200;
    const DIGEST: u32 = DATA + 0x300;
    assert_eq!(kernel::crypto::SHA_CONTEXT_SIZE, 116);
    h.bytes(CTX, &[0xcc; 116]);
    h.success(335, &[CTX]);
    // Bytes past the state are the caller's.
    assert_eq!(h.get(CTX + 112), 0xcccccccc);
    h.bytes(INPUT, b"abc");
    h.success(336, &[CTX, INPUT, 1]);
    h.success(336, &[CTX, INPUT + 1, 0]);
    h.success(336, &[CTX, INPUT + 1, 2]);
    h.bytes(DIGEST, &[0; 24]);
    h.success(337, &[CTX, DIGEST]);
    let mut digest = [0u8; 24];
    h.runtime
        .memory()
        .read_bytes(DIGEST, &mut digest, 1)
        .unwrap();
    let hex: String = digest[..20].iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, "a9993e364706816aba3e25717850c26c9cd0d89d");
    assert_eq!(&digest[20..], &[0; 4]);
    // A copied context continues independently (callers memcpy them).
    let long: Vec<u8> = (0..10_000u32).map(|i| i as u8).collect();
    h.bytes(INPUT, &long);
    h.success(335, &[CTX]);
    h.success(336, &[CTX, INPUT, 10_000]);
    let mut host = kernel::crypto::Sha1::new();
    host.update(&long);
    h.success(337, &[CTX, DIGEST]);
    h.runtime
        .memory()
        .read_bytes(DIGEST, &mut digest, 1)
        .unwrap();
    assert_eq!(&digest[..20], &host.finish());
    // An uninitialised context stops; unmapped spans fault before any write.
    h.bytes(CTX, &[0xff; 116]);
    h.stopped(336, &[CTX, INPUT, 1]);
    h.stopped(337, &[CTX, DIGEST]);
    h.success(335, &[CTX]);
    let before = h.get(CTX);
    assert_eq!(
        h.call(336, &[CTX, 0xdead_0000, 4]).kind,
        runtime::abi::FAULT
    );
    assert_eq!(h.call(337, &[CTX, 0xdead_0000]).kind, runtime::abi::FAULT);
    assert_eq!(h.get(CTX), before);
}

/// XNetGetEthernetLinkStatus (0x36c47d): PhyInitialize(0, 0) then
/// PhyGetLinkState(0). No network device is modelled: no link.
#[test]
fn phy_initializes_and_reports_no_link() {
    let mut h = Harness::new();
    assert_eq!(h.success(253, &[0, 0]), status::SUCCESS);
    assert_eq!(h.success(252, &[0]), 0);
}

fn read(h: &Harness, address: u32, n: usize) -> Vec<u8> {
    let mut out = vec![0u8; n];
    h.runtime.memory().read_bytes(address, &mut out, 1).unwrap();
    out
}

/// XcHMAC through the guest ABI: HMAC-SHA1 of input || input2 (RFC 2202
/// case 2, split every way), a NULL second input of length 0 as XAPI passes
/// it, and stops/faults before any write on unsupported or unmapped spans.
#[test]
fn xc_hmac_macs_both_guest_spans() {
    let mut h = Harness::new();
    const KEY: u32 = DATA + 0x100;
    const INPUT: u32 = DATA + 0x200;
    const DIGEST: u32 = DATA + 0x300;
    let message = b"what do ya want for nothing?";
    let want = "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79";
    h.bytes(KEY, b"Jefe");
    h.bytes(INPUT, message);
    let hex = |d: &[u8]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
    for split in [0, 1, 4, 27, 28] {
        h.bytes(DIGEST, &[0xee; 24]);
        let rest = 28 - split;
        let second = if rest == 0 { 0 } else { INPUT + split };
        h.success(340, &[KEY, 4, INPUT, split, second, rest, DIGEST]);
        assert_eq!(hex(&read(&h, DIGEST, 20)), want, "split {split}");
        assert_eq!(read(&h, DIGEST + 20, 4), [0xee; 4]);
    }
    assert_eq!(
        kernel::crypto::hmac_sha1(b"Jefe", &[message]),
        read(&h, DIGEST, 20)[..]
    );
    h.bytes(DIGEST, &[0xee; 20]);
    h.stopped(340, &[KEY, 65, INPUT, 28, 0, 0, DIGEST]);
    assert_eq!(
        h.call(340, &[KEY, 4, 0xdead_0000, 4, 0, 0, DIGEST]).kind,
        runtime::abi::FAULT
    );
    assert_eq!(
        h.call(340, &[KEY, 4, INPUT, 4, 0, 0, 0xdead_0000]).kind,
        runtime::abi::FAULT
    );
    assert_eq!(read(&h, DIGEST, 20), [0xee; 20]);
}

/// The kernel fills XboxHDKey (323 DATA) with the key the content signer
/// uses, so imported content and the guest agree on it.
#[test]
fn xbox_hd_key_export_holds_the_console_key() {
    let h = Harness::config(kernel::Config {
        data_exports: [(323, DATA + 0x800)].into(),
        ..kernel::Config::default()
    });
    h.bytes(DATA + 0x810, &[0xcc; 4]);
    assert_eq!(read(&h, DATA + 0x800, 16), kernel::crypto::XBOX_HD_KEY);
    assert_eq!(read(&h, DATA + 0x810, 4), [0xcc; 4]);
}

/// A ContentMeta.xbx signed by `kernel::content::sign` passes the check
/// Halo 2's XAPI makes (0x2d2938, from its readers 0x2d2a1d and 0x2d2f6d),
/// done here the guest's way through the kernel exports: the per-title key
/// from XcHMAC over XboxHDKey as the guest reads it from its DATA export,
/// then HMAC-SHA1 of header bytes 0x14..header size built on XcSHA* with
/// the 0x36/0x5c pads, as XAPI's own HMAC does. A disc's unsigned header,
/// or one signed for another title, fails it.
#[test]
fn signed_content_meta_passes_the_guest_signature_check() {
    const HD_KEY: u32 = DATA + 0x800;
    const TITLE: u32 = DATA + 0x820;
    const KEY: u32 = DATA + 0x840;
    const CTX: u32 = DATA + 0x880;
    const PAD: u32 = DATA + 0x900;
    const META: u32 = DATA + 0x1000;
    const INNER: u32 = DATA + 0x980;
    const MAC: u32 = DATA + 0x9a0;
    let mut h = Harness::config(kernel::Config {
        data_exports: [(323, HD_KEY)].into(),
        ..kernel::Config::default()
    });
    let title = 0x4d53_0064u32;

    // Synthetic: magic, header size 0x88, title, then a body of names and a
    // file table that the header's own SHA-1 digests would cover.
    let mut meta = vec![0u8; 0x200];
    meta[0x14..0x18].copy_from_slice(b"XCSF");
    meta[0x18..0x1c].copy_from_slice(&0x88u32.to_le_bytes());
    meta[0x1c..0x20].copy_from_slice(&1u32.to_le_bytes());
    meta[0x24..0x28].copy_from_slice(&title.to_le_bytes());
    meta[0x28..0x30].copy_from_slice(&((title as u64) << 32 | 1).to_le_bytes());
    for (i, b) in meta[0x30..].iter_mut().enumerate() {
        *b = (i * 13) as u8;
    }
    meta[..20].fill(0);
    meta[0x20..0x24].fill(0);
    let unsigned = meta.clone();

    let guest_check = |h: &mut Harness, meta: &[u8], title: u32| -> bool {
        h.bytes(META, meta);
        h.set(TITLE, title);
        // XAPI 0x2d2919: XcHMAC(XboxHDKey, 16, &title, 4, NULL, 0, key).
        h.success(340, &[HD_KEY, 16, TITLE, 4, 0, 0, KEY]);
        let key = read(h, KEY, 20);
        let size = h.get(META + 0x18).min(0x88);
        let mut pad = [0u8; 64];
        pad[..20].copy_from_slice(&key);
        // Inner: SHA-1(key ^ ipad || header[0x14..size]).
        h.bytes(PAD, &pad.map(|b| b ^ 0x36));
        h.success(335, &[CTX]);
        h.success(336, &[CTX, PAD, 64]);
        h.success(336, &[CTX, META + 0x14, size - 0x14]);
        h.success(337, &[CTX, INNER]);
        // Outer: SHA-1(key ^ opad || inner).
        h.bytes(PAD, &pad.map(|b| b ^ 0x5c));
        h.success(335, &[CTX]);
        h.success(336, &[CTX, PAD, 64]);
        h.success(336, &[CTX, INNER, 20]);
        h.success(337, &[CTX, MAC]);
        let magic = read(h, META + 0x14, 4) == b"XCSF";
        let installed = h.get(META + 0x20) & 1 == 1;
        magic && installed && read(h, MAC, 20) == read(h, META, 20)
    };

    assert!(!guest_check(&mut h, &unsigned, title));
    let mut signed = unsigned.clone();
    kernel::content::sign(&mut signed, title).unwrap();
    assert!(guest_check(&mut h, &signed, title));
    // Only the signature and the installed flag changed.
    let changed: Vec<usize> = (0..signed.len())
        .filter(|&i| signed[i] != unsigned[i])
        .collect();
    assert!(changed.iter().all(|&i| i < 20 || i == 0x20), "{changed:x?}");
    // Another title's key (as another title's XAPI would derive) or a
    // changed header byte: rejected.
    assert!(!guest_check(&mut h, &signed, title + 1));
    let mut tampered = signed.clone();
    tampered[0x40] ^= 1;
    assert!(!guest_check(&mut h, &tampered, title));
    // A good signature without the installed flag (the readers' test).
    tampered = signed.clone();
    tampered[0x20] &= !1;
    let mac = kernel::content::signature(&tampered, title).unwrap();
    tampered[..20].copy_from_slice(&mac);
    assert!(!guest_check(&mut h, &tampered, title));
}
