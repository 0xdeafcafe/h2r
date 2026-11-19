use super::*;

#[test]
fn feet_accelerate_and_brake_on_a_reversal() {
    let mut feet = Feet::new(27);
    let f = feet.step(Some([1.0, 0.0]), 1, 1.0, false, |_| true);
    assert!((0.3..0.5).contains(&f[0]));
    for _ in 0..4 {
        assert!(feet.step(Some([1.0, 1.0]), 1, 1.0, false, |_| true)[0] > 0.5);
    }
    let [x, y] = feet.step(Some([-1.0, -1.0]), 1, 1.0, false, |_| true);
    assert!(x > 0.0 && y > 0.0, "{x}, {y}");
    let [x, y] = feet.step(Some([-1.0, -1.0]), 1, 0.55, false, |_| true);
    assert!(x.hypot(y) <= 0.550_001);
    assert_eq!(feet.step(None, 1, 1.0, false, |_| true), [0.0; 2]);
}

#[test]
fn eased_direction_gives_way_when_the_corner_is_blocked() {
    let mut feet = Feet::new(27);
    feet.step(Some([1.0, 0.0]), 1, 1.0, true, |_| true);
    let [x, y] = feet.step(Some([0.0, 1.0]), 1, 1.0, false, |d| {
        assert!(d[0] > 0.0 && d[1] > 0.0);
        false
    });
    assert_eq!(x, 0.0);
    assert!(y > 0.9);
}

#[test]
fn face_button_intent_can_cancel_before_a_press_and_weapon_holds_release_for_a_flag() {
    let mut buttons = Buttons::new(1);
    assert_eq!(
        buttons.step(Some(Tap::Switch), 0, Skill::Legendary, true),
        None
    );
    assert_eq!(buttons.step(None, 10, Skill::Legendary, true), None);
    let first = (11..24)
        .find(|&t| buttons.step(Some(Tap::Switch), t, Skill::Legendary, true) == Some(SWITCH))
        .unwrap();
    assert!((16..=20).contains(&first));
    assert_eq!(
        buttons.step(None, first + 1, Skill::Legendary, true),
        Some(SWITCH)
    );
    assert_eq!(buttons.step(None, first + 2, Skill::Legendary, false), None);
}

#[test]
fn a_jump_holds_then_releases_even_if_a_gap_keeps_requesting_it() {
    let mut buttons = Buttons::new(1);
    let down: Vec<_> = (0..30).map(|t| buttons.jump(true, t)).collect();
    let hold = down.iter().take_while(|d| **d).count();
    assert!((3..=4).contains(&hold));
    assert!(!down[hold..].contains(&true));
    assert!(!buttons.jump(false, 30));
    assert!(buttons.jump(true, 31));
}

#[test]
fn waiting_fidgets_never_walk_or_press_anything() {
    let mut fidget = Fidget::new(2);
    let mut moved = 0;
    for t in 0..240 {
        let pad = fidget.pad(t);
        assert_eq!(pad.buttons.bits(), 0);
        assert_eq!(pad.left_stick, (0.0, 0.0));
        assert_eq!((pad.left_trigger, pad.right_trigger), (0.0, 0.0));
        assert!(pad.right_stick.0.hypot(pad.right_stick.1) <= 0.23);
        moved += u32::from(pad.right_stick != (0.0, 0.0));
    }
    assert!((4..40).contains(&moved), "{moved}");
}
