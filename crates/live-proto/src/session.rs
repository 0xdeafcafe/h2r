//! Session advertisements: public discovery data, never the exchange key.
use crate::Xuid;
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
}
fn custom() -> String {
    "custom".into()
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
    pub fn validate(&self) -> Result<(), String> {
        if self.port == 0
            || self.host_address.len() != 36
            || !self.slots.validate()
            || !matches!(self.flags.as_str(), "custom" | "hopper" | "ranked")
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
        validate_attributes(&self.attributes)
    }
}
impl Session {
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
}
