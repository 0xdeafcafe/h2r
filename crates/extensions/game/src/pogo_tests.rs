//! The rider's rule on lists of ticks: the mount, the piston through a
//! touchdown and its verdict, the dismount, and the trick of each flight.

use super::*;

const STYLE: Style = Style::Unsc;

fn tick(airborne: bool, vz: f32) -> RiderTick {
    RiderTick {
        style: STYLE,
        airborne,
        vz,
        bounce: None,
    }
}

fn bounce(vz: f32, chain: u32, monster: bool) -> RiderTick {
    RiderTick {
        bounce: Some(Bounce { chain, monster }),
        ..tick(false, vz)
    }
}

fn run(r: &mut Rider, ticks: &[RiderTick]) -> Vec<Step> {
    ticks.iter().map(|t| r.step(Some(t))).collect()
}

fn sample(s: &Step) -> Sample {
    s.sample.expect("a rider with a stick has a sample")
}

/// A rider up in the air and on the stick, falling at -6.
fn riding() -> Rider {
    let mut r = Rider::new();
    run(
        &mut r,
        &[tick(true, -6.0); (AIR_TICKS + MOUNT_TICKS + 2) as usize],
    );
    assert_eq!(r.phase(), Phase::Ride);
    r
}

/// The ticks from a touchdown to the chain's verdict of a bounce at
/// `chain`, launched at `vz`: the touchdown, the launch window with the
/// airborne bit clear, then the verdict.
fn bounced(chain: u32, monster: bool, vz: f32) -> Vec<RiderTick> {
    let mut v = vec![tick(false, vz)];
    v.extend((1..LAUNCH_TICKS).map(|_| tick(false, vz)));
    v.push(bounce(vz, chain, monster));
    v
}

#[test]
fn the_trick_is_the_chains_count_and_a_monster_is_a_front_flip() {
    use Trick::*;
    assert_eq!(trick_for(0, false), None);
    assert_eq!(trick_for(0, true), None);
    let by_count: Vec<_> = (1..=8).map(|n| trick_for(n, false).unwrap()).collect();
    assert_eq!(
        by_count,
        [
            NoHander,
            CanCan,
            BarSpin,
            Superman,
            Backflip,
            DoubleBackflip,
            DoubleBackflip,
            DoubleBackflip
        ]
    );
    for n in 1..=8 {
        assert_eq!(trick_for(n, true), Some(FrontFlip), "chain {n}");
    }
}

#[test]
fn every_trick_has_its_own_id_and_reads_back() {
    for t in Trick::ALL {
        assert_eq!(Trick::from_id(t.id()), Some(t));
        assert!(!t.name().is_empty());
        assert!(t.longest() > 0.0);
    }
    let mut ids: Vec<_> = Trick::ALL.iter().map(|t| t.id()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), Trick::ALL.len());
    assert_eq!(Trick::from_id("flourish"), None);
    assert_eq!(Trick::Backflip.flips(), 1.0);
    assert_eq!(Trick::DoubleBackflip.flips(), 2.0);
    assert_eq!(Trick::FrontFlip.flips(), -1.0);
    assert_eq!(Trick::BarSpin.flips(), 0.0);
}

#[test]
fn no_stick_is_no_sample_and_starts_the_rider_over() {
    let mut r = riding();
    let s = r.step(None);
    assert_eq!(s.sample, None);
    assert_eq!(s.cue, None);
    assert_eq!(r, Rider::new());
}

#[test]
fn on_the_ground_the_stick_is_carried() {
    let mut r = Rider::new();
    for s in run(&mut r, &[tick(false, 0.0); 40]) {
        let s = sample(&s);
        assert_eq!(s.phase, Phase::Carried);
        assert_eq!(s.mounted, 0.0);
        assert_eq!(s.compress, 0.0);
        assert_eq!(s.trick, None);
        assert!(!s.posed());
        assert_eq!(s.style, STYLE);
    }
}

#[test]
fn a_step_down_a_stair_is_not_a_mount() {
    let mut r = Rider::new();
    let mut ticks = Vec::new();
    for _ in 0..10 {
        ticks.extend([tick(true, -0.4); (AIR_TICKS - 1) as usize]);
        ticks.push(tick(false, 0.0));
    }
    for s in run(&mut r, &ticks) {
        assert_eq!(sample(&s).phase, Phase::Carried);
        assert_eq!(sample(&s).mounted, 0.0);
    }
}

#[test]
fn a_takeoff_mounts_over_the_mount_ticks() {
    let mut r = Rider::new();
    let steps = run(
        &mut r,
        &[tick(true, 3.0); (AIR_TICKS + MOUNT_TICKS + 1) as usize],
    );
    let phases: Vec<_> = steps.iter().map(|s| sample(s).phase).collect();
    let before = (AIR_TICKS - 1) as usize;
    assert!(phases[..before].iter().all(|p| *p == Phase::Carried));
    assert_eq!(phases[before], Phase::Mount);
    assert_eq!(*phases.last().unwrap(), Phase::Ride);
    // The blend only rises, and ends on the stick.
    let mounted: Vec<_> = steps.iter().map(|s| sample(s).mounted).collect();
    assert!(mounted.windows(2).all(|w| w[1] >= w[0]), "{mounted:?}");
    assert_eq!(*mounted.last().unwrap(), 1.0);
    assert!(steps.iter().all(|s| s.cue.is_none()));
}

#[test]
fn a_touchdown_compresses_and_a_bounce_springs_with_the_chains_trick() {
    let mut r = riding();
    let steps = run(&mut r, &bounced(5, false, 12.0));
    assert_eq!(sample(&steps[0]).phase, Phase::Compress);
    assert_eq!(steps[0].cue, Some(Cue::Compress));
    // The piston is in by the second tick and stays in until the verdict.
    assert!(sample(&steps[0]).compress > 0.0);
    for s in &steps[1..steps.len() - 1] {
        assert_eq!(sample(s).phase, Phase::Compress);
        assert_eq!(sample(s).compress, 1.0);
        assert_eq!(s.cue, None);
    }
    let last = steps.last().unwrap();
    assert_eq!(sample(last).phase, Phase::Spring);
    assert_eq!(last.cue, Some(Cue::Spring));
    assert_eq!(sample(last).flare, 1.0);
    assert_eq!(sample(last).mounted, 1.0);

    // The flight up and down: the piston out, then the Backflip centred
    // on the apex.
    let mut seen = Vec::new();
    let mut vz = 12.0;
    while vz > -12.0 {
        vz -= GRAVITY / TICK_RATE;
        let s = sample(&r.step(Some(&tick(true, vz))));
        assert!(matches!(s.phase, Phase::Spring | Phase::Ride));
        if let Some((trick, p)) = s.trick {
            assert_eq!(trick, Trick::Backflip);
            seen.push((vz, p));
        }
    }
    assert_eq!(r.phase(), Phase::Ride);
    assert!(seen.len() > 20, "{} ticks of trick", seen.len());
    assert!(
        seen.windows(2).all(|w| w[1].1 > w[0].1),
        "progress only rises"
    );
    // As long before the apex as after it.
    let (first, last) = (seen[0].0, seen[seen.len() - 1].0);
    assert!(
        (first + last).abs() < 2.0 * GRAVITY / TICK_RATE,
        "{first} {last}"
    );
    // Half way at the apex.
    let apex = seen
        .iter()
        .min_by(|a, b| a.0.abs().total_cmp(&b.0.abs()))
        .unwrap();
    assert!((apex.1 - 0.5).abs() < 0.05, "{apex:?}");
    // No longer than the trick's longest.
    let seconds = seen.len() as f32 / TICK_RATE;
    assert!(
        seconds <= Trick::Backflip.longest() + 2.0 / TICK_RATE,
        "{seconds}"
    );
}

#[test]
fn the_piston_springs_out_and_the_flare_fades() {
    let mut r = riding();
    run(&mut r, &bounced(1, false, 12.0));
    let steps = run(
        &mut r,
        &[tick(true, 11.5); (SPRING_TICKS + FLARE_TICKS + 1) as usize],
    );
    let compress: Vec<_> = steps.iter().map(|s| sample(s).compress).collect();
    assert!(compress.windows(2).all(|w| w[1] <= w[0]), "{compress:?}");
    assert_eq!(compress[SPRING_TICKS as usize - 1], 0.0);
    let flare: Vec<_> = steps.iter().map(|s| sample(s).flare).collect();
    assert!(flare.windows(2).all(|w| w[1] <= w[0]), "{flare:?}");
    assert_eq!(*flare.last().unwrap(), 0.0);
}

#[test]
fn every_chain_count_shows_its_trick_and_a_monster_the_front_flip() {
    for (chain, monster, want) in [
        (1, false, Trick::NoHander),
        (2, false, Trick::CanCan),
        (3, false, Trick::BarSpin),
        (4, false, Trick::Superman),
        (5, false, Trick::Backflip),
        (6, false, Trick::DoubleBackflip),
        (9, false, Trick::DoubleBackflip),
        (2, true, Trick::FrontFlip),
    ] {
        let mut r = riding();
        run(&mut r, &bounced(chain, monster, 17.0));
        let at_apex = sample(&r.step(Some(&tick(true, 0.0))));
        assert_eq!(at_apex.trick.map(|t| t.0), Some(want), "chain {chain}");
        let p = at_apex.trick.unwrap().1;
        assert!((p - 0.5).abs() < 1e-6, "chain {chain}: {p}");
    }
}

#[test]
fn a_short_bounce_gets_the_tuck_alone() {
    // Under MIN_HANG: up and back in well under a second.
    let vz = MIN_HANG * GRAVITY / 2.0 * 0.8;
    assert_eq!(trick_seconds(Trick::NoHander, vz), None);
    let mut r = riding();
    run(&mut r, &bounced(3, false, vz));
    let mut v = vz;
    let mut tucked = 0.0f32;
    while v > -vz {
        v -= GRAVITY / TICK_RATE;
        let s = sample(&r.step(Some(&tick(true, v))));
        assert_eq!(s.trick, None);
        tucked = tucked.max(s.tuck);
    }
    assert_eq!(tucked, 1.0);
}

#[test]
fn a_trick_takes_a_share_of_the_hang_time_up_to_its_longest() {
    // Just long enough: the share of the hang time.
    let vz = MIN_HANG * GRAVITY / 2.0 + 0.001;
    let s = trick_seconds(Trick::Superman, vz).unwrap();
    assert!((s - MIN_HANG * TRICK_SHARE).abs() < 1e-2, "{s}");
    // A high bounce: the trick's longest.
    assert_eq!(
        trick_seconds(Trick::Superman, 13.0),
        Some(Trick::Superman.longest())
    );
    assert_eq!(hang_time(-3.0), 0.0);
    assert_eq!(trick_progress(0.0, 1.0), 0.5);
    assert_eq!(trick_progress(100.0, 1.0), 0.0);
    assert_eq!(trick_progress(-100.0, 1.0), 1.0);
    assert_eq!(trick_progress(f32::NAN, 1.0), 1.0);
    assert_eq!(trick_progress(1.0, 0.0), 1.0);
}

#[test]
fn a_landing_with_no_bounce_settles_and_dismounts() {
    let mut r = riding();
    let steps = run(
        &mut r,
        &[tick(false, 0.0); (VERDICT_TICKS + DISMOUNT_TICKS + 3) as usize],
    );
    assert_eq!(steps[0].cue, Some(Cue::Compress));
    let settle = steps
        .iter()
        .position(|s| s.cue == Some(Cue::Settle))
        .unwrap();
    assert_eq!(settle as u32, VERDICT_TICKS);
    assert_eq!(sample(&steps[settle]).phase, Phase::Dismount);
    assert!(steps.iter().all(|s| s.cue != Some(Cue::Spring)));
    let last = sample(steps.last().unwrap());
    assert_eq!(last.phase, Phase::Carried);
    assert_eq!(last.mounted, 0.0);
    assert_eq!(last.compress, 0.0);
    // Getting off only goes one way.
    let mounted: Vec<_> = steps[settle..].iter().map(|s| sample(s).mounted).collect();
    assert!(mounted.windows(2).all(|w| w[1] <= w[0]), "{mounted:?}");
}

#[test]
fn a_landing_caught_in_the_air_rides_on() {
    // Stopped above the floor with the airborne bit still set.
    let mut r = riding();
    let steps = run(&mut r, &[tick(true, 0.0); (VERDICT_TICKS + 2) as usize]);
    assert_eq!(sample(&steps[0]).phase, Phase::Compress);
    assert_eq!(sample(steps.last().unwrap()).phase, Phase::Ride);
    assert_eq!(sample(steps.last().unwrap()).mounted, 1.0);
}

#[test]
fn a_soft_set_down_dismounts_without_the_piston() {
    // Falling slower than a touchdown counts.
    let mut r = Rider::new();
    run(
        &mut r,
        &[tick(true, -0.3); (AIR_TICKS + MOUNT_TICKS + 2) as usize],
    );
    let steps = run(&mut r, &[tick(false, 0.0); 3]);
    assert_eq!(sample(&steps[0]).phase, Phase::Dismount);
    assert!(steps.iter().all(|s| s.cue.is_none()));
    assert!(steps.iter().all(|s| sample(s).compress == 0.0));
}

#[test]
fn a_takeoff_during_the_dismount_mounts_again() {
    let mut r = riding();
    run(&mut r, &[tick(false, 0.0); (VERDICT_TICKS + 2) as usize]);
    assert_eq!(r.phase(), Phase::Dismount);
    run(&mut r, &[tick(true, 4.0); AIR_TICKS as usize]);
    assert_eq!(r.phase(), Phase::Mount);
}

#[test]
fn a_bounce_filed_while_carried_still_springs() {
    // The verdict of a touchdown the rider did not see (a stick swapped
    // to in the launch window).
    let mut r = Rider::new();
    r.step(Some(&tick(false, 0.0)));
    let s = r.step(Some(&bounce(12.0, 2, false)));
    assert_eq!(sample(&s).phase, Phase::Spring);
    assert_eq!(s.cue, Some(Cue::Spring));
    let at_apex = sample(&run(&mut r, &[tick(true, 6.0), tick(true, 0.0)])[1]);
    assert_eq!(at_apex.trick.map(|t| t.0), Some(Trick::CanCan));
}

#[test]
fn a_forced_trick_plays_from_its_start_on_the_ground() {
    let mut r = Rider::new();
    r.step(Some(&tick(false, 0.0)));
    r.force(Trick::BarSpin);
    let of = (TEST_SECONDS.min(Trick::BarSpin.longest()) * TICK_RATE) as usize;
    let steps = run(&mut r, &vec![tick(false, 0.0); of + 2]);
    let shown: Vec<_> = steps.iter().filter_map(|s| sample(s).trick).collect();
    assert_eq!(shown.len(), of - 1);
    assert!(shown.iter().all(|(t, _)| *t == Trick::BarSpin));
    assert!(shown.windows(2).all(|w| w[1].1 > w[0].1));
    assert!(shown[0].1 > 0.0 && shown[shown.len() - 1].1 < 1.0);
    // Shown on the stick, then back to the hands.
    assert_eq!(sample(&steps[0]).mounted, 1.0);
    assert_eq!(sample(steps.last().unwrap()).trick, None);
    assert_eq!(sample(steps.last().unwrap()).mounted, 0.0);
}

#[test]
fn riders_keep_one_per_slot_and_drop_those_who_left() {
    let mut riders = Riders::new();
    for _ in 0..(AIR_TICKS + 1) {
        riders.step(2, Some(&tick(true, 2.0)));
        riders.step(5, Some(&tick(false, 0.0)));
    }
    assert_eq!(riders.rider(2).unwrap().phase(), Phase::Mount);
    assert_eq!(riders.rider(5).unwrap().phase(), Phase::Carried);
    assert_eq!(
        riders.step(SLOTS as u8, Some(&tick(true, 0.0))).sample,
        None
    );
    assert!(riders.force(5, Trick::Superman));
    assert!(!riders.force(SLOTS as u8, Trick::Superman));
    riders.keep(&[5]);
    assert_eq!(riders.rider(2), Some(&Rider::new()));
    assert_ne!(riders.rider(5), Some(&Rider::new()));
}
