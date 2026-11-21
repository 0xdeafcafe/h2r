//! A machine's monitor looks as the agent API sees them: the session's
//! look, the saved one, the catalogue, what each seat is drawn as, and the
//! change of a look (docs/game/forge-monitor-appearance.md, "Interfaces").
//!
//! Offline there is one seat, the local player's. The seat table the host
//! owns in a network session (slice ML7) is a second user of the same
//! rules, [`apply`] and [`drawn_as`].
use serde_json::{Map, Value, json};
use settings::Store;

use super::census::Entry;
use super::saved::{self, Saved};
use super::{BODIES_DRAWN, MonitorLook, Patch, Refusal, apply, locks};

/// Who is looking: the signed-in profile's name and its palette colour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Who {
    pub profile: String,
    /// The seat's colour: a palette index.
    pub colour: u8,
}

/// What a machine's other players would see for a look.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrawnAs {
    /// `model`, `marker` or `hidden`.
    pub kind: &'static str,
    /// Why it is not what the look asks for.
    pub reason: Option<String>,
}

impl DrawnAs {
    fn new(kind: &'static str, reason: Option<&str>) -> DrawnAs {
        DrawnAs {
            kind,
            reason: reason.map(str::to_owned),
        }
    }

    pub fn to_value(&self) -> Value {
        match &self.reason {
            Some(r) => json!({"as": self.kind, "reason": r}),
            None => json!({"as": self.kind}),
        }
    }
}

/// What another machine that holds the same models draws for `look`:
/// the marker always; a disc model only where the renderer draws bodies
/// and the catalogue has it. Anything else is the marker with its reason.
pub fn drawn_as(look: &MonitorLook, catalogue: &[Entry], bodies: bool) -> DrawnAs {
    if look.is_marker() {
        DrawnAs::new("marker", None)
    } else if !catalogue.iter().any(|e| e.key == look.model) {
        DrawnAs::new(
            "marker",
            Some("This machine's catalogue does not have the model"),
        )
    } else if !bodies {
        DrawnAs::new(
            "marker",
            Some("This build does not draw model bodies, so the marker is shown"),
        )
    } else {
        DrawnAs::new("model", None)
    }
}

/// The monitor looks of one machine: the catalogue it can draw and, while
/// a Forge session is open, the local seat's look.
#[derive(Clone, Debug)]
pub struct Looks {
    catalogue: Vec<Entry>,
    /// Why the catalogue is empty, when it is.
    note: Option<String>,
    bodies: bool,
    session: Option<MonitorLook>,
    notice: Option<String>,
}

impl Looks {
    pub fn new(catalogue: Vec<Entry>, note: Option<String>) -> Looks {
        Looks {
            catalogue,
            note,
            bodies: BODIES_DRAWN,
            session: None,
            notice: None,
        }
    }

    /// As [`Looks::new`], for a renderer that does (or does not) draw
    /// bodies, whatever this build says.
    pub fn with_bodies(mut self, bodies: bool) -> Looks {
        self.bodies = bodies;
        self
    }

    pub fn catalogue(&self) -> &[Entry] {
        &self.catalogue
    }

    /// The local seat's look while a session is open.
    pub fn session(&self) -> Option<&MonitorLook> {
        self.session.as_ref()
    }

    /// Starts a session: the profile's saved look as this machine can use
    /// it. Returns the notice to show once when it is not the saved look
    /// (a model this install lacks; the file is left alone).
    pub fn open(&mut self, store: Option<&Store>, who: &Who) -> Option<String> {
        let saved = self.saved(store, who).0;
        let resolved = saved::resolve(&saved, &self.catalogue);
        self.session = Some(resolved.look);
        self.notice = resolved.notice.clone();
        resolved.notice
    }

    /// Ends the session.
    pub fn close(&mut self) {
        self.session = None;
        self.notice = None;
    }

    fn saved(&self, store: Option<&Store>, who: &Who) -> (Saved, Vec<String>) {
        match store {
            Some(s) => {
                let l = saved::load(s, &who.profile, who.colour);
                (l.saved, l.problems)
            }
            None => (Saved::marker(who.colour), Vec::new()),
        }
    }

    /// `forge.look`: the seat's look, how it is drawn, the saved look, the
    /// catalogue and the locks. With no session the look is the saved one
    /// as this machine can use it.
    pub fn report(
        &self,
        store: Option<&Store>,
        who: &Who,
        seat: Option<u64>,
    ) -> Result<Value, Refusal> {
        if seat.is_some_and(|s| s != 0) {
            return Err(Refusal::invalid(
                "seat",
                "is not a seat of this session: offline there is seat 0",
            ));
        }
        let (saved, problems) = self.saved(store, who);
        let resolved = saved::resolve(&saved, &self.catalogue);
        let look = self.session.clone().unwrap_or(resolved.look);
        let notice = if self.session.is_some() {
            self.notice.clone()
        } else {
            resolved.notice
        };
        let mut out = Map::new();
        out.insert("session".into(), json!(self.session.is_some()));
        out.insert("look".into(), look.to_value());
        out.insert(
            "drawn_as".into(),
            drawn_as(&look, &self.catalogue, self.bodies).to_value(),
        );
        out.insert(
            "saved".into(),
            json!({"look": saved.look.to_value(), "source": saved.source}),
        );
        out.insert("catalogue".into(), self.catalogue_value());
        out.insert("locks".into(), json!(locks(&look.model, &self.catalogue)));
        if let Some(n) = notice {
            out.insert("notice".into(), json!(n));
        }
        if let Some(n) = &self.note {
            out.insert("catalogue_note".into(), json!(n));
        }
        if !problems.is_empty() {
            out.insert("problems".into(), json!(problems));
        }
        Ok(Value::Object(out))
    }

    /// The catalogue as the agent API lists it: each entry, whether this
    /// machine draws it, and the refusal when it does not.
    pub fn catalogue_value(&self) -> Value {
        Value::Array(
            self.catalogue
                .iter()
                .map(|e| {
                    let mut v = e.to_value();
                    v["available"] = json!(self.bodies);
                    if !self.bodies {
                        v["refusal"] = Refusal::Renderer {
                            machine: "this machine".into(),
                        }
                        .to_value();
                    }
                    v
                })
                .collect(),
        )
    }

    /// `forge.look.set`: `{reset?, save?, ...fields}`. `reset` is the
    /// default look and takes no field. A change that is accepted is the
    /// session's look; with `save` it is also written to the profile.
    /// Refused with [`Refusal::NoSession`] outside a session, and otherwise
    /// with the reason, leaving the look as it was.
    pub fn set(
        &mut self,
        store: Option<&Store>,
        who: &Who,
        params: &Value,
    ) -> Result<Value, Refusal> {
        let current = self.session.clone().ok_or(Refusal::NoSession)?;
        let mut fields = match params {
            Value::Object(o) => o.clone(),
            Value::Null => Map::new(),
            _ => return Err(Refusal::invalid("look", "must be an object")),
        };
        let mut flag = |name: &str| match fields.remove(name) {
            None => Ok(false),
            Some(Value::Bool(b)) => Ok(b),
            Some(_) => Err(Refusal::invalid(name, "must be true or false")),
        };
        let reset = flag("reset")?;
        let save = flag("save")?;
        let next = if reset {
            if let Some(extra) = fields.keys().next() {
                return Err(Refusal::invalid(extra, "cannot be set with reset"));
            }
            MonitorLook::default_for(who.colour)
        } else {
            let patch = Patch::from_value(&Value::Object(fields))?;
            if patch.is_empty() {
                return Err(Refusal::invalid(
                    "look",
                    "needs a field to change, or reset: true",
                ));
            }
            apply(&current, &patch, &self.catalogue)?
        };
        let mut out = Map::new();
        if save {
            let source = self
                .catalogue
                .iter()
                .find(|e| e.key == next.model && !next.is_marker())
                .map(|e| e.sha256.clone());
            let Some(store) = store else {
                return Err(Refusal::invalid(
                    "save",
                    "needs the settings store, which this run does not have",
                ));
            };
            let to_save = Saved {
                look: next.clone(),
                source,
            };
            match saved::save(store, &who.profile, &to_save) {
                Ok(None) => {}
                Ok(Some(why)) => {
                    out.insert("not_saved".into(), json!(why));
                }
                Err(e) => return Err(Refusal::invalid("save", e)),
            }
            out.insert("saved".into(), json!(true));
        } else {
            out.insert("saved".into(), json!(false));
        }
        out.insert("changed".into(), json!(next != current));
        out.insert(
            "drawn_as".into(),
            drawn_as(&next, &self.catalogue, self.bodies).to_value(),
        );
        out.insert("look".into(), next.to_value());
        self.session = Some(next);
        Ok(Value::Object(out))
    }

    /// `forge.status.monitors`: each seat's pose and look. Offline: the
    /// local seat, which is never drawn in its own view. `pose` is the
    /// monitor camera's position, yaw and pitch while there is one.
    pub fn monitors(&self, who: &Who, monitor: bool, pose: Option<([f32; 3], f32, f32)>) -> Value {
        let Some(look) = &self.session else {
            return json!({"monitors": []});
        };
        let hidden = if monitor {
            "A monitor is never drawn in its own view"
        } else {
            "The player is in their biped"
        };
        json!({"monitors": [{
            "seat": 0,
            "name": who.profile,
            "monitor": monitor,
            "pose": pose.map(|(position, yaw, pitch)| json!({
                "position": position, "yaw": yaw, "pitch": pitch,
            })),
            "look": look.to_value(),
            "drawn_as": DrawnAs::new("hidden", Some(hidden)).to_value(),
            "peers_see": if monitor {
                drawn_as(look, &self.catalogue, self.bodies).to_value()
            } else {
                DrawnAs::new("hidden", Some(hidden)).to_value()
            },
            "seen_ticks_ago": 0,
        }]})
    }
}
