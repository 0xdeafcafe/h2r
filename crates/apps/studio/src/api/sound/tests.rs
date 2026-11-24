//! `sound.*` over the scenario crate's invented cache, given an invented
//! `snd!` and `ugh!` and 600 invented ADPCM blocks (no game data).
use std::path::PathBuf;

use map::FourCC;
use scenario_edit::fixture::{self, INDEX, META, META_FILE, put};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use crate::api::{Api, Reply};
use crate::app::tag_tests;

const BLOCKS: usize = 600;
const DATA_AT: usize = 0x3300;
const SND: &str = r"snd!:objects\test\blaster";
const SIZE: u32 = BLOCKS as u32 * 36;

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-sound-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn retag(b: &mut [u8], slot: usize, class: &[u8; 4]) {
    let groups = INDEX + 32;
    let tags = groups + 4 * 12;
    put(b, groups + slot * 12, FourCC(*class).as_u32());
    put(b, tags + slot * 16, FourCC(*class).as_u32());
}

/// The fixture with its weapon tag made a `snd!` of mono 22.05 kHz Xbox
/// ADPCM (600 blocks, 38,400 frames, one range, one permutation, one
/// chunk) and its collection tag the `ugh!` gestalt.
pub(crate) fn sound_map() -> Vec<u8> {
    let mut b = fixture::fixture();
    b.resize(0x9000, 0);
    put(&mut b, 8, 0x9000);
    retag(&mut b, 2, b"snd!");
    retag(&mut b, 3, b"ugh!");
    let tags = INDEX + 32 + 4 * 12;
    put(&mut b, tags + 2 * 16 + 12, 0x14);
    put(&mut b, tags + 3 * 16 + 12, 0x200);
    let h = META_FILE + 0x1100;
    b[h + 5] = 1; // Xbox ADPCM; rate 0 is 22.05 kHz and encoding 0 is mono
    b[h + 10] = 1; // one pitch range, the first
    b[h + 0xe..h + 0x10].copy_from_slice(&u16::MAX.to_le_bytes()); // no extra info
    put(&mut b, h + 0x10, 1741);
    let g = META_FILE + 0x1200;
    let base = META + 0x1200;
    for (idx, at) in [(4usize, 0x58u32), (5, 0x64), (8, 0x74)] {
        put(&mut b, g + idx * 8, 1);
        put(&mut b, g + idx * 8 + 4, base + at);
    }
    b[g + 0x58 + 10] = 1; // the range has one permutation
    put(&mut b, g + 0x64 + 8, SIZE);
    b[g + 0x64 + 14] = 1; // the permutation has one chunk
    put(&mut b, g + 0x74, DATA_AT as u32);
    put(&mut b, g + 0x74 + 4, SIZE);
    put(&mut b, g + 0x74 + 8, u32::MAX);
    for k in 0..BLOCKS {
        let at = DATA_AT + k * 36;
        b[at..at + 2].copy_from_slice(&((k as i16) * 10).to_le_bytes());
    }
    b
}

/// The ticks of permutation 0's mouth curve, rising from 0 to 255 over the
/// sound's 1,741 ms (52 ticks at 30 Hz).
const CURVE: usize = 52;
/// Where the extra-info resource sits in the map's raw data.
const RESOURCE_AT: usize = 0x8000;

/// The sound map with two permutations and one extra info: permutation 0
/// is voiced (a 52-tick curve), permutation 1 unvoiced (a zero-length
/// entry). Everything is invented, in the retail extra-info layout. The
/// permutations block grows to two elements, so the chunk moves from
/// `+0x74` to `+0x90` and the extra info follows it.
pub(crate) fn mouth_map() -> Vec<u8> {
    let mut b = sound_map();
    let h = META_FILE + 0x1100;
    b[h + 0xe..h + 0x10].copy_from_slice(&0u16.to_le_bytes()); // the extra-info index
    let g = META_FILE + 0x1200;
    let base = META + 0x1200;
    b[g + 0x58 + 10] = 2; // the range has two permutations
    put(&mut b, g + 5 * 8, 2); // the permutations block has two elements
    // The chunks block moves out from under the second permutation.
    put(&mut b, g + 8 * 8 + 4, base + 0x90);
    put(&mut b, g + 0x90, DATA_AT as u32);
    put(&mut b, g + 0x90 + 4, SIZE);
    put(&mut b, g + 0x90 + 8, u32::MAX);
    // The range's second permutation: no chunks, no bytes, unvoiced.
    put(&mut b, g + 0x64 + 16 + 8, 0);
    b[g + 0x64 + 16 + 14] = 0;
    b[g + 0x64 + 5] = 0; // permutation 0's descriptor
    b[g + 0x64 + 16 + 5] = 1; // permutation 1's descriptor

    // The extra-info resource, laid out as the converter writes it.
    let curve: Vec<u8> = (0..CURVE).map(|i| (i * 255 / (CURVE - 1)) as u8).collect();
    let data_len = curve.len();
    let block_start = (4 + data_len).next_multiple_of(4) + 4;
    let descriptors = 2 * 16;
    let resource_size = block_start + descriptors;
    let mut raw = b"hklb".to_vec();
    for value in [resource_size as u32, data_len as u32, 0, 2, 0] {
        raw.extend_from_slice(&value.to_le_bytes());
    }
    raw.extend_from_slice(b"crsr");
    raw.extend_from_slice(&curve);
    raw.resize(24 + block_start - 4, 0);
    raw.extend_from_slice(b"crsr");
    // Descriptor 0: permutation 0's curve, at 0 for its length. Descriptor
    // 1: permutation 1's zero-length entry, at the end of the data.
    for (at, len) in [(0u32, data_len as u32), (data_len as u32, 0)] {
        raw.extend_from_slice(&at.to_le_bytes());
        raw.extend_from_slice(&len.to_le_bytes());
        raw.extend_from_slice(&[0; 8]); // no facial sequence
    }
    raw.extend_from_slice(b"fklb");
    b[RESOURCE_AT..RESOURCE_AT + raw.len()].copy_from_slice(&raw);

    // The gestalt's extra-info element and its locators.
    let extra_at = 0xa0;
    let locators_at = extra_at + 0x2c;
    put(&mut b, g + 10 * 8, 1);
    put(&mut b, g + 10 * 8 + 4, base + extra_at as u32);
    let e = g + extra_at;
    put(&mut b, e + 8, RESOURCE_AT as u32); // the resource pointer, local
    put(&mut b, e + 0xc, raw.len() as u32);
    put(&mut b, e + 0x10, 16);
    put(&mut b, e + 0x14, resource_size as u32);
    put(&mut b, e + 0x18, 2); // two locators
    put(&mut b, e + 0x1c, base + locators_at as u32);
    put(&mut b, e + 0x24, 0x0100_fff8);
    put(&mut b, e + 0x28, u32::MAX);
    let l = g + locators_at;
    b[l..l + 8].copy_from_slice(&[1, 2, 0, 0, 0, 0, 0xff, 0xff]);
    put(&mut b, l + 8, data_len as u32);
    put(&mut b, l + 0xc, 4);
    b[l + 16..l + 24].copy_from_slice(&[0, 2, 8, 0, 8, 0, 16, 0]);
    put(&mut b, l + 16 + 8, descriptors as u32);
    put(&mut b, l + 16 + 0xc, block_start as u32);
    b
}

fn open(t: &Temp) -> (crate::App, Api) {
    let (mut app, ..) = tag_tests::app_with_registry(&t.0, sound_map(), tag_tests::registry());
    app.audition.headless = true;
    (app, Api::new())
}

fn open_mouth(t: &Temp) -> (crate::App, Api) {
    let (mut app, ..) = tag_tests::app_with_registry(&t.0, mouth_map(), tag_tests::registry());
    app.audition.headless = true;
    (app, Api::new())
}

fn call(api: &mut Api, app: &mut crate::App, method: &str, params: Value) -> Value {
    match api.call(app, method, params.clone()) {
        Ok(v) => v,
        Err(e) => panic!("{method} {params}: {e}"),
    }
}

fn code_of(r: Reply) -> i64 {
    match r {
        Ok(v) => panic!("expected an error, got {v}"),
        Err(e) => e.code,
    }
}

#[test]
fn the_methods_are_listed() {
    let t = Temp::new("list");
    let (mut app, mut api) = open(&t);
    let list = call(&mut api, &mut app, m::METHODS, json!({}));
    for name in [m::SOUND_READ, m::SOUND_PLAY, m::SOUND_STOP, m::SOUND_MOUTH] {
        let row = list
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap_or_else(|| panic!("{name} is not listed"));
        assert_eq!(row["status"], "ready");
    }
}

#[test]
fn sound_read_lists_the_permutations_and_the_waveform() {
    let t = Temp::new("read");
    let (mut app, mut api) = open(&t);
    let r = call(
        &mut api,
        &mut app,
        m::SOUND_READ,
        json!({"tag": SND, "bins": 16}),
    );
    assert_eq!(r["sample_rate"], 22050);
    assert_eq!(r["compression"], "XboxAdpcm");
    assert_eq!(r["max_play_time_ms"], 1741);
    assert_eq!(
        r["ranges"],
        json!([{"permutations": [{"encoded_bytes": SIZE, "chunks": 1}]}])
    );
    let s = &r["selected"];
    assert_eq!(s["frames"], BLOCKS * 64);
    assert_eq!(
        (s["rate"].as_u64(), s["channels"].as_u64()),
        (Some(22050), Some(1))
    );
    assert_eq!(s["duration_ms"], (BLOCKS * 64 * 1000 / 22050) as u64);
    let peaks = s["peaks"].as_array().unwrap();
    assert_eq!(peaks.len(), 16);
    // Block k holds the constant k * 10: the peaks rise with the clip.
    assert_eq!(peaks[0][0], 0);
    assert_eq!(peaks[15][1], 5990);
    assert_eq!(r["playback"]["playing"], false);
    // Nothing played, no output opened.
    assert!(r["playback"]["output"].is_null());

    // A permutation that is not there is named, and bad bins refuse.
    let r = call(
        &mut api,
        &mut app,
        m::SOUND_READ,
        json!({"tag": SND, "permutation": 4}),
    );
    assert!(
        r["selected"]["error"]
            .as_str()
            .unwrap()
            .contains("no permutation 4")
    );
    assert_eq!(
        code_of(api.call(&mut app, m::SOUND_READ, json!({"tag": SND, "bins": 0}))),
        code::INVALID_PARAMS
    );
}

#[test]
fn sound_read_refuses_a_tag_that_is_not_a_sound() {
    let t = Temp::new("notsound");
    let (mut app, mut api) = open(&t);
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::SOUND_READ,
            json!({"tag": "ugh!:objects\\test\\kit"})
        )),
        code::REFUSED
    );
}

#[test]
fn sound_play_plays_to_the_headless_output_and_stops() {
    let t = Temp::new("play");
    let (mut app, mut api) = open(&t);
    let r = call(
        &mut api,
        &mut app,
        m::SOUND_PLAY,
        json!({"tag": SND, "from_ms": 100}),
    );
    assert_eq!(r["playing"], true);
    assert_eq!(r["output"], "headless");
    assert_eq!(r["frames"], BLOCKS * 64);
    assert_eq!(r["from_ms"], 100);
    let r = call(&mut api, &mut app, m::SOUND_READ, json!({"tag": SND}));
    assert_eq!(r["playback"]["playing"], true);
    assert_eq!(r["playback"]["range"], 0);
    assert_eq!(r["playback"]["output"], "headless");
    assert!(r["playback"]["position_ms"].as_u64().unwrap() >= 100);
    let r = call(&mut api, &mut app, m::SOUND_STOP, json!({}));
    assert_eq!(r["stopped"], true);
    let r = call(&mut api, &mut app, m::SOUND_STOP, json!({}));
    assert_eq!(r["stopped"], false);
}

#[test]
fn sound_mouth_reads_one_curve_and_one_null() {
    let t = Temp::new("mouth");
    let (mut app, mut api) = open_mouth(&t);
    // Permutation 0 is voiced: 52 ticks at 30 Hz, rising 0 to 1.
    let r = call(&mut api, &mut app, m::SOUND_MOUTH, json!({"tag": SND}));
    assert_eq!(r["ticks"], CURVE);
    let curve = r["curve"].as_array().unwrap();
    assert_eq!(curve.len(), CURVE);
    assert_eq!(curve[0], 0.0);
    assert_eq!(curve[CURVE - 1], 1.0);
    // Permutation 1 is unvoiced: a zero-length mouth entry.
    let r = call(
        &mut api,
        &mut app,
        m::SOUND_MOUTH,
        json!({"tag": SND, "permutation": 1}),
    );
    assert_eq!(r["ticks"], 0);
    assert!(r["curve"].is_null());
    // sound.read carries the same mouth on its selected permutation.
    let r = call(&mut api, &mut app, m::SOUND_READ, json!({"tag": SND}));
    assert_eq!(r["selected"]["mouth"]["ticks"], CURVE);
    assert_eq!(r["selected"]["mouth"]["curve"][0], 0.0);
    assert_eq!(r["selected"]["mouth"]["curve"][CURVE - 1], 1.0);
    let r = call(
        &mut api,
        &mut app,
        m::SOUND_READ,
        json!({"tag": SND, "permutation": 1}),
    );
    assert!(r["selected"]["mouth"]["curve"].is_null());
    // The curve is read even where the audio cannot be: the codec-stream
    // map, without the executable.
    if std::env::var_os("H2_XBE").is_none() {
        let t = Temp::new("mouth-wma");
        let mut b = mouth_map();
        b[META_FILE + 0x1100 + 4] = 2; // encoding: codec
        let (mut app, ..) = tag_tests::app_with_registry(&t.0, b, tag_tests::registry());
        let mut api = Api::new();
        let r = call(&mut api, &mut app, m::SOUND_READ, json!({"tag": SND}));
        assert_eq!(
            r["selected"]["refusal"],
            crate::app::audition::NEEDS_EXECUTABLE
        );
        assert_eq!(r["selected"]["mouth"]["ticks"], CURVE);
    }
}

#[test]
fn a_codec_stream_without_the_executable_is_refused_by_name() {
    // `H2_XBE` names an executable for any source: then there is one.
    if std::env::var_os("H2_XBE").is_some() {
        return;
    }
    let t = Temp::new("wma");
    let mut b = sound_map();
    b[META_FILE + 0x1100 + 4] = 2; // encoding: codec
    let (mut app, ..) = tag_tests::app_with_registry(&t.0, b, tag_tests::registry());
    app.audition.headless = true;
    let mut api = Api::new();
    let r = call(&mut api, &mut app, m::SOUND_READ, json!({"tag": SND}));
    assert_eq!(
        r["selected"]["refusal"],
        crate::app::audition::NEEDS_EXECUTABLE
    );
    assert_eq!(
        code_of(api.call(&mut app, m::SOUND_PLAY, json!({"tag": SND}))),
        code::REFUSED
    );
}
