//! Package sources open before a map contains the tag they make.
use std::{fs, io::Write, path::Path};

use super::*;
use crate::app::{audition::Clip, sound_import};

#[derive(Clone, Debug, PartialEq)]
pub enum CreateAsset {
    Copy {
        asset: String,
        to: Option<String>,
    },
    Sound {
        file: PathBuf,
        tag: String,
        class: Option<String>,
    },
    Model {
        file: PathBuf,
        tag: String,
    },
}

pub(crate) enum SourceView {
    Tag(String),
    Sound { clip: Clip, peaks: Vec<[i16; 2]> },
    Model(content_kit::mesh::Mesh),
}

pub(crate) struct SourceAsset {
    pub id: String,
    pub package: String,
    pub key: TagKey,
    pub view: SourceView,
}

impl App {
    pub(crate) fn create_in_target(&mut self, request: CreateAsset) {
        let Some(package) = self.edit_target() else {
            self.edits.held_asset = Some(request);
            self.edits.open_picker();
            return;
        };
        if let Err(e) = self.create_asset(request, &package) {
            match e {
                CopyRefusal::Params(m) | CopyRefusal::Refused(m) | CopyRefusal::Failed(m) => {
                    self.error(m)
                }
            }
        }
    }

    pub(crate) fn create_asset(
        &mut self,
        request: CreateAsset,
        package: &str,
    ) -> Result<Value, CopyRefusal> {
        let data = self.data_root();
        let mut reply = match request {
            CreateAsset::Copy { asset, to } => {
                let id = AssetId::parse(&asset).map_err(CopyRefusal::Params)?;
                self.asset_copy(&id, package, to.as_deref())?
            }
            CreateAsset::Sound { file, tag, class } => {
                sound_import::import_sound(&data, &file, package, &tag, class.as_deref())?
            }
            CreateAsset::Model { file, tag } => import_model(&data, &file, package, &tag)?,
        };
        self.refresh_package_document();
        let id = format!("item:{package}:{}", reply["item"]);
        if !self.open_source_asset(&id, true) {
            return Err(CopyRefusal::Failed(format!(
                "{id} was written but its source could not open"
            )));
        }
        reply["doc"] = json!(id);
        Ok(reply)
    }

    pub(crate) fn source_asset_revision(&self) -> usize {
        self.source_assets.len()
    }

    pub(crate) fn source_asset(&self, doc: Doc) -> Option<&SourceAsset> {
        let Doc::SourceAsset(index) = doc else {
            return None;
        };
        self.source_assets.get(index)?.as_ref()
    }

    pub(crate) fn open_source_asset(&mut self, asset: &str, authored: bool) -> bool {
        let Ok(id) = AssetId::parse(asset) else {
            return false;
        };
        let (package, index) = match id {
            AssetId::Item(package, index) => (package, index),
            AssetId::Tag(key) => {
                let name = format!("{}:{}", fourcc_text(key.group), key.path);
                if tag_api::find_tag(&self.tags, &name).is_some() {
                    return false;
                }
                let set = read_set(&self.data_root());
                let Some(found) = set.on.iter().find_map(|entry| {
                    let loaded = &entry.loaded;
                    let index = loaded.package.manifest.items.iter().position(|item| {
                        added_tag(item).as_ref() == Some(&key)
                            && matches!(
                                item.kind,
                                ItemKind::Tag | ItemKind::Sound | ItemKind::Model
                            )
                    })?;
                    Some((loaded.package.name().to_owned(), index))
                }) else {
                    return false;
                };
                found
            }
            _ => return false,
        };
        let Some(source) = source_asset(&self.data_root(), &package, index) else {
            return false;
        };
        let name = format!("{}:{}", fourcc_text(source.key.group), source.key.path);
        if !authored && tag_api::find_tag(&self.tags, &name).is_some() {
            return false;
        }
        let index = match self
            .source_assets
            .iter()
            .position(|s| s.as_ref().is_some_and(|s| s.id == source.id))
        {
            Some(index) => {
                self.source_assets[index] = Some(source);
                index
            }
            None => {
                self.source_assets.push(Some(source));
                self.source_assets.len() - 1
            }
        };
        self.open_doc(Doc::SourceAsset(index));
        true
    }
}

fn source_asset(data: &Path, package: &str, index: usize) -> Option<SourceAsset> {
    let dir = sound_import::package_dir(data, package).ok()?;
    let loaded = Loaded::from_dir(&dir).ok()?;
    let item = loaded.package.manifest.items.get(index)?;
    let key = added_tag(item)?;
    let bytes = loaded.files.get(item.source.as_ref()?)?;
    let view = match item.kind {
        ItemKind::Tag => SourceView::Tag(std::str::from_utf8(bytes).ok()?.to_owned()),
        ItemKind::Sound => {
            let clip = ::workspace::import::read_wav(bytes).ok()?;
            let clip = Clip {
                rate: clip.rate,
                channels: clip.channels,
                pcm: clip.pcm.into(),
            };
            let peaks = clip.peaks(512);
            SourceView::Sound { clip, peaks }
        }
        ItemKind::Model => {
            let source = content_kit::source::parse(std::str::from_utf8(bytes).ok()?).ok()?;
            SourceView::Model(content_kit::mesh::build(&source).ok()?)
        }
        _ => return None,
    };
    Some(SourceAsset {
        id: format!("item:{package}:{index}"),
        package: package.into(),
        key,
        view,
    })
}

fn import_model(data: &Path, file: &Path, package: &str, tag: &str) -> Result<Value, CopyRefusal> {
    use CopyRefusal::{Failed, Params, Refused};
    let tag = tag.trim().replace('/', "\\");
    if tag
        .split('\\')
        .any(|part| part.is_empty() || part.contains(['.', ':', '\0']))
    {
        return Err(Params(
            "the tag must be a path such as models\\invented\\model".into(),
        ));
    }
    let read = |path: &Path| fs::read(path).map_err(|e| Failed(format!("{}: {e}", path.display())));
    let yaml = read(file)?;
    let source =
        content_kit::source::parse(std::str::from_utf8(&yaml).map_err(|e| Refused(e.to_string()))?)
            .map_err(|e| Refused(format!("{e:#}")))?;
    if source.atlas.is_empty() || source.atlas.contains(['/', '\\']) || source.atlas.contains("..")
    {
        return Err(Refused(
            "the atlas must name a PNG beside the model source".into(),
        ));
    }
    let dir = sound_import::package_dir(data, package)?;
    let png_name = format!("{}.png", source.atlas);
    let png = read(&file.with_file_name(&png_name))?;
    let mut loaded = Loaded::from_dir(&dir).map_err(|e| Failed(e.to_string()))?;
    let mut manifest = loaded.package.manifest;
    let folder = format!("models/{}", tag.replace('\\', "/"));
    let model = format!("{folder}/model.yaml");
    let atlas = format!("{folder}/{png_name}");
    let make_item = |kind, source, tag| Item {
        kind,
        source: Some(source),
        tag,
        inputs: Vec::new(),
        maps: Maps::default(),
        replaces: None,
        was: None,
    };
    let item = make_item(ItemKind::Model, model.clone(), Some(tag.clone()));
    if manifest
        .items
        .iter()
        .any(|i| added_tag(i) == added_tag(&item))
    {
        return Err(Refused(format!(
            "package {package:?} adds hlmt:{tag} already"
        )));
    }
    let index = manifest.items.len();
    let files = [(model.clone(), yaml), (atlas.clone(), png)];
    for (path, bytes) in &files {
        loaded.files.insert(path.clone(), bytes.clone());
    }
    let builder = ::workspace::builder(ItemKind::Model).expect("registered model builder");
    (builder.check)(&::workspace::Job {
        item: &item,
        source: &loaded.files[&model],
        files: &loaded.files,
    })
    .map_err(|e| Refused(e.why))?;
    manifest.items.push(item);
    manifest.items.push(make_item(ItemKind::Data, atlas, None));
    manifest.validate().map_err(|e| Refused(e.to_string()))?;

    let manifest_path = dir.join(MANIFEST_FILE);
    let before = read(&manifest_path)?;
    let lock_path = dir.join(LOCK_FILE);
    let old_lock = lock_path.exists().then(|| read(&lock_path)).transpose()?;
    let mut written = Vec::new();
    let mut manifest_touched = false;
    let mut lock_touched = false;
    let write = || -> std::io::Result<()> {
        fs::create_dir_all(dir.join(folder))?;
        for (path, bytes) in &files {
            let path = dir.join(path);
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            written.push(path);
            file.write_all(bytes)?;
        }
        manifest_touched = true;
        fs::write(&manifest_path, manifest.to_toml())?;
        let updated = Loaded::from_dir(&dir).map_err(std::io::Error::other)?;
        lock_touched = true;
        fs::write(&lock_path, updated.package.lock.to_toml())
    };
    if let Err(e) = (write)() {
        if manifest_touched {
            let _ = fs::write(manifest_path, before);
        }
        if lock_touched {
            if let Some(lock) = old_lock {
                let _ = fs::write(lock_path, lock);
            } else {
                let _ = fs::remove_file(lock_path);
            }
        }
        for path in written {
            let _ = fs::remove_file(path);
        }
        return Err(Failed(format!("writing model to {package}: {e}")));
    }
    Ok(
        json!({"package": package, "tag": format!("tag:hlmt:{tag}"), "source": model, "item": index, "dir": dir}),
    )
}
