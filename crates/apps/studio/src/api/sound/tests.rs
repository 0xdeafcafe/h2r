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

fn open(t: &Temp) -> (crate::App, Api) {
    let (mut app, ..) = tag_tests::app_with_registry(&t.0, sound_map(), tag_tests::registry());
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
    for name in [m::SOUND_READ, m::SOUND_PLAY, m::SOUND_STOP] {
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
