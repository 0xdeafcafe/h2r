use super::*;
use crate::map_select::Map;

fn maps() -> Catalogue {
    Catalogue::new([Map {
        id: 50,
        name: "Lockout".into(),
        path: r"scenarios\multi\lockout\lockout".into(),
        sort: 0,
    }])
}

#[test]
fn validate_config_before_asking_retail() {
    let maps = maps();
    let parse = |name: &str, variant: Option<&str>, max_players, live| {
        Config::parse(
            OpenParams {
                synchronous: false,
                map: name.to_owned(),
                variant: variant.map(str::to_owned),
                max_players,
                live,
            },
            &maps,
            &Variants::default(),
            false,
        )
    };
    assert_eq!(
        parse("LOCKOUT", None, 1, false).unwrap(),
        Config {
            map: 50,
            variant: Variant::Stock(sim_bench::config::GameType::Slayer),
            max_players: 1,
            live: false,
        }
    );
    assert!(matches!(
        parse("lockout", Some("Team Slayer"), 16, false)
            .unwrap()
            .variant,
        Variant::Named(_)
    ));
    for (name, variant, max) in [
        ("lockout", None, 0),
        ("lockout", None, 17),
        ("", None, 1),
        ("mainmenu", None, 1),
        ("lockout", Some(""), 1),
        ("lockout", Some("unknown"), 1),
        ("lockout", Some("../../slayer"), 1),
    ] {
        assert_eq!(
            parse(name, variant, max, false).unwrap_err().code,
            code::INVALID_PARAMS
        );
    }
    assert_eq!(
        parse("lockout", None, 2, true).unwrap_err().code,
        code::UNAVAILABLE
    );
}

#[test]
fn a_map_name_shared_by_two_packages_requires_the_scenario_path() {
    let maps = Catalogue::new([1, 2].map(|id| Map {
        id,
        name: "Arena".into(),
        path: format!("t:\\$C\\{id}\\arena"),
        sort: 0,
    }));
    let p = OpenParams {
        synchronous: false,
        map: "arena".into(),
        variant: None,
        max_players: 2,
        live: true,
    };
    assert_eq!(
        Config::parse(p.clone(), &maps, &Variants::default(), true)
            .unwrap_err()
            .code,
        code::INVALID_PARAMS
    );
    let config = Config::parse(
        OpenParams {
            map: r"t:\$C\2\arena".into(),
            ..p
        },
        &maps,
        &Variants::default(),
        true,
    )
    .unwrap();
    assert_eq!((config.map, config.class()), (2, 2));
}

#[test]
fn variant_free_setup_is_opt_in_and_system_link_only() {
    let params = OpenParams {
        map: "lockout".into(),
        variant: None,
        synchronous: true,
        max_players: 2,
        live: false,
    };
    let parse = |params| Config::parse(params, &maps(), &Variants::default(), true);
    let config = parse(params.clone()).unwrap();
    assert_eq!(config.variant, Variant::Absent);
    assert_eq!(config.protocol(), 1);
    for invalid in [
        OpenParams { live: true, ..params.clone() },
        OpenParams { variant: Some("slayer".into()), ..params },
    ] {
        assert_eq!(parse(invalid).unwrap_err().code, code::INVALID_PARAMS);
    }
}
