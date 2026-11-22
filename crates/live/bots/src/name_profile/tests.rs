use super::*;

fn values(w: Weights) -> [f32; 12] {
    [w.radar_use, w.noob_combo, w.dual_wield, w.aggression, w.grenades, w.melee,
     w.weapon_preference, w.range_preference, w.strafe, w.jump, w.reaction_jitter, w.retreat]
}

#[test]
fn ascii_folded_utf8_hash_has_golden_vectors() {
    for (name, hash) in [("", 0xcbf2_9ce4_8422_2325), ("foobar", 0x8594_4171_f739_67e8)] {
        let p = NameProfile::for_name(name);
        assert_eq!(p.seed, hash);
        assert_eq!(p, NameProfile::for_name(name));
        assert_eq!(p.summary(), NameProfile::for_name(name).summary());
    }
    assert_eq!(NameProfile::for_name("Sample"), NameProfile::for_name("sample"));
    assert_ne!(NameProfile::for_name("Sample"), NameProfile::for_name("Sample "));
    assert_ne!(NameProfile::for_name("é"), NameProfile::for_name("e\u{301}"));
}

#[test]
fn each_named_weight_stays_inside_its_skill_band_under_both_menu_extremes() {
    let mut archetypes = [0; 6];
    for n in 0..1024 {
        let p = NameProfile::for_name(&format!("Player {n}"));
        archetypes[p.archetype as usize] += 1;
        assert!(p.try_hard > 0.0 && p.try_hard < 1.0);
        for skill in Skill::ALL {
            for bias in [-0.35, 0.0, 0.35] {
                let t = Tuning { push_bias: bias, twitchy: true, ..Default::default() };
                for (trait_, x) in TRAITS.iter().zip(values(p.styled_weights(skill, &t))) {
                    let (lo, hi) = trait_.bands[skill as usize];
                    assert!(lo < x && x < hi, "{n} {skill:?} {lo} < {x} < {hi}");
                }
            }
        }
    }
    assert!(archetypes.into_iter().all(|count| count > 10));
}

#[test]
fn latent_pressure_correlates_with_continuous_choices_in_both_directions() {
    const N: u32 = 8192;
    let mut sum_x = 0.0;
    let mut sum_xx = 0.0;
    let mut sum_y = [0.0; 12];
    let mut sum_yy = [0.0; 12];
    let mut sum_xy = [0.0; 12];
    let mut min = [1.0_f32; 12];
    let mut max = [0.0_f32; 12];
    let mut midweights = std::collections::BTreeSet::new();
    for n in 0..N {
        let p = NameProfile::for_name(&format!("Player {n}"));
        let x = f64::from(p.try_hard);
        let w = p.weights(Skill::Normal);
        sum_x += x;
        sum_xx += x * x;
        midweights.insert(w.radar_use.to_bits());
        for (i, weight) in values(w).into_iter().enumerate() {
            let y = f64::from(weight);
            sum_y[i] += y;
            sum_yy[i] += y * y;
            sum_xy[i] += x * y;
            min[i] = min[i].min(weight);
            max[i] = max[i].max(weight);
        }
    }
    assert!(midweights.len() > 8000);
    let samples = f64::from(N);
    for (i, trait_) in TRAITS.iter().enumerate() {
        let covariance = sum_xy[i] - sum_x * sum_y[i] / samples;
        let xx = sum_xx - sum_x * sum_x / samples;
        let yy = sum_yy[i] - sum_y[i] * sum_y[i] / samples;
        let pearson = covariance / (xx * yy).sqrt();
        let c = f64::from(trait_.correlation);
        let lower_bound = 0.65 * c.abs();
        assert!(pearson * c.signum() > lower_bound, "row {i}: Pearson r={pearson}");
        let (lo, hi) = trait_.bands[Skill::Normal as usize];
        let tail = 0.1 * (hi - lo);
        assert!(min[i] < lo + tail && max[i] > hi - tail, "row {i}: missing a tail");
    }
}

#[test]
fn equipment_range_stays_usable_for_any_named_preference() {
    for name in ["Example 1", "Example 2", "Example 3"] {
        let w = NameProfile::for_name(name).weights(Skill::Easy);
        for band in [(1.5, 8.0), (12.0, 60.0), (0.0, 1.5), (2.0, 2.0)] {
            let (near, far) = w.spacing(band);
            assert!(band.0 <= near && near <= far && far <= band.1);
        }
    }
}
