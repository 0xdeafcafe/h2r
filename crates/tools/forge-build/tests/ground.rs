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
use forge_build::{Built, Error, KIT_RECIPE, VERSIONS, build};
use forge_doc::{
    BlockKind, Edit, Endpoint, F32, FORMAT, FieldWrite, Forge, ForgeDoc, GroundRecipe, Label,
    MapRef, Op, OpId, Placement, Recipe, Value,
};
use scenario_edit::MapSource;

const FLAGS: BlockKind = BlockKind::NetgameFlags;

fn source() -> MapSource {
    let mut source = MapSource::file("invented/court.map");
    source.bytes = Some(testkit::court().into());
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
