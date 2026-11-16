use super::*;

fn halo1() -> CampaignManifest {
    CampaignManifest::parse(HALO1).unwrap()
}

/// The retail shape on the user's disc, as ids only: campaign 1 with ids
/// in the low hundreds, free slots `-1`.
fn retail() -> (Vec<u32>, Vec<u32>) {
    (
        vec![1, FREE, FREE, FREE],
        vec![0x1, 0x65, 0x69, 0x12d, 0x131, 0x325, FREE, FREE],
    )
}

#[test]
fn the_halo1_manifest_lists_its_ten_levels_in_order() {
    let m = halo1();
    assert_eq!(m.campaign.key, "halo1");
    let maps: Vec<&str> = m.levels.iter().map(|l| l.map.as_str()).collect();
    assert_eq!(
        maps,
        ["a10", "a30", "a50", "b30", "b40", "c10", "c20", "c40", "d20", "d40"]
    );
    assert_eq!(m.levels[3].name, "The Silent Cartographer");
    assert_eq!(m.levels[0].scenario_path(), "scenarios\\solo\\a10\\a10");
    CampaignManifest::parse(FIXTURE).unwrap();
}

#[test]
fn ids_come_after_retail_and_rise_in_play_order() {
    let (c, m) = retail();
    let built = build(&[halo1()], &c, &m).unwrap();
    assert_eq!(built.len(), 1);
    let b = &built[0];
    assert_eq!(b.campaign_id, 2);
    let ids: Vec<u32> = b.levels.iter().map(|l| l.map_id).collect();
    assert_eq!(ids, (0x1001..=0x100a).collect::<Vec<_>>());
    check_built(b).unwrap();
    let mut all: Vec<(u32, u32)> = m.iter().map(|&id| (if id == FREE { FREE } else { 1 }, id)).collect();
    all.extend(b.levels.iter().map(|l| (b.campaign_id, l.map_id)));
    check(&all).unwrap();
}

#[test]
fn campaigns_are_ordered_by_order_then_key_and_never_share_ids() {
    let (c, m) = retail();
    let fixture = CampaignManifest::parse(FIXTURE).unwrap();
    let mut other = halo1();
    other.campaign.key = "halo1b".into();
    other.campaign.order = Some(10);
    for l in &mut other.levels {
        l.map = format!("{}_b", l.map);
        l.scenario = None;
    }
    let built = build(&[fixture, other, halo1()], &c, &m).unwrap();
    let keys: Vec<&str> = built.iter().map(|b| b.key.as_str()).collect();
    assert_eq!(keys, ["halo1", "halo1b", "test"]);
    assert_eq!(
        built.iter().map(|b| b.campaign_id).collect::<Vec<_>>(),
        [2, 3, 4]
    );
    assert_eq!(built[1].levels[0].map_id, 0x2001);
    assert_eq!(built[2].levels[0].map_id, 0x3001);
    let mut all = Vec::new();
    for b in &built {
        check_built(b).unwrap();
        all.extend(b.levels.iter().map(|l| (b.campaign_id, l.map_id)));
    }
    check(&all).unwrap();
}

#[test]
fn a_campaign_id_comes_after_the_largest_retail_one() {
    let built = build(&[halo1()], &[1, 5, FREE], &[]).unwrap();
    assert_eq!(built[0].campaign_id, 6);
    let built = build(&[halo1()], &[], &[]).unwrap();
    assert_eq!(built[0].campaign_id, 1);
}

#[test]
fn repeats_and_taken_ids_are_refused() {
    let (c, m) = retail();
    assert!(build(&[halo1(), halo1()], &c, &m).is_err());
    let mut other = halo1();
    other.campaign.key = "other".into();
    assert!(
        build(&[halo1(), other], &c, &m)
            .unwrap_err()
            .0
            .contains("in two campaigns")
    );
    assert!(build(&[halo1()], &c, &[0x1003]).unwrap_err().0.contains("taken"));
}

#[test]
fn manifests_are_validated() {
    let bad = |edit: &dyn Fn(&mut CampaignManifest)| {
        let mut m = halo1();
        edit(&mut m);
        m.validate().is_err()
    };
    assert!(bad(&|m| m.format = 2));
    assert!(bad(&|m| m.campaign.key = "Halo 1".into()));
    assert!(bad(&|m| m.campaign.name = " ".into()));
    assert!(bad(&|m| m.levels.clear()));
    assert!(bad(&|m| {
        let l = m.levels[0].clone();
        m.levels = vec![l; MAX_LEVELS + 1];
    }));
    assert!(bad(&|m| m.levels[1].map = "a10".into()));
    assert!(bad(&|m| m.levels[1].map = "A30".into()));
    assert!(bad(&|m| m.levels[1].map = "x".repeat(MAX_MAP_NAME + 1)));
    assert!(bad(&|m| m.levels[1].name = String::new()));
    assert!(bad(&|m| m.levels[1].scenario = Some("scenarios\\solo\\a30\\other".into())));
    assert!(bad(&|m| m.levels[1].scenario = Some(format!("{}\\a30", "x".repeat(PATH_LEN)))));
    assert!(bad(&|m| m.levels[1].picture = Some("No Such".into())));
    assert!(!bad(&|m| m.levels[1].scenario = Some("scenarios\\halo1\\a30".into())));
    assert!(CampaignManifest::parse("format = 1\n[campaign]\nkey = \"x\"\nname = \"y\"\nextra = 1\n").is_err());
}

#[test]
fn the_rules_catch_a_shared_id_and_a_full_campaign() {
    assert!(check(&[(1, 5), (2, 5)]).is_err());
    assert!(check(&[(FREE, FREE), (FREE, FREE), (1, 5)]).is_ok());
    let full: Vec<(u32, u32)> = (0..=MAX_LEVELS as u32).map(|i| (2, 0x1001 + i)).collect();
    assert!(check(&full).is_err());
    assert!(check(&full[1..]).is_ok());
    let (c, m) = retail();
    let mut b = build(&[halo1()], &c, &m).unwrap().remove(0);
    b.levels.swap(0, 1);
    assert!(check_built(&b).is_err());
    b.levels.swap(0, 1);
    b.levels[0].map_id = FIRST_SHOWN_ID - 1;
    assert!(check_built(&b).is_err());
}

#[test]
fn records_read_back_field_for_field() {
    let (c, m) = retail();
    let b = build(&[halo1()], &c, &m).unwrap().remove(0);
    let l = &b.levels[2];
    let s = start_record(b.campaign_id, l);
    assert_eq!(
        read_start(&s),
        Some((2, 0x1003, "scenarios\\solo\\a50\\a50".into()))
    );
    assert_eq!(read_start(&free_start()), Some((FREE, FREE, String::new())));
    let u = ui_level_record(b.campaign_id, l, [0x6269_746d, 0xe6ec_04ca]);
    assert_eq!(u.len(), UI_LEVEL_LEN);
    assert_eq!(
        read_ui_level(&u),
        Some((
            2,
            0x1003,
            [0x6269_746d, 0xe6ec_04ca],
            "The Truth and Reconciliation".into()
        ))
    );
    // Every language holds the name; the descriptions are empty.
    for i in 0..9 {
        let at = UI_LEVEL_NAMES + i * 64;
        assert_eq!(utf16_from(&u[at..at + 64]), "The Truth and Reconciliation");
        let at = UI_LEVEL_DESCRIPTIONS + i * 256;
        assert_eq!(utf16_from(&u[at..at + 256]), "");
    }
    let k = ui_campaign_record(&b);
    assert_eq!(k.len(), UI_CAMPAIGN_LEN);
    assert_eq!(read_ui_campaign(&k), Some((2, "Halo: Combat Evolved".into())));
    assert_eq!(UI_CAMPAIGN_DESCRIPTIONS + 9 * 256, UI_CAMPAIGN_LEN);
    assert_eq!(UI_LEVEL_DESCRIPTIONS + 9 * 256, UI_LEVEL_LEN);
}

#[test]
fn long_names_are_cut_to_fit_with_their_nul() {
    let mut m = halo1();
    m.levels[0].name = "é".repeat(40);
    let b = build(&[m], &[1], &[]).unwrap().remove(0);
    let u = ui_level_record(2, &b.levels[0], [0, 0]);
    let (_, _, _, name) = read_ui_level(&u).unwrap();
    assert_eq!(name.chars().count(), 31);
    assert_eq!(&u[UI_LEVEL_NAMES + 62..UI_LEVEL_NAMES + 64], &[0, 0]);
}
