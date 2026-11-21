//! Optional object-edit hooks on the invented game's ordinary control and stream connections.
use super::*;
use agent_proto::sim::{ObjectDeleteParams, ObjectRespawnParams, ObjectSetParams, method};
use agent_proto::{Kind, ObjectId};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Objects {
    // The game remembers the slot of an unlinked preview, even though the stream cannot report it.
    weapons: BTreeMap<u16, live::Object>,
    salt: u32,
    allow_append: bool,
    fail_next: Option<RpcError>,
    hold_on_delete: bool,
    fail_creation: bool,
    replace_before_respawn: Option<u16>,
    peer_change: Option<(u32, bool)>,
}

impl Objects {
    pub(super) fn capture(&self, world: &mut live::World) {
        world.objects.extend(self.weapons.values().copied());
    }

    fn weapon(&mut self, slot: u16, linked: bool) -> live::Object {
        self.salt += 1;
        let x = 10.0 * f32::from(slot);
        live::Object {
            id: (self.salt << 16) | u32::from(slot + 4),
            kind: 2,
            tag: Some(scenario_edit::fixture::id(2).0),
            placement: linked.then_some(slot),
            position: [x, x + 1., x + 2.],
            forward: [1., 0., 0.],
            up: [0., 0., 1.],
            scale: 1.,
            ..Default::default()
        }
    }

    fn slot(&self, id: &str) -> Result<u16, RpcError> {
        let id = ObjectId::parse(id)
            .filter(|id| id.kind == Kind::Weapon)
            .ok_or_else(|| RpcError::new(code::INVALID_PARAMS, "expected a salted weapon id"))?;
        self.weapons
            .iter()
            .find(|(_, object)| object.id == id.key && object.parent.is_none())
            .map(|(&slot, _)| slot)
            .ok_or_else(|| RpcError::new(code::GONE, format!("{id} is gone")))
    }

    fn edit(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        if let Some(err) = self.fail_next.take() {
            return Err(err);
        }
        match method {
            method::OBJECT_SET => {
                let p: ObjectSetParams = serde_json::from_value(params).map_err(invalid)?;
                let slot = self.slot(&p.id)?;
                let object = self.weapons.get_mut(&slot).unwrap();
                if let Some(pos) = p.pos {
                    object.position = pos;
                }
                if let Some(forward) = p.forward {
                    object.forward = forward;
                }
                if let Some(up) = p.up {
                    object.up = up;
                }
                Ok(json!({"object": p.id}))
            }
            method::OBJECT_DELETE => {
                let p: ObjectDeleteParams = serde_json::from_value(params).map_err(invalid)?;
                let slot = self.slot(&p.id)?;
                if std::mem::take(&mut self.hold_on_delete) {
                    // The pickup races the editor's last streamed free weapon.
                    self.weapons.get_mut(&slot).unwrap().parent = Some(UNIT);
                    self.slot(&p.id)?;
                }
                self.weapons.remove(&slot);
                Ok(json!({"deleted": p.id}))
            }
            method::OBJECT_RESPAWN => {
                let p: ObjectRespawnParams = serde_json::from_value(params).map_err(invalid)?;
                if let Some(slot) = self.replace_before_respawn.take() {
                    let replacement = self.weapon(slot, true);
                    self.weapons.insert(slot, replacement);
                }
                let slot = match (p.placement, p.id) {
                    (Some(place), None) if place.block == "weapons" => place.index,
                    (Some(place), None) => {
                        return Err(RpcError::new(agent_proto::sim::REFUSED, format!(
                            "{} has no supported object palette and world id", place.block
                        )));
                    }
                    (None, Some(id)) => self.slot(&id)?,
                    _ => {
                        return Err(RpcError::new(code::INVALID_PARAMS, "give exactly one of id or placement"));
                    }
                };
                // The fixture's loaded block has two entries. Separate tests opt in to a
                // successful appended-slot reply to exercise consumers of an unlinked preview.
                if slot >= 2 && !self.allow_append {
                    return Err(RpcError::new(agent_proto::sim::REFUSED,
                        format!("index {slot} is outside a block of 2 records")));
                }
                if self.weapons.get(&slot).is_some_and(|o| o.parent.is_some()) {
                    return Err(RpcError::new(
                        agent_proto::sim::REFUSED,
                        "the placement's object is held or attached",
                    ));
                }
                let old = self.weapons.remove(&slot).map(|o| name(o.id));
                if std::mem::take(&mut self.fail_creation) {
                    let mut err = RpcError::new(agent_proto::sim::REFUSED, "object_new refused the replacement");
                    err.data = Some(json!({"deleted": old}));
                    return Err(err);
                }
                let object = self.weapon(slot, false);
                self.weapons.insert(slot, object);
                Ok(json!({"old": old, "object": name(object.id), "linked": false}))
            }
            _ => unreachable!(),
        }
    }
}

fn invalid(e: serde_json::Error) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, e.to_string())
}

fn refused(why: &str) -> RpcError {
    RpcError {
        code: agent_proto::sim::REFUSED,
        message: why.to_owned(),
        data: Some(json!({"why": why})),
    }
}

fn name(handle: u32) -> String {
    ObjectId { kind: Kind::Weapon, key: handle }.to_string()
}

impl Game {
    pub(crate) fn install_object_edits(&self, with_set: bool) {
        let kind = self.kind;
        let mut edits = self.edits.lock().unwrap();
        for slot in 0..2 {
            let object = edits.objects.weapon(slot, true);
            edits.objects.weapons.insert(slot, object);
        }
        drop(edits);
        for name in [method::OBJECT_SET, method::OBJECT_DELETE, method::OBJECT_RESPAWN] {
            if name == method::OBJECT_SET && !with_set {
                continue;
            }
            let calls = self.calls.clone();
            let edits = self.edits.clone();
            self.agent.add_game_method(name, Access::Act, move |v, ctx| {
                calls.lock().unwrap().push((name.to_owned(), v.clone()));
                let mut state = edits.lock().unwrap();
                kind.writes().map_err(|why| RpcError::new(code::INVALID_REQUEST, why))?;
                if state.external_live {
                    return Err(refused("external Live session"));
                }
                if kind == SessionKind::MpHost {
                    let change = state.objects.peer_change.take();
                    if let Some((peers, false)) = change {
                        state.peers = Some(peers);
                    }
                    let peers = state.peers.ok_or_else(|| {
                        RpcError::new(code::INVALID_REQUEST, "the multiplayer peer count is unknown")
                    })?;
                    if peers > 1 && v["confirm_peers"].as_u64() != Some(u64::from(peers)) {
                        return Err(RpcError::new(
                            code::INVALID_REQUEST,
                            "confirm the current peer count for host-only edits",
                        ));
                    }
                    if let Some((changed, true)) = change {
                        state.peers = Some(changed);
                        return Err(refused("the session or its peer count changed before the edit"));
                    }
                }
                let mut reply = state.objects.edit(name, v)?;
                reply["at"] = json!(ctx.at);
                Ok(reply)
            });
        }
    }

    pub(crate) fn replace_weapon(&self, slot: u16) -> String {
        let mut state = self.edits.lock().unwrap();
        let weapon = state.objects.weapon(slot, true);
        state.objects.weapons.insert(slot, weapon);
        name(weapon.id)
    }

    pub(crate) fn allow_appended_preview(&self) {
        self.edits.lock().unwrap().objects.allow_append = true;
    }

    pub(crate) fn pick_up_weapon_on_next_delete(&self) {
        self.edits.lock().unwrap().objects.hold_on_delete = true;
    }

    pub(crate) fn put_down_weapon(&self, slot: u16) {
        self.edits.lock().unwrap().objects.weapons.get_mut(&slot).unwrap().parent = None;
    }

    pub(crate) fn replace_weapon_before_respawn(&self, slot: u16) {
        self.edits.lock().unwrap().objects.replace_before_respawn = Some(slot);
    }

    pub(crate) fn fail_next_creation(&self) {
        self.edits.lock().unwrap().objects.fail_creation = true;
    }

    pub(crate) fn change_peers_on_next_object_edit(&self, peers: u32, at_execution: bool) {
        self.edits.lock().unwrap().objects.peer_change = Some((peers, at_execution));
    }

    pub(crate) fn fail_next_object_edit(&self, code: i64, why: &str) {
        self.edits.lock().unwrap().objects.fail_next = Some(RpcError::new(code, why));
    }
}
