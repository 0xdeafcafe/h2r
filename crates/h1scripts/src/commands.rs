//! Scenario operands used by generated command scripts. The encounter reader
//! and the compiler share the same command-list decoder and point-set order.

use crate::{Result, invalid};
use h1ai::h1::{self, CommandList};
use h1map::{Group, H1Map, Space, TagId, TagRef};

/// A graph required by command atoms independently of cinematic settings.
pub const ANIMATION_GRAPH: &str = "command_animation_graph";

#[derive(Debug, Clone)]
pub struct Animation {
    pub name: String,
    pub graph_id: TagId,
    pub graph: Option<String>,
    /// For an implicit graph: the converted name in each compatible unit graph.
    pub unit_names: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct ActorTiming {
    pub idle: [f32; 2],
    pub recovery: f32,
    pub placements: Vec<String>,
}

/// Starting-location names are local to a squad. The actor retains this
/// string ID when it migrates (H2 0x272c13), so the tuning follows the unit.
pub fn placement_name(variant: i16, index: usize) -> String {
    format!("h1_actor_{variant}_{index}")
}

#[derive(Debug, Clone, Default)]
pub struct Lists {
    pub lists: Vec<CommandList>,
    pub timing: Vec<ActorTiming>,
    pub objects: Vec<String>,
    pub scripts: Vec<String>,
    pub recordings: Vec<String>,
    pub animations: Vec<Animation>,
    pub voices: Vec<(String, Vec<Option<String>>)>,
}

impl Lists {
    pub(crate) fn decode(scenario: &[u8], space: &Space) -> Result<Self> {
        let lists = h1::command_lists(scenario, space).map_err(|e| invalid(e.to_string()))?;
        if lists.is_empty() {
            return Ok(Self::default());
        }
        let names = |at, stride| -> Result<Vec<String>> {
            space
                .block_at(scenario, at, stride)?
                .into_iter()
                .map(super::layout::name)
                .collect()
        };
        let (at, stride) = h1::layout::AI_ANIMATIONS;
        let animations = space
            .block_at(scenario, at, stride)?
            .into_iter()
            .map(|a| {
                Ok(Animation {
                    name: super::layout::name(a)?,
                    graph_id: TagRef::read(a, 0x20)
                        .ok_or_else(|| invalid("AI animation reference"))?
                        .id,
                    graph: None,
                    unit_names: Vec::new(),
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            lists,
            animations,
            voices: Vec::new(),
            timing: Vec::new(),
            objects: names(0x204, 0x24)?,
            scripts: names(h1::layout::AI_SCRIPTS.0, h1::layout::AI_SCRIPTS.1)?,
            recordings: names(h1::layout::AI_RECORDINGS.0, h1::layout::AI_RECORDINGS.1)?,
        })
    }

    pub(crate) fn resolve(&mut self, map: &H1Map) -> Result<()> {
        if self
            .lists
            .iter()
            .flat_map(|l| &l.atoms)
            .any(|a| a.kind == 16)
        {
            for tag in map.tags().iter().filter(|t| t.group == Group::new(b"udlg")) {
                let bytes = map.tag_data(tag)?;
                let mut slots = Vec::with_capacity(256);
                for i in 0..256 {
                    let r = TagRef::read(bytes, 16 + 16 * i)
                        .ok_or_else(|| invalid("short command dialogue table"))?;
                    slots.push(if r.id.0 != u32::MAX && r.group == Group::new(b"snd!") {
                        Some(map.tag(r.id)?.name.clone())
                    } else {
                        None
                    });
                }
                self.voices.push((tag.name.clone(), slots));
            }
        }
        for a in &mut self.animations {
            if a.graph_id.0 != u32::MAX {
                let tag = map.tag(a.graph_id)?;
                if tag.group != Group::new(b"antr") {
                    return Err(invalid(format!(
                        "AI animation {} names a {}",
                        a.name, tag.group
                    )));
                }
                a.graph = Some(tag.name.clone());
            }
        }
        if self
            .lists
            .iter()
            .flat_map(|l| &l.atoms)
            .any(|a| a.kind == 7 || (a.kind == 23 && a.parameters == [0.0, 0.0]))
        {
            let ai = h1::H1Ai::read(map).map_err(|e| invalid(e.to_string()))?;
            let mut births =
                std::collections::BTreeMap::<i16, std::collections::BTreeSet<usize>>::new();
            for squad in ai.encounters.iter().flat_map(|e| &e.squads) {
                for (i, loc) in squad.starting_locations.iter().enumerate() {
                    let v = if loc.actor >= 0 {
                        loc.actor
                    } else {
                        squad.actor
                    };
                    if v >= 0 {
                        births.entry(v).or_default().insert(i);
                    }
                }
            }
            for (v, locations) in births {
                let Some(variant) = ai.actor_palette.get(v as usize).and_then(Option::as_ref)
                else {
                    continue;
                };
                let Some(tag) = map.find(Group::new(b"actv"), &variant.name) else {
                    continue;
                };
                let variant_data = map.tag_data(tag)?;
                let recovery = f32::from_bits(
                    super::u32_at(variant_data, 132)
                        .ok_or_else(|| invalid("actv firing interval"))?,
                )
                .max(2.0);
                let idle = if variant.actor.id.0 == u32::MAX {
                    [0.0; 2]
                } else {
                    let actor = map.tag_data(map.tag(variant.actor.id)?)?;
                    let word = |off| {
                        super::u32_at(actor, off)
                            .map(f32::from_bits)
                            .ok_or_else(|| invalid("actr idle-look interval"))
                    };
                    [word(236)?, word(240)?]
                };
                self.timing.push(ActorTiming {
                    idle,
                    recovery,
                    placements: locations
                        .into_iter()
                        .map(|i| placement_name(v, i))
                        .collect(),
                });
            }
        }
        let implicit: std::collections::BTreeSet<_> = self
            .lists
            .iter()
            .flat_map(|l| &l.atoms)
            .filter(|a| a.kind == 13)
            .filter_map(|a| usize::try_from(a.animation).ok())
            .filter(|&i| self.animations.get(i).is_some_and(|a| a.graph.is_none()))
            .collect();
        if !implicit.is_empty() {
            for tag in map.tags().iter().filter(|t| t.group == Group::new(b"antr")) {
                let names = h1models::h1::animation_names(map, tag.data_address)
                    .map_err(|e| invalid(e.to_string()))?;
                let hits: Vec<_> = implicit
                    .iter()
                    .filter_map(|&i| {
                        names
                            .iter()
                            .position(|n| n.eq_ignore_ascii_case(&self.animations[i].name))
                            .map(|j| (i, j))
                    })
                    .collect();
                if hits.is_empty() {
                    continue;
                }
                let graph = h1models::h1::Graph::read(map, tag.data_address)
                    .map_err(|e| invalid(e.to_string()))?;
                let converted = h1models::jmad::animation_names(&graph);
                for (i, j) in hits {
                    self.animations[i]
                        .unit_names
                        .push((tag.name.clone(), converted[j].0.full()));
                }
            }
        }
        Ok(())
    }
}

/// Kind 13 sets the same two death flags as H1 (0x2592e1 and H1 0x16c033).
pub(crate) fn identify(
    out: &mut std::collections::HashMap<String, crate::translate::H2Builtin>,
    catalog: &hs::Catalog,
) {
    if let Ok(f) = catalog.function(418)
        && f.evaluate() == 0x2a7d70
        && f.return_type() == 4
        && f.parameters() == [7]
    {
        out.entry("cs_die".into())
            .or_insert_with(|| crate::translate::H2Builtin {
                index: 418,
                return_type: 4,
                parameters: vec![7],
                special: false,
            });
    }
}
