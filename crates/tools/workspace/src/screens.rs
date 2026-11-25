//! Screen docs and retail edit files (docs/game/ui-in-maps.md).

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;

use content::package::Maps;
use game::ui_kit::{data, inject, retail_edits};
use map::Map;
use map::build::TagSet;
use tags::ui::SCREEN;

use crate::build::{Cx, Failure};
use crate::builders::Job;
use crate::{MAX_FILE_BYTES, MapUnpin};

fn plan(job: &Job<'_>) -> Result<Vec<inject::Entry>, Failure> {
    let text = std::str::from_utf8(job.source).map_err(|e| e.to_string())?;
    let design = data::DesignDoc::parse(text).map_err(|e| e.to_string())?;
    let mut docs = BTreeMap::new();
    for path in &job.item.inputs {
        let file = Path::new(path);
        let name = file
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|_| file.extension().is_some_and(|ext| ext == "yaml"))
            .ok_or_else(|| format!("{path}: expected a screen YAML file"))?;
        let bytes = job
            .files
            .get(path)
            .ok_or_else(|| format!("{path}: missing screen file"))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(format!("{path}: over the {MAX_FILE_BYTES} bytes a file may be").into());
        }
        let text = std::str::from_utf8(bytes).map_err(|e| format!("{path}: {e}"))?;
        let doc = data::ScreenDoc::parse(name, text, &design).map_err(|e| e.to_string())?;
        if docs.insert(name.to_owned(), doc).is_some() {
            return Err(format!("{path}: duplicate screen {name:?}").into());
        }
    }
    let entries = inject::package_plan(design, &docs)?;
    if entries.is_empty() {
        return Err("the item has no screen with a game ID".into());
    }
    Ok(entries)
}

pub fn check(job: &Job<'_>) -> Result<(), Failure> {
    plan(job).map(|_| ())
}

pub fn apply(cx: &mut Cx<'_>, job: &Job<'_>) -> Result<(), Failure> {
    let plan = plan(job)?;
    // An earlier screen edit can move or grow the template. Materialize the
    // current set, then pin that snapshot so the injected panes' references
    // still point into the same template blocks when the final map is laid out.
    let (map, _) = cx
        .set
        .build()
        .map_err(|e| format!("laying out UI templates: {e}"))?;
    let mut map =
        Map::from_reader(Cursor::new(map)).map_err(|e| format!("reading UI templates: {e}"))?;
    let facts = inject::tag::read_facts(&mut map, &plan).map_err(|e| format!("{e:#}"))?;
    let mut set = TagSet::from_map(&mut map).map_err(|e| format!("pinning UI templates: {e}"))?;
    let result =
        inject::tag::add_screens_to(&mut set, &facts, &plan).map_err(|e| format!("{e:#}"))?;

    for (name, _) in &result.injected {
        cx.check_free(&Cx::key(SCREEN, name))?;
    }
    for id in result.changed {
        let entry = set.tag(id).ok_or("a wgtz left the set")?;
        let name = set.slots[id.index()]
            .name
            .as_deref()
            .ok_or("a wgtz has no name")?;
        let key = Cx::key(entry.group, name);
        cx.touch(id)?;
        cx.note_changed(&key);
    }
    let unpin = MapUnpin::new(&mut map).map(|u| u.preserving(result.templates));
    cx.pin_snapshot(set, unpin);
    for (name, _) in result.injected {
        let key = Cx::key(SCREEN, &name);
        let id = cx.find(&key).ok_or("an injected screen left the set")?;
        cx.note_added(&key, id);
    }
    for (name, why) in result.skipped {
        eprintln!("package {:?}: {name} not added: {why}", cx.package);
    }
    Ok(())
}

fn edit_file(job: &Job<'_>) -> Result<retail_edits::EditFile, Failure> {
    let source = job
        .item
        .source
        .as_deref()
        .ok_or("expected a screen edit file")?;
    let text = std::str::from_utf8(job.source).map_err(|e| e.to_string())?;
    let name = source
        .strip_suffix(".yaml")
        .ok_or("expected a screen edit YAML file")?;
    let file = retail_edits::parse(name, text)?;
    let maps_match = match &job.item.maps {
        Maps::Names(names) => {
            matches!(names.as_slice(), [name] if name.eq_ignore_ascii_case(&file.map))
        }
        Maps::Scope(_) => false,
    };
    if !maps_match {
        return Err(format!("maps must name only {:?}, the edit file's map", file.map).into());
    }
    Ok(file)
}

pub fn check_edits(job: &Job<'_>) -> Result<(), Failure> {
    edit_file(job).map(|_| ())
}

pub fn apply_edits(cx: &mut Cx<'_>, job: &Job<'_>) -> Result<(), Failure> {
    let file = edit_file(job)?;
    let id = cx
        .set
        .slots
        .iter()
        .find_map(|slot| {
            let tag = slot.tag.as_ref()?;
            (tag.group == SCREEN && slot.name.as_ref()?.eq_ignore_ascii_case(&file.screen))
                .then_some(tag.id)
        })
        .ok_or_else(|| format!("the map has no screen tag {}", file.screen))?;
    let key = Cx::key(
        SCREEN,
        cx.set.slots[id.index()]
            .name
            .as_deref()
            .ok_or("a screen has no name")?,
    );
    let mut block = cx.read_block(id)?;
    retail_edits::apply::edit_block(&mut block, &file)?;
    cx.touch(id)?;
    cx.write_block(id, &key, block)?;
    cx.set.check_tag_memory = false;
    Ok(())
}
