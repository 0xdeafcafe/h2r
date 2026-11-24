//! Session animation documents for the Tags tool's animation graphs.

use anim_edit::{AnimDoc, Check, Node};
use map::TagId;
use tags::anim_keys::Tolerance;
use tags::anim_sample::{NodeTransform, Pose};

#[path = "save.rs"]
pub(super) mod save;
#[path = "transfer.rs"]
pub(super) mod transfer;

use super::{
    Anim, AnimError, AnimRead, AnimSummary, App, GraphKey, Loaded, PoseAnswer, Raw, model_defaults,
};

/// A session document on a graph. Canonical documents replace their
/// source animation in the editor; new and duplicate documents have
/// their own picker rows.
pub(super) struct AnimationDocument {
    graph: GraphKey,
    summary: AnimSummary,
    retail_index: Option<usize>,
    clip: AnimDoc,
}

impl Anim {
    fn insert(&mut self, doc: AnimationDocument) -> u64 {
        self.docs.push(doc);
        self.docs.len() as u64
    }

    fn retail_doc(&self, graph: &GraphKey, animation: usize) -> Option<u64> {
        self.docs
            .iter()
            .position(|doc| &doc.graph == graph && doc.retail_index == Some(animation))
            .map(|i| i as u64 + 1)
    }
}

fn doc_index(doc: u64) -> Result<usize, AnimError> {
    doc.checked_sub(1)
        .and_then(|index| usize::try_from(index).ok())
        .ok_or_else(|| unknown_doc(doc))
}

fn unknown_doc(doc: u64) -> AnimError {
    AnimError::Refused(format!("no animation document {doc}"))
}

impl Loaded {
    fn skeleton(&self) -> Vec<Node> {
        self.graph
            .nodes
            .iter()
            .zip(&self.node_names)
            .map(|(node, name)| Node {
                name: name.clone(),
                parent: node.parent,
            })
            .collect()
    }
}

fn document_from_read(read: &AnimRead, name: String) -> Result<AnimDoc, AnimError> {
    if read.animation.aim_grid {
        return Err(AnimError::Refused(
            "an aim grid is not a time clip".to_owned(),
        ));
    }
    let nodes = read
        .nodes
        .iter()
        .map(|n| Node {
            name: n.name.clone(),
            parent: n.parent,
        })
        .collect();
    AnimDoc::from_reduced(name, nodes, read.keys.clone()).map_err(AnimError::Refused)
}

fn authored_summary(mut summary: AnimSummary, clip: &AnimDoc) -> AnimSummary {
    summary.name = clip.name().to_owned();
    summary.codecs = [None, None];
    summary
}

fn document_frame(clip: &AnimDoc, frame: f32) -> f32 {
    if frame.is_nan() {
        0.0
    } else {
        frame.clamp(0.0, clip.frames().saturating_sub(1) as f32)
    }
}

impl App {
    /// A new session document, its steps numbered on the tag document's
    /// clock (`TagDoc::next_step`): the steps of a graph's tab are its
    /// tag's writes and its documents' edits, undone in the order made
    /// (docs/editor/studio-flow.md, "Shared patterns").
    fn anim_add(&mut self, doc: AnimationDocument) -> u64 {
        let mut doc = doc;
        doc.clip.share_steps(self.tags.step_clock());
        self.anim.insert(doc)
    }

    fn anim_document(&self, doc: u64) -> Result<&AnimationDocument, AnimError> {
        self.anim
            .docs
            .get(doc_index(doc)?)
            .ok_or_else(|| unknown_doc(doc))
    }

    pub fn anim_doc(&self, doc: u64) -> Result<&AnimDoc, AnimError> {
        Ok(&self.anim_document(doc)?.clip)
    }

    pub fn anim_doc_mut(&mut self, doc: u64) -> Result<&mut AnimDoc, AnimError> {
        self.anim
            .docs
            .get_mut(doc_index(doc)?)
            .map(|entry| &mut entry.clip)
            .ok_or_else(|| unknown_doc(doc))
    }

    pub fn anim_doc_info(&self, doc: u64) -> Result<&AnimSummary, AnimError> {
        Ok(&self.anim_document(doc)?.summary)
    }

    /// Resolve a document's graph against the map the Tags tool shows.
    /// Editing keys is independent of navigation; posing needs this map.
    pub fn anim_doc_tag(&self, doc: u64) -> Result<TagId, AnimError> {
        let graph = &self.anim_document(doc)?.graph;
        if self.anim_graph_key(graph.tag) != *graph {
            return Err(AnimError::Refused(format!(
                "animation document {doc} belongs to {}, which the Tags tool is not showing",
                graph.map.as_deref().unwrap_or("another map"),
            )));
        }
        Ok(graph.tag)
    }

    /// Authored documents for this graph, in creation order.
    pub fn anim_docs(&self, id: TagId) -> Vec<u64> {
        let graph = self.anim_graph_key(id);
        self.anim
            .docs
            .iter()
            .enumerate()
            .filter(|(_, doc)| doc.graph == graph && doc.retail_index.is_none())
            .map(|(i, _)| i as u64 + 1)
            .collect()
    }

    /// The canonical document ID, if the animation has been opened.
    pub fn anim_opened(&self, id: TagId, animation: usize) -> Option<u64> {
        self.anim.retail_doc(&self.anim_graph_key(id), animation)
    }

    /// Open the canonical edit document for a retail animation.
    pub fn anim_open(&mut self, id: TagId, animation: usize) -> Result<u64, AnimError> {
        if let Some(doc) = self.anim_opened(id, animation) {
            return Ok(doc);
        }
        let read = self.anim_read(id, animation, Tolerance::DEFAULT)?;
        self.anim_open_read(id, &read)
    }

    /// Reuse the default-tolerance read when the caller already has it.
    pub fn anim_open_read(&mut self, id: TagId, read: &AnimRead) -> Result<u64, AnimError> {
        if read.keys.tolerance != Tolerance::DEFAULT {
            return self.anim_open(id, read.animation.index);
        }
        let graph = self.anim_graph_key(id);
        let index = read.animation.index;
        if let Some(doc) = self.anim.retail_doc(&graph, index) {
            return Ok(doc);
        }
        let mut summary = read.animation.clone();
        if summary.name.trim().is_empty() {
            summary.name = format!("animation {index}");
        }
        let clip = document_from_read(read, summary.name.clone())?;
        Ok(self.anim_add(AnimationDocument {
            graph,
            summary,
            retail_index: Some(index),
            clip,
        }))
    }

    /// Start an empty base clip on the graph's skeleton.
    pub fn anim_new(&mut self, id: TagId, name: &str, frames: usize) -> Result<u64, AnimError> {
        self.ensure_anim(id)?;
        let loaded = self.anim.loaded.as_ref().expect("loaded");
        let clip =
            AnimDoc::new(name.to_owned(), frames, loaded.skeleton()).map_err(AnimError::Refused)?;
        let graph = loaded.key.clone();
        Ok(self.anim_add(AnimationDocument {
            graph,
            summary: AnimSummary {
                index: 0,
                name: clip.name().to_owned(),
                kind: "base",
                frames,
                codecs: [None, None],
                aim_grid: false,
                error: None,
            },
            retail_index: None,
            clip,
        }))
    }

    /// Start a separate document from the decoded retail animation.
    pub fn anim_duplicate(
        &mut self,
        id: TagId,
        animation: usize,
        name: &str,
    ) -> Result<u64, AnimError> {
        let read = self.anim_read(id, animation, Tolerance::DEFAULT)?;
        let clip = document_from_read(&read, name.to_owned())?;
        let graph = self.anim_graph_key(id);
        Ok(self.anim_add(AnimationDocument {
            graph,
            summary: authored_summary(read.animation, &clip),
            retail_index: None,
            clip,
        }))
    }

    /// Copy a document's current keys into a new document with its own history.
    pub fn anim_duplicate_doc(&mut self, doc: u64, name: &str) -> Result<u64, AnimError> {
        let source = self.anim_document(doc)?;
        let clip = source
            .clip
            .duplicate(name.to_owned())
            .map_err(AnimError::Refused)?;
        let doc = AnimationDocument {
            graph: source.graph.clone(),
            summary: authored_summary(source.summary.clone(), &clip),
            retail_index: None,
            clip,
        };
        Ok(self.anim_add(doc))
    }

    /// The number of the newest step of `tag`'s documents not undone:
    /// the documents are those of the map the Tags tool shows, and the
    /// steps of a graph's tab are undone in the order of these numbers
    /// (`App::anim_add`).
    pub(crate) fn anim_step_serial(&self, tag: TagId) -> Option<u64> {
        let graph = self.anim_graph_key(tag);
        self.anim
            .docs
            .iter()
            .filter(|doc| doc.graph == graph)
            .filter_map(|doc| doc.clip.step_serial())
            .max()
    }

    /// The number of the step a redo of `tag`'s documents takes next: the
    /// oldest undone step of a document (a document undoes newest first)
    /// that is newer than `floor`, the tab's newest step not undone. An
    /// undone step at or below it was made wrong by a later step.
    pub(crate) fn anim_redo_serial(&self, tag: TagId, floor: u64) -> Option<u64> {
        let graph = self.anim_graph_key(tag);
        self.anim
            .docs
            .iter()
            .filter(|doc| doc.graph == graph)
            .filter_map(|doc| doc.clip.redo_serial())
            .filter(|serial| *serial > floor)
            .min()
    }

    /// Undo the newest step of `tag`'s documents; whether one moved.
    pub(crate) fn anim_undo_in(&mut self, tag: TagId) -> bool {
        let graph = self.anim_graph_key(tag);
        self.anim
            .docs
            .iter_mut()
            .filter(|doc| doc.graph == graph)
            .filter_map(|doc| doc.clip.step_serial().map(|serial| (serial, doc)))
            .max_by_key(|(serial, _)| *serial)
            .is_some_and(|(_, doc)| doc.clip.undo())
    }

    /// Redo the step [`Self::anim_redo_serial`] names; whether one moved.
    pub(crate) fn anim_redo_in(&mut self, tag: TagId, floor: u64) -> bool {
        let graph = self.anim_graph_key(tag);
        self.anim
            .docs
            .iter_mut()
            .filter(|doc| doc.graph == graph)
            .filter_map(|doc| {
                doc.clip
                    .redo_serial()
                    .filter(|serial| *serial > floor)
                    .map(|serial| (serial, doc))
            })
            .min_by_key(|(serial, _)| *serial)
            .is_some_and(|(_, doc)| doc.clip.redo())
    }

    /// Every session document made from scratch, any map: its id, name,
    /// kind and the map it was made on (`None` for the open map's own).
    /// The sources a retarget can start from.
    pub fn anim_all_docs(&self) -> Vec<(u64, String, &'static str, Option<String>)> {
        self.anim
            .docs
            .iter()
            .enumerate()
            .filter(|(_, d)| d.retail_index.is_none())
            .map(|(i, d)| {
                (
                    i as u64 + 1,
                    d.summary.name.clone(),
                    d.summary.kind,
                    d.graph.map.clone(),
                )
            })
            .collect()
    }

    /// Pose a document on the graph's model. An overlay document applies
    /// its deltas over the rest pose when no base animation is selected.
    pub fn anim_doc_pose(
        &mut self,
        doc: u64,
        frame: f32,
        overlay: Option<(usize, f32)>,
        model: Option<&str>,
    ) -> Result<PoseAnswer, AnimError> {
        let id = self.anim_doc_tag(doc)?;
        self.ensure_anim(id)?;
        let entry = self.anim_document(doc)?;
        let frame = document_frame(&entry.clip, frame);
        let summary = entry.summary.clone();
        let sample = entry.clip.sample(frame);
        if summary.kind == "overlay" {
            if overlay.is_some() {
                return Err(AnimError::Refused(
                    "an overlay document needs a base animation for composition".to_owned(),
                ));
            }
            let rest = Pose {
                nodes: vec![NodeTransform::default(); sample.nodes.len()],
            };
            return self.anim.loaded.as_ref().expect("loaded").pose(
                summary,
                frame,
                rest,
                Some((&sample, 1.0)),
                model,
            );
        }
        let overlay = overlay
            .map(|(index, weight)| {
                let pose = self.anim_supporting_pose(id, index, frame)?;
                Ok::<_, AnimError>((pose, weight))
            })
            .transpose()?;
        self.anim.loaded.as_ref().expect("loaded").pose(
            summary,
            frame,
            sample,
            overlay.as_ref().map(|(pose, weight)| (pose, *weight)),
            model,
        )
    }

    /// Pose an overlay document over a selected base animation.
    pub fn anim_doc_pose_over(
        &mut self,
        doc: u64,
        frame: f32,
        base: usize,
        model: Option<&str>,
    ) -> Result<PoseAnswer, AnimError> {
        let id = self.anim_doc_tag(doc)?;
        self.ensure_anim(id)?;
        let entry = self.anim_document(doc)?;
        if entry.summary.kind != "overlay" {
            return Err(AnimError::Refused(
                "the document is not an overlay".to_owned(),
            ));
        }
        let frame = document_frame(&entry.clip, frame);
        let summary = entry.summary.clone();
        let overlay = entry.clip.sample(frame);
        let loaded = self.anim.loaded.as_ref().expect("loaded");
        if !loaded
            .graph
            .animations
            .get(base)
            .is_some_and(|a| a.anim_type == tags::anim::AnimationType::Base)
        {
            return Err(AnimError::Refused(format!(
                "animation {base} is not a base clip"
            )));
        }
        let pose = self.anim_supporting_pose(id, base, frame)?;
        self.anim.loaded.as_ref().expect("loaded").pose(
            summary,
            frame,
            pose,
            Some((&overlay, 1.0)),
            model,
        )
    }

    /// A supporting clip in a document preview, using its edit document
    /// if one is open.
    fn anim_supporting_pose(
        &mut self,
        id: TagId,
        index: usize,
        frame: f32,
    ) -> Result<Pose, AnimError> {
        if let Some(doc) = self.anim_opened(id, index) {
            return Ok(self.anim_doc(doc)?.sample(frame));
        }
        let mut files = self
            .shown_raw_files()
            .map_err(|e| AnimError::Failed(format!("the map's resource files: {e}")))?;
        let clip = self
            .anim
            .loaded
            .as_mut()
            .expect("loaded")
            .clip(index, &mut Raw(&mut files))
            .map_err(AnimError::Refused)?;
        if clip.is_aim_grid() {
            return Err(AnimError::Refused(format!(
                "animation {index} is an aim grid"
            )));
        }
        clip.sample(frame)
            .map_err(|e| AnimError::Failed(format!("sampling: {e}")))
    }

    /// Check authored samples and use the selected model's rest skeleton
    /// for world-space checks when the graph has one.
    pub fn anim_check(&mut self, doc: u64, model: Option<&str>) -> Result<Check, AnimError> {
        let tag = self.anim_doc_tag(doc)?;
        self.ensure_anim(tag)?;
        let entry = self.anim_document(doc)?;
        let loaded = self.anim.loaded.as_ref().expect("loaded");
        let defaults = loaded
            .pick_model(model)?
            .map(|i| {
                let (user, piece) = &loaded.models[i];
                piece
                    .as_deref()
                    .map(model_defaults)
                    .map_err(|e| AnimError::Refused(format!("{}: {e}", user.name)))
            })
            .transpose()?;
        entry
            .clip
            .check(defaults.as_deref(), entry.summary.kind == "overlay")
            .map_err(AnimError::Refused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tags::anim_keys::Keys;

    #[test]
    fn document_ids_follow_their_graph_across_map_changes() {
        let mut app = App::new();
        app.loaded = Some("first".to_owned());
        let tag = TagId(1);
        let read = AnimRead {
            animation: AnimSummary {
                index: 0,
                name: "source".to_owned(),
                kind: "base",
                frames: 1,
                codecs: [None, None],
                aim_grid: false,
                error: None,
            },
            frames: 1,
            nodes: vec![],
            keys: Keys {
                frames: 1,
                nodes: 0,
                tolerance: Tolerance::DEFAULT,
                channels: vec![],
            },
            dense_values: 0,
            max_error: [0.0; 3],
            events: [0; 3],
            notes: vec![],
        };

        let first = app.anim_open_read(tag, &read).unwrap();
        let copy = app.anim_duplicate_doc(first, "copy").unwrap();
        assert_eq!(app.anim_open_read(tag, &read).unwrap(), first);
        assert_eq!(app.anim_docs(tag), [copy]);

        app.loaded = Some("second".to_owned());
        let second = app.anim_open_read(tag, &read).unwrap();
        assert_ne!(second, first);
        assert!(app.anim_docs(tag).is_empty());
        assert!(app.anim_doc_tag(first).is_err());
        assert_eq!(app.anim_doc(copy).unwrap().name(), "copy");

        app.loaded = Some("first".to_owned());
        assert_eq!(app.anim_open_read(tag, &read).unwrap(), first);
        assert_eq!(app.anim_doc_tag(copy).unwrap(), tag);
        assert_eq!(app.anim_docs(tag), [copy]);
    }
}
