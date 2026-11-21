use super::*;

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn an_enum_lists_its_named_values_and_search_narrows_them() {
    let all = names(&["off", "_", "on", "", "full_auto"]);
    let listed = values(&all, "");
    assert_eq!(
        listed,
        [
            Choice {
                value: 0,
                name: "Off".into()
            },
            Choice {
                value: 2,
                name: "On".into()
            },
            Choice {
                value: 4,
                name: "Full auto".into()
            },
        ]
    );
    let auto: Vec<u32> = values(&all, "AUTO full").iter().map(|c| c.value).collect();
    assert_eq!(auto, [4]);
    let by_number: Vec<u32> = values(&all, "2").iter().map(|c| c.value).collect();
    assert_eq!(by_number, [2]);
}

#[test]
fn switches_list_every_bit_named_or_not() {
    let all = bits(&names(&["loud", "_", "quiet"]), 8);
    assert_eq!(all.len(), 8);
    assert_eq!(all[0].name.as_deref(), Some("loud"));
    assert_eq!(all[0].label, "Loud");
    assert_eq!(
        (all[1].name.as_deref(), all[1].label.as_str()),
        (None, "Bit 1")
    );
    assert_eq!(all[7].label, "Bit 7");
}

#[test]
fn a_typed_number_must_fit_the_field() {
    assert_eq!(number("9", 2), Ok(9));
    assert_eq!(number(" 0xff ", 1), Ok(255));
    assert_eq!(number("65535", 2), Ok(65535));
    assert_eq!(
        number("65536", 2),
        Err("Type a whole number from 0 to 65535.".into())
    );
    assert!(number("-1", 2).is_err());
    assert!(number("auto", 4).is_err());
}

#[test]
fn decimal_is_offered_for_four_bytes_only() {
    assert_eq!(views(4), View::ALL);
    assert_eq!(views(2), [View::Int, View::Hex]);
}

/// 5,000 tags of one kind and one of another, in an invented map: the
/// reference list has every tag the kind allows, with no cap, and search
/// and the kind narrow it.
#[test]
fn a_reference_lists_every_allowed_tag_with_no_cap() {
    use map::{
        CacheType, Map,
        build::{Block, TagPayload, TagSet},
    };
    const COUNT: usize = 5_000;
    let demo = FourCC(*b"demo");
    let other = FourCC(*b"othr");
    let mut set = TagSet::new(CacheType::Multiplayer, "many_tags", "test");
    set.add_group(demo, None, None);
    set.add_group(other, None, None);
    for i in 0..COUNT {
        set.add_tag(
            demo,
            &format!("things\\thing_{i:04}"),
            TagPayload::Built(Block::zeroed(8)),
        )
        .unwrap();
    }
    set.add_tag(
        other,
        "things\\odd_one",
        TagPayload::Built(Block::zeroed(8)),
    )
    .unwrap();
    let (bytes, _) = set.build().unwrap();
    let mut doc = TagDoc::default();
    doc.load(&mut Map::from_reader(std::io::Cursor::new(bytes)).unwrap())
        .unwrap();

    assert_eq!(tags(&doc, &[demo], None, "").len(), COUNT);
    assert_eq!(kinds(&doc, &[demo]), [demo]);
    assert_eq!(
        tags(&doc, &[], None, "").len(),
        COUNT + 1,
        "no groups: any tag"
    );
    assert_eq!(kinds(&doc, &[]), [demo, other]);
    assert_eq!(tags(&doc, &[], Some(other), "").len(), 1);
    let found: Vec<&str> = tags(&doc, &[demo], None, "thing 4999")
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(found, ["things\\thing_4999"]);
    assert!(tags(&doc, &[demo], None, "odd").is_empty());
}
