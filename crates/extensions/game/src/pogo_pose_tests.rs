//! The pose on a made-up skeleton: the contacts (feet on pegs, hands on
//! grips), the piston, each trick's start, height and end, the flips as
//! rigid turns, and what is left alone.

use pogo_kit::{RIG, Style};

use super::*;
use crate::pogo::Phase;

/// A millimetre, in world units.
const MM: f32 = 0.0003;

pub(crate) fn keys() -> &'static Keys {
    super::keys().expect("the embedded keys parse")
}

fn row(m: M3, position: V3) -> NodeMatrix {
    NodeMatrix {
        scale: 1.0,
        forward: [m[0][0], m[1][0], m[2][0]],
        left: [m[0][1], m[1][1], m[2][1]],
        up: [m[0][2], m[1][2], m[2][2]],
        position,
    }
}

/// The nodes of a Spartan-sized biped (0.69 units tall): name, parent,
/// rest position in model space. Every node's rest frame is turned its
/// own way, as a real model's are.
const NODES: [(&str, Option<usize>, V3); 19] = [
    ("pelvis", None, [0.0, 0.0, 0.34]),
    ("spine", Some(0), [0.0, 0.0, 0.40]),
    ("spine1", Some(1), [0.0, 0.0, 0.47]),
    ("neck", Some(2), [0.0, 0.0, 0.56]),
    ("head", Some(3), [0.0, 0.0, 0.60]),
    ("l_thigh", Some(0), [0.0, 0.035, 0.33]),
    ("l_calf", Some(5), [0.0, 0.035, 0.18]),
    ("l_foot", Some(6), [0.0, 0.035, 0.03]),
    ("l_toe", Some(7), [0.05, 0.035, 0.0]),
    ("r_thigh", Some(0), [0.0, -0.035, 0.33]),
    ("r_calf", Some(9), [0.0, -0.035, 0.18]),
    ("r_foot", Some(10), [0.0, -0.035, 0.03]),
    ("r_toe", Some(11), [0.05, -0.035, 0.0]),
    ("l_upperarm", Some(2), [0.0, 0.07, 0.53]),
    ("l_forearm", Some(13), [0.0, 0.17, 0.53]),
    ("l_hand", Some(14), [0.0, 0.26, 0.53]),
    ("r_upperarm", Some(2), [0.0, -0.07, 0.53]),
    ("r_forearm", Some(16), [0.0, -0.17, 0.53]),
    ("r_hand", Some(17), [0.0, -0.26, 0.53]),
];
const PELVIS: usize = 0;
const HEAD: usize = 4;
const FEET: [usize; 2] = [7, 11];
const HANDS: [usize; 2] = [15, 18];

fn bones(rename: &dyn Fn(&str) -> String) -> Vec<Bone> {
    NODES
        .iter()
        .enumerate()
        .map(|(i, (name, parent, at))| Bone {
            name: rename(name),
            parent: *parent,
            rest: row(m_mul(rz(0.3 * i as f32), ry(0.2 * i as f32)), *at),
        })
        .collect()
}

pub(crate) fn spartan() -> Skeleton {
    let s = Skeleton::new(bones(&|n| n.to_owned()), keys());
    s.roles().expect("the made-up skeleton has every role");
    s
}

/// The rows of `sk` standing at rest at `at`, facing `yaw`.
pub(crate) fn standing(sk: &Skeleton, yaw: f32, at: V3) -> Vec<NodeMatrix> {
    sk.bones()
        .iter()
        .map(|b| {
            row(
                m_mul(rz(yaw), axes(&b.rest)),
                add(at, m_vec(rz(yaw), b.rest.position)),
            )
        })
        .collect()
}

/// A stick in the hand: somewhere by the body, leaning.
pub(crate) fn carried_stick(at: V3) -> [NodeMatrix; 2] {
    let frame = row(m_mul(rz(0.7), ry(0.9)), add(at, [0.1, -0.2, 0.3]));
    [frame, piston_row(&frame, &RIG, 0.0)]
}

pub(crate) fn sample(
    mounted: f32,
    compress: f32,
    tuck: f32,
    trick: Option<(Trick, f32)>,
) -> Sample {
    Sample {
        style: Style::Unsc,
        phase: Phase::Ride,
        mounted,
        compress,
        flare: 0.0,
        tuck,
        trick,
    }
}

const AT: V3 = [12.0, -7.0, 3.0];
const YAW: f32 = 1.0;

/// The pose of the made-up Spartan standing at [`AT`] facing [`YAW`].
fn posed(s: &Sample) -> Posed {
    let sk = spartan();
    pose(
        &sk,
        keys(),
        &RIG,
        &standing(&sk, YAW, AT),
        &carried_stick(AT),
        s,
    )
    .expect("a mounted sample is posed")
}

fn mounted() -> Posed {
    posed(&sample(1.0, 0.0, 1.0, None))
}

fn at_trick(trick: Trick, progress: f32) -> Posed {
    posed(&sample(1.0, 0.0, 1.0, Some((trick, progress))))
}

fn dist(a: V3, b: V3) -> f32 {
    len(sub(a, b))
}

/// How far a side's sole point is from its peg.
fn foot_off(sk: &Skeleton, p: &Posed, side: usize) -> f32 {
    let foot = FEET[side];
    let sole = attach(
        &p.biped[foot],
        &sk.bones()[foot].rest,
        mirror(keys().mounted.sole, side),
    );
    dist(sole, p.stick[0].point([RIG.peg_l, RIG.peg_r][side]))
}

/// How far a side's palm point is from its grip.
fn hand_off(sk: &Skeleton, p: &Posed, side: usize) -> f32 {
    let hand = HANDS[side];
    let palm = attach(
        &p.biped[hand],
        &sk.bones()[hand].rest,
        mirror(keys().mounted.palm, side),
    );
    dist(palm, p.stick[0].point([RIG.grip_l, RIG.grip_r][side]))
}

/// The most two poses differ by: positions, and axes.
fn apart(a: &Posed, b: &Posed) -> (f32, f32) {
    let rows = |p: &Posed| p.biped.iter().chain(&p.stick).copied().collect::<Vec<_>>();
    let mut most = (0.0f32, 0.0f32);
    for (x, y) in rows(a).iter().zip(rows(b).iter()) {
        most.0 = most.0.max(dist(x.position, y.position));
        for (u, v) in [(x.forward, y.forward), (x.left, y.left), (x.up, y.up)] {
            most.1 = most.1.max(dist(u, v));
        }
    }
    most
}

fn square(r: &NodeMatrix) -> bool {
    (len(r.forward) - 1.0).abs() < 1e-4
        && (len(r.left) - 1.0).abs() < 1e-4
        && (len(r.up) - 1.0).abs() < 1e-4
        && dot(r.forward, r.left).abs() < 1e-4
        && dist(cross(r.forward, r.left), r.up) < 1e-4
}

/// Every node at its rest distance from its parent.
fn lengths_kept(sk: &Skeleton, p: &Posed) {
    for (i, b) in sk.bones().iter().enumerate() {
        let Some(parent) = b.parent else { continue };
        let rest = dist(b.rest.position, sk.bones()[parent].rest.position);
        let now = dist(p.biped[i].position, p.biped[parent].position);
        assert!(
            (rest - now).abs() < 1e-4,
            "{}: {rest} at rest, {now}",
            b.name
        );
    }
}

#[test]
fn the_embedded_keys_parse_and_refuse_a_field_they_do_not_know() {
    assert!(super::keys().is_ok());
    let extra = format!("{KEYS_YAML}\nflourish: 1\n");
    assert!(Keys::parse(&extra).is_err());
}

#[test]
fn a_rest_frame_comes_back_from_its_inverse() {
    let turn = m_mul(rz(0.8), ry(-0.4));
    let at = [0.3, -0.2, 0.5];
    // The inverse: the transposed axes, and minus them times the place.
    let back = scale(m_vec(m_t(turn), at), -1.0);
    let inv = row(m_t(turn), back);
    let words = [
        1.0,
        inv.forward[0],
        inv.forward[1],
        inv.forward[2],
        inv.left[0],
        inv.left[1],
        inv.left[2],
        inv.up[0],
        inv.up[1],
        inv.up[2],
        back[0],
        back[1],
        back[2],
        // The node's last word, which is not the frame's.
        77.0,
    ];
    let rest = rest_of(&words).unwrap();
    let want = row(turn, at);
    assert!(dist(rest.position, want.position) < 1e-5);
    assert!(dist(rest.forward, want.forward) < 1e-5);
    assert!(dist(rest.left, want.left) < 1e-5);
    assert!(dist(rest.up, want.up) < 1e-5);
    let mut bad = words;
    bad[5] = f32::NAN;
    assert_eq!(rest_of(&bad), None);
    assert_eq!(rest_of(&words[..12]), None);
}

#[test]
fn carried_is_left_alone() {
    let sk = spartan();
    let rows = standing(&sk, YAW, AT);
    let stick = carried_stick(AT);
    let s = sample(0.0, 0.0, 0.0, None);
    assert_eq!(pose(&sk, keys(), &RIG, &rows, &stick, &s), None);
    // The piston in with the stick in the hand: the biped's rows are the
    // game's to the bit, and only the piston moves.
    let p = pose(
        &sk,
        keys(),
        &RIG,
        &rows,
        &stick,
        &sample(0.0, 1.0, 0.0, None),
    )
    .unwrap();
    for (a, b) in p.biped.iter().zip(&rows) {
        assert_eq!(bytemuck_bits(a), bytemuck_bits(b));
    }
    assert_eq!(bytemuck_bits(&p.stick[0]), bytemuck_bits(&stick[0]));
    let slid = dist(p.stick[1].position, stick[1].position);
    assert!((slid - RIG.travel).abs() < 1e-5, "{slid}");
}

fn bytemuck_bits(r: &NodeMatrix) -> [u32; 13] {
    let f = [
        r.scale,
        r.forward[0],
        r.forward[1],
        r.forward[2],
        r.left[0],
        r.left[1],
        r.left[2],
        r.up[0],
        r.up[1],
        r.up[2],
        r.position[0],
        r.position[1],
        r.position[2],
    ];
    f.map(f32::to_bits)
}

#[test]
fn mounted_has_the_feet_on_the_pegs_and_the_hands_on_the_grips() {
    let sk = spartan();
    for (yaw, at) in [
        (0.0, [0.0, 0.0, 0.0]),
        (1.0, AT),
        (-2.5, [-40.0, 3.0, -9.0]),
        (3.0, [5.0, 60.0, 20.0]),
    ] {
        for (compress, tuck) in [(0.0, 0.0), (0.0, 1.0), (1.0, 0.0), (0.4, 0.6)] {
            let s = sample(1.0, compress, tuck, None);
            let p = pose(
                &sk,
                keys(),
                &RIG,
                &standing(&sk, yaw, at),
                &carried_stick(at),
                &s,
            )
            .unwrap();
            for side in [LEFT, RIGHT] {
                let (foot, hand) = (foot_off(&sk, &p, side), hand_off(&sk, &p, side));
                assert!(foot < MM, "yaw {yaw} side {side}: foot {foot} off its peg");
                assert!(hand < MM, "yaw {yaw} side {side}: hand {hand} off its grip");
            }
            lengths_kept(&sk, &p);
            assert!(p.biped.iter().chain(&p.stick).all(square));
            // The stick upright, its handlebar across the body, ahead of
            // the pelvis.
            let frame = &p.stick[0];
            let (forward, left) = (
                m_vec(rz(yaw), [1.0, 0.0, 0.0]),
                m_vec(rz(yaw), [0.0, 1.0, 0.0]),
            );
            assert!(dist(frame.up, [0.0, 0.0, 1.0]) < 1e-4);
            assert!(dist(frame.left, left) < 1e-4);
            assert!(dist(frame.forward, forward) < 1e-4);
            let grips = scale(add(frame.point(RIG.grip_l), frame.point(RIG.grip_r)), 0.5);
            let ahead = dot(sub(grips, p.biped[PELVIS].position), forward);
            assert!(ahead > 0.03, "the grips are {ahead} ahead of the pelvis");
            // The left of the body is the stick's left.
            let across = sub(frame.point(RIG.grip_l), frame.point(RIG.grip_r));
            assert!(dot(across, left) > 0.1);
            // The pelvis where the game has it, less the piston's drop.
            let pelvis = sub(
                p.biped[PELVIS].position,
                add(at, m_vec(rz(yaw), NODES[PELVIS].2)),
            );
            assert!(dist(pelvis, [0.0, 0.0, -RIG.travel * compress]) < 1e-5);
        }
    }
}

#[test]
fn compress_brings_the_rider_down_and_leaves_the_foot_still() {
    let out = posed(&sample(1.0, 0.0, 0.0, None));
    let foot_of = |p: &Posed| p.stick[1].point(sub(RIG.foot, RIG.piston));
    for compress in [0.25, 1.0] {
        let p = posed(&sample(1.0, compress, 0.0, None));
        let down = RIG.travel * compress;
        let frame = sub(p.stick[0].position, out.stick[0].position);
        assert!(dist(frame, [0.0, 0.0, -down]) < 1e-5, "{frame:?}");
        let pelvis = sub(p.biped[PELVIS].position, out.biped[PELVIS].position);
        assert!(dist(pelvis, [0.0, 0.0, -down]) < 1e-5, "{pelvis:?}");
        let head = sub(p.biped[HEAD].position, out.biped[HEAD].position);
        assert!(dist(head, [0.0, 0.0, -down]) < 1e-5, "{head:?}");
        assert!(dist(foot_of(&p), foot_of(&out)) < 1e-5);
        // The piston up the frame by the same.
        let piston = sub(p.stick[1].position, p.stick[0].position);
        assert!((dot(piston, p.stick[0].up) - (RIG.piston[2] + down)).abs() < 1e-5);
        assert!((dot(piston, p.stick[0].left) - RIG.piston[1]).abs() < 1e-5);
        assert!((dot(piston, p.stick[0].forward) - RIG.piston[0]).abs() < 1e-5);
        assert_eq!(p.stick[1].up, p.stick[0].up);
    }
}

#[test]
fn tuck_brings_the_pegs_up_toward_the_pelvis() {
    let out = posed(&sample(1.0, 0.0, 0.0, None));
    let tucked = posed(&sample(1.0, 0.0, 1.0, None));
    let lift = tucked.stick[0].position[2] - out.stick[0].position[2];
    assert!((lift - keys().mounted.tuck_lift).abs() < 1e-5, "{lift}");
    assert!(dist(tucked.biped[PELVIS].position, out.biped[PELVIS].position) < 1e-5);
}

#[test]
fn every_trick_starts_and_ends_on_the_mounted_pose_and_leaves_it_between() {
    let m = mounted();
    for trick in Trick::ALL {
        for edge in [0.001, 0.999] {
            let (place, turn) = apart(&at_trick(trick, edge), &m);
            assert!(place < 2e-3, "{trick:?} at {edge}: {place} from mounted");
            assert!(turn < 2e-2, "{trick:?} at {edge}: axes {turn} from mounted");
        }
        let (place, turn) = apart(&at_trick(trick, 0.3), &m);
        assert!(
            place > 0.02 || turn > 0.2,
            "{trick:?} at 0.3 is the mounted pose ({place}, {turn})"
        );
        for p in [0.1, 0.3, 0.5, 0.8] {
            let posed = at_trick(trick, p);
            assert!(posed.biped.iter().chain(&posed.stick).all(square));
            lengths_kept(&spartan(), &posed);
        }
    }
    assert_eq!(trick_weight(0.0), 0.0);
    assert_eq!(trick_weight(1.0), 0.0);
    assert_eq!(trick_weight(0.5), 1.0);
    assert_eq!(trick_weight(TRICK_EASE), 1.0);
}

/// `p` turned by `angle` (a pitch in the body frame) about the left axis
/// through `about`.
fn turned(p: &Posed, angle: f32, about: V3) -> Posed {
    let r = m_mul(m_mul(rz(YAW), ry(angle)), m_t(rz(YAW)));
    let turn = |x: &NodeMatrix| NodeMatrix {
        scale: x.scale,
        forward: m_vec(r, x.forward),
        left: m_vec(r, x.left),
        up: m_vec(r, x.up),
        position: add(about, m_vec(r, sub(x.position, about))),
    };
    Posed {
        biped: p.biped.iter().map(turn).collect(),
        stick: [turn(&p.stick[0]), turn(&p.stick[1])],
    }
}

#[test]
fn a_flip_is_the_mounted_pose_turned_whole_about_the_pelvis() {
    let m = mounted();
    let pelvis = m.biped[PELVIS].position;
    let half = std::f32::consts::PI;
    // Half way: half a turn, and for the double a whole one.
    let (place, turn) = apart(&at_trick(Trick::Backflip, 0.5), &turned(&m, -half, pelvis));
    assert!(place < 1e-4 && turn < 1e-4, "backflip: {place}, {turn}");
    let (place, turn) = apart(&at_trick(Trick::FrontFlip, 0.5), &turned(&m, half, pelvis));
    assert!(place < 1e-4 && turn < 1e-4, "front flip: {place}, {turn}");
    let (place, turn) = apart(&at_trick(Trick::DoubleBackflip, 0.5), &m);
    assert!(place < 1e-4 && turn < 1e-4, "double: {place}, {turn}");
    // A quarter of the way: the eased share of the turn, backward for
    // the backflips and forward for the front flip.
    let share = std::f32::consts::TAU * 0.15625;
    for (trick, angle) in [
        (Trick::Backflip, -share),
        (Trick::DoubleBackflip, -2.0 * share),
        (Trick::FrontFlip, share),
    ] {
        let flipped = at_trick(trick, 0.25);
        let (place, turn) = apart(&flipped, &turned(&m, angle, pelvis));
        assert!(place < 1e-4 && turn < 1e-4, "{trick:?}: {place}, {turn}");
        // Rigid: the rider still on the stick.
        let sk = spartan();
        for side in [LEFT, RIGHT] {
            assert!(foot_off(&sk, &flipped, side) < MM);
            assert!(hand_off(&sk, &flipped, side) < MM);
        }
        let head = dist(flipped.biped[HEAD].position, flipped.stick[1].position);
        let was = dist(m.biped[HEAD].position, m.stick[1].position);
        assert!((head - was).abs() < 1e-4);
    }
    // Backward is the head going back.
    let forward = m_vec(rz(YAW), [1.0, 0.0, 0.0]);
    let head = |p: &Posed| dot(sub(p.biped[HEAD].position, pelvis), forward);
    assert!(head(&at_trick(Trick::Backflip, 0.25)) < head(&m) - 0.05);
    assert!(head(&at_trick(Trick::FrontFlip, 0.25)) > head(&m) + 0.05);
}

#[test]
fn the_no_hander_lets_go_and_keeps_the_feet_on() {
    let sk = spartan();
    let p = at_trick(Trick::NoHander, 0.5);
    for side in [LEFT, RIGHT] {
        assert!(hand_off(&sk, &p, side) > 0.1);
        assert!(foot_off(&sk, &p, side) < MM);
    }
    // Out to its own side.
    let left = m_vec(rz(YAW), [0.0, 1.0, 0.0]);
    let out = |side: usize| {
        dot(
            sub(p.biped[HANDS[side]].position, p.biped[PELVIS].position),
            left,
        )
    };
    assert!(out(LEFT) > 0.15 && out(RIGHT) < -0.15);
}

#[test]
fn the_can_can_takes_the_right_foot_off_and_across() {
    let sk = spartan();
    let p = at_trick(Trick::CanCan, 0.5);
    assert!(foot_off(&sk, &p, LEFT) < MM);
    assert!(foot_off(&sk, &p, RIGHT) > 0.1);
    for side in [LEFT, RIGHT] {
        assert!(hand_off(&sk, &p, side) < MM);
    }
    // Across: the right foot on the left of the shaft.
    let left = m_vec(rz(YAW), [0.0, 1.0, 0.0]);
    let shaft = p.stick[0].point([0.0, shaft_y(&RIG), 0.0]);
    assert!(dot(sub(p.biped[FEET[RIGHT]].position, shaft), left) > 0.0);
}

#[test]
fn the_bar_spin_turns_the_frame_once_about_the_shaft() {
    let sk = spartan();
    let m = mounted();
    let p = at_trick(Trick::BarSpin, 0.5);
    // Half way round: the handlebar the other way, the shaft where it was.
    assert!(dist(p.stick[0].left, scale(m.stick[0].left, -1.0)) < 1e-4);
    assert!(dist(p.stick[0].up, m.stick[0].up) < 1e-4);
    let shaft = [0.0, shaft_y(&RIG), -0.2];
    assert!(dist(p.stick[0].point(shaft), m.stick[0].point(shaft)) < 1e-4);
    // The hands and the feet clear of where the grips and pegs were.
    for side in [LEFT, RIGHT] {
        let hand = HANDS[side];
        let palm = attach(
            &p.biped[hand],
            &sk.bones()[hand].rest,
            mirror(keys().mounted.palm, side),
        );
        let grip = m.stick[0].point([RIG.grip_l, RIG.grip_r][side]);
        assert!(dist(palm, grip) > 0.05);
        let lift = p.biped[FEET[side]].position[2] - m.biped[FEET[side]].position[2];
        assert!((lift - keys().tricks.bar_spin.hop).abs() < 1e-4, "{lift}");
    }
}

#[test]
fn the_superman_holds_the_grips_with_the_feet_out_behind() {
    let sk = spartan();
    let p = at_trick(Trick::Superman, 0.5);
    let forward = m_vec(rz(YAW), [1.0, 0.0, 0.0]);
    for side in [LEFT, RIGHT] {
        assert!(hand_off(&sk, &p, side) < MM, "{}", hand_off(&sk, &p, side));
        assert!(foot_off(&sk, &p, side) > 0.2);
        // Behind the pelvis, which is behind the stick.
        let foot = sub(p.biped[FEET[side]].position, p.biped[PELVIS].position);
        assert!(dot(foot, forward) < -0.2);
    }
    let back = sub(p.biped[PELVIS].position, p.stick[0].position);
    assert!(dot(back, forward) < -0.2);
    // Laid out: the head ahead of the pelvis more than above it.
    let head = sub(p.biped[HEAD].position, p.biped[PELVIS].position);
    assert!(dot(head, forward) > head[2].abs());
}

#[test]
fn a_target_out_of_reach_leaves_the_limb_straight_and_finite() {
    let root = [1.0, 2.0, 3.0];
    let (mid, end) = two_bone(root, [50.0, 2.0, 3.0], 0.15, 0.1, [0.0, 0.0, 1.0]);
    assert!((dist(mid, root) - 0.15).abs() < 1e-4);
    assert!((dist(end, mid) - 0.1).abs() < 1e-3);
    assert!((dist(end, root) - 0.25).abs() < 1e-4);
    // On the root, toward the pole's own line, and not a number at all.
    for target in [root, [1.0, 2.0, 9.0], [f32::NAN, 0.0, 0.0]] {
        let (mid, end) = two_bone(root, target, 0.15, 0.1, [0.0, 0.0, 1.0]);
        assert!(mid.iter().chain(&end).all(|v| v.is_finite()), "{target:?}");
        assert!((dist(mid, root) - 0.15).abs() < 1e-4);
    }
    // A pose whose hands are sent far away.
    let mut far = keys().clone();
    far.tricks.no_hander.hand = [5.0, 5.0, 5.0];
    let sk = spartan();
    let s = sample(1.0, 0.0, 1.0, Some((Trick::NoHander, 0.5)));
    let p = pose(
        &sk,
        &far,
        &RIG,
        &standing(&sk, YAW, AT),
        &carried_stick(AT),
        &s,
    )
    .unwrap();
    assert!(p.biped.iter().all(NodeMatrix::is_finite));
    lengths_kept(&sk, &p);
}

#[test]
fn the_mount_blends_from_the_games_rows_to_the_mounted_ones() {
    let sk = spartan();
    let rows = standing(&sk, YAW, AT);
    let stick = carried_stick(AT);
    let full = posed(&sample(1.0, 0.0, 0.0, None));
    let half = posed(&sample(0.5, 0.0, 0.0, None));
    for (i, r) in half.biped.iter().enumerate() {
        let middle = scale(add(rows[i].position, full.biped[i].position), 0.5);
        assert!(dist(r.position, middle) < 1e-4, "node {i}");
        assert!(square(r));
    }
    let middle = scale(add(stick[0].position, full.stick[0].position), 0.5);
    assert!(dist(half.stick[0].position, middle) < 1e-4);
    assert!(square(&half.stick[0]));
    // A little mounted is a little moved, and more is more.
    let moved = |m: f32| {
        apart(
            &posed(&sample(m, 0.0, 0.0, None)),
            &Posed {
                biped: rows.clone(),
                stick,
            },
        )
        .0
    };
    assert!(moved(0.05) < moved(0.5) && moved(0.5) < moved(1.0));
    assert!(moved(0.05) < 0.02);
}

#[test]
fn other_spellings_of_the_nodes_resolve() {
    let rename = |n: &str| match n {
        "pelvis" => "Bip01 Pelvis".to_owned(),
        "spine" | "spine1" | "neck" | "head" => format!("bip01 {n}"),
        n if n.starts_with("l_") => format!("Bip01 L {}", &n[2..]),
        n if n.starts_with("r_") => format!("Bip01 R {}", &n[2..]),
        n => n.to_owned(),
    };
    let sk = Skeleton::new(bones(&rename), keys());
    let roles = sk.roles().unwrap();
    assert_eq!(roles.pelvis, PELVIS);
    assert_eq!(roles.spine, [1, 2]);
    assert_eq!((roles.neck, roles.head), (Some(3), Some(HEAD)));
    assert_eq!([roles.legs[LEFT].end, roles.legs[RIGHT].end], FEET);
    assert_eq!([roles.arms[LEFT].end, roles.arms[RIGHT].end], HANDS);
    assert!((roles.legs[LEFT].l1 - 0.15).abs() < 1e-6);
    assert!((roles.arms[RIGHT].l2 - 0.09).abs() < 1e-6);
    let s = sample(1.0, 0.0, 0.0, None);
    assert!(
        pose(
            &sk,
            keys(),
            &RIG,
            &standing(&sk, YAW, AT),
            &carried_stick(AT),
            &s
        )
        .is_some()
    );
}

#[test]
fn a_model_missing_a_role_is_not_posed() {
    let s = sample(1.0, 0.0, 0.0, None);
    for (gone, why) in [
        ("l_hand", "left arm"),
        ("r_calf", "right leg"),
        ("pelvis", "pelvis"),
    ] {
        let sk = Skeleton::new(
            bones(&|n| if n == gone { "other".into() } else { n.into() }),
            keys(),
        );
        let said = sk.roles().unwrap_err();
        assert!(said.contains(why), "{gone}: {said}");
        let rows = standing(&sk, YAW, AT);
        assert_eq!(pose(&sk, keys(), &RIG, &rows, &carried_stick(AT), &s), None);
    }
    // No spine, neck or head is still a rider.
    let sk = Skeleton::new(
        bones(&|n| match n {
            "spine" | "spine1" | "neck" | "head" => format!("x_{n}"),
            n => n.to_owned(),
        }),
        keys(),
    );
    assert!(sk.roles().is_ok());
    // Parents that loop.
    let mut looped = bones(&|n| n.to_owned());
    looped[0].parent = Some(1);
    assert!(Skeleton::new(looped, keys()).roles().is_err());
}

#[test]
fn rows_that_are_not_the_skeletons_or_not_finite_are_left_alone() {
    let sk = spartan();
    let s = sample(1.0, 0.0, 0.0, None);
    let rows = standing(&sk, YAW, AT);
    let stick = carried_stick(AT);
    assert_eq!(
        pose(&sk, keys(), &RIG, &rows[..rows.len() - 1], &stick, &s),
        None
    );
    let mut bad = rows.clone();
    bad[6].left[1] = f32::NAN;
    assert_eq!(pose(&sk, keys(), &RIG, &bad, &stick, &s), None);
    let mut bad_stick = stick;
    bad_stick[1].position[0] = f32::INFINITY;
    assert_eq!(pose(&sk, keys(), &RIG, &rows, &bad_stick, &s), None);
    // A sample that is not a number mounts nothing.
    let nan = sample(f32::NAN, 0.0, 0.0, None);
    assert_eq!(pose(&sk, keys(), &RIG, &rows, &stick, &nan), None);
}
