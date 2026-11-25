//! Ground labels on netgame flags of a map in Studio
//! (docs/game/game-types-without-carriers.md, "In Studio"): which flag
//! marks a wave drop, a base, a cover position or a siege place, and the
//! stamp of what the AI ground is built over.
//!
//! A label belongs to its flag, as an area does: it is kept by placement,
//! so a swap-removal or an append moves nothing, and an entry whose flag is
//! gone waits for its undo. The position, facing and team of a label are
//! the flag's own fields.
use content::Sha256;

use super::*;

/// The flag type a labelled flag is given: one the game reads no marker
/// for (`geometry::flag_type_name`), so a label never doubles as an
/// objective.
pub const LABEL_FLAG_TYPE: i64 = 5;

/// The blocks whose entries are solid: placing, moving or removing one
/// changes the collision the ground is built over.
pub const SOLID_BLOCKS: [BlockKind; 4] = [
    BlockKind::Scenery,
    BlockKind::Crates,
    BlockKind::Machines,
    BlockKind::Vehicles,
];

/// What a label marks, as `firefight_kit::ground::Labels` reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LabelKind {
    /// Where a wave arrives; a facing.
    Drop,
    /// A base the actors push at; a team.
    Base,
    /// A firing position; a facing.
    Cover,
    /// A site the attackers take.
    Site,
    /// The final stand.
    Goal,
    /// Where a side spawns in a phase: a team and a phase.
    Line,
}

impl LabelKind {
    pub const ALL: [LabelKind; 6] = [
        Self::Drop,
        Self::Base,
        Self::Cover,
        Self::Site,
        Self::Goal,
        Self::Line,
    ];

    /// The label's text, as a script reads it.
    pub const fn text(self) -> &'static str {
        match self {
            Self::Drop => "ff_drop",
            Self::Base => "ff_base",
            Self::Cover => "ff_cover",
            Self::Site => "siege_site",
            Self::Goal => "siege_goal",
            Self::Line => "siege_line",
        }
    }

    /// The name the group shows.
    pub const fn title(self) -> &'static str {
        match self {
            Self::Drop => "Wave drop",
            Self::Base => "Base",
            Self::Cover => "Cover",
            Self::Site => "Siege site",
            Self::Goal => "Siege goal",
            Self::Line => "Siege line",
        }
    }

    /// The label a text names.
    pub fn of(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.text() == text)
    }

    /// Whether the flag's team matters to the build.
    pub const fn has_team(self) -> bool {
        matches!(self, Self::Base | Self::Line)
    }

    /// Whether the label carries a phase.
    pub const fn has_phase(self) -> bool {
        matches!(self, Self::Line)
    }
}

/// The label a flag carries: what it marks, and a `siege_line`'s phase
/// (0 for every other kind).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlagLabel {
    pub kind: LabelKind,
    pub phase: u16,
}

impl Scenario {
    /// The label of netgame flag `index`, if it has one.
    pub fn flag_label(&self, index: usize) -> Option<FlagLabel> {
        let id = self.block(BlockKind::NetgameFlags).placement_id(index)?;
        self.labels.get(&id).copied()
    }

    /// Every labelled netgame flag, by flag index.
    pub fn flag_labels(&self) -> Vec<(usize, FlagLabel)> {
        (0..self.block(BlockKind::NetgameFlags).len())
            .filter_map(|i| Some((i, self.flag_label(i)?)))
            .collect()
    }

    /// Sets or clears (`None`) netgame flag `index`'s label and returns the
    /// one it replaced. A phase on a kind that takes none is refused.
    pub fn set_label(
        &mut self,
        index: usize,
        label: Option<FlagLabel>,
    ) -> Result<Option<FlagLabel>> {
        if let Some(l) = label
            && !l.kind.has_phase()
            && l.phase != 0
        {
            return Err(Error::Declined(format!("{} takes no phase", l.kind.text())));
        }
        self.write_label(index, label)
    }

    /// [`Self::set_label`] without the checks, for an undo.
    pub(crate) fn write_label(
        &mut self,
        index: usize,
        label: Option<FlagLabel>,
    ) -> Result<Option<FlagLabel>> {
        let flags = BlockKind::NetgameFlags;
        let id = self
            .block(flags)
            .placement_id(index)
            .ok_or(Error::NoEntry(flags, index))?;
        Ok(match label {
            Some(l) => self.labels.insert(id, l),
            None => self.labels.remove(&id),
        })
    }

    /// The digest of what the collision under the AI ground is made of:
    /// where each solid object stands and what it is
    /// ([`SOLID_BLOCKS`]), and the Modeling tool's writes. An edit that
    /// touches none of them (a spawn, a name, a script) leaves it as it was.
    pub fn collision_stamp(&self) -> Sha256 {
        let mut out = b"collision-stamp-1".to_vec();
        for kind in SOLID_BLOCKS {
            let block = self.block(kind);
            out.extend(format!("{kind:?}:{}\n", block.len()).bytes());
            let fields = layout::block(kind).fields.iter().filter(|f| {
                matches!(
                    f.role,
                    Role::Position | Role::Rotation | Role::Scale | Role::PaletteIndex
                )
            });
            let fields: Vec<_> = fields.collect();
            for i in 0..block.len() {
                for f in &fields {
                    if let Ok(v) = self.get(kind, i, f.name) {
                        out.extend(format!("{v:?};").bytes());
                    }
                }
                out.push(b'\n');
            }
        }
        out.extend(format!("{:?}", self.geometry).bytes());
        out.extend(format!("{:?}", self.bsp_patches).bytes());
        Sha256::of(&out)
    }
}
