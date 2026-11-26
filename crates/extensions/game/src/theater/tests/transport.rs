use super::*;
use std::time::{Duration, Instant};

#[test]
fn five_taps_make_one_seek_to_the_combined_target() {
    let start = Instant::now();
    let mut taps = SeekTaps::default();
    for i in 0..5 {
        let at = start + Duration::from_millis(i * 40);
        taps.push(0, 5, (0, 60), at);
        assert!(taps.take_due(at).is_none());
    }
    let targets: Vec<_> = [409, 410, 411, 800]
        .into_iter()
        .filter_map(|ms| taps.take_due(start + Duration::from_millis(ms)))
        .collect();
    assert_eq!(targets, [25]);
}
