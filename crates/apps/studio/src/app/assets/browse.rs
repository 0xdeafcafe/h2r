//! The Browser's link views (docs/editor/studio-redesign.md, "The
//! browser"): **Where used** lists what uses an asset and **Show edits**
//! the package items that change it, both read from the link table
//! (`app::links`, `asset.links`). The view is the app's, so a menu entry
//! and `asset.where_used` or `asset.edits` leave the same rows in the
//! panel ([`super::super::Action::WhereUsed`],
//! [`super::super::Action::ShowEdits`]).
use super::*;
use crate::app::links::Link;

/// Which question a view answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowseMode {
    WhereUsed,
    Edits,
}

impl BrowseMode {
    /// The mode as the API names it.
    pub const fn id(self) -> &'static str {
        match self {
            Self::WhereUsed => "where_used",
            Self::Edits => "edits",
        }
    }

    /// The heading the panel draws.
    pub const fn title(self) -> &'static str {
        match self {
            Self::WhereUsed => "Where used",
            Self::Edits => "Edits",
        }
    }
}

/// One row: an asset that opens, the word it is grouped by and where the
/// row was read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowseRow {
    pub asset: String,
    pub role: String,
    pub via: String,
}

/// The rows the Browser shows in place of its list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowseView {
    pub mode: BrowseMode,
    /// The asset the question was asked of.
    pub of: String,
    pub rows: Vec<BrowseRow>,
    /// What could not be read, one line each.
    pub problems: Vec<String>,
}

impl BrowseView {
    pub fn json(&self) -> Value {
        json!({
            "asset": self.of,
            "view": self.mode.id(),
            "rows": self.rows.iter().map(|r| json!({
                "asset": r.asset,
                "role": r.role,
                "via": r.via,
            })).collect::<Vec<_>>(),
            "total": self.rows.len(),
            "problems": self.problems,
        })
    }

    /// The panel's heading: the question, the asset's last path part and
    /// the number of rows.
    pub fn heading(&self) -> String {
        let leaf = self
            .of
            .rsplit(['\\', ':'])
            .next()
            .unwrap_or(self.of.as_str());
        format!("{}: {leaf} ({})", self.mode.title(), self.rows.len())
    }
}

/// The Browser's link view, when it shows one.
#[derive(Default)]
pub struct Browse {
    pub view: Option<BrowseView>,
    /// Counts the views shown, so the panel comes forward for a new one.
    pub serial: u32,
}

fn row(link: &Link) -> BrowseRow {
    BrowseRow {
        asset: link.asset.clone(),
        role: link.role.to_owned(),
        via: link.via.clone(),
    }
}

/// One field line of a patch, as a row's `via` says it.
fn field_text(f: &crate::app::mods::FieldChange) -> String {
    let text = format!("{} = {}", f.path, f.value);
    if f.wins { text } else { format!("{text} (overridden)") }
}

/// The `via` of one package's patch: where it reaches, then its lines.
fn patch_text(c: &crate::app::mods::ChangedBy) -> String {
    let mut parts = vec![if c.forked {
        format!("fork for {}", c.maps)
    } else {
        c.maps.clone()
    }];
    parts.extend(c.fields.iter().map(field_text));
    if c.blocks > 0 {
        parts.push(format!("{} block lines", c.blocks));
    }
    parts.join("; ")
}

impl App {
    /// **Where used**: `used_by` of the link table for `asset`.
    pub fn where_used(&self, asset: &str) -> Result<BrowseView, String> {
        let id = AssetId::parse(asset)?;
        let links = self.asset_links(&id)?;
        Ok(BrowseView {
            mode: BrowseMode::WhereUsed,
            of: asset.to_owned(),
            rows: links.used_by.iter().map(row).collect(),
            problems: links.problems,
        })
    }

    /// **Show edits**: for a tag, the package items with a patch on it in
    /// load order (`changed_by` of the link table); for a package, its
    /// `edits` and `tag-delta` items and the items that replace an asset.
    /// An item and a voice have no edit lines of their own.
    pub fn asset_edits(&self, asset: &str) -> Result<BrowseView, String> {
        let id = AssetId::parse(asset)?;
        let mut view = BrowseView {
            mode: BrowseMode::Edits,
            of: asset.to_owned(),
            rows: Vec::new(),
            problems: Vec::new(),
        };
        match &id {
            AssetId::Tag(_) => {
                let links = self.asset_links(&id)?;
                view.rows = links
                    .changed_by
                    .iter()
                    .map(|c| BrowseRow {
                        asset: format!("item:{}:{}", c.package, c.item),
                        role: c.change.to_owned(),
                        via: patch_text(c),
                    })
                    .collect();
                view.problems = links.problems;
            }
            AssetId::Package(package) => {
                let set = read_set(&self.data_root());
                let entry = set
                    .on
                    .iter()
                    .find(|e| e.loaded.package.name() == package.as_str())
                    .ok_or_else(|| format!("no package {package:?} is on"))?;
                for (n, item) in entry.loaded.package.manifest.items.iter().enumerate() {
                    let role = match item.kind {
                        ItemKind::Edits => "edit",
                        ItemKind::TagDelta => "patch",
                        _ if item.replaces.is_some() => "replaces",
                        _ => continue,
                    };
                    view.rows.push(BrowseRow {
                        asset: format!("item:{package}:{n}"),
                        role: role.to_owned(),
                        via: item
                            .source
                            .clone()
                            .or_else(|| item.tag.clone())
                            .unwrap_or_default(),
                    });
                }
            }
            AssetId::Item(..) | AssetId::Voice(_) | AssetId::Forge(..) => {
                return Err(format!(
                    "{asset}: Show edits is for a tag or a package (asset.links lists what any other asset touches)"
                ));
            }
        }
        Ok(view)
    }

    fn show_browse(&mut self, view: Result<BrowseView, String>) {
        match view {
            Ok(view) => {
                self.browse.view = Some(view);
                self.browse.serial = self.browse.serial.wrapping_add(1);
            }
            Err(e) => self.error(e),
        }
    }

    /// [`super::super::Action::WhereUsed`].
    pub(crate) fn show_where_used(&mut self, asset: &str) {
        let view = self.where_used(asset);
        self.show_browse(view);
    }

    /// [`super::super::Action::ShowEdits`].
    pub(crate) fn show_edits(&mut self, asset: &str) {
        let view = self.asset_edits(asset);
        self.show_browse(view);
    }

    /// [`super::super::Action::ClearBrowse`]: the Browser lists again.
    pub(crate) fn clear_browse(&mut self) {
        self.browse.view = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_heads_with_its_question_the_assets_leaf_and_the_row_count() {
        let view = BrowseView {
            mode: BrowseMode::WhereUsed,
            of: "tag:weap:objects\\weapons\\rifle".into(),
            rows: vec![BrowseRow {
                asset: "package:kit-test".into(),
                role: "Package".into(),
                via: "edits".into(),
            }],
            problems: Vec::new(),
        };
        assert_eq!(view.heading(), "Where used: rifle (1)");
        let v = view.json();
        assert_eq!(v["view"], "where_used");
        assert_eq!(v["total"], 1);
        assert_eq!(v["rows"][0]["asset"], "package:kit-test");
    }

    #[test]
    fn a_patch_reads_as_its_reach_then_its_lines() {
        use crate::app::mods::{ChangedBy, FieldChange};
        let c = ChangedBy {
            package: "kit-test".into(),
            item: 1,
            change: "edit",
            maps: "alpha".into(),
            forked: true,
            blocks: 2,
            fields: vec![
                FieldChange { path: "x".into(), from: None, value: json!(1), wins: true },
                FieldChange { path: "y".into(), from: None, value: json!(2), wins: false },
            ],
        };
        assert_eq!(
            patch_text(&c),
            "fork for alpha; x = 1; y = 2 (overridden); 2 block lines"
        );
    }
}
