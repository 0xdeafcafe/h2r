//! Retargeting and glTF import for Studio's animation documents.
//! See docs/editor/studio-redesign.md, "The animation editor".

use anim_edit::{ClipKind, GltfClip, Node, NodeMapping, RetargetReport, retarget, retarget_report};
use map::TagId;

use super::super::{AnimError, AnimSummary, id_text};
use super::{AnimationDocument, App, authored_summary};

/// An open animation that can serve as the source of a transfer.
pub struct AnimationSource {
    pub doc: u64,
    pub name: String,
    pub graph: String,
    pub map: Option<String>,
    pub models: Vec<String>,
}

#[derive(Clone, Copy, Default)]
pub struct RetargetOptions<'a> {
    pub mapping: &'a [NodeMapping],
    pub name: Option<&'a str>,
    pub from_model: Option<&'a str>,
    pub to_model: Option<&'a str>,
    /// Report the mapping without opening a document or choosing models.
    pub preview: bool,
}

pub struct Transfer {
    pub doc: Option<u64>,
    pub name: String,
    pub source: Vec<Node>,
    pub target: Vec<Node>,
    pub report: RetargetReport,
    pub notes: Vec<String>,
}

enum Input<'a> {
    Document(u64),
    Gltf(&'a GltfClip),
}

impl App {
    /// Open documents keep their graphs' rest skeletons across map changes.
    pub fn anim_sources(&self) -> Vec<AnimationSource> {
        self.anim
            .docs
            .iter()
            .enumerate()
            .map(|(index, doc)| {
                let graph = self.anim.graph_rests.get(&doc.graph);
                AnimationSource {
                    doc: index as u64 + 1,
                    name: doc.clip.name().to_owned(),
                    graph: graph
                        .map(|g| g.name.clone())
                        .unwrap_or_else(|| id_text(doc.graph.tag)),
                    map: doc.graph.map.clone(),
                    models: graph
                        .into_iter()
                        .flat_map(|g| &g.models)
                        .map(|(user, _)| user.name.clone())
                        .collect(),
                }
            })
            .collect()
    }

    pub fn anim_retarget(
        &mut self,
        from: u64,
        to: TagId,
        options: RetargetOptions<'_>,
    ) -> Result<Transfer, AnimError> {
        self.transfer_animation(Input::Document(from), to, options)
    }

    pub fn anim_import(
        &mut self,
        imported: &GltfClip,
        to: TagId,
        options: RetargetOptions<'_>,
    ) -> Result<Transfer, AnimError> {
        self.transfer_animation(Input::Gltf(imported), to, options)
    }

    fn transfer_animation(
        &mut self,
        input: Input<'_>,
        to: TagId,
        options: RetargetOptions<'_>,
    ) -> Result<Transfer, AnimError> {
        self.ensure_anim(to)?;
        let destination = self.anim.loaded.as_ref().expect("graph loaded");
        let to_key = destination.key.clone();
        let target = destination.skeleton();

        let (source, kind, source_graph, notes) = match input {
            Input::Document(id) => {
                let entry = self.anim_document(id)?;
                (
                    &entry.clip,
                    entry.summary.kind,
                    Some(&entry.graph),
                    Vec::new(),
                )
            }
            Input::Gltf(imported) => (&imported.clip, "base", None, imported.notes.clone()),
        };
        let name = options.name.unwrap_or_else(|| source.name()).to_owned();
        if name.trim().is_empty() {
            return Err(AnimError::Refused("the animation needs a name".to_owned()));
        }
        let mut output = Transfer {
            doc: None,
            name,
            source: source.nodes().to_vec(),
            target,
            report: retarget_report(source, &destination.skeleton(), options.mapping)
                .map_err(AnimError::Refused)?,
            notes,
        };
        if options.preview {
            return Ok(output);
        }
        let kind_id = match kind {
            "base" => ClipKind::Base,
            "overlay" => ClipKind::Overlay,
            "replacement" => ClipKind::Replacement,
            _ => {
                return Err(AnimError::Refused(format!(
                    "animation type {kind:?} cannot be retargeted"
                )));
            }
        };
        let from_rest = match input {
            Input::Document(_) => {
                let graph = source_graph.expect("document graph");
                self.anim
                    .graph_rests
                    .get(graph)
                    .ok_or_else(|| {
                        AnimError::Refused(
                            "the source graph has no cached rest skeleton".to_owned(),
                        )
                    })?
                    .model(options.from_model)?
            }
            Input::Gltf(imported) => imported.rest.as_slice(),
        };
        let to_rest = self
            .anim
            .graph_rests
            .get(&to_key)
            .expect("loaded rest skeleton")
            .model(options.to_model)?;
        let (clip, report) = retarget(
            source,
            from_rest,
            &output.target,
            to_rest,
            options.mapping,
            output.name.clone(),
            kind_id,
        )
        .map_err(AnimError::Refused)?;
        let summary = AnimSummary {
            index: 0,
            name: output.name.clone(),
            kind,
            frames: clip.frames(),
            codecs: [None, None],
            aim_grid: false,
            error: None,
        };
        output.report = report;
        output.doc = Some(self.anim.insert(AnimationDocument {
            graph: to_key,
            summary: authored_summary(summary, &clip),
            retail_index: None,
            clip,
        }));
        Ok(output)
    }
}
