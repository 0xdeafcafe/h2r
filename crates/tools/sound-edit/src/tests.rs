//! Each edit over invented waves (no game data): 1000 Hz clips, so a frame
//! is a millisecond, and a 48 kHz sine for the loudness.
use super::*;

fn mono(pcm: Vec<i16>) -> Wave {
    Wave {
        channels: 1,
        rate: 1000,
        pcm,
    }
}

fn sine(amp: f64, frames: usize) -> Wave {
    Wave {
        channels: 1,
        rate: 48_000,
        pcm: (0..frames)
            .map(|i| (amp * (i as f64 * 1000.0 * std::f64::consts::TAU / 48_000.0).sin()).round() as i16)
            .collect(),
    }
}

fn ramp() -> Wave {
    mono((0..100).collect())
}

fn flat(n: usize) -> Wave {
    mono(vec![1000; n])
}

#[test]
fn trim_keeps_the_range() {
    let w = apply(&ramp(), &Edit::Trim { start_ms: 10.0, end_ms: Some(20.0) }).unwrap();
    assert_eq!(w.pcm, (10..20).collect::<Vec<i16>>());
    let w = apply(&ramp(), &Edit::Trim { start_ms: 95.0, end_ms: None }).unwrap();
    assert_eq!(w.pcm, vec![95, 96, 97, 98, 99]);
    // An end past the clip is the clip's end.
    let w = apply(&ramp(), &Edit::Trim { start_ms: 98.0, end_ms: Some(500.0) }).unwrap();
    assert_eq!(w.pcm, vec![98, 99]);
}

#[test]
fn trim_keeps_whole_frames_of_a_stereo_clip() {
    let w = Wave { channels: 2, rate: 1000, pcm: vec![1, -1, 2, -2, 3, -3, 4, -4] };
    let t = apply(&w, &Edit::Trim { start_ms: 1.0, end_ms: Some(3.0) }).unwrap();
    assert_eq!((t.channels, t.pcm), (2, vec![2, -2, 3, -3]));
}

#[test]
fn a_trim_that_leaves_nothing_is_refused_by_place() {
    let list = [Edit::Gain { db: 0.0 }, Edit::Trim { start_ms: 50.0, end_ms: Some(50.0) }];
    let e = render(&ramp(), &list).unwrap_err();
    assert_eq!(e.index, 1);
    assert!(e.to_string().starts_with("edit 1: trim leaves nothing"), "{e}");
    assert!(apply(&ramp(), &Edit::Trim { start_ms: -1.0, end_ms: None }).is_err());
    assert!(apply(&ramp(), &Edit::Trim { start_ms: f64::NAN, end_ms: None }).is_err());
}

#[test]
fn fade_in_ramps_up_from_silence() {
    let w = apply(&flat(10), &Edit::FadeIn { ms: 4.0 }).unwrap();
    assert_eq!(w.pcm, vec![0, 250, 500, 750, 1000, 1000, 1000, 1000, 1000, 1000]);
}

#[test]
fn fade_out_ramps_down_to_silence() {
    let w = apply(&flat(10), &Edit::FadeOut { ms: 4.0 }).unwrap();
    assert_eq!(w.pcm, vec![1000, 1000, 1000, 1000, 1000, 1000, 750, 500, 250, 0]);
}

#[test]
fn a_fade_longer_than_the_clip_covers_the_clip() {
    let w = apply(&flat(4), &Edit::FadeIn { ms: 400.0 }).unwrap();
    assert_eq!(w.pcm, vec![0, 250, 500, 750]);
}

#[test]
fn a_fade_scales_each_channel() {
    let w = Wave { channels: 2, rate: 1000, pcm: vec![1000, -1000, 1000, -1000] };
    let f = apply(&w, &Edit::FadeIn { ms: 2.0 }).unwrap();
    assert_eq!(f.pcm, vec![0, 0, 500, -500]);
}

#[test]
fn gain_scales_and_holds_to_16_bits() {
    let six = 20.0 * 2f64.log10();
    let w = mono(vec![1000, -1000, 20_000, -20_000]);
    let g = apply(&w, &Edit::Gain { db: six }).unwrap();
    assert_eq!(g.pcm, vec![2000, -2000, 32767, -32768]);
    let g = apply(&w, &Edit::Gain { db: -six }).unwrap();
    assert_eq!(g.pcm, vec![500, -500, 10_000, -10_000]);
    assert!(apply(&w, &Edit::Gain { db: 61.0 }).is_err());
    assert!(apply(&w, &Edit::Gain { db: f64::INFINITY }).is_err());
}

#[test]
fn normalise_brings_the_loudness_to_the_target() {
    let w = sine(8000.0, 48_000);
    let before = w.levels().lufs;
    let n = apply(&w, &Edit::Normalise { lufs: before - 6.0, ceiling_dbfs: -1.0 }).unwrap();
    assert!((n.levels().lufs - (before - 6.0)).abs() < 0.05, "{}", n.levels().lufs);
    // Half the samples, within rounding.
    for (a, b) in w.pcm.iter().zip(&n.pcm) {
        assert!((f64::from(*a) * 0.501_187 - f64::from(*b)).abs() <= 1.0);
    }
}

#[test]
fn normalise_stops_at_the_peak_ceiling() {
    let w = sine(8000.0, 48_000);
    let n = apply(&w, &Edit::Normalise { lufs: 0.0, ceiling_dbfs: -1.0 }).unwrap();
    assert!((n.levels().peak_dbfs - -1.0).abs() < 0.01, "{}", n.levels().peak_dbfs);
    assert!(n.levels().lufs < 0.0);
}

#[test]
fn normalise_refuses_silence() {
    let e = apply(&mono(vec![0; 100]), &Edit::Normalise { lufs: -20.0, ceiling_dbfs: -1.0 }).unwrap_err();
    assert_eq!(e, "the clip is silent");
}

#[test]
fn silence_zeroes_the_range() {
    let w = apply(&ramp(), &Edit::Silence { start_ms: 3.0, end_ms: 6.0 }).unwrap();
    assert_eq!(&w.pcm[..8], &[0, 1, 2, 0, 0, 0, 6, 7]);
    assert_eq!(w.pcm.len(), 100);
    assert!(apply(&ramp(), &Edit::Silence { start_ms: 6.0, end_ms: 3.0 }).is_err());
}

#[test]
fn the_edits_apply_in_order_and_the_source_is_left_alone() {
    let src = flat(10);
    let a = render(&src, &[Edit::Trim { start_ms: 0.0, end_ms: Some(4.0) }, Edit::FadeOut { ms: 4.0 }]).unwrap();
    assert_eq!(a.pcm, vec![750, 500, 250, 0]);
    let b = render(&src, &[Edit::FadeOut { ms: 4.0 }, Edit::Trim { start_ms: 0.0, end_ms: Some(4.0) }]).unwrap();
    assert_eq!(b.pcm, vec![1000; 4]);
    assert_eq!(src, flat(10));
    assert_eq!(render(&src, &[]).unwrap(), src);
}

#[test]
fn undo_and_reorder_give_the_earlier_render() {
    let src = flat(10);
    let mut list = vec![Edit::Trim { start_ms: 0.0, end_ms: Some(4.0) }];
    let first = render(&src, &list).unwrap();
    insert(&mut list, Edit::FadeOut { ms: 4.0 }, None).unwrap();
    insert(&mut list, Edit::Gain { db: -6.0 }, None).unwrap();
    assert_ne!(render(&src, &list).unwrap(), first);
    assert!(matches!(undo(&mut list), Some(Edit::Gain { .. })));
    undo(&mut list);
    assert_eq!(render(&src, &list).unwrap(), first);
    // Reordering swaps the meaning; moving back restores it.
    insert(&mut list, Edit::FadeOut { ms: 4.0 }, None).unwrap();
    let faded = render(&src, &list).unwrap();
    reorder(&mut list, 1, 0).unwrap();
    assert_eq!(render(&src, &list).unwrap().pcm, vec![1000; 4]);
    reorder(&mut list, 0, 1).unwrap();
    assert_eq!(render(&src, &list).unwrap(), faded);
    assert_eq!(undo(&mut Vec::new()), None);
}

#[test]
fn list_operations_check_their_places() {
    let mut list = vec![Edit::Gain { db: 1.0 }, Edit::Gain { db: 2.0 }, Edit::Gain { db: 3.0 }];
    insert(&mut list, Edit::Gain { db: 0.0 }, Some(0)).unwrap();
    assert!(matches!(list[0], Edit::Gain { db } if db == 0.0));
    assert!(insert(&mut list, Edit::Gain { db: 0.0 }, Some(9)).is_err());
    assert!(matches!(remove(&mut list, 0).unwrap(), Edit::Gain { db } if db == 0.0));
    assert!(remove(&mut list, 3).is_err());
    reorder(&mut list, 0, 2).unwrap();
    let dbs: Vec<f64> = list.iter().map(|e| if let Edit::Gain { db } = e { *db } else { 0.0 }).collect();
    assert_eq!(dbs, vec![2.0, 3.0, 1.0]);
    assert!(reorder(&mut list, 0, 3).is_err());
}

#[test]
fn a_wav_round_trips_and_stereo_keeps_its_channels() {
    let w = Wave { channels: 2, rate: 22_050, pcm: vec![1, -2, 3, -4, 32767, -32768] };
    let bytes = w.to_wav();
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(Wave::from_wav(&bytes).unwrap(), w);
    assert_eq!((w.frames(), w.duration_ms()), (3, 0));
    assert_eq!(Wave::from_wav(&mono(vec![5; 2000]).to_wav()).unwrap().duration_ms(), 2000);
}

#[test]
fn a_file_that_is_not_a_clip_says_why() {
    assert_eq!(Wave::from_wav(b"nope").unwrap_err(), "not a RIFF WAVE file");
    assert!(Wave::from_wav(b"version https://git-lfs.github.com/spec/v1\n").unwrap_err().contains("git lfs pull"));
    let mut b = mono(vec![1, 2]).to_wav();
    b[34] = 8; // 8 bits
    assert!(Wave::from_wav(&b).unwrap_err().contains("16-bit PCM"));
}

#[test]
fn peaks_are_the_lowest_and_highest_of_each_column() {
    let w = mono(vec![1, -5, 3, 9, -2, 0]);
    assert_eq!(w.peaks(3), vec![[-5, 1], [3, 9], [-2, 0]]);
    assert_eq!(w.peaks(0), Vec::<[i16; 2]>::new());
}

#[test]
fn the_edit_list_is_yaml() {
    let doc = SoundDoc::from_yaml(
        "class: weapon_fire\nedits:\n- op: trim\n  start_ms: 20\n- op: fade_out\n  ms: 50\n- op: normalise\n  lufs: -18\n",
    )
    .unwrap();
    assert_eq!(doc.class.as_deref(), Some("weapon_fire"));
    assert_eq!(doc.edits[0], Edit::Trim { start_ms: 20.0, end_ms: None });
    assert_eq!(doc.edits[2], Edit::Normalise { lufs: -18.0, ceiling_dbfs: -1.0 });
    assert_eq!(SoundDoc::from_yaml(&doc.to_yaml()).unwrap(), doc);
}

#[test]
fn a_class_only_sidecar_reads_and_writes_as_before() {
    let doc = SoundDoc::from_yaml("class: weapon_fire\n").unwrap();
    assert_eq!(doc.to_yaml(), "class: weapon_fire\n");
    assert_eq!(SoundDoc::from_yaml("").unwrap(), SoundDoc::default());
}

#[test]
fn a_misspelt_field_or_op_is_refused() {
    assert!(SoundDoc::from_yaml("edit: []\n").is_err());
    assert!(SoundDoc::from_yaml("edits:\n- op: reverb\n").is_err());
    assert!(SoundDoc::from_yaml("edits:\n- op: gain\n  db: 1\n  extra: 2\n").is_err());
}

#[test]
fn permutations_are_files_with_weights() {
    let mut doc = SoundDoc::default();
    let a = doc.add_permutation(Permutation::new("sounds/a.wav")).unwrap();
    let b = doc.add_permutation(Permutation::new("sounds/b.wav")).unwrap();
    assert_eq!((a, b), (0, 1));
    doc.set_permutation(1, &PermutationPatch { weight: Some(3.0), gain_db: Some(-2.0), skip_fraction: Some(0.25) }).unwrap();
    assert_eq!((doc.permutations[1].weight, doc.permutations[1].gain_db, doc.permutations[1].skip_fraction), (3.0, -2.0, 0.25));
    assert_eq!(SoundDoc::from_yaml(&doc.to_yaml()).unwrap(), doc);
    doc.reorder_permutations(1, 0).unwrap();
    assert_eq!(doc.permutations[0].file, "sounds/b.wav");
    assert_eq!(doc.remove_permutation(0).unwrap().file, "sounds/b.wav");
    assert_eq!(doc.permutations.len(), 1);
    assert!(doc.remove_permutation(1).is_err());
}

#[test]
fn a_permutation_is_checked() {
    let mut doc = SoundDoc::default();
    doc.add_permutation(Permutation::new("a.wav")).unwrap();
    assert!(doc.add_permutation(Permutation::new("a.wav")).is_err());
    for bad in ["", "../x.wav", "/x.wav"] {
        assert!(doc.add_permutation(Permutation::new(bad)).is_err(), "{bad:?}");
    }
    let mut zero = Permutation::new("z.wav");
    zero.weight = 0.0;
    assert!(doc.add_permutation(zero).is_err());
    assert!(doc.set_permutation(0, &PermutationPatch { skip_fraction: Some(1.5), ..Default::default() }).is_err());
    assert_eq!(doc.permutations[0].skip_fraction, 0.0);
    assert!(SoundDoc::from_yaml("permutations:\n- file: a.wav\n  weight: -1\n").is_err());
}

#[test]
fn a_permutation_has_its_own_edit_list() {
    let mut doc = SoundDoc::default();
    doc.add_permutation(Permutation::new("a.wav")).unwrap();
    insert(doc.edits_mut(Some(0)).unwrap(), Edit::Gain { db: 1.0 }, None).unwrap();
    assert!(doc.edits.is_empty());
    assert_eq!(doc.permutations[0].edits.len(), 1);
    assert!(doc.edits_mut(Some(1)).is_err());
}
