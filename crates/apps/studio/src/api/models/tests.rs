//! `model.*` on the Pogo kit's invented map (`content_kit::fixture`, no game
//! data): the kit's weapons listed and read, a look with its first-person
//! block taken out found by `model.check` and refused by a first-person
//! render (the check that catches a weapon whose first-person view goes
//! black), the view the kit copies from the fixture pistol checked, and a
//! weapon held in its own model checked and drawn. Renders skip
//! their pictures where there is no GPU; refusals answer on any machine.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use map::{Map, TagId};
use map::build::{Block, TagPayload, TagSet};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::super::{Api, Reply};
use crate::App;
use crate::app::Action;

/// Writes `bytes` to `path` and opens it as the source (a lone map).
pub(crate) fn open_court(app: &mut App, path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    app.dispatch(Action::OpenSource(path.to_path_buf()));
}

struct Dir(PathBuf);
impl Dir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-model-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(dir)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An app with `bytes` open as `court.map`.
fn court(tag: &str, bytes: &[u8]) -> (Dir, App, Api) {
    let d = Dir::new(tag);
    let mut app = App::new();
    app.set_data_root(d.path("data"));
    open_court(&mut app, &d.path("court.map"), bytes);
    (d, app, Api::new())
}

fn call(api: &mut Api, app: &mut App, method: &str, params: Value) -> Value {
    match api.call(app, method, params.clone()) {
        Ok(v) => v,
        Err(e) => panic!("{method} {params}: {e}"),
    }
}

fn refusal(r: Reply) -> (i64, Value) {
    match r {
        Ok(v) => panic!("expected an error, got {v}"),
        Err(e) => (e.code, e.data.unwrap_or(Value::Null)),
    }
}

/// The kit's invented map with the UNSC look's first-person block replaced
/// by `elements` (the model of each, no animations); none leaves it empty.
fn unsc_first_person(models: impl Fn(&content_kit::kit::Tags) -> Vec<TagId>) -> Vec<u8> {
    let mut map = Map::from_reader(Cursor::new(content_kit::fixture::fixture(0))).unwrap();
    let facts =
        content_kit::kit::Facts::read_with_class(&mut map, content_kit::fixture::fixture_class())
            .unwrap();
    let mut set = TagSet::from_map(&mut map).unwrap();
    let looks = content_kit::kit::install_all(&mut set, &facts).unwrap();
    let unsc = looks[content_kit::Style::Unsc.index()];
    let models = models(&unsc);
    let entry = set.tag_mut(unsc.weapon).expect("the weap");
    let TagPayload::Built(weap) = &mut entry.payload else {
        panic!("the kit builds its weap");
    };
    weap.fixups.retain(|f| f.offset != 0x2a8);
    if !models.is_empty() {
        let mut block = Block::zeroed(0x10 * models.len());
        for (i, model) in models.iter().enumerate() {
            block.tag_ref(0x10 * i as u32, *model);
        }
        weap.block(0x2a8, models.len() as u32, block);
    }
    set.build().unwrap().0
}

/// The UNSC look held in its own render model: one that loads.
fn with_first_person() -> Vec<u8> {
    unsc_first_person(|unsc| vec![unsc.render])
}

/// The UNSC look with no first-person block: the black first-person
/// screen. (The kit itself copies the fixture pistol's view onto the
/// stick.)
pub(crate) fn without_first_person() -> Vec<u8> {
    unsc_first_person(|_| Vec::new())
}

/// The kit's weapons in a `model.list` reply.
fn pogo_weapons(r: &Value) -> Vec<&Value> {
    r["objects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["name"].as_str().is_some_and(|n| n.contains("pogo")))
        .collect()
}

fn finding<'a>(r: &'a Value, code: &str) -> Option<&'a Value> {
    r["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["code"] == code)
}

#[test]
fn list_names_the_kits_weapons_and_their_first_person_entries() {
    let (_d, mut app, mut api) = court("list", &content_kit::fixture::kitted().bytes);
    let r = call(&mut api, &mut app, m::MODEL_LIST, json!({"map": "court", "group": "weap"}));
    assert_eq!(r["map"], "court");
    // The fixture's rifle and pistol, then the kit's three looks.
    let objects = r["objects"].as_array().unwrap();
    assert_eq!(objects.len(), 2 + 3, "{r}");
    for o in objects {
        assert_eq!(o["group"], "weap");
        assert_eq!(o.get("error"), None, "every weapon resolves: {o}");
    }
    let held = content_kit::fixture::FIXTURE_FIRST_PERSON.len();
    let kit = pogo_weapons(&r);
    assert_eq!(kit.len(), 3, "{r}");
    for o in kit {
        assert_eq!(o["first_person"], held, "the stick takes the pistol's first-person view: {o}");
        assert!(o["model"].as_str().is_some_and(|n| n.contains("pogo_")), "{o}");
        assert!(o["render_model"].as_str().is_some(), "{o}");
    }
    // The fixture's own weapons have a first-person view and no model.
    let pistol = objects
        .iter()
        .find(|o| o["name"].as_str().is_some_and(|n| n.ends_with("pistol")))
        .unwrap_or_else(|| panic!("{r}"));
    assert_eq!(pistol["first_person"], held);
    assert_eq!(pistol["model"], Value::Null);
    // The map came with the kit: its weapons are named, nothing was added.
    assert_eq!(r["pogo"]["weapons"].as_array().unwrap().len(), 3);
    assert_eq!(r["pogo"]["costs"], Value::Null);
    assert!(r["tag_memory"]["free"].as_i64().unwrap() > 0);
    // Bipeds by default: the invented map has none.
    let bipeds = call(&mut api, &mut app, m::MODEL_LIST, json!({}));
    assert_eq!(bipeds["objects"], json!([]));
}

/// The check that would have caught the black first-person screen.
#[test]
fn check_finds_the_missing_first_person_block() {
    let (_d, mut app, mut api) = court("check", &without_first_person());
    let r = call(&mut api, &mut app, m::MODEL_CHECK, json!({"tag": "pogo_unsc", "map": "court"}));
    let f = finding(&r, "no_first_person").unwrap_or_else(|| panic!("{r}"));
    assert_eq!(f["severity"], "error");
    assert_eq!(f["path"], "first_person");
    let tag = f["tag"].as_str().unwrap();
    assert!(tag.starts_with("weap:") && tag.ends_with("pogo_unsc"), "{tag}");
    assert!(r["counts"]["error"].as_u64().unwrap() >= 1);
    let memory = finding(&r, "tag_memory").expect("the headroom is a finding");
    assert_eq!(memory["severity"], "note", "{memory}");
    // Nothing else is broken: every link resolves, the model loads.
    for code in ["unresolved_link", "no_model", "model_fails", "no_sections", "render_model_unreadable"] {
        assert!(finding(&r, code).is_none(), "{code}: {r}");
    }
}

/// A view that cannot be drawn says why, before any GPU is asked for.
#[test]
fn a_first_person_render_without_a_first_person_block_is_refused_with_its_reason() {
    let (d, mut app, mut api) = court("fp-refused", &without_first_person());
    let out = d.path("fp.png");
    let (c, data) = refusal(api.call(
        &mut app,
        m::MODEL_RENDER,
        json!({"tag": "pogo_unsc", "map": "court", "view": "first_person", "out": out.display().to_string()}),
    ));
    assert_eq!(c, code::REFUSED);
    assert_eq!(data["reason"], "no_first_person");
    assert_eq!(data["path"], "first_person");
    assert!(!out.exists(), "no empty picture");
    // No player biped in the invented globals: third person says so.
    let (c, data) = refusal(api.call(
        &mut app,
        m::MODEL_RENDER,
        json!({"tag": "pogo_unsc", "view": "third_person", "out": d.path("tp.png").display().to_string()}),
    ));
    assert_eq!(c, code::REFUSED);
    assert_eq!(data["reason"], "no_player_biped");
}

/// The kit holds the stick in the fixture pistol's first-person view; the
/// fixture's invented first-person models do not load, and both the check
/// and a first-person render say so at the element.
#[test]
fn the_kits_copied_first_person_view_is_checked_and_its_failure_named() {
    let (d, mut app, mut api) = court("fp-copied", &content_kit::fixture::kitted().bytes);
    let r = call(&mut api, &mut app, m::MODEL_CHECK, json!({"tag": "pogo_unsc", "map": "court"}));
    assert!(finding(&r, "no_first_person").is_none(), "{r}");
    let f = finding(&r, "first_person_model_fails").unwrap_or_else(|| panic!("{r}"));
    assert_eq!(f["path"], "first_person[0].model");
    assert!(finding(&r, "unresolved_link").is_none(), "{r}");
    let out = d.path("fp.png");
    let (c, data) = refusal(api.call(
        &mut app,
        m::MODEL_RENDER,
        json!({"tag": "pogo_unsc", "view": "first_person", "out": out.display().to_string()}),
    ));
    assert_eq!(c, code::REFUSED);
    assert_eq!(data["reason"], "first_person_model_fails");
    assert!(!out.exists());
}

#[test]
fn a_render_is_refused_in_a_git_checkout() {
    let (d, mut app, mut api) = court("git", &content_kit::fixture::kitted().bytes);
    let repo = d.path("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let (c, _) = refusal(api.call(
        &mut app,
        m::MODEL_RENDER,
        json!({"tag": "pogo_unsc", "map": "court", "out": repo.join("m.png").display().to_string()}),
    ));
    assert_eq!(c, code::REFUSED);
}

#[test]
fn read_gives_the_model_its_markers_and_the_first_person_paths() {
    let (_d, mut app, mut api) = court("read", &with_first_person());
    let r = call(&mut api, &mut app, m::MODEL_READ, json!({"tag": "pogo_unsc", "map": "court"}));
    assert_eq!(r["group"], "weap");
    assert_eq!(r["nodes"], 2, "{r}");
    let markers: Vec<&str> = r["markers"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|mk| mk["name"].as_str())
        .collect();
    for spec in &content_kit::source::manifest().markers {
        let name = spec.name.as_str();
        assert!(markers.contains(&name), "{name}: {markers:?}");
    }
    let fp = &r["first_person"][0];
    assert!(fp["model"].as_str().is_some_and(|n| n.ends_with("pogo_unsc")), "{fp}");
    assert_eq!(fp["animations"], Value::Null);
    assert_eq!(fp["paths"]["model"], "first_person[0].model");
    assert_eq!(r["paths"]["first_person"], "first_person");
}

#[test]
fn a_weapon_with_a_first_person_model_passes_and_draws() {
    let (d, mut app, mut api) = court("fp", &with_first_person());
    let r = call(&mut api, &mut app, m::MODEL_CHECK, json!({"tag": "pogo_unsc", "map": "court"}));
    assert!(finding(&r, "no_first_person").is_none(), "{r}");
    assert!(finding(&r, "first_person_model_fails").is_none(), "{r}");
    let f = finding(&r, "first_person_no_animations").expect("no animations named");
    assert_eq!(f["severity"], "warning");
    assert_eq!(f["path"], "first_person[0].animations");
    // The other looks keep the view the kit copied from the fixture's
    // pistol, whose invented models are not models.
    let other = call(&mut api, &mut app, m::MODEL_CHECK, json!({"tag": "pogo_covenant"}));
    assert!(finding(&other, "no_first_person").is_none(), "{other}");
    assert!(finding(&other, "first_person_model_fails").is_some(), "{other}");

    for view in ["ground", "first_person"] {
        let out = d.path(&format!("{view}.png"));
        let r = api.call(
            &mut app,
            m::MODEL_RENDER,
            json!({"tag": "pogo_unsc", "view": view, "size": [96, 64], "out": out.display().to_string()}),
        );
        let r = match r {
            Err(e) if e.code == code::NO_GPU => return,
            Err(e) => panic!("{view}: {e}"),
            Ok(v) => v,
        };
        assert!(out.is_file(), "{view}");
        assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(96), Some(64)));
        assert_eq!(r["view"], view);
        let pieces = r["pieces"].as_array().unwrap();
        assert!(pieces[0]["triangles"].as_u64().unwrap() > 0, "{r}");
        if view == "ground" {
            assert!(r["covered"].as_u64().unwrap() > 0, "the stick is in the picture: {r}");
        }
    }
}

#[test]
fn pogo_on_a_map_the_kit_cannot_go_into_is_refused_by_name() {
    // The bare invented map has no player animation graphs to take the
    // weapon class from.
    let (_d, mut app, mut api) = court("pogo", &content_kit::fixture::fixture(0));
    let (c, data) = refusal(api.call(
        &mut app,
        m::MODEL_LIST,
        json!({"map": "court", "pogo": true, "group": "weap"}),
    ));
    assert_eq!(c, code::REFUSED);
    assert_eq!(data["reason"], "pogo_refused");
    // Without the kit it reads the map as it is: the fixture's rifle and
    // pistol, each resolving (no model, null references), and no kit.
    let r = call(&mut api, &mut app, m::MODEL_LIST, json!({"map": "court", "pogo": false, "group": "weap"}));
    let objects = r["objects"].as_array().unwrap();
    assert_eq!(objects.len(), 2, "{r}");
    for o in objects {
        assert_eq!(o.get("error"), None, "{o}");
        assert_eq!(o["model"], Value::Null, "{o}");
    }
    assert!(pogo_weapons(&r).is_empty(), "{r}");
    assert_eq!(r["pogo"], Value::Null);
}

#[test]
fn an_unknown_tag_or_view_is_invalid() {
    let (_d, mut app, mut api) = court("bad", &content_kit::fixture::kitted().bytes);
    let (c, _) = refusal(api.call(&mut app, m::MODEL_READ, json!({"tag": "no_such_thing", "map": "court"})));
    assert_eq!(c, code::INVALID_PARAMS);
    // Five weapons answer to their group: give one.
    let (c, _) = refusal(api.call(&mut app, m::MODEL_READ, json!({"tag": "weap"})));
    assert_eq!(c, code::INVALID_PARAMS);
    let (c, _) = refusal(api.call(&mut app, m::MODEL_RENDER, json!({"tag": "pogo_unsc", "view": "side"})));
    assert_eq!(c, code::INVALID_PARAMS);
}
