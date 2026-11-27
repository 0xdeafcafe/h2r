//! Studio's package saves, shared by the document controls and the socket.

use std::path::{Path, PathBuf};

use content::package::Kind;

use super::{Action, App, Doc};
use super::anim::DraftPlacement;
use super::package_doc::{self, Refusal};

impl App {
    pub(crate) fn doc_save_action(&self, doc: Doc) -> Option<Action> {
        Some(match doc {
            Doc::Rules => Action::SaveRules,
            Doc::Variant => {
                let editor = self.variants.as_ref()?;
                if editor.is_read_only() || editor.current().is_none() {
                    return None;
                }
                Action::SaveVariant
            }
            Doc::Playlists => Action::SaveHopper,
            Doc::Map => Action::Save,
            Doc::Tag(id) => {
                if let Some((tag, doc, placement)) = &self.anim.save_selection
                    && *tag == id
                    && self.anim_doc_tag(*doc).ok() == Some(id)
                {
                    return Some(Action::SaveAnimation {
                        doc: *doc,
                        placement: placement.clone(),
                    });
                }
                let tag = self.shown_tags().tag(id)?;
                if tag.group.0 == *b"snd!" {
                    let ids = ready_line_ids(self.sound_lines(&tag.name).into_iter());
                    if !ids.is_empty() {
                        return Some(Action::SaveVoiceLines(ids));
                    }
                }
                Action::Save
            }
            Doc::Voice => {
                let voice = self.voices.doc.as_deref()?;
                if voice == super::voices::ANNOUNCER {
                    let (file, clip) = self.voices.announcer_save.clone()?;
                    return Some(Action::SaveVoiceClip { file, clip });
                }
                let batch = &self.voices.batch;
                let ids = ready_line_ids(batch.rows.iter().filter(|r| {
                    r.character.as_deref().unwrap_or(&batch.default_character) == voice
                }));
                if ids.is_empty() {
                    return None;
                }
                Action::SaveVoiceLines(ids)
            }
            Doc::Forge => {
                let open = self.forge_variants.open.as_ref()?;
                if !open.dirty() {
                    return None;
                }
                Action::Forge(super::forge_variants::ForgeAction::Save)
            }
            _ => return None,
        })
    }

    pub(crate) fn save_variant_item(&mut self, target: &str) -> Result<PathBuf, Refusal> {
        let data = self.data_root();
        let editor = self.variants.as_mut()
            .ok_or_else(|| Refusal::Refused("no variant is open".into()))?;
        if editor.is_read_only() {
            return Err(Refusal::Refused("copy the built-in variant to edit it".into()));
        }
        let text = editor.export_current()
            .ok_or_else(|| Refusal::Refused("no variant is open".into()))?;
        let name = editor.current().expect("exported").name();
        let leaf = super::edit_target::slug_of(&::variants::panel::strip_icon(&name));
        let origin = editor.package_source()
            .filter(|p| p.extension().is_some_and(|ext| ext == "h2variant"));
        let path = package_doc::put_document(
            &data, target, Kind::Variant, origin,
            &format!("variants/{leaf}.h2variant"), text.as_bytes(),
        )?;
        editor.mark_packaged(path.clone());
        Ok(path)
    }

    pub(crate) fn save_rules_item(&mut self, target: &str) -> Result<PathBuf, Refusal> {
        if !self.rules.opened {
            return Err(Refusal::Refused("no script is open".into()));
        }
        let name = super::rules_edit::script_file_name(&self.rules.title);
        let name = if name.contains("..") || name.contains(':') {
            format!("{}.rules", super::edit_target::slug_of(&self.rules.title))
        } else {
            name
        };
        let path = package_doc::put_document(
            &self.data_root(), target, Kind::Rules, self.rules.path.as_deref(),
            &format!("rules/{name}"), self.rules.text.as_bytes(),
        )?;
        self.rules.mark_saved(&path);
        Ok(path)
    }

    pub(crate) fn save_playlist_item(&mut self, target: &str) -> Result<PathBuf, Refusal> {
        let file = self.hoppers.build().map_err(|e| Refusal::Refused(e.to_string()))?;
        let format = self.hoppers.format().expect("built");
        let bytes = hopper_edit::write_file(&file, format).map_err(|e| Refusal::Refused(e.to_string()))?;
        let origin = self.hoppers.path()
            .filter(|p| p.extension().is_some_and(|ext| ext == "dat"));
        let path = package_doc::put_document(
            &self.data_root(), target, Kind::Playlist, origin,
            "playlists/multiplayer_hopper.dat", &bytes,
        )?;
        self.hoppers.mark_saved(&path, file);
        Ok(path)
    }

    pub(crate) fn save_voice_item(
        &self, target: &str, file: &Path, clip: &str, tag: Option<&str>,
    ) -> Result<PathBuf, String> {
        super::announcer::check_committable(file).map_err(|e| e.to_string())?;
        if tag.is_none() {
            super::announcer::check_clip_id(clip).map_err(|e| e.to_string())?;
        }
        let bytes = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
        package_doc::put_sound(&self.data_root(), target, clip, tag, &bytes).map_err(|e| e.to_string())
    }

    pub(super) fn save_to_target(&mut self, action: Action) {
        let Some(target) = self.edit_target() else {
            self.hold_target_save(action);
            return;
        };
        let result = match action {
            Action::SaveVariant => self.save_variant_item(&target).map_err(|e| e.to_string()),
            Action::SaveHopper => self.save_playlist_item(&target).map_err(|e| e.to_string()),
            Action::SaveRules => {
                let result = self.save_rules_item(&target).map_err(|e| e.to_string());
                if result.is_ok() && self.rules.reload_on_save
                    && let Err(why) = self.rules.queue_reload()
                {
                    self.error(format!("Script not reloaded: {why}"));
                }
                result
            }
            Action::SaveAnimation { doc, placement } => {
                let placement = placement.as_ref().map(DraftPlacement::as_placement).unwrap_or_default();
                match self.anim_save(doc, &target, placement) {
                    Ok(saved) => self.info(format!("saved {} in {target}", saved.source)),
                    Err(super::assets::CopyRefusal::Params(why)
                        | super::assets::CopyRefusal::Refused(why)
                        | super::assets::CopyRefusal::Failed(why)) => self.error(why),
                }
                return;
            }
            Action::SaveVoiceLines(ids) => {
                for (_, result) in self.voice_batch_save(Some(&ids), None, Some(&target)) {
                    match result {
                        Ok(path) => self.info(format!("saved {path}")),
                        Err(why) => self.error(why),
                    }
                }
                return;
            }
            Action::SaveVoiceClip { file, clip } => self.save_voice_item(&target, &file, &clip, None),
            _ => unreachable!("not a package save action"),
        };
        match result {
            Ok(path) => self.info(format!("saved {}", path.display())),
            Err(why) => self.error(why),
        }
    }
}

fn ready_line_ids<'a>(rows: impl Iterator<Item = &'a super::voice_batch::Row>) -> Vec<u64> {
    rows.filter(|row| {
        row.state == super::voice_batch::RowState::Done
            && (row.tag.is_some() || !row.clip.is_empty())
    })
    .map(|row| row.id)
    .collect()
}
