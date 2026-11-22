//! The start gate's check: does a map hold what a script's label and tag
//! tables require?
//!
//! [`check`] is pure: it compares the IR's tables with a [`Census`] of the
//! map for the teams in play and returns every problem, each with the text
//! a player or an admin reads. [`minimal`] gives the smallest census the
//! check accepts. The lobby, the host, Live's hopper builder and the
//! workbench call the same check. See `docs/game/rules-start-gate.md`, "The
//! check", and `docs/game/rules-language-types.md`, "The start gate".

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ir::{Ir, LabelDef, LabelKind, TagGroup};

/// What a labelled placement is, which the map decides and a label does
/// not store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// A placement in an object block (scenery, crates, vehicles, weapons,
    /// equipment, machines, netgame equipment), or a kit barrier.
    Object,
    /// A netgame flag, with its own label or its type's built-in one.
    Flag,
    /// A Forge trigger volume.
    Volume,
    /// A starting location.
    Spawn,
}

impl Class {
    /// Whether a member of this class satisfies a label of `kind`.
    pub fn satisfies(self, kind: LabelKind) -> bool {
        match kind {
            LabelKind::Object => self != Class::Spawn,
            LabelKind::Zone => matches!(self, Class::Flag | Class::Volume),
            LabelKind::Spawn => self == Class::Spawn,
        }
    }

    fn a(self) -> &'static str {
        match self {
            Class::Object => "an object",
            Class::Flag => "a flag",
            Class::Volume => "a trigger volume",
            Class::Spawn => "a starting location",
        }
    }
}

/// The object type of a tag the map holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectType {
    Biped,
    Vehicle,
    Weapon,
    Equipment,
    Garbage,
    Projectile,
    Scenery,
    Machine,
    Control,
    LightFixture,
    SoundScenery,
    Crate,
    Creature,
}

impl ObjectType {
    fn a(self) -> &'static str {
        match self {
            ObjectType::Biped => "a biped",
            ObjectType::Vehicle => "a vehicle",
            ObjectType::Weapon => "a weapon",
            ObjectType::Equipment => "equipment",
            ObjectType::Garbage => "garbage",
            ObjectType::Projectile => "a projectile",
            ObjectType::Scenery => "scenery",
            ObjectType::Machine => "a machine",
            ObjectType::Control => "a control",
            ObjectType::LightFixture => "a light fixture",
            ObjectType::SoundScenery => "sound scenery",
            ObjectType::Crate => "a crate",
            ObjectType::Creature => "a creature",
        }
    }

    /// Whether a tag of this type is of `group`.
    pub fn is(self, group: TagGroup) -> bool {
        match group {
            TagGroup::Any => true,
            TagGroup::Weapon => self == ObjectType::Weapon,
        }
    }
}

/// One labelled placement of the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub class: Class,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<u8>,
}

/// What a map holds that a script can require: for each label text, its
/// members; and the object type of each tag the map holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Census {
    #[serde(default)]
    pub labels: BTreeMap<String, Vec<Member>>,
    #[serde(default)]
    pub tags: BTreeMap<String, ObjectType>,
}

/// What is wrong, by the IR's label- or tag-table index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProblemKind {
    /// None of a label that needs at least one.
    Missing { label: u16 },
    /// Fewer than the label's `min`, or more than a stated `max`.
    Count { label: u16, found: usize },
    /// A member whose class does not satisfy the label's kind (the first).
    Class { label: u16, class: Class },
    /// A number of the label's range with no member.
    MissingNumber { label: u16, number: i32 },
    /// A member's number outside the label's range.
    OutOfRange { label: u16, number: i32 },
    /// A member with no number, of a numbered label.
    Unnumbered { label: u16 },
    /// A team in play with no member, of a `per team` label.
    Team { label: u16, team: u8 },
    /// A tag the map holds of another type than the tag table's group.
    TagGroup { tag: u16, found: ObjectType },
}

/// One problem and its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub kind: ProblemKind,
    pub text: String,
}

/// Every problem the map has with the script's tables, labels in table
/// order, then tags. `teams` are the teams in play, which `per team` reads.
/// Labels the script does not declare, and tags the map does not hold, are
/// not problems.
pub fn check(ir: &Ir, census: &Census, teams: &[u8]) -> Vec<Problem> {
    let name = ir.header.name.as_str();
    let mut out = Vec::new();
    for (i, def) in ir.labels.iter().enumerate() {
        label(name, i as u16, def, census, teams, &mut out);
    }
    for (i, t) in ir.tags.iter().enumerate() {
        if let Some(&found) = census.tags.get(&t.path)
            && !found.is(t.group)
        {
            out.push(Problem {
                kind: ProblemKind::TagGroup { tag: i as u16, found },
                text: format!(
                    "\"{}\" on this map is {}; {name} gives it as {}",
                    t.path,
                    found.a(),
                    group_a(t.group)
                ),
            });
        }
    }
    out
}

fn label(name: &str, index: u16, def: &LabelDef, census: &Census, teams: &[u8], out: &mut Vec<Problem>) {
    let text = def.text.as_str();
    let all = census.labels.get(text).map_or(&[][..], Vec::as_slice);
    let good: Vec<&Member> = all.iter().filter(|m| m.class.satisfies(def.kind)).collect();
    let wrong = all.iter().find(|m| !m.class.satisfies(def.kind));
    let one = noun(def.kind, 1);
    let mut push = |kind, text| out.push(Problem { kind, text });

    if let Some(w) = wrong {
        push(
            ProblemKind::Class { label: index, class: w.class },
            format!("\"{text}\" on this map is {}; {name} needs {}", w.class.a(), kind_a(def.kind)),
        );
    }
    let n = good.len();
    if n == 0 && (def.min > 0 || wrong.is_some()) {
        // Nothing to number or to give a team: the class or the missing
        // label is the problem.
        if wrong.is_none() {
            push(
                ProblemKind::Missing { label: index },
                format!("the map has no {one} labelled \"{text}\"; {name} needs {}", needs(def)),
            );
        }
        return;
    }
    if n < def.min as usize || (def.bounded() && n > def.max as usize) {
        push(
            ProblemKind::Count { label: index, found: n },
            format!(
                "the map has {n} {} labelled \"{text}\"; {name} needs {}",
                noun(def.kind, n),
                needs(def)
            ),
        );
    }
    if def.numbered() && good.iter().any(|m| m.number.is_none()) {
        push(
            ProblemKind::Unnumbered { label: index },
            format!("a {one} labelled \"{text}\" has no number; {name} needs every one numbered"),
        );
    }
    if let Some(range) = def.range() {
        let mut outside: Vec<i32> = good
            .iter()
            .filter_map(|m| m.number)
            .filter(|k| !range.contains(k))
            .collect();
        outside.sort_unstable();
        outside.dedup();
        for number in outside {
            push(
                ProblemKind::OutOfRange { label: index, number },
                format!(
                    "a {one} labelled \"{text}\" has the number {number}; {name} needs only {}",
                    range_text(def.lo, def.hi)
                ),
            );
        }
        for number in range {
            if !good.iter().any(|m| m.number == Some(number)) {
                push(
                    ProblemKind::MissingNumber { label: index, number },
                    format!(
                        "no {one} labelled \"{text}\" has the number {number}; {name} needs {}",
                        range_text(def.lo, def.hi)
                    ),
                );
            }
        }
    }
    if def.per_team() {
        for &team in teams {
            if !good.iter().any(|m| m.team == Some(team)) {
                push(
                    ProblemKind::Team { label: index, team },
                    format!("team {team} has no {one} labelled \"{text}\""),
                );
            }
        }
    }
}

/// The smallest census [`check`] accepts for `teams`: for each label, the
/// most of its `min`, one member per number of its range and one per team
/// in play; volumes for a zone label, objects and starting locations for
/// the others. It holds no tags. A label whose `max` is below what its
/// numbers or teams need has no passing census, and this one fails it.
pub fn minimal(ir: &Ir, teams: &[u8]) -> Census {
    let mut census = Census::default();
    for def in &ir.labels {
        let class = match def.kind {
            LabelKind::Object => Class::Object,
            LabelKind::Zone => Class::Volume,
            LabelKind::Spawn => Class::Spawn,
        };
        let numbers = def.range().map_or(0, |r| (def.hi as i64 - *r.start() as i64 + 1) as usize);
        let per_team = if def.per_team() { teams.len() } else { 0 };
        let n = (def.min as usize).max(numbers).max(per_team);
        if n == 0 {
            continue;
        }
        let members = (0..n)
            .map(|i| Member {
                class,
                number: if numbers > 0 {
                    Some(def.lo + (i % numbers) as i32)
                } else if def.numbered() {
                    Some(i as i32)
                } else {
                    None
                },
                team: (per_team > 0).then(|| teams[i % per_team]),
            })
            .collect();
        census.labels.insert(def.text.clone(), members);
    }
    census
}

fn noun(kind: LabelKind, n: usize) -> &'static str {
    match (kind, n == 1) {
        (LabelKind::Object, true) => "object",
        (LabelKind::Object, false) => "objects",
        (LabelKind::Zone, true) => "zone",
        (LabelKind::Zone, false) => "zones",
        (LabelKind::Spawn, true) => "starting location",
        (LabelKind::Spawn, false) => "starting locations",
    }
}

fn kind_a(kind: LabelKind) -> &'static str {
    match kind {
        LabelKind::Object => "an object (anything placed but a starting location)",
        LabelKind::Zone => "a zone (a trigger volume or a flag)",
        LabelKind::Spawn => "starting locations",
    }
}

fn group_a(group: TagGroup) -> &'static str {
    match group {
        TagGroup::Any => "an object",
        TagGroup::Weapon => "a weapon",
    }
}

/// A label's requirement in Forge's words: `exactly 1 zone labelled
/// "goal"`, `at most 16 zones labelled "ctf_flag_spawn", one for each
/// team`, `any number of objects labelled "barrier"`. The workbench's
/// "What a map needs" lists it.
pub fn describe(def: &LabelDef) -> String {
    let text = def.text.as_str();
    let mut out = if def.range().is_some() {
        format!(
            "{} labelled \"{text}\", numbered {}",
            noun(def.kind, 2),
            range_text(def.lo, def.hi)
        )
    } else {
        let one = def.single() || def.max == 1 || (def.min == 1 && !def.bounded());
        let count = if def.min == 0 && !def.bounded() {
            "any number of".to_string()
        } else {
            needs(def)
        };
        let mut s = format!("{count} {} labelled \"{text}\"", noun(def.kind, if one { 1 } else { 2 }));
        if def.numbered() {
            s.push_str(", each with a number");
        }
        s
    };
    if def.per_team() {
        out.push_str(", one for each team");
    }
    out
}

/// How many members the label needs, in the words of a requirement.
fn needs(def: &LabelDef) -> String {
    if def.single() {
        return if def.min == 1 { "exactly 1".into() } else { "at most 1".into() };
    }
    if def.min == def.max {
        format!("exactly {}", def.min)
    } else if !def.bounded() {
        format!("at least {}", def.min)
    } else if def.min == 0 {
        format!("at most {}", def.max)
    } else {
        format!("{} to {}", def.min, def.max)
    }
}

fn range_text(lo: i32, hi: i32) -> String {
    if lo == hi {
        lo.to_string()
    } else if hi as i64 == lo as i64 + 1 {
        format!("{lo} and {hi}")
    } else {
        format!("{lo} to {hi}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::label_flags::*;
    use crate::ir::tests::sample;

    fn def(text: &str, kind: LabelKind, min: u16, max: u16, flags: u8, lo: i32, hi: i32) -> LabelDef {
        LabelDef {
            text: text.into(),
            kind,
            min,
            max,
            flags,
            lo,
            hi,
        }
    }

    /// A Siege-shaped table: each kind of requirement once.
    fn siege() -> Ir {
        let mut ir = sample();
        ir.header.name = "Siege".into();
        ir.labels = vec![
            def("goal", LabelKind::Zone, 1, 1, SINGLE, 0, 0),
            def("core", LabelKind::Object, 1, 1, SINGLE, 0, 0),
            def("zone", LabelKind::Zone, 0, 256, NUMBERED | RANGE, 0, 1),
            def("hill_corner", LabelKind::Zone, 0, 256, NUMBERED, 0, 0),
            def("ctf_flag_spawn", LabelKind::Zone, 0, 16, PER_TEAM, 0, 0),
            def("bonus", LabelKind::Object, 0, 1, SINGLE, 0, 0),
            def("attack", LabelKind::Spawn, 1, 256, 0, 0, 0),
            LabelDef::objects("barrier"),
        ];
        crate::verify(&ir).unwrap();
        ir
    }

    const TEAMS: &[u8] = &[0, 1];

    fn texts(ir: &Ir, census: &Census) -> Vec<String> {
        check(ir, census, TEAMS).into_iter().map(|p| p.text).collect()
    }

    fn member(class: Class, number: Option<i32>, team: Option<u8>) -> Member {
        Member { class, number, team }
    }

    #[test]
    fn the_minimal_census_passes() {
        let ir = siege();
        let census = minimal(&ir, TEAMS);
        assert_eq!(check(&ir, &census, TEAMS), vec![]);
        assert_eq!(census.labels["zone"].iter().map(|m| m.number).collect::<Vec<_>>(), vec![Some(0), Some(1)]);
        assert_eq!(
            census.labels["ctf_flag_spawn"].iter().map(|m| m.team).collect::<Vec<_>>(),
            vec![Some(0), Some(1)]
        );
        assert!(!census.labels.contains_key("bonus") && !census.labels.contains_key("barrier"));
    }

    #[test]
    fn every_builtin_passes_its_minimal_census() {
        let all = crate::builtin::ALL
            .iter()
            .map(|b| (b.name, b.source))
            .chain(crate::builtin::examples().iter().copied());
        let team_sets: [&[u8]; 4] = [&[], &[0], &[0, 1], &[0, 1, 2, 3, 4, 5, 6, 7]];
        for (name, src) in all {
            let ir = crate::compile(src).unwrap();
            for teams in team_sets {
                assert_eq!(check(&ir, &minimal(&ir, teams), teams), vec![], "{name}");
            }
        }
    }

    #[test]
    fn an_empty_map_lacks_each_required_label_and_number() {
        let ir = siege();
        assert_eq!(
            texts(&ir, &Census::default()),
            vec![
                "the map has no zone labelled \"goal\"; Siege needs exactly 1",
                "the map has no object labelled \"core\"; Siege needs exactly 1",
                "no zone labelled \"zone\" has the number 0; Siege needs 0 and 1",
                "no zone labelled \"zone\" has the number 1; Siege needs 0 and 1",
                "team 0 has no zone labelled \"ctf_flag_spawn\"",
                "team 1 has no zone labelled \"ctf_flag_spawn\"",
                "the map has no starting location labelled \"attack\"; Siege needs at least 1",
            ]
        );
        let kinds: Vec<_> = check(&ir, &Census::default(), TEAMS).into_iter().map(|p| p.kind).collect();
        assert_eq!(kinds[0], ProblemKind::Missing { label: 0 });
        assert_eq!(kinds[2], ProblemKind::MissingNumber { label: 2, number: 0 });
        assert_eq!(kinds[5], ProblemKind::Team { label: 4, team: 1 });
    }

    #[test]
    fn too_many_and_too_few() {
        let ir = siege();
        let mut census = minimal(&ir, TEAMS);
        census.labels.insert("core".into(), vec![member(Class::Object, None, None); 3]);
        census.labels.insert("bonus".into(), vec![member(Class::Object, None, None); 2]);
        let spawns: Vec<Member> = (0..17).map(|i| member(Class::Flag, None, Some(i % 2))).collect();
        census.labels.insert("ctf_flag_spawn".into(), spawns);
        // An unstated limit blocks nothing.
        census.labels.insert("barrier".into(), vec![member(Class::Object, None, None); 300]);
        assert_eq!(
            texts(&ir, &census),
            vec![
                "the map has 3 objects labelled \"core\"; Siege needs exactly 1",
                "the map has 17 zones labelled \"ctf_flag_spawn\"; Siege needs at most 16",
                "the map has 2 objects labelled \"bonus\"; Siege needs at most 1",
            ]
        );
        assert_eq!(check(&ir, &census, TEAMS)[0].kind, ProblemKind::Count { label: 1, found: 3 });

        let mut ir = siege();
        ir.labels[7].min = 2;
        ir.labels[7].max = 4;
        let mut census = minimal(&ir, TEAMS);
        census.labels.insert("barrier".into(), vec![member(Class::Object, None, None)]);
        assert_eq!(
            texts(&ir, &census),
            vec!["the map has 1 object labelled \"barrier\"; Siege needs 2 to 4"]
        );
    }

    #[test]
    fn a_member_of_the_wrong_class() {
        let ir = siege();
        let mut census = minimal(&ir, TEAMS);
        census.labels.insert("goal".into(), vec![member(Class::Object, None, None)]);
        // A flag is an object and a zone.
        census.labels.insert("core".into(), vec![member(Class::Flag, None, None)]);
        census.labels.get_mut("attack").unwrap().push(member(Class::Volume, None, None));
        assert_eq!(
            texts(&ir, &census),
            vec![
                "\"goal\" on this map is an object; Siege needs a zone (a trigger volume or a flag)",
                "\"attack\" on this map is a trigger volume; Siege needs starting locations",
            ]
        );
        assert_eq!(
            check(&ir, &census, TEAMS)[0].kind,
            ProblemKind::Class { label: 0, class: Class::Object }
        );
    }

    #[test]
    fn numbering_gaps_strays_and_unnumbered_members() {
        let ir = siege();
        let mut census = minimal(&ir, TEAMS);
        census.labels.insert(
            "zone".into(),
            vec![member(Class::Volume, Some(0), None), member(Class::Flag, Some(3), None)],
        );
        census.labels.insert(
            "hill_corner".into(),
            vec![member(Class::Volume, Some(0), None), member(Class::Volume, None, None)],
        );
        assert_eq!(
            texts(&ir, &census),
            vec![
                "a zone labelled \"zone\" has the number 3; Siege needs only 0 and 1",
                "no zone labelled \"zone\" has the number 1; Siege needs 0 and 1",
                "a zone labelled \"hill_corner\" has no number; Siege needs every one numbered",
            ]
        );
        let kinds: Vec<_> = check(&ir, &census, TEAMS).into_iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            vec![
                ProblemKind::OutOfRange { label: 2, number: 3 },
                ProblemKind::MissingNumber { label: 2, number: 1 },
                ProblemKind::Unnumbered { label: 3 },
            ]
        );
    }

    #[test]
    fn per_team_reads_the_teams_in_play() {
        let ir = siege();
        let mut census = minimal(&ir, TEAMS);
        census
            .labels
            .insert("ctf_flag_spawn".into(), vec![member(Class::Flag, None, Some(0))]);
        assert_eq!(texts(&ir, &census), vec!["team 1 has no zone labelled \"ctf_flag_spawn\""]);
        // A one-team game asks only for team 0's.
        assert_eq!(check(&ir, &census, &[0]), vec![]);
        // A neutral member serves no team.
        census
            .labels
            .insert("ctf_flag_spawn".into(), vec![member(Class::Flag, None, None)]);
        assert_eq!(check(&ir, &census, &[0]).len(), 1);
    }

    #[test]
    fn a_tag_of_the_wrong_group() {
        let ir = siege();
        let path = ir.tags[0].path.clone();
        let mut census = minimal(&ir, TEAMS);
        census.tags.insert(path.clone(), ObjectType::Vehicle);
        let problems = check(&ir, &census, TEAMS);
        assert_eq!(
            problems,
            vec![Problem {
                kind: ProblemKind::TagGroup { tag: 0, found: ObjectType::Vehicle },
                text: format!("\"{path}\" on this map is a vehicle; Siege gives it as a weapon"),
            }]
        );
        census.tags.insert(path.clone(), ObjectType::Weapon);
        assert_eq!(check(&ir, &census, TEAMS), vec![]);
        // A tag of any group takes any object; a tag the map lacks is no
        // problem of the gate's.
        let mut any = ir.clone();
        any.tags[0].group = TagGroup::Any;
        census.tags.insert(path, ObjectType::Vehicle);
        assert_eq!(check(&any, &census, TEAMS), vec![]);
        assert_eq!(check(&ir, &minimal(&ir, TEAMS), TEAMS), vec![]);
    }

    #[test]
    fn a_requirement_reads_in_forges_words() {
        let ir = siege();
        let words: Vec<String> = ir.labels.iter().map(describe).collect();
        assert_eq!(
            words,
            vec![
                "exactly 1 zone labelled \"goal\"",
                "exactly 1 object labelled \"core\"",
                "zones labelled \"zone\", numbered 0 and 1",
                "any number of zones labelled \"hill_corner\", each with a number",
                "at most 16 zones labelled \"ctf_flag_spawn\", one for each team",
                "at most 1 object labelled \"bonus\"",
                "at least 1 starting location labelled \"attack\"",
                "any number of objects labelled \"barrier\"",
            ]
        );
    }

    #[test]
    fn a_census_reads_from_json() {
        let census: Census = serde_json::from_str(
            r#"{"labels":{"zone":[{"class":"volume","number":1},{"class":"flag","team":0}]},
                "tags":{"objects/weapons/x":"light_fixture"}}"#,
        )
        .unwrap();
        assert_eq!(census.labels["zone"][0], member(Class::Volume, Some(1), None));
        assert_eq!(census.labels["zone"][1], member(Class::Flag, None, Some(0)));
        assert_eq!(census.tags["objects/weapons/x"], ObjectType::LightFixture);
        let back: Census = serde_json::from_str(&serde_json::to_string(&census).unwrap()).unwrap();
        assert_eq!(back, census);
    }
}
