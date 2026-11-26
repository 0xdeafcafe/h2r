//! The AI ground a document asks for, on the kit's invented court: no disc
//! bytes. The court's middle flag (the one that is not a stand, so the
//! kit's bases are unchanged) is moved to where a drop is wanted and
//! labelled `ff_drop`; the build must equal the kit's own over the same
//! labels.
use content::Sha256;
use firefight_kit::{
    Mode,
    ground::{self, Labels, Marker},
    testkit::{self, NAME},
};
use forge_build::{Built, ClassTable, Error, KIT_RECIPE, VERSIONS, build, build_with};
use forge_doc::{
    BlockKind, Edit, Endpoint, F32, FORMAT, FieldWrite, Forge, ForgeDoc, GroundClass,
    GroundRecipe, Label, MapRef, Op, OpId, Placement, Recipe, Value,
};
use scenario_edit::MapSource;

const FLAGS: BlockKind = BlockKind::NetgameFlags;

fn source() -> MapSource {
    source_of(testkit::court())
}

fn source_of(bytes: Vec<u8>) -> MapSource {
    let mut source = MapSource::file("invented/court.map");
    source.bytes = Some(bytes.into());
    source
}

fn draft(input: &MapSource) -> ForgeDoc {
    ForgeDoc {
        format: FORMAT,
        builder: VERSIONS,
        base: MapRef {
            map: NAME.into(),
            sha256: Sha256::of(input.bytes.as_deref().unwrap()),
        },
        sources: vec![],
        title: "Court".into(),
        author: "Editor".into(),
        description: "Invented inputs".into(),
        recipes: vec![],
        imports: vec![],
        ops: vec![],
        expect: None,
        waves: None,
    }
}

fn materialise(doc: &ForgeDoc, source: &MapSource) -> Result<Built, Error> {
    build(doc, |name| (name == NAME).then(|| source.clone()))
}

fn recipe() -> GroundRecipe {
    GroundRecipe {
        kit_version: firefight_kit::KIT_VERSION,
        home: 0,
        probe: false,
        classes: vec![],
    }
}

fn op(id: u32, edit: Edit) -> Op {
    Op { id: OpId(id), edit }
}

/// Flag `index` moved to (`x`, `y`, 0).
fn move_flag(id: u32, index: u32, x: f32, y: f32) -> Op {
    let f = |v: f32| F32::new(v).unwrap();
    op(
        id,
        Edit::Placement(Placement::Set {
            block: FLAGS,
            index,
            write: FieldWrite {
                field: "position".into(),
                value: Value::Vec3([f(x), f(y), f(0.)]),
            },
        }),
    )
}

fn label(id: u32, index: u32, text: &str) -> Op {
    op(
        id,
        Edit::Forge(Forge::Label {
            target: Endpoint::base(FLAGS, index),
            label: Some(Label {
                text: text.into(),
                number: None,
            }),
        }),
    )
}

/// The court with its middle flag moved to a drop of the kit's own ground
/// tests, and labelled.
fn dropped(source: &MapSource) -> ForgeDoc {
    let mut doc = draft(source);
    doc.ops = vec![move_flag(1, 1, 9., 6.), label(2, 1, "ff_drop")];
    doc.format = doc.required_format();
    doc
}

fn drop_at(x: f32, y: f32) -> Marker {
    Marker {
        position: [x, y, 0.],
        facing: 0.,
    }
}

#[test]
fn materialising_a_document_equals_the_kits_build_of_the_same_inputs() {
    let source = source();
    let mut doc = dropped(&source);
    // Without the recipe the labels are document data: the map is the one
    // the edits alone make, and no ground is built.
    let plain = materialise(&doc, &source).unwrap();
    assert!(plain.ground.is_none());

    doc.recipes.push(recipe().to_recipe());
    let built = materialise(&doc, &source).unwrap();
    let labels = Labels {
        drops: vec![drop_at(9., 6.)],
        ..Labels::default()
    };
    let expected = ground::build(
        plain.map_bytes(),
        NAME,
        &labels,
        &ground::Recipe::new(Mode::Kit),
    )
    .unwrap();
    assert_eq!(built.map_bytes(), expected.bytes.as_slice());
    let report = built.ground.as_ref().unwrap();
    assert_eq!(report.zones, expected.zones);
    assert_eq!(report.squads, expected.squads);
    assert_eq!(report.receipt, expected.receipt);
    assert_eq!(report.problems, expected.problems);
    assert_eq!(report.zones, 1, "one zone for each placed drop");
    assert_ne!(built.map_bytes(), plain.map_bytes());
}

#[test]
fn two_machines_build_the_same_map() {
    let source = source();
    let mut doc = dropped(&source);
    doc.recipes.push(recipe().to_recipe());
    let first = materialise(&doc, &source).unwrap();
    let second = materialise(&doc, &source).unwrap();
    assert_eq!(first.map_bytes(), second.map_bytes());
    assert_eq!(first.manifest(), second.manifest());
}

#[test]
fn a_moved_drop_moves_its_zone_and_the_map() {
    let source = source();
    let mut doc = dropped(&source);
    doc.recipes.push(recipe().to_recipe());
    let before = materialise(&doc, &source).unwrap();
    doc.ops[0] = move_flag(1, 1, 10., 7.);
    let after = materialise(&doc, &source).unwrap();
    assert_ne!(before.map_bytes(), after.map_bytes());
    assert_ne!(
        before.ground.unwrap().receipt.labels,
        after.ground.unwrap().receipt.labels
    );
}

#[test]
fn bases_that_are_not_one_for_each_team_are_refused_by_label() {
    let source = source();
    let mut doc = draft(&source);
    // Flags 0 and 1 are both team 0 on the court.
    doc.ops = vec![label(1, 0, "ff_base"), label(2, 1, "ff_base")];
    doc.format = doc.required_format();
    doc.recipes.push(recipe().to_recipe());
    let error = materialise(&doc, &source).err().unwrap();
    assert!(
        matches!(&error, Error::Ground(why) if why.contains("ff_base")),
        "{error}"
    );
}

#[test]
fn a_recipe_for_another_kit_is_refused_before_anything_is_built() {
    let source = source();
    let mut doc = dropped(&source);
    doc.recipes.push(
        GroundRecipe {
            kit_version: firefight_kit::KIT_VERSION + 1,
            ..recipe()
        }
        .to_recipe(),
    );
    let error = materialise(&doc, &source).err().unwrap();
    assert!(
        matches!(&error, Error::Ground(why) if why.contains("kit")),
        "{error}"
    );
}

#[test]
fn the_ground_and_the_session_kit_are_not_built_together() {
    let source = source();
    let mut doc = dropped(&source);
    doc.recipes.push(recipe().to_recipe());
    doc.recipes.push(Recipe::Generate {
        generator: KIT_RECIPE.into(),
        parameters: vec![],
    });
    let error = materialise(&doc, &source).err().unwrap();
    assert!(matches!(error, Error::Recipe(_)));
}

#[test]
fn a_ground_label_off_a_flag_is_a_document_error() {
    let source = source();
    let mut doc = draft(&source);
    doc.ops = vec![op(
        1,
        Edit::Forge(Forge::Label {
            target: Endpoint::base(BlockKind::StartingLocations, 0),
            label: Some(Label {
                text: "ff_drop".into(),
                number: None,
            }),
        }),
    )];
    doc.format = doc.required_format();
    doc.recipes.push(recipe().to_recipe());
    assert!(matches!(
        materialise(&doc, &source).err().unwrap(),
        Error::Document(_)
    ));
}

/// The session numbers zones with `forge_doc::waves::zone_order` over its
/// markers; the kit numbers them with `Labels::canonical`. Over invented
/// drops, ties and signed zeros included, the two give one order.
#[test]
fn the_documents_zone_order_is_the_kits() {
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    // A few coordinates so positions tie and the facing decides.
    let coords = [-2.5_f32, -0.0, 0.0, 1.0, 7.25];
    for case in 0..200 {
        let count = (next() % 13) as usize;
        let drops: Vec<(usize, [f32; 3], f32)> = (0..count)
            .map(|i| {
                let mut pick = || coords[(next() % coords.len() as u64) as usize];
                (i, [pick(), pick(), pick()], pick())
            })
            .collect();
        let labels = Labels {
            drops: drops
                .iter()
                .map(|&(_, position, facing)| Marker { position, facing })
                .collect(),
            ..Labels::default()
        };
        let kit: Vec<(Vec<u32>, u32)> = labels
            .canonical()
            .drops
            .iter()
            .map(|m| (m.position.map(f32::to_bits).to_vec(), m.facing.to_bits()))
            .collect();
        let ours: Vec<(Vec<u32>, u32)> = forge_doc::waves::zone_order(&drops)
            .into_iter()
            .map(|i| {
                let (_, position, facing) = drops[i];
                (position.map(f32::to_bits).to_vec(), facing.to_bits())
            })
            .collect();
        assert_eq!(ours, kit, "case {case}");
    }
}

/// The classes a document's wave set may name are the Elite, then the
/// kit's retail table in its order.
#[test]
fn the_documents_classes_are_the_kits() {
    let kit: Vec<&str> = std::iter::once("elite")
        .chain(firefight_kit::retail::CLASSES.iter().map(|c| c.name))
        .collect();
    assert_eq!(forge_doc::waves::CLASSES.to_vec(), kit);
    assert_eq!(forge_doc::waves::GROUND_CLASSES, ["elite"]);
}

/// The invented donor map, as a machine's disc gives it.
fn donor_source() -> MapSource {
    let mut source = MapSource::file("invented/donor.map");
    source.bytes = Some(testkit::donor().into());
    source
}

/// `names` from the invented donor, by its name and hash.
fn with_classes(names: &[&str]) -> GroundRecipe {
    let donor = MapRef {
        map: testkit::DONOR.into(),
        sha256: Sha256::of(&testkit::donor()),
    };
    GroundRecipe {
        classes: names
            .iter()
            .map(|name| GroundClass {
                name: (*name).into(),
                donor: donor.clone(),
            })
            .collect(),
        ..recipe()
    }
}

/// The class table of the invented donor: its schema and `char` paths.
fn table(schema: &tags::schema::Schema) -> ClassTable<'_> {
    ClassTable {
        schema,
        paths: testkit::donor_classes()
            .into_iter()
            .map(|c| (c.name, c.path))
            .collect(),
    }
}

/// `doc` built over `base` and the invented donor with the donor's table.
fn with_donor(doc: &ForgeDoc, base: &MapSource, donor: &MapSource) -> Result<Built, Error> {
    let schema = testkit::donor_schema();
    build_with(doc, None, &table(&schema), |name| match name {
        NAME => Some(base.clone()),
        testkit::DONOR => Some(donor.clone()),
        _ => None,
    })
}

#[test]
fn a_document_with_classes_builds_the_kits_stage_two_ground() {
    let source = source();
    let plain = materialise(&dropped(&source), &source).unwrap();
    let mut doc = dropped(&source);
    doc.recipes.push(with_classes(&["grunt", "jackal"]).to_recipe());
    let built = with_donor(&doc, &source, &donor_source()).unwrap();

    let (donor, schema) = (testkit::donor(), testkit::donor_schema());
    let all = testkit::donor_classes();
    let classes = firefight_kit::Classes {
        schema: &schema,
        donors: vec![firefight_kit::Donor {
            name: testkit::DONOR,
            bytes: &donor,
        }],
        specs: all[..2].to_vec(),
        cut_to_fit: true,
    };
    let labels = Labels {
        drops: vec![drop_at(9., 6.)],
        ..Labels::default()
    };
    let expected = ground::build_with(
        plain.map_bytes(),
        NAME,
        &labels,
        &ground::Recipe::new(Mode::Kit),
        Some(&classes),
    )
    .unwrap();
    assert_eq!(built.map_bytes(), expected.bytes.as_slice());
    let report = built.ground.as_ref().unwrap();
    assert_eq!(report.classes, expected.classes);
    assert_eq!(report.costs, expected.costs);
    assert!(report.cut.is_empty());
    assert_eq!(
        report.classes.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["grunt", "jackal"]
    );
    // The classes are in the map: it is not the stage-one ground.
    let stage_one = {
        let mut doc = dropped(&source);
        doc.recipes.push(recipe().to_recipe());
        materialise(&doc, &source).unwrap()
    };
    assert_ne!(built.map_bytes(), stage_one.map_bytes());
}

#[test]
fn a_donor_of_another_hash_is_refused_by_name() {
    let source = source();
    let mut doc = dropped(&source);
    doc.recipes.push(with_classes(&["grunt"]).to_recipe());
    // This machine's donor is another map under the same name.
    let mut other = donor_source();
    let mut bytes = testkit::donor();
    bytes.push(0);
    other.bytes = Some(bytes.into());
    let error = with_donor(&doc, &source, &other).err().unwrap();
    assert!(
        matches!(&error, Error::SourceHash { map, .. } if map == testkit::DONOR),
        "{error}"
    );
    assert!(error.to_string().contains("donor.map"), "{error}");
    // And a machine with no such map is refused by its name.
    let schema = testkit::donor_schema();
    let error = build_with(&doc, None, &table(&schema), |name| {
        (name == NAME).then(|| source.clone())
    })
    .err()
    .unwrap();
    assert!(
        matches!(&error, Error::MissingMap(map) if map == testkit::DONOR),
        "{error}"
    );
}

#[test]
fn classes_cut_from_the_end_and_the_report_names_them() {
    let source = source_of(testkit::tight(&testkit::court(), 0x2000));
    let mut doc = dropped(&source);
    doc.recipes
        .push(with_classes(&["grunt", "jackal", "hunter"]).to_recipe());
    let built = with_donor(&doc, &source, &donor_source()).unwrap();
    let report = built.ground.as_ref().unwrap();
    assert_eq!(report.cut, ["hunter"]);
    assert_eq!(
        report.classes.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["grunt", "jackal"]
    );
    assert!(report.costs.tag_growth <= report.costs.available);
    assert_eq!(
        report.costs.classes,
        report.classes.iter().map(|c| c.bytes).sum::<usize>()
    );
}

#[test]
fn two_builds_of_one_recipe_are_byte_equal() {
    let source = source();
    let mut doc = dropped(&source);
    doc.recipes
        .push(with_classes(&["grunt", "jackal", "hunter"]).to_recipe());
    // Each machine reads its own copy of the base and the donor.
    let first = with_donor(&doc, &source, &donor_source()).unwrap();
    let second = with_donor(&doc, &source_of(testkit::court()), &donor_source()).unwrap();
    assert_eq!(first.map_bytes(), second.map_bytes());
    assert_eq!(first.manifest(), second.manifest());
    let (a, b) = (first.ground.unwrap(), second.ground.unwrap());
    assert_eq!(a.classes, b.classes);
    assert_eq!(a.cut, b.cut);
    assert_eq!(a.costs, b.costs);
}
