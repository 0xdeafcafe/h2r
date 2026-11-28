//! Slice 2 of `docs/engine/sim-render-split.md`: which code the game tick
//! reaches reads the flare visibility array (coupling 4) or what the
//! push-buffer callback `0x00024970` stores (coupling 5). A static reader
//! census over the tick's closure (`emit::sim::readers`). Needs the user's
//! own XBE; prints markdown for the page and writes it under Cargo's test
//! scratch directory.
//!
//! `H2_XBE=<path>/default.xbe cargo test -p emit --release --test render_readback_readers -- --ignored --nocapture`

mod common;

use std::collections::BTreeSet;
use std::path::Path;

use emit::sim::Closure;
use emit::sim::census;
use emit::sim::readers::{self, Watch};

/// `game_tick`: the root of everything a simulation tick runs.
const GAME_TICK: u32 = 0x0013_7fe0;
const LIMIT: usize = 40_000;

/// The flare state: the enable bytes `0x0051f408`/`0x0051f409`, the
/// 32-byte entries from `0x0051f40c`, the count `0x005233ec`, the three
/// ended bitmaps from `0x005233f0` and the frame and view words to
/// `0x005234c4` (`scene_extract::flares`).
const FLARES: Watch = Watch { name: "flare array and its state", start: 0x0051_f408, end: 0x0052_34c8, through: &[] };

/// The 256-float luminance histogram `0x00024a70` fills from the texels
/// (`render_screen_effects` hands `0x000245c0` its address in EAX).
const HISTOGRAM: Watch = Watch { name: "callback histogram", start: 0x0051_ecf0, end: 0x0051_f0f0, through: &[] };

/// The callback's record: `[0x00485890]`, the push-buffer slot's argument
/// (`0x0002461b`). `0x00024c70` writes its six floats `+0x00..+0x14`;
/// `+0x18` is the histogram, `+0x1c` the word `[0x00485600]`, `+0x20` a
/// counter.
const RECORD: Watch = Watch { name: "callback record [0x00485890]", start: 0x0048_5890, end: 0x0048_5894, through: &[0x0048_5890] };

/// The tick's readers of the flare array: `object_new` (`0x000b7b40`)
/// calls `0x0002e1e0`, which reads each entry's head for its in-use byte
/// and owner index (bits 6-17, not the visibility byte 3) and frees the
/// matching entries with `0x00020e50`.
const TICK_FLARE_READERS: [u32; 2] = [0x0002_e1e0, 0x0002_0e50];

/// Render code the callback's value and the flare byte flow through. None
/// is in the tick's closure.
const RENDER_ONLY: [u32; 10] = [
    0x0001_3870, // render_screen_effects: reads the record's +0x10
    0x0002_45c0, // inserts the callback, resets the record
    0x0002_4970, // the callback
    0x0002_4a70, // histogram of the texels
    0x0002_4b80, // normalises it
    0x0002_4c70, // percentiles and mean into the record
    0x0002_4ee0, // 0x245c0 hands it the record
    0x0002_0770, // the flare reader: count to byte
    0x0002_0bb0, // the flare walker: the queries
    0x0002_df00, // the flare draw pass: reads the byte
];

#[test]
#[ignore = "needs the XBE; prints the reader census"]
fn render_readback_readers() {
    let Some(path) = common::xbe_path() else {
        common::skip("render_readback_readers", "needs the XBE (H2_XBE)");
        return;
    };
    let data = std::fs::read(path).unwrap();
    let x = xbe::Xbe::parse(&data).unwrap();
    let analysis = analyze::analyze(&x).unwrap();
    let lifter = lift::Lifter::new(&analysis, &x).unwrap();

    let tick = Closure::build(&[GAME_TICK], LIMIT, |pc| lifter.lift(pc));
    assert!(tick.functions.contains_key(&GAME_TICK), "the tick itself lifts");
    let c = census::census(&tick);

    let mut out = format!(
        "## Reader census of the game tick\n\n{} functions, {} loads, {} indirect call/jump sites not followed{}.\n\n",
        c.functions,
        c.loads.len(),
        c.indirect.len(),
        if c.truncated { ", walk truncated at its limit" } else { "" }
    );
    let flares = readers::readers(&c.loads, &FLARES);
    let histogram = readers::readers(&c.loads, &HISTOGRAM);
    let record = readers::readers(&c.loads, &RECORD);
    for (w, by) in [(&FLARES, &flares), (&HISTOGRAM, &histogram), (&RECORD, &record)] {
        out.push_str(&readers::markdown(w, by));
        out.push('\n');
    }
    let in_tick: Vec<String> = RENDER_ONLY.iter().filter(|f| tick.functions.contains_key(f)).map(|f| format!("{f:#010x}")).collect();
    out.push_str(&format!("Render-only functions in the tick's closure: {}.\n", if in_tick.is_empty() { "none".to_string() } else { in_tick.join(" ") }));

    let file = Path::new(env!("CARGO_TARGET_TMPDIR")).join("render-readback-readers.md");
    std::fs::write(&file, &out).unwrap();
    println!("{out}");
    eprintln!("written to {}", file.display());

    assert!(histogram.is_empty(), "a tick load reads the callback's histogram");
    assert!(record.is_empty(), "a tick load reads the callback's record");
    let found: BTreeSet<u32> = flares.keys().copied().collect();
    let allowed: BTreeSet<u32> = TICK_FLARE_READERS.into_iter().collect();
    assert!(found.is_subset(&allowed), "new tick readers of the flare array: {:x?}", found.difference(&allowed).collect::<Vec<_>>());
    assert!(in_tick.is_empty(), "render-only functions in the tick: {in_tick:?}");
}
