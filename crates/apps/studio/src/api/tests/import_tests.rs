//! `import.sound` over an empty local package and invented tones (no game
//! data): the WAV is copied in, the manifest and lock follow, and the item
//! checks clean the way `h2 package check` reads it.

use super::*;

/// A 16-bit PCM WAV of `frames` frames of a 440 Hz tone (and a 660 Hz one
/// on a second channel).
pub(super) fn tone(channels: u16, rate: u32, frames: usize) -> Vec<u8> {
    let mut pcm = Vec::new();
    for i in 0..frames {
        let t = i as f32 / rate as f32;
        pcm.push(((t * 440.0 * std::f32::consts::TAU).sin() * 10_000.0) as i16);
        if channels == 2 {
            pcm.push(((t * 660.0 * std::f32::consts::TAU).sin() * 5_000.0) as i16);
        }
    }
    let data = pcm.len() as u32 * 2;
    let mut b = b"RIFF".to_vec();
    b.extend((36 + data).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(channels.to_le_bytes());
    b.extend(rate.to_le_bytes());
    b.extend((rate * u32::from(channels) * 2).to_le_bytes());
    b.extend((channels * 2).to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend(data.to_le_bytes());
    for s in pcm {
        b.extend(s.to_le_bytes());
    }
    b
}

/// A package with no items.
pub(super) fn package(dir: &std::path::Path, id: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("package.toml"),
        format!("[package]\nformat = 1\nid = \"{id}\"\nname = \"{id}\"\n"),
    )
    .unwrap();
}

fn setup(name: &str) -> (Temp, App, Api) {
    let t = Temp::new(name);
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    package(&t.path("data/packages/mine"), "mine");
    (t, app, Api::new())
}

fn manifest(t: &Temp) -> content::package::Manifest {
    content::package::Manifest::from_toml(
        &std::fs::read_to_string(t.path("data/packages/mine/package.toml")).unwrap(),
    )
    .unwrap()
}

#[test]
fn import_copies_the_wav_in_and_the_item_checks_clean() {
    let (t, mut app, mut api) = setup("import-sound");
    let wav = tone(1, 22_050, 640);
    std::fs::write(t.path("clip.wav"), &wav).unwrap();
    let v = call(
        &mut api,
        &mut app,
        m::IMPORT_SOUND,
        json!({"file": t.path("clip.wav").display().to_string(), "package": "mine", "tag": "sounds/pogo/boing"}),
    );
    assert_eq!(v["tag"], "tag:snd!:sounds\\pogo\\boing");
    assert_eq!(v["source"], "sounds/boing.wav");
    assert_eq!(v["sidecar"], Value::Null);
    assert_eq!(v["item"], 0);
    assert_eq!(v["class"], 51);
    let doc = app.front_doc().unwrap();
    assert_eq!(v["doc"], app.doc_id(doc));
    let tab = app.tabs().iter().find(|tab| tab.doc == doc).unwrap();
    assert_eq!(app.tab_json(tab)["editor"], "audition");
    assert!(matches!(
        app.source_asset(doc).map(|source| &source.view),
        Some(crate::app::assets::SourceView::Sound { clip, .. }) if clip.frames() == 640
    ));
    assert_eq!(v["from"], json!({"rate": 22_050, "channels": 1, "frames": 640}));
    // 640 frames at the clip's own rate: ten 36-byte blocks.
    assert_eq!((v["rate"].clone(), v["frames"].clone()), (json!(22_050), json!(640)));
    assert_eq!(v["bytes"], 360);
    assert_eq!(v["duration_ms"], 640 * 1000 / 22_050);
    // The file is the user's, unchanged.
    let dir = t.path("data/packages/mine");
    assert_eq!(std::fs::read(dir.join("sounds/boing.wav")).unwrap(), wav);
    let man = manifest(&t);
    assert_eq!(man.items.len(), 1);
    assert_eq!(man.items[0].kind, content::package::Kind::Sound);
    assert_eq!(man.items[0].tag.as_deref(), Some("sounds\\pogo\\boing"));
    assert!(std::fs::read_to_string(dir.join("package.lock")).unwrap().contains("sounds/boing.wav"));
    // As `h2 package check` reads it: the item resamples and encodes.
    let loaded = ::workspace::Loaded::from_dir(&dir).unwrap();
    assert_eq!(::workspace::check_package(&loaded), vec![]);

    app.dispatch(crate::app::Action::CloseDoc(doc));
    let open = call(&mut api, &mut app, m::SHELL_OPEN, json!({"asset": v["doc"]}));
    assert_eq!(open["tab"]["editor"], "audition");
}

#[test]
fn a_clip_at_another_rate_is_reported_at_the_rate_the_build_writes() {
    let (t, mut app, mut api) = setup("import-sound-rate");
    std::fs::write(t.path("c.wav"), tone(2, 48_000, 4800)).unwrap();
    let v = call(
        &mut api,
        &mut app,
        m::IMPORT_SOUND,
        json!({"file": t.path("c.wav").display().to_string(), "package": "mine", "tag": "sounds\\wide", "class": "weapon_fire"}),
    );
    assert_eq!(v["from"], json!({"rate": 48_000, "channels": 2, "frames": 4800}));
    assert_eq!((v["rate"].clone(), v["frames"].clone()), (json!(44_100), json!(4410)));
    // 69 blocks of two channels.
    assert_eq!(v["bytes"], 69 * 72);
    assert_eq!(v["class"], 4);
    // A class other than the default is a sidecar `data` item.
    assert_eq!(v["sidecar"], "sounds/wide.sound.yaml");
    let dir = t.path("data/packages/mine");
    assert_eq!(
        std::fs::read_to_string(dir.join("sounds/wide.sound.yaml")).unwrap(),
        "class: weapon_fire\n"
    );
    let man = manifest(&t);
    assert_eq!(man.items.len(), 2);
    assert_eq!(man.items[1].kind, content::package::Kind::Data);
    assert_eq!(v["item"], 0);
    let loaded = ::workspace::Loaded::from_dir(&dir).unwrap();
    assert_eq!(::workspace::check_package(&loaded), vec![]);
    // A second import after it lands after both items; a number works too.
    std::fs::write(t.path("d.wav"), tone(1, 8_000, 800)).unwrap();
    let v = call(
        &mut api,
        &mut app,
        m::IMPORT_SOUND,
        json!({"file": t.path("d.wav").display().to_string(), "package": "mine", "tag": "sounds\\low", "class": 4}),
    );
    assert_eq!(v["item"], 2);
    assert_eq!(v["rate"], 22_050);
}

#[test]
fn import_refuses_what_it_cannot_build() {
    let (t, mut app, mut api) = setup("import-sound-refuse");
    let wav = t.path("clip.wav").display().to_string();
    std::fs::write(t.path("clip.wav"), tone(1, 22_050, 200)).unwrap();
    let ok = json!({"file": wav, "package": "mine", "tag": "sounds\\a"});
    call(&mut api, &mut app, m::IMPORT_SOUND, ok.clone());
    let refused = |api: &mut Api, app: &mut App, p: Value| code_of(api.call(app, m::IMPORT_SOUND, p));
    // The same tag again.
    assert_eq!(refused(&mut api, &mut app, ok.clone()), code::REFUSED);
    // Not a WAV.
    std::fs::write(t.path("bad.wav"), b"not a wav").unwrap();
    assert_eq!(
        refused(&mut api, &mut app, json!({"file": t.path("bad.wav").display().to_string(), "package": "mine", "tag": "sounds\\b"})),
        code::REFUSED
    );
    // A file that is not there, a package that is not, a path, a class.
    assert_eq!(
        refused(&mut api, &mut app, json!({"file": t.path("none.wav").display().to_string(), "package": "mine", "tag": "sounds\\b"})),
        code::FAILED
    );
    for bad in [
        json!({"file": wav, "package": "nope", "tag": "sounds\\b"}),
        json!({"file": wav, "package": "mine", "tag": "..\\b"}),
        json!({"file": wav, "package": "mine", "tag": ""}),
        json!({"file": wav, "package": "mine", "tag": "sounds\\b", "class": "nonsense"}),
        json!({"file": wav, "package": "mine", "tag": "sounds\\b", "class": true}),
    ] {
        assert_eq!(refused(&mut api, &mut app, bad), code::INVALID_PARAMS);
    }
    // Nothing the refusals wrote is left in the package.
    assert_eq!(manifest(&t).items.len(), 1);
    assert!(!t.path("data/packages/mine/sounds/b.wav").exists());
}

/// An RGBA PNG of `w` x `h` invented pixels.
pub(super) fn png(w: u32, h: u32) -> Vec<u8> {
    let rgba: Vec<u8> = (0..w * h * 4).map(|i| (i * 7) as u8).collect();
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        let mut writer = e.write_header().unwrap();
        writer.write_image_data(&rgba).unwrap();
        writer.finish().unwrap();
    }
    out
}

#[test]
fn a_png_import_copies_it_in_and_the_item_checks_clean() {
    let (t, mut app, mut api) = setup("import-bitmap");
    let bytes = png(3, 2);
    std::fs::write(t.path("pic.png"), &bytes).unwrap();
    let v = call(
        &mut api,
        &mut app,
        m::IMPORT_BITMAP,
        json!({"file": t.path("pic.png").display().to_string(), "package": "mine", "tag": "ui/pogo/icon"}),
    );
    assert_eq!(v["tag"], "tag:bitm:ui\\pogo\\icon");
    assert_eq!(v["source"], "bitmaps/icon.png");
    assert_eq!((v["width"].clone(), v["height"].clone()), (json!(3), json!(2)));
    assert_eq!((v["format"].clone(), v["bytes"].clone()), (json!("argb8888"), json!(24)));
    // A new tag is a Shape change.
    assert_eq!(v["class"], "shape");
    let dir = t.path("data/packages/mine");
    assert_eq!(std::fs::read(dir.join("bitmaps/icon.png")).unwrap(), bytes);
    let man = manifest(&t);
    assert_eq!(man.items.len(), 1);
    assert_eq!(man.items[0].kind, content::package::Kind::Bitmap);
    assert_eq!(man.items[0].tag.as_deref(), Some("ui\\pogo\\icon"));
    assert!(std::fs::read_to_string(dir.join("package.lock")).unwrap().contains("bitmaps/icon.png"));
    let loaded = ::workspace::Loaded::from_dir(&dir).unwrap();
    assert_eq!(::workspace::check_package(&loaded), vec![]);
}

#[test]
fn a_png_import_refuses_what_it_cannot_build() {
    let (t, mut app, mut api) = setup("import-bitmap-refuse");
    let pic = t.path("pic.png").display().to_string();
    std::fs::write(t.path("pic.png"), png(2, 2)).unwrap();
    let ok = json!({"file": pic, "package": "mine", "tag": "ui\\a"});
    call(&mut api, &mut app, m::IMPORT_BITMAP, ok.clone());
    let refused = |api: &mut Api, app: &mut App, p: Value| code_of(api.call(app, m::IMPORT_BITMAP, p));
    // The same tag again.
    assert_eq!(refused(&mut api, &mut app, ok), code::REFUSED);
    // Not a PNG.
    std::fs::write(t.path("bad.png"), b"not a png").unwrap();
    assert_eq!(
        refused(&mut api, &mut app, json!({"file": t.path("bad.png").display().to_string(), "package": "mine", "tag": "ui\\b"})),
        code::REFUSED
    );
    // A file that is not there.
    assert_eq!(
        refused(&mut api, &mut app, json!({"file": t.path("none.png").display().to_string(), "package": "mine", "tag": "ui\\b"})),
        code::FAILED
    );
    // A package that is not, a path that is not one.
    for bad in [
        json!({"file": pic, "package": "nope", "tag": "ui\\b"}),
        json!({"file": pic, "package": "mine", "tag": "..\\b"}),
        json!({"file": pic, "package": "mine", "tag": ""}),
    ] {
        assert_eq!(refused(&mut api, &mut app, bad), code::INVALID_PARAMS);
    }
    // Nothing the refusals wrote is left in the package.
    assert_eq!(manifest(&t).items.len(), 1);
    assert!(!t.path("data/packages/mine/bitmaps/b.png").exists());
}

pub(super) fn model_files(dir: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    let frame = content_kit::NODE_FRAME;
    std::fs::write(dir.join("model.yaml"), format!(
        "name: invented\natlas: invented\nnodes:\n  - name: {frame}\n    at: [0, 0, 0]\nmarkers: []\nparts:\n  - box:\n      node: {frame}\n      region: body\n      min: [-1, -1, -1]\n      max: [1, 1, 1]\n"
    )).unwrap();
    let (w, h) = content_kit::source::atlas_size();
    std::fs::write(dir.join("invented.png"), png(w.into(), h.into())).unwrap();
}

#[test]
fn import_model_adds_its_atlas_and_opens_the_model_source() {
    let (t, mut app, mut api) = setup("import-model");
    model_files(&t.path("out"));
    let v = call(
        &mut api,
        &mut app,
        m::IMPORT_MODEL,
        json!({"file": t.path("out/model.yaml"), "package": "mine", "tag": "models/invented/box"}),
    );
    assert_eq!(v["tag"], "tag:hlmt:models\\invented\\box");
    assert_eq!(v["doc"], "item:mine:0");
    let doc = app.front_doc().unwrap();
    assert!(matches!(
        app.source_asset(doc).map(|source| &source.view),
        Some(crate::app::assets::SourceView::Model(mesh)) if mesh.triangles() == 12
    ));
    let loaded = ::workspace::Loaded::from_dir(&t.path("data/packages/mine")).unwrap();
    assert_eq!(loaded.package.manifest.items[0].kind, content::package::Kind::Model);
    assert_eq!(loaded.package.manifest.items[1].kind, content::package::Kind::Data);
    assert_eq!(::workspace::check_package(&loaded), vec![]);
}

#[test]
fn a_dropped_source_file_becomes_the_item_of_its_kind() {
    let (t, mut app, mut api) = setup("asset-drop");
    std::fs::write(t.path("Boing Loud.wav"), tone(1, 22_050, 640)).unwrap();
    std::fs::write(t.path("pic.png"), png(3, 2)).unwrap();
    let file = |name: &str| t.path(name).display().to_string();
    let v = call(&mut api, &mut app, m::ASSET_DROP, json!({"file": file("Boing Loud.wav"), "package": "mine"}));
    assert_eq!((v["kind"].clone(), v["package"].clone()), (json!("sound"), json!("mine")));
    let sound = v["tag"].as_str().unwrap().to_owned();
    assert!(sound.starts_with("sounds\\"), "{sound}");
    // A sound opens its source tab.
    assert!(v["front"].as_str().is_some_and(|d| d.starts_with("item:mine:")), "{v}");
    // With no package the edit target takes it.
    call(&mut api, &mut app, m::PACKAGE_TARGET, json!({"id": "mine"}));
    let v = call(&mut api, &mut app, m::ASSET_DROP, json!({"file": file("pic.png")}));
    assert_eq!((v["kind"].clone(), v["tag"].clone()), (json!("bitmap"), json!("bitmaps\\pic")));
    let man = manifest(&t);
    assert_eq!(man.items.len(), 2);
    assert_eq!(man.items[0].kind, content::package::Kind::Sound);
    assert_eq!(man.items[0].tag.as_deref(), Some(sound.as_str()));
    assert_eq!(man.items[1].kind, content::package::Kind::Bitmap);
    assert_eq!(man.items[1].tag.as_deref(), Some("bitmaps\\pic"));
    let loaded = ::workspace::Loaded::from_dir(&t.path("data/packages/mine")).unwrap();
    assert_eq!(::workspace::check_package(&loaded), vec![]);
}

#[test]
fn a_drop_refuses_a_file_that_is_no_source_and_a_second_copy() {
    let (t, mut app, mut api) = setup("asset-drop-refused");
    std::fs::write(t.path("pic.png"), png(3, 2)).unwrap();
    let pic = t.path("pic.png").display().to_string();
    // A map is not a source file: source.open opens it.
    let e = api
        .call(&mut app, m::ASSET_DROP, json!({"file": t.path("lockout.map").display().to_string(), "package": "mine"}))
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
    // No package and no edit target.
    let e = api.call(&mut app, m::ASSET_DROP, json!({"file": pic})).unwrap_err();
    assert_eq!(e.code, code::NO_TARGET);
    call(&mut api, &mut app, m::ASSET_DROP, json!({"file": pic, "package": "mine"}));
    // The tag is taken: the second drop adds nothing.
    assert!(api.call(&mut app, m::ASSET_DROP, json!({"file": pic, "package": "mine"})).is_err());
    assert_eq!(manifest(&t).items.len(), 1);
}
