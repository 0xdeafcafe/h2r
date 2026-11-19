#![cfg(feature = "live-floor-bench")]
use live_proto::pack::speed;

#[test]
fn speed_round_trips_and_joins() {
    speed::verify();
}

#[test]
#[ignore = "paired release measurements on the full seeded match"]
fn compare_speed() {
    speed::compare();
}
