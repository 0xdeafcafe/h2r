use super::*;
use crate::Kind;

#[test]
fn reads_invented_fields_and_block_descriptions() {
    let plugin = parse(
        "zzzz",
        r#"<plugin game="Halo2Xbox" baseSize="0x20">
            <revisions><entry name="Invented author"/></revisions>
            <tagblock name="Invented collection" offset="0" elementSize="8">
              <enum16 name="Invented phase" offset="0" comment="Our invented scale">
                <option name="Invented small" value="-1"/>
                <option name="Invented large" value="0x40"/>
                <comment>Upper &amp; lower &#x2014; invented.</comment>
              </enum16>
              <bitfield16 name="Invented flags" offset="2">
                <bit name="Invented mark" index="11"/>
              </bitfield16>
              <newElement name="Invented opaque" offset="4" extra="ignored"/>
            </tagblock>
            <uint32 name="Invented hidden" offset="0x10" visible="false"/>
        </plugin>"#,
    )
    .unwrap();
    assert_eq!(plugin.base_size, 32);
    assert_eq!(plugin.fields.len(), 2);
    let block = &plugin.fields[0];
    assert_eq!(block.kind, Kind::Block);
    assert_eq!(block.size, Some(8));
    assert_eq!(block.element_size, Some(8));
    assert!(block.visible);
    let phase = &block.children[0];
    assert_eq!(phase.comment, "Our invented scale\nUpper & lower — invented.");
    assert_eq!(
        phase.options,
        vec![(-1, "Invented small".into()), (64, "Invented large".into())]
    );
    assert_eq!(block.children[1].bits, vec![(11, "Invented mark".into())]);
    assert_eq!(block.children[2].kind, Kind::Unknown("newElement".into()));
    assert_eq!(block.children[2].size, None);
    assert!(!plugin.fields[1].visible);
    assert_eq!(plugin.fields[1].offset, 16);
}

#[test]
fn field_widths_use_retail_pairs_and_declared_lengths() {
    let cases = [
        ("int8", "", 1),
        ("int16", "", 2),
        ("int32", "", 4),
        ("uint8", "", 1),
        ("uint16", "", 2),
        ("uint32", "", 4),
        ("float32", "", 4),
        ("enum8", "", 1),
        ("enum16", "", 2),
        ("enum32", "", 4),
        ("bitfield8", "", 1),
        ("bitfield16", "", 2),
        ("bitfield32", "", 4),
        ("degree", "", 4),
        ("stringId", "", 4),
        ("tagRef", "", 8),
        ("dataref", "", 8),
        ("tagblock", "elementSize='1'", 8),
        ("point2", "", 8),
        ("vector3", "", 12),
        ("colorRGBA", "", 16),
        ("ascii", "length='17'", 17),
        ("ascii", "size='9'", 9),
        ("undefined", "", 4),
        ("undefined", "size='6'", 6),
    ];
    for (kind, attrs, width) in cases {
        let xml = format!(
            "<plugin game='{GAME}' baseSize='42'><{kind} offset='0' {attrs}/></plugin>"
        );
        assert_eq!(
            parse("zzzz", &xml).unwrap().fields[0].size,
            Some(width),
            "{kind}"
        );
    }
}

#[test]
fn retail_type_spellings_preserve_width_and_component_kind() {
    let cases = [
        ("flags8", "", Kind::Bits, Some(1)),
        ("flags16", "", Kind::Bits, Some(2)),
        ("flags32", "", Kind::Bits, Some(4)),
        ("degree2", "", Kind::Degree, Some(8)),
        ("ranged", "", Kind::Degree, Some(8)),
        ("rangef", "", Kind::Bounds, Some(8)),
        ("point16", "", Kind::Int16Array, Some(4)),
        ("range16", "", Kind::Int16Array, Some(4)),
        ("rect16", "", Kind::Int16Array, Some(8)),
        ("quat16", "", Kind::Int16Array, Some(8)),
        ("plane2", "", Kind::Vector, Some(12)),
        ("plane3", "", Kind::Vector, Some(16)),
        ("color32", "alpha='false'", Kind::PackedColour, Some(4)),
        ("color32", "alpha='true'", Kind::PackedColour, Some(4)),
        ("colorf", "alpha='false'", Kind::Colour, Some(12)),
        ("colorf", "alpha='true'", Kind::Colour, Some(16)),
        ("colorf", "", Kind::Colour, None),
        ("datum", "", Kind::Datum, Some(4)),
        ("tagRef", "withGroup='false'", Kind::Datum, Some(4)),
        ("tagRef", "withGroup='true'", Kind::TagRef, Some(8)),
    ];
    for (element, attrs, kind, size) in cases {
        let xml = format!(
            "<plugin game='{GAME}' baseSize='42'><{element} name='Invented value' offset='0' {attrs}/></plugin>"
        );
        let plugin = parse("zzzz", &xml).unwrap();
        assert_eq!(plugin.fields[0].kind, kind, "{element} {attrs}");
        assert_eq!(plugin.fields[0].size, size, "{element} {attrs}");
    }
}

#[test]
fn group_spelling_keeps_case_punctuation_and_trailing_spaces() {
    let short = tags::defs::fourcc_text(tags::defs::parse_fourcc("zZ  ").unwrap());
    for stem in ["zZ", "zZ  ", "zZ%20%20", short.as_str()] {
        assert_eq!(
            file_group(Path::new(&format!("{stem}.xml"))),
            Some(short.clone())
        );
    }
    assert_eq!(file_group(Path::new("zZ!_.xml")).as_deref(), Some("zZ!_"));
    assert_eq!(file_group(Path::new("zZ%21_.xml")).as_deref(), Some("zZ!_"));
    assert_eq!(file_group(Path::new("invented.xml")), None);
}

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        loop {
            let path = std::env::temp_dir().join(format!(
                "tag-names-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("could not create fixture directory: {e}"),
            }
        }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn tree_root_selects_only_retail_and_loads_fields_on_demand() {
    let temp = TestDir::new();
    let retail = temp.0.join(GAME);
    fs::create_dir(&retail).unwrap();
    fs::write(temp.0.join("zzzz.xml"), "invented sibling").unwrap();
    fs::write(retail.join("zZzz.xml"), format!("<plugin game='{GAME}' baseSize='0'/>")).unwrap();
    let bad_field = format!("<plugin game='{GAME}' baseSize='4'><int32 offset='oops'/></plugin>");
    fs::write(retail.join("zZzx.xml"), bad_field).unwrap();
    let folder = Folder::open(&temp.0).unwrap();
    assert_eq!(folder.dir(), fs::canonicalize(&retail).unwrap());
    assert_eq!(folder.groups(), ["zZzx", "zZzz"]);
    assert!(folder.load("zzzz").unwrap().is_none());
    assert_eq!(folder.load("zZzz").unwrap().unwrap().group, "zZzz");
    assert!(folder.load("zZzx").unwrap_err().contains("offset"));
}

#[test]
fn folder_refuses_known_builds_and_any_nonretail_root() {
    let temp = TestDir::new();
    for name in ["Halo2Alpha", "Halo2E3_04", "Halo2Vista"] {
        assert!(Folder::open(&temp.0.join(name)).unwrap_err().contains(name));
    }
    fs::write(temp.0.join("zzzz.xml"), "<plugin game='InventedGame' baseSize='0'/>").unwrap();
    fs::write(temp.0.join("zzzx.xml"), format!("<plugin game='{GAME}' baseSize='0'/>")).unwrap();
    let reason = Folder::open(&temp.0).unwrap_err();
    assert!(reason.contains("InventedGame") && reason.contains(GAME));
}
