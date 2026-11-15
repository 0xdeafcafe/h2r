//! Owned team model and deterministic rules shared by the local HLE and event
//! projection. Guest layouts and HTTP/signing live in their respective crates.
use crate::Xuid;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DELETE: u32 = 1;
pub const MODIFY_DATA: u32 = 2;
pub const MODIFY_MEMBERS: u32 = 4;
pub const REMOVE_MEMBER: u32 = 8;
pub const RECRUIT: u32 = 16;
pub const LEADER: u32 = u32::MAX;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub xuid: Xuid,
    pub gamertag: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Properties {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub motto: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub data: Vec<u8>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberProperties {
    pub privileges: u32,
    #[serde(default)]
    pub data: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub identity: Identity,
    pub properties: MemberProperties,
    pub joined_at: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    pub id: Xuid,
    pub creator: Xuid,
    pub properties: Properties,
    pub max_members: u32,
    pub created_at: String,
    pub members: BTreeMap<Xuid, Member>,
    pub recruits: BTreeMap<Xuid, Member>,
    #[serde(default)]
    pub deleted: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Action {
    Create {
        id: Xuid,
        properties: Properties,
        member: MemberProperties,
        max_members: u32,
    },
    Properties {
        id: Xuid,
        properties: Properties,
    },
    Delete {
        id: Xuid,
    },
    Recruit {
        id: Xuid,
        target: Identity,
        properties: MemberProperties,
    },
    Answer {
        id: Xuid,
        accept: bool,
    },
    MemberProperties {
        id: Xuid,
        user: Xuid,
        properties: MemberProperties,
    },
    Remove {
        id: Xuid,
        user: Xuid,
    },
}
impl Action {
    pub fn id(&self) -> Xuid {
        match self {
            Self::Create { id, .. }
            | Self::Properties { id, .. }
            | Self::Delete { id }
            | Self::Recruit { id, .. }
            | Self::Answer { id, .. }
            | Self::MemberProperties { id, .. }
            | Self::Remove { id, .. } => *id,
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Create { .. } => "TeamCreated",
            Self::Properties { .. } => "TeamPropertiesChanged",
            Self::Delete { .. } => "TeamDeleted",
            Self::Recruit { .. } => "TeamMemberRecruited",
            Self::Answer { accept: true, .. } => "TeamRecruitAccepted",
            Self::Answer { accept: false, .. } => "TeamRecruitDeclined",
            Self::MemberProperties { .. } => "TeamMemberPropertiesChanged",
            Self::Remove { .. } => "TeamMemberRemoved",
        }
    }
    pub fn is_kind(kind: &str) -> bool {
        matches!(
            kind,
            "TeamCreated"
                | "TeamPropertiesChanged"
                | "TeamDeleted"
                | "TeamMemberRecruited"
                | "TeamRecruitAccepted"
                | "TeamRecruitDeclined"
                | "TeamMemberPropertiesChanged"
                | "TeamMemberRemoved"
        )
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TeamError {
    Invalid,
    NotFound,
    UserNotFound,
    AlreadyExists,
    AlreadyMember,
    Pending,
    NotMember,
    Privileges,
    NoAdmin,
    Full,
    UserTeamsFull,
}
impl TeamError {
    pub fn hresult(self) -> u32 {
        match self {
            Self::Invalid => 0x80070057,
            Self::NotFound => 0x8015210b,
            Self::UserNotFound => 0x80152105,
            Self::AlreadyExists => 0x8015210a,
            Self::AlreadyMember => 0x80152104,
            Self::Pending => 0x80152102,
            Self::NotMember => 0x80152111,
            Self::Privileges => 0x8015210c,
            Self::NoAdmin => 0x80152112,
            Self::Full => 0x80152101,
            Self::UserTeamsFull => 0x80152106,
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Store {
    pub teams: BTreeMap<Xuid, Team>,
}
type Result<T> = std::result::Result<T, TeamError>;
impl Store {
    pub fn of(&self, user: Xuid) -> Vec<Team> {
        self.teams
            .values()
            .filter(|t| {
                !t.deleted && (t.members.contains_key(&user) || t.recruits.contains_key(&user))
            })
            .cloned()
            .collect()
    }
    pub fn get(&self, id: Xuid) -> Result<&Team> {
        self.teams
            .get(&id)
            .filter(|t| !t.deleted)
            .ok_or(TeamError::NotFound)
    }
    pub fn apply(&mut self, actor: &Identity, action: &Action, at: &str) -> Result<()> {
        use TeamError::*;
        let id = action.id();
        match action {
            Action::Create {
                properties,
                member,
                max_members,
                ..
            } => {
                validate_properties(properties)?;
                validate_member(member)?;
                if id.0 == 0 || *max_members == 0 || *max_members > 100 {
                    return Err(Invalid);
                }
                if self.teams.contains_key(&id) || self.name_taken(&properties.name, None) {
                    return Err(AlreadyExists);
                }
                if self.of(actor.xuid).len() >= 8 {
                    return Err(UserTeamsFull);
                }
                let mut props = member.clone();
                props.privileges = LEADER;
                let creator = Member {
                    identity: actor.clone(),
                    properties: props,
                    joined_at: at.into(),
                };
                self.teams.insert(
                    id,
                    Team {
                        id,
                        creator: actor.xuid,
                        properties: properties.clone(),
                        max_members: *max_members,
                        created_at: at.into(),
                        members: BTreeMap::from([(actor.xuid, creator)]),
                        recruits: BTreeMap::new(),
                        deleted: false,
                    },
                );
                return Ok(());
            }
            Action::Properties { properties, .. } => {
                validate_properties(properties)?;
                if self.name_taken(&properties.name, Some(id)) {
                    return Err(AlreadyExists);
                }
            }
            Action::Recruit { properties, .. } | Action::MemberProperties { properties, .. } => {
                validate_member(properties)?
            }
            _ => {}
        }
        if let Action::Recruit { target, .. } = action
            && self.of(target.xuid).len() >= 8
        {
            return Err(UserTeamsFull);
        }
        let can_join = matches!(action, Action::Answer { accept: true, .. });
        if can_join
            && self
                .of(actor.xuid)
                .iter()
                .filter(|t| t.members.contains_key(&actor.xuid))
                .count()
                >= 8
        {
            return Err(UserTeamsFull);
        }
        let team = self
            .teams
            .get_mut(&id)
            .filter(|t| !t.deleted)
            .ok_or(NotFound)?;
        match action {
            Action::Create { .. } => unreachable!(),
            Action::Properties { properties, .. } => {
                allowed(team, actor.xuid, MODIFY_DATA)?;
                team.properties = properties.clone();
            }
            Action::Delete { .. } => {
                allowed(team, actor.xuid, DELETE)?;
                if team.creator != actor.xuid {
                    return Err(Privileges);
                }
                team.deleted = true;
                team.recruits.clear();
            }
            Action::Recruit {
                target, properties, ..
            } => {
                let rights = allowed(team, actor.xuid, RECRUIT)?;
                if target.xuid.0 == 0 || target.gamertag.is_empty() {
                    return Err(UserNotFound);
                }
                if team.members.contains_key(&target.xuid) {
                    return Err(AlreadyMember);
                }
                if team.recruits.contains_key(&target.xuid) {
                    return Err(Pending);
                }
                if team.members.len() + team.recruits.len() >= team.max_members as usize {
                    return Err(Full);
                }
                if properties.privileges == LEADER || properties.privileges & !rights != 0 {
                    return Err(Privileges);
                }
                team.recruits.insert(
                    target.xuid,
                    Member {
                        identity: target.clone(),
                        properties: properties.clone(),
                        joined_at: at.into(),
                    },
                );
            }
            Action::Answer { accept, .. } => {
                let Some(recruit) = team.recruits.get(&actor.xuid) else {
                    return Err(UserNotFound);
                };
                if *accept {
                    if team.members.len() >= team.max_members as usize {
                        return Err(Full);
                    }
                    let mut member = recruit.clone();
                    member.joined_at = at.into();
                    team.members.insert(actor.xuid, member);
                }
                team.recruits.remove(&actor.xuid);
            }
            Action::MemberProperties {
                user, properties, ..
            } => {
                let rights = allowed(team, actor.xuid, MODIFY_MEMBERS)?;
                let target = team.members.get(user).ok_or(UserNotFound)?;
                if *user == team.creator && properties.privileges != LEADER {
                    return Err(NoAdmin);
                }
                if *user != team.creator
                    && (properties.privileges == LEADER || properties.privileges & !rights != 0)
                {
                    return Err(Privileges);
                }
                if actor.xuid != team.creator && target.properties.privileges & !rights != 0 {
                    return Err(Privileges);
                }
                team.members.get_mut(user).unwrap().properties = properties.clone();
            }
            Action::Remove { user, .. } => {
                if *user != actor.xuid {
                    allowed(team, actor.xuid, REMOVE_MEMBER)?;
                }
                if *user == team.creator {
                    return Err(NoAdmin);
                }
                if team.members.remove(user).is_none() && team.recruits.remove(user).is_none() {
                    return Err(UserNotFound);
                }
            }
        }
        Ok(())
    }
    fn name_taken(&self, name: &str, except: Option<Xuid>) -> bool {
        self.teams.values().any(|t| {
            !t.deleted
                && Some(t.id) != except
                && t.properties.name.to_lowercase() == name.to_lowercase()
        })
    }
}
fn allowed(team: &Team, actor: Xuid, required: u32) -> Result<u32> {
    let member = team.members.get(&actor).ok_or(TeamError::NotMember)?;
    let rights = member.properties.privileges;
    if rights & required != required {
        return Err(TeamError::Privileges);
    }
    Ok(rights)
}
pub fn validate_properties(p: &Properties) -> Result<()> {
    let bounded = |s: &str, n: usize| !s.contains('\0') && s.encode_utf16().count() < n;
    if p.name.trim().is_empty()
        || !bounded(&p.name, 16)
        || !bounded(&p.description, 256)
        || !bounded(&p.motto, 256)
        || !bounded(&p.url, 256)
        || p.data.len() > 100
    {
        Err(TeamError::Invalid)
    } else {
        Ok(())
    }
}
pub fn validate_member(p: &MemberProperties) -> Result<()> {
    if p.data.len() > 100 || (p.privileges != LEADER && p.privileges & !0x1f != 0) {
        Err(TeamError::Invalid)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn user(n: u64) -> Identity {
        Identity {
            xuid: Xuid(n),
            gamertag: format!("Player{n}"),
        }
    }
    fn create(id: u64, name: &str) -> Action {
        Action::Create {
            id: Xuid(id),
            properties: Properties {
                name: name.into(),
                ..Default::default()
            },
            member: MemberProperties::default(),
            max_members: 100,
        }
    }
    #[test]
    fn permissions_pending_members_and_the_unique_leader_survive_serialization() {
        let a = user(1);
        let b = user(2);
        let mut store = Store::default();
        store
            .apply(&a, &create(10, "Clan"), "2026-01-01T00:00:00Z")
            .unwrap();
        store
            .apply(
                &a,
                &Action::Recruit {
                    id: Xuid(10),
                    target: b.clone(),
                    properties: MemberProperties {
                        privileges: 0x10,
                        data: vec![1, 2],
                    },
                },
                "2026-01-01T00:00:01Z",
            )
            .unwrap();
        assert!(store.of(b.xuid)[0].recruits.contains_key(&b.xuid));
        store
            .apply(
                &b,
                &Action::Answer {
                    id: Xuid(10),
                    accept: true,
                },
                "2026-01-01T00:00:02Z",
            )
            .unwrap();
        let before = store.clone();
        for (actor, action, error) in [
            (&b, Action::Delete { id: Xuid(10) }, TeamError::Privileges),
            (
                &a,
                Action::MemberProperties {
                    id: Xuid(10),
                    user: b.xuid,
                    properties: MemberProperties {
                        privileges: LEADER,
                        data: vec![],
                    },
                },
                TeamError::Privileges,
            ),
            (
                &a,
                Action::Remove {
                    id: Xuid(10),
                    user: a.xuid,
                },
                TeamError::NoAdmin,
            ),
        ] {
            assert_eq!(
                store.apply(actor, &action, "2026-01-01T00:00:03Z"),
                Err(error)
            );
            assert_eq!(store, before);
        }
        let wire = serde_json::to_vec(&store).unwrap();
        assert_eq!(serde_json::from_slice::<Store>(&wire).unwrap(), store);
    }
    #[test]
    fn memberships_and_pending_recruits_share_the_eight_team_limit() {
        let a = user(1);
        let b = user(2);
        let mut store = Store::default();
        for i in 10..18 {
            store
                .apply(&a, &create(i, &format!("Clan{i}")), "2026-01-01T00:00:00Z")
                .unwrap();
        }
        assert_eq!(
            store.apply(&a, &create(19, "Ninth"), "2026-01-01T00:00:00Z"),
            Err(TeamError::UserTeamsFull)
        );
        assert_eq!(
            store.apply(&b, &create(20, "CLAN10"), "2026-01-01T00:00:00Z"),
            Err(TeamError::AlreadyExists)
        );
        store
            .apply(&b, &create(20, "Other"), "2026-01-01T00:00:00Z")
            .unwrap();
        assert_eq!(
            store.apply(
                &b,
                &Action::Recruit {
                    id: Xuid(20),
                    target: a,
                    properties: MemberProperties::default()
                },
                "2026-01-01T00:00:00Z"
            ),
            Err(TeamError::UserTeamsFull)
        );
    }
}
