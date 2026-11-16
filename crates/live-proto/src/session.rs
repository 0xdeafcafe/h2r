//! Session advertisements: public discovery data, never the exchange key.
use crate::Xuid;
pub use content::MapId;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::Ipv4Addr};
pub const CREATED: &str = "SessionCreated";
pub const UPDATED: &str = "SessionUpdated";
pub const DELETED: &str = "SessionDeleted";
pub const EXPIRED: &str = "SessionExpired";
pub const JOINED: &str = "PlayerJoined";
pub const LEFT: &str = "PlayerLeft";
pub fn stream(id: &str) -> String {
    format!("session-{id}")
}
pub fn valid_id(id: &str) -> bool {
    id.len() == 16
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum AttributeValue {
    Integer(u64),
    String(String),
    Blob(Vec<u8>),
    Null,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slots {
    pub public_filled: u32,
    pub public_open: u32,
    pub private_filled: u32,
    pub private_open: u32,
}
impl Slots {
    pub fn validate(self) -> bool {
        [
            self.public_filled,
            self.public_open,
            self.private_filled,
            self.private_open,
        ]
        .into_iter()
        .map(u64::from)
        .sum::<u64>()
            <= 16
    }
}
/// Who may find a session in `GET /v1/sessions` (and so in the game's
/// custom games browser). Joining by id (invites, `GET /v1/sessions/{id}`)
/// is unaffected: the id is the capability. Absent means `public`, which is
/// how every session before this field behaved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Listed to everyone.
    #[default]
    Public,
    /// Listed only to the host's friends (and the host).
    Friends,
    /// Never listed; reached through invitations only.
    Invite,
}
impl Visibility {
    pub fn is_public(&self) -> bool {
        *self == Visibility::Public
    }
    /// Whether the session appears in lists at all (for someone).
    pub fn is_listed(self) -> bool {
        self != Visibility::Invite
    }
    /// Whether a list read by `viewer` (None: anonymous) shows a session
    /// hosted by `host`; `friends` answers "are these two friends".
    pub fn shows_to(
        self,
        host: Xuid,
        viewer: Option<Xuid>,
        friends: impl Fn(Xuid, Xuid) -> bool,
    ) -> bool {
        if viewer == Some(host) {
            return true;
        }
        match self {
            Visibility::Public => true,
            Visibility::Friends => viewer.is_some_and(|v| friends(host, v)),
            Visibility::Invite => false,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Friends => "friends",
            Visibility::Invite => "invite",
        }
    }
}
/// Whether an IPv4 address is reachable only inside one network: private
/// (RFC 1918), loopback, link-local or carrier-grade NAT shared space. A
/// session whose declared address is one of these cannot aim strangers'
/// discovery probes at someone else's internet host.
pub fn is_local_ipv4(a: Ipv4Addr) -> bool {
    let [x, y, ..] = a.octets();
    a.is_private() || a.is_loopback() || a.is_link_local() || (x == 100 && (64..128).contains(&y))
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    pub addr: Ipv4Addr,
    pub port: u16,
    pub source: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewSession {
    /// Optional declared address; None asks the server for observed client IPv4.
    pub addr: Option<Ipv4Addr>,
    pub port: u16,
    pub host_address: Vec<u8>,
    pub slots: Slots,
    pub attributes: BTreeMap<u32, AttributeValue>,
    #[serde(default = "custom")]
    pub flags: String,
    #[serde(default)]
    pub hopper_id: Option<u32>,
    #[serde(default)]
    pub map: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    /// Who finds this session in lists; omitted on the wire when public, so
    /// older servers and logs read the same bytes.
    #[serde(default, skip_serializing_if = "Visibility::is_public")]
    pub visibility: Visibility,
    /// Exactly which map (docs/game/maps.md §5): the base plus layers the host
    /// has loaded, beside `map`, its name. None for a host that does not
    /// say (every one before this field); omitted on the wire then, so
    /// older servers and logs read the same bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_id: Option<MapId>,
    /// The skulls the host has on for its next game (docs/game/skulls.md "In
    /// multiplayer"): engine indices, ascending, only
    /// [`crate::skulls::MULTIPLAYER`] ones. Guests read it to apply the
    /// same set at game start. Left out when none, and by every host before
    /// this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skulls: Option<Vec<u8>>,
    /// A co-op campaign session's level and difficulty; only with
    /// `flags: "coop"`. Left out otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub campaign: Option<Campaign>,
    /// The round trip in milliseconds between the host and its farthest
    /// guest, as the host last measured it (docs/live/live-coop.md, "Ping").
    /// Left out until there is a guest and a measurement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ping_ms: Option<u16>,
}
/// The highest round trip a session reports; anything slower is this.
pub const PING_MAX_MS: u16 = 9_999;
fn custom() -> String {
    "custom".into()
}
/// The session flag of a co-op campaign session (docs/live/live-coop.md,
/// "Co-op over Live").
pub const COOP: &str = "coop";
/// The campaign difficulties, by index.
pub const DIFFICULTIES: [&str; 4] = ["easy", "normal", "heroic", "legendary"];
/// What a co-op campaign session plays: the level and the difficulty the
/// host chose. Only a `coop` session carries one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Campaign {
    /// The level's map file stem, e.g. `03a_oldmombasa`.
    pub level: String,
    /// The level's name as the game shows it, when the host knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 0 easy, 1 normal, 2 heroic, 3 legendary ([`DIFFICULTIES`]).
    pub difficulty: u8,
}
impl Campaign {
    pub fn validate(&self) -> Result<(), String> {
        let stem_ok = !self.level.is_empty()
            && self.level.len() <= 64
            && self
                .level
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        let name_ok = self
            .name
            .as_ref()
            .is_none_or(|n| !n.is_empty() && n.chars().count() <= 64 && !n.contains('\0'));
        if !stem_ok || !name_ok || usize::from(self.difficulty) >= DIFFICULTIES.len() {
            return Err("invalid campaign level, name or difficulty".into());
        }
        Ok(())
    }
    pub fn difficulty_name(&self) -> &'static str {
        DIFFICULTIES[usize::from(self.difficulty).min(DIFFICULTIES.len() - 1)]
    }
    /// What presence and lists show: the name, else the map stem.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.level)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub host: String,
    pub host_xuid: Xuid,
    pub host_gamertag: String,
    pub xnkid: String,
    pub endpoint: Endpoint,
    pub advertisement: NewSession,
    pub since: String,
    pub last_activity: i64,
    pub state: String,
    #[serde(default)]
    pub players: BTreeMap<Xuid, String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub slots: Option<Slots>,
    pub attributes: Option<BTreeMap<u32, AttributeValue>>,
    pub name: Option<String>,
    pub map: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Visibility>,
    /// A new `map_id` (the host changed map). A session cannot go back to
    /// none: it is the same session on some map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_id: Option<MapId>,
    /// The host's skulls changed: the new set, empty for none. Absent keeps
    /// them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skulls: Option<Vec<u8>>,
    /// The session became (or stopped being) a co-op one: `coop` with a
    /// `campaign`, or `custom`, which drops the campaign.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flags: Option<String>,
    /// A co-op session's new level or difficulty (makes it `coop`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub campaign: Option<Campaign>,
    /// A new round trip ([`NewSession::ping_ms`]); 0 clears it (no guest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ping_ms: Option<u16>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Player {
    pub xuid: Xuid,
    #[serde(default)]
    pub slot: Option<String>,
}
pub fn validate_attributes(attrs: &BTreeMap<u32, AttributeValue>) -> Result<(), String> {
    if attrs.len() > 64 {
        return Err("at most 64 attributes".into());
    }
    for (id, value) in attrs {
        let valid = match (id & 0x00f00000, value) {
            (0, AttributeValue::Integer(_)) | (0x00f00000, AttributeValue::Null) => true,
            (0x00100000, AttributeValue::String(s)) => {
                s.encode_utf16().count() <= 400 && !s.contains('\0')
            }
            (0x00200000, AttributeValue::Blob(b)) => b.len() <= 800,
            _ => false,
        };
        if !valid {
            return Err("invalid attribute type or length".into());
        }
    }
    Ok(())
}
impl NewSession {
    pub fn is_coop(&self) -> bool {
        self.flags == COOP
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.port == 0
            || self.host_address.len() != 36
            || !self.slots.validate()
            || !matches!(self.flags.as_str(), "custom" | "hopper" | "ranked" | "coop")
        {
            return Err("invalid session address, slots or flags".into());
        }
        for s in [&self.name, &self.map, &self.language]
            .into_iter()
            .flatten()
        {
            if s.len() > 256 || s.contains('\0') {
                return Err("invalid session text".into());
            }
        }
        if let Some(skulls) = &self.skulls {
            crate::skulls::validate(skulls)?;
            // A co-op campaign plays every skull; the rest are multiplayer.
            if !self.is_coop() && !crate::skulls::multiplayer_ok(skulls) {
                return Err("a session names only skulls that work in multiplayer".into());
            }
        }
        if self.ping_ms.is_some_and(|p| p == 0 || p > PING_MAX_MS) {
            return Err(format!("ping_ms is 1 to {PING_MAX_MS}"));
        }
        if let Some(c) = &self.campaign {
            if !self.is_coop() {
                return Err("only a coop session names a campaign".into());
            }
            c.validate()?;
        }
        validate_attributes(&self.attributes)
    }
}
impl Endpoint {
    /// Whether a session at this endpoint may be listed: the address is the
    /// one the server saw the host connect from, or a local-network one.
    /// A declared internet address is only good for invitations.
    pub fn listable(&self) -> bool {
        self.source == "observed" || is_local_ipv4(self.addr)
    }
}
impl Session {
    /// Whether `GET /v1/sessions` read by `viewer` includes this session.
    pub fn listed_for(&self, viewer: Option<Xuid>, friends: impl Fn(Xuid, Xuid) -> bool) -> bool {
        self.endpoint.listable()
            && self
                .advertisement
                .visibility
                .shows_to(self.host_xuid, viewer, friends)
    }
    pub fn update(&mut self, u: &Update) -> Result<bool, String> {
        let mut next = self.advertisement.clone();
        if let Some(v) = u.slots {
            next.slots = v;
        }
        if let Some(v) = &u.attributes {
            next.attributes = v.clone();
        }
        if let Some(v) = &u.name {
            next.name = Some(v.clone());
        }
        if let Some(v) = &u.map {
            next.map = Some(v.clone());
        }
        if let Some(v) = u.visibility {
            next.visibility = v;
        }
        if let Some(v) = u.map_id {
            next.map_id = Some(v);
        }
        if let Some(v) = &u.flags {
            next.flags = v.clone();
            if !next.is_coop() {
                next.campaign = None;
            }
        }
        if let Some(c) = &u.campaign {
            next.flags = COOP.into();
            next.campaign = Some(c.clone());
        }
        if let Some(v) = &u.skulls {
            next.skulls = (!v.is_empty()).then(|| v.clone());
        }
        if let Some(v) = u.ping_ms {
            next.ping_ms = (v != 0).then_some(v);
        }
        next.validate()?;
        let changed = next != self.advertisement;
        self.advertisement = next;
        Ok(changed)
    }
    pub fn apply(
        &mut self,
        kind: &str,
        data: &serde_json::Value,
        _at: &str,
        seconds: i64,
    ) -> Result<bool, String> {
        if self.state != "open" {
            return Err("session is closed".into());
        }
        let changed = match kind {
            UPDATED => {
                self.update(&serde_json::from_value(data.clone()).map_err(|_| "invalid update")?)?
            }
            DELETED | EXPIRED => {
                if kind == DELETED {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct End {
                        reason: String,
                    }
                    let end: End =
                        serde_json::from_value(data.clone()).map_err(|_| "invalid delete")?;
                    if !matches!(end.reason.as_str(), "closed" | "host_left") {
                        return Err("invalid delete reason".into());
                    }
                }
                self.state = if kind == EXPIRED {
                    "expired"
                } else {
                    "deleted"
                }
                .into();
                true
            }
            JOINED => {
                let p: Player =
                    serde_json::from_value(data.clone()).map_err(|_| "invalid joined player")?;
                if self.players.contains_key(&p.xuid) || self.players.len() >= 16 {
                    return Err("duplicate/full player roster".into());
                }
                self.players.insert(p.xuid, p.slot.unwrap_or_default());
                true
            }
            LEFT => {
                let p: Player =
                    serde_json::from_value(data.clone()).map_err(|_| "invalid left player")?;
                if self.players.remove(&p.xuid).is_none() {
                    return Err("player is not in session".into());
                }
                true
            }
            _ => return Err("unknown session event".into()),
        };
        if changed {
            self.last_activity = self.last_activity.max(seconds);
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn new() -> NewSession {
        NewSession {
            addr: None,
            port: 3074,
            host_address: vec![0; 36],
            slots: Slots {
                public_filled: 1,
                public_open: 7,
                ..Default::default()
            },
            attributes: BTreeMap::new(),
            flags: "custom".into(),
            hopper_id: None,
            map: None,
            name: None,
            language: None,
            visibility: Visibility::Public,
            map_id: None,
            skulls: None,
            campaign: None,
            ping_ms: None,
        }
    }
    #[test]
    fn attributes_and_slots_are_typed_and_bounded() {
        let mut n = new();
        assert!(n.validate().is_ok());
        n.slots.public_open = u32::MAX;
        assert!(n.validate().is_err());
        n = new();
        n.attributes
            .insert(1, AttributeValue::String("bad type".into()));
        assert!(n.validate().is_err());
        n.attributes.clear();
        n.attributes
            .insert(0x100001, AttributeValue::String("é".repeat(400)));
        assert!(n.validate().is_ok());
        n.attributes
            .insert(0x100001, AttributeValue::String("é".repeat(401)));
        assert!(n.validate().is_err());
        n.attributes.clear();
        n.attributes
            .insert(0x200001, AttributeValue::Blob(vec![0; 800]));
        assert!(n.validate().is_ok());
        n.attributes
            .insert(0x200001, AttributeValue::Blob(vec![0; 801]));
        assert!(n.validate().is_err());
        assert!(valid_id("80abcdef01234567"));
        assert!(!valid_id("80ABCDEF01234567"));
        assert!(!valid_id("short"));
        assert!(
            serde_json::from_value::<NewSession>(
                json!({"exchange_key":"must not cross public API"})
            )
            .is_err()
        );
    }
    #[test]
    fn updates_are_atomic_noops_and_activity_never_moves_backwards() {
        let mut s = Session {
            session_id: "8000000000000001".into(),
            host: "synthetic".into(),
            host_xuid: Xuid(1),
            host_gamertag: "Host".into(),
            xnkid: "8000000000000001".into(),
            endpoint: Endpoint {
                addr: "127.0.0.1".parse().unwrap(),
                port: 3074,
                source: "declared".into(),
            },
            advertisement: new(),
            since: "synthetic".into(),
            last_activity: 100,
            state: "open".into(),
            players: BTreeMap::new(),
        };
        let before = s.clone();
        assert!(!s.apply(UPDATED, &json!({}), "", 101).unwrap());
        assert_eq!(s, before);
        assert!(s.apply(UPDATED,&json!({"slots":{"public_filled":1,"public_open":100,"private_filled":0,"private_open":0}}),"",102).is_err());
        assert_eq!(s, before);
        assert!(s.apply(UPDATED, &json!({"name":"New"}), "", 99).unwrap());
        assert_eq!(s.last_activity, 100);
        assert!(s.apply(JOINED, &json!({"xuid":Xuid(2)}), "", 101).unwrap());
        assert!(s.apply(JOINED, &json!({"xuid":Xuid(2)}), "", 101).is_err());
        assert!(s.apply(LEFT, &json!({"xuid":Xuid(2)}), "", 102).unwrap());
        assert!(
            s.apply(DELETED, &json!({"reason":"closed"}), "", 103)
                .unwrap()
        );
        assert!(s.apply(UPDATED, &json!({}), "", 104).is_err());
    }
    fn session(source: &str, addr: &str, v: Visibility) -> Session {
        Session {
            session_id: "8000000000000001".into(),
            host: "synthetic".into(),
            host_xuid: Xuid(1),
            host_gamertag: "Host".into(),
            xnkid: "8000000000000001".into(),
            endpoint: Endpoint {
                addr: addr.parse().unwrap(),
                port: 3074,
                source: source.into(),
            },
            advertisement: NewSession {
                visibility: v,
                ..new()
            },
            since: "synthetic".into(),
            last_activity: 100,
            state: "open".into(),
            players: BTreeMap::new(),
        }
    }
    #[test]
    fn visibility_defaults_to_public_and_public_is_not_written() {
        let old = json!({"addr":null,"port":3074,"host_address":vec![0u8;36],
            "slots":{"public_filled":1,"public_open":7,"private_filled":0,"private_open":0},
            "attributes":{}});
        let n: NewSession = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(n.visibility, Visibility::Public);
        assert!(
            serde_json::to_value(&n)
                .unwrap()
                .get("visibility")
                .is_none()
        );
        let mut f = old;
        f["visibility"] = json!("friends");
        let n: NewSession = serde_json::from_value(f).unwrap();
        assert_eq!(n.visibility, Visibility::Friends);
        assert_eq!(serde_json::to_value(&n).unwrap()["visibility"], "friends");
        assert!(serde_json::from_value::<Visibility>(json!("secret")).is_err());
        assert!(
            serde_json::to_value(Update::default())
                .unwrap()
                .get("visibility")
                .is_none()
        );
    }
    #[test]
    fn map_id_is_optional_and_left_out_when_none() {
        let old = json!({"addr":null,"port":3074,"host_address":vec![0u8;36],
            "slots":{"public_filled":1,"public_open":7,"private_filled":0,"private_open":0},
            "attributes":{},"map":"lockout"});
        let n: NewSession = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(n.map_id, None);
        let back = serde_json::to_value(&n).unwrap();
        assert!(back.get("map_id").is_none(), "{back}");
        let id = MapId([0xab; 32]);
        let mut with = old;
        with["map_id"] = json!(id.to_string());
        let n: NewSession = serde_json::from_value(with).unwrap();
        assert_eq!(n.map_id, Some(id));
        assert_eq!(serde_json::to_value(&n).unwrap()["map_id"], json!("ab".repeat(32)));
        let mut bad = serde_json::to_value(&n).unwrap();
        bad["map_id"] = json!("not hex");
        assert!(serde_json::from_value::<NewSession>(bad).is_err());
        // Updates: absent keeps the id, present replaces it.
        assert!(serde_json::to_value(Update::default()).unwrap().get("map_id").is_none());
        let mut s = session("observed", "203.0.113.5", Visibility::Public);
        let other = MapId([0xcd; 32]);
        assert!(s.apply(UPDATED, &json!({"map_id": other.to_string()}), "", 102).unwrap());
        assert_eq!(s.advertisement.map_id, Some(other));
        assert!(!s.apply(UPDATED, &json!({}), "", 103).unwrap());
        assert_eq!(s.advertisement.map_id, Some(other));
    }
    #[test]
    fn skulls_are_optional_checked_and_updated() {
        let old = json!({"addr":null,"port":3074,"host_address":vec![0u8;36],
            "slots":{"public_filled":1,"public_open":7,"private_filled":0,"private_open":0},
            "attributes":{},"map":"lockout"});
        // An advertisement from before the field parses, and none is left out.
        let n: NewSession = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(n.skulls, None);
        assert!(serde_json::to_value(&n).unwrap().get("skulls").is_none());
        let mut with = old;
        with["skulls"] = json!([4, 6]);
        let n: NewSession = serde_json::from_value(with.clone()).unwrap();
        assert_eq!(n.skulls, Some(vec![4, 6]));
        assert_eq!(n.validate(), Ok(()));
        for bad in [json!([]), json!([6, 4]), json!([10]), json!([15])] {
            with["skulls"] = bad.clone();
            let n: NewSession = serde_json::from_value(with.clone()).unwrap();
            assert!(n.validate().is_err(), "{bad}");
        }
        // Updates: absent keeps them, a set replaces them, empty clears.
        assert!(
            serde_json::to_value(Update::default())
                .unwrap()
                .get("skulls")
                .is_none()
        );
        let mut s = session("observed", "203.0.113.5", Visibility::Public);
        assert!(s.apply(UPDATED, &json!({"skulls": [6]}), "", 102).unwrap());
        assert_eq!(s.advertisement.skulls, Some(vec![6]));
        assert!(!s.apply(UPDATED, &json!({}), "", 103).unwrap());
        assert_eq!(s.advertisement.skulls, Some(vec![6]));
        assert!(s.apply(UPDATED, &json!({"skulls": []}), "", 104).unwrap());
        assert_eq!(s.advertisement.skulls, None);
        assert!(
            s.apply(UPDATED, &json!({"skulls": [8]}), "", 105).is_err(),
            "Black Eye does nothing in multiplayer"
        );
    }
    #[test]
    fn a_coop_session_names_its_campaign_and_may_carry_any_skull() {
        let campaign = Campaign {
            level: "03a_oldmombasa".into(),
            name: Some("Outskirts".into()),
            difficulty: 2,
        };
        let mut n = NewSession {
            flags: COOP.into(),
            campaign: Some(campaign.clone()),
            skulls: Some(vec![0, 4]),
            ..new()
        };
        n.validate().unwrap();
        assert_eq!(campaign.difficulty_name(), "heroic");
        assert_eq!(campaign.label(), "Outskirts");
        let wire = serde_json::to_value(&n).unwrap();
        assert_eq!(
            wire["campaign"],
            json!({"level": "03a_oldmombasa", "name": "Outskirts", "difficulty": 2})
        );
        // Only a coop session names a campaign; a custom one keeps the
        // multiplayer skull rule and leaves the field off the wire.
        n.flags = "custom".into();
        assert!(n.validate().is_err());
        n.campaign = None;
        assert!(n.validate().is_err(), "skull 0 is campaign-only");
        n.skulls = None;
        n.validate().unwrap();
        assert!(serde_json::to_value(&n).unwrap().get("campaign").is_none());
        for bad in [
            Campaign {
                level: "../x".into(),
                ..campaign.clone()
            },
            Campaign {
                level: String::new(),
                ..campaign.clone()
            },
            Campaign {
                difficulty: 4,
                ..campaign.clone()
            },
            Campaign {
                name: Some("x".repeat(65)),
                ..campaign.clone()
            },
        ] {
            let n = NewSession {
                flags: COOP.into(),
                campaign: Some(bad.clone()),
                ..new()
            };
            assert!(n.validate().is_err(), "{bad:?}");
        }
    }
    #[test]
    fn an_update_turns_a_lobby_into_coop_and_back() {
        let mut s = session("observed", "203.0.113.9", Visibility::Friends);
        let c = Campaign {
            level: "01a_tutorial".into(),
            name: None,
            difficulty: 1,
        };
        let u = Update {
            campaign: Some(c.clone()),
            ..Default::default()
        };
        assert!(s.update(&u).unwrap());
        assert!(s.advertisement.is_coop());
        assert_eq!(s.advertisement.campaign, Some(c));
        assert!(!s.update(&u).unwrap(), "same again: no change");
        assert!(
            s.apply(UPDATED, &json!({"flags": "custom"}), "", 200)
                .unwrap()
        );
        assert_eq!(s.advertisement.flags, "custom");
        assert_eq!(s.advertisement.campaign, None);
        // A custom session cannot be handed a campaign without becoming coop,
        // and an unknown flag is refused.
        assert!(
            s.apply(UPDATED, &json!({"flags": "nonsense"}), "", 201)
                .is_err()
        );
    }
    #[test]
    fn the_host_reports_its_round_trip_and_clears_it_with_zero() {
        let mut s = session("observed", "203.0.113.9", Visibility::Friends);
        assert!(s.apply(UPDATED, &json!({"ping_ms": 42}), "", 1).unwrap());
        assert_eq!(s.advertisement.ping_ms, Some(42));
        let wire = serde_json::to_value(&s.advertisement).unwrap();
        assert_eq!(wire["ping_ms"], 42);
        assert!(s.apply(UPDATED, &json!({"ping_ms": 0}), "", 2).unwrap());
        assert_eq!(s.advertisement.ping_ms, None);
        let wire = serde_json::to_value(&s.advertisement).unwrap();
        assert!(wire.get("ping_ms").is_none(), "left out when none");
        assert!(
            s.apply(UPDATED, &json!({"ping_ms": PING_MAX_MS + 1}), "", 3)
                .is_err()
        );
    }
    #[test]
    fn lists_show_public_to_all_friends_to_friends_invite_to_host_only() {
        let friends = |a: Xuid, b: Xuid| (a, b) == (Xuid(1), Xuid(2));
        let p = session("observed", "203.0.113.5", Visibility::Public);
        let f = session("observed", "203.0.113.5", Visibility::Friends);
        let i = session("observed", "203.0.113.5", Visibility::Invite);
        for viewer in [None, Some(Xuid(2)), Some(Xuid(3)), Some(Xuid(1))] {
            assert!(p.listed_for(viewer, friends));
        }
        assert!(!f.listed_for(None, friends));
        assert!(f.listed_for(Some(Xuid(2)), friends));
        assert!(!f.listed_for(Some(Xuid(3)), friends));
        assert!(f.listed_for(Some(Xuid(1)), friends));
        assert!(!i.listed_for(None, friends));
        assert!(!i.listed_for(Some(Xuid(2)), friends));
        assert!(i.listed_for(Some(Xuid(1)), friends));
    }
    #[test]
    fn declared_internet_addresses_are_never_listed() {
        let none = |_: Xuid, _: Xuid| false;
        assert!(!session("declared", "198.51.100.7", Visibility::Public).listed_for(None, none));
        for local in [
            "192.168.1.50",
            "10.0.0.2",
            "172.16.4.4",
            "127.0.0.1",
            "169.254.1.1",
            "100.64.0.9",
        ] {
            assert!(
                session("declared", local, Visibility::Public).listed_for(None, none),
                "{local}"
            );
        }
        assert!(!is_local_ipv4("100.128.0.1".parse().unwrap()));
        assert!(session("observed", "198.51.100.7", Visibility::Public).listed_for(None, none));
        // Switching such a session to public changes the field, not the gate.
        let mut s = session("declared", "198.51.100.7", Visibility::Invite);
        assert!(
            s.apply(UPDATED, &json!({"visibility":"public"}), "", 101)
                .unwrap()
        );
        assert!(!s.listed_for(None, none));
        let mut s = session("observed", "198.51.100.7", Visibility::Invite);
        assert!(
            s.apply(UPDATED, &json!({"visibility":"friends"}), "", 101)
                .unwrap()
        );
        assert_eq!(s.advertisement.visibility, Visibility::Friends);
    }
}
