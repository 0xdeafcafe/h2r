use firefight::{Count, Def, Drop, FORMAT, Limits, MAX_DOC_BYTES, Refusal, builtin, validate};
use serde_json::{Value, json};

fn fixture() -> (String, Limits) {
    let [def, _, _] = builtin::definitions();
    (def.canonical_json().unwrap(), Limits::new(["elite"]))
}

#[test]
fn the_byte_limit_applies_before_parsing_and_hashing() {
    let (base, limits) = fixture();
    let mut bytes = vec![b' '; MAX_DOC_BYTES - base.len()];
    bytes.extend(base.as_bytes());
    Def::from_bytes(&bytes, &limits).unwrap();
    bytes.push(b' ');
    let refusal = Refusal::Limit {
        field: "document bytes",
        max: MAX_DOC_BYTES,
    };
    assert_eq!(Def::from_bytes(&bytes, &limits), Err(refusal.clone()));

    // An authored document can also exceed the byte limit while fitting its
    // individual collection bounds and the actor budget.
    let [mut doc, _, _] = builtin::definitions();
    let class = "a".repeat(32);
    doc.scoring.by_class = [(class.clone(), 1)].into();
    doc.caps.live_actors_by_players = vec![(16, 16)];
    let mut round = doc.rounds[0].clone();
    round.skulls_on.clear();
    let mut squad = round.waves[0].squads[0].clone();
    squad.class = class.clone();
    squad.count = Count {
        per_player: 0,
        min: 1,
        max: 1,
    };
    squad.drop = Drop::PreferZone(11);
    for wave in &mut round.waves {
        wave.squads = vec![squad.clone(); 16];
    }
    doc.rounds = vec![round; 12];
    assert_eq!(validate(&doc, &Limits::new([class])), Err(refusal.clone()));
    assert_eq!(doc.canonical_json(), Err(refusal.clone()));
    assert_eq!(doc.id(), Err(refusal));
}

#[test]
fn malformed_documents_are_refused_at_the_wire_boundary() {
    let (base, limits) = fixture();
    let cases = [
        base.replace("\"format\":1", "\"format\":1,\"format\":1"),
        base.replace("\"format\":1", "\"format\":1,\"data\":[1,2,3]"),
        base.replace("\"max\":5", "\"max\":5,\"raw_count\":15"),
        base.replace("\"max\":5", "\"max\":-1"),
        base.replace("\"max\":5", "\"max\":5.0"),
        base.replace("\"max\":5", "\"max\":65536"),
        base.replace("\"gap_ticks\":600", "\"gap_ticks\":1e300"),
        base.replace("\"gap_ticks\":600", "\"gap_ticks\":4294967296"),
        base.replace(r#""drop":"edge""#, r#""drop":{"teleport":0}"#),
        base.replace(
            r#""drop":"edge""#,
            r#""drop":{"prefer_zone":0,"prefer_zone":1}"#,
        ),
        base.replace(
            r#""skulls_on":["blind"]"#,
            r#""skulls_on":["unimplemented"]"#,
        ),
        base.replace(r#""elite":100"#, r#""elite":100,"\u0065lite":0"#),
        base.replace(r#""blind":1100"#, r#""blind":1100,"blind":1100"#),
        base.replace(r#""heroic":1250,"#, ""),
        base.replace(r#""name":"Holdout""#, r#""name":"\ud800""#),
        base.replace(
            r#""name":"Holdout""#,
            &format!(r#""name":{}0{}"#, "[".repeat(512), "]".repeat(512)),
        ),
        format!("{base}{{}}"),
        base[..base.len() - 1].into(),
    ];
    for bytes in cases {
        assert_ne!(bytes, base);
        assert!(
            matches!(
                Def::from_bytes(bytes.as_bytes(), &limits),
                Err(Refusal::Json(_))
            ),
            "{bytes}"
        );
    }
    let version = FORMAT + 1;
    let bytes = base.replace("\"format\":1", &format!("\"format\":{version}"));
    assert_eq!(
        Def::from_bytes(bytes.as_bytes(), &limits),
        Err(Refusal::Format(version))
    );
    assert!(matches!(
        Def::from_bytes(b"\xff", &limits),
        Err(Refusal::Json(_))
    ));
}

fn json_refusal(bytes: &str, limits: &Limits) -> String {
    match Def::from_bytes(bytes.as_bytes(), limits) {
        Err(Refusal::Json(message)) => message,
        result => panic!("expected a JSON refusal, got {result:?}"),
    }
}

#[test]
fn decoding_stops_at_the_first_excess_array_element() {
    let (base, limits) = fixture();
    let original: Value = serde_json::from_str(&base).unwrap();
    let cases: [(&str, usize); 5] = [
        ("/rounds", 12),
        ("/rounds/0/waves", 5),
        ("/rounds/0/waves/0/squads", 16),
        ("/rounds/0/skulls_on", 9),
        ("/caps/live_actors_by_players", 16),
    ];
    for (path, cap) in cases {
        let mut json = original.clone();
        let sequence = json.pointer_mut(path).unwrap().as_array_mut().unwrap();
        sequence.resize(cap, sequence[0].clone());
        sequence.push(Value::Null);
        let message = json_refusal(&json.to_string(), &limits);
        assert!(
            message.contains(&format!("at most {cap} items")),
            "{path}: {message}"
        );
    }
}

#[test]
fn decoding_stops_before_an_excess_scoring_value() {
    let (base, limits) = fixture();
    let mut json: Value = serde_json::from_str(&base).unwrap();
    let scores = json["scoring"]["by_class"].as_object_mut().unwrap();
    scores.clear();
    for index in 0..32 {
        scores.insert(format!("fixture_{index:02}"), json!(1));
    }
    // There are 32 earlier valid entries. The next value would have the wrong type.
    scores.insert("zz_excess".into(), Value::Null);
    let message = json_refusal(&json.to_string(), &limits);
    assert!(message.contains("at most 32 entries"), "{message}");
}
