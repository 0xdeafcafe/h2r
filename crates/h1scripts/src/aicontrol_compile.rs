//! Lower scripted platoon actions into the level's Halo 2 orders and squads.

use h1ai::control::{Action, Control, Platoon};
use hs::h1_ai_slots::{CONTROL, CONTROL_TOKEN, ManeuverEdge, Operation as RuntimeOperation};
use hs::{Datum, Node};

use super::{CALL, Compiler, GLOBAL, New, definition, h2t};
use crate::{Result, aicontrol, catalog::types, invalid};

fn switch_name(platoon: Platoon) -> String {
    format!("h1_maneuver_{}_{}", platoon.encounter, platoon.index)
}

impl Compiler<'_> {
    pub(super) fn install_maneuver_switches(&mut self) -> Result<()> {
        let Some(control) = self.ai_control else {
            return Ok(());
        };
        for &platoon in &control.switches {
            let index = self.out.globals.len();
            let h2_index = u16::try_from(index)
                .ok()
                .filter(|&i| i < 0x7fff)
                .ok_or_else(|| invalid("too many globals for maneuver switches"))?;
            let name = switch_name(platoon);
            if self
                .t
                .globals
                .iter()
                .any(|g| g.name.eq_ignore_ascii_case(&name))
            {
                return Err(invalid(format!("global name collision: {name}")));
            }
            let root = self.number(h2t::BOOLEAN, 1);
            self.out
                .globals
                .push(definition(&name, h2t::BOOLEAN, 0, root, index)?);
            self.maneuver_switches.insert(platoon, h2_index);
        }
        Ok(())
    }

    fn maneuver_switch(&mut self, platoon: Platoon) -> Option<Datum> {
        let index = *self.maneuver_switches.get(&platoon)?;
        let string = self.intern(switch_name(platoon).as_bytes());
        Some(self.push(New {
            opcode: h2t::BOOLEAN,
            ty: h2t::BOOLEAN,
            flags: GLOBAL,
            string,
            data: u32::from(index),
        }))
    }

    fn ai_literal(&mut self, index: u16) -> Result<Datum> {
        let ty = self.ty(types::AI)?;
        let token = format!("h1_squad_{index}");
        Ok(self.literal(ty, ty, u32::from(index), token.as_bytes()))
    }

    pub(super) fn ai_control(
        &mut self,
        name: &str,
        args: &[Datum],
        expected: u16,
    ) -> Result<Option<Datum>> {
        let op = match name {
            "ai_magically_see_players" => Some(RuntimeOperation::SeePlayers),
            "ai_vehicle_encounter" => Some(RuntimeOperation::VehicleEncounter),
            "ai_vehicle_enterable_distance" => Some(RuntimeOperation::VehicleDistance),
            "ai_vehicle_enterable_actor_type" => Some(RuntimeOperation::VehicleActorType),
            "ai_vehicle_enterable_team" => Some(RuntimeOperation::VehicleTeam),
            "ai_vehicle_enterable_actors" => Some(RuntimeOperation::VehicleActors),
            "ai_vehicle_enterable_disable" => Some(RuntimeOperation::VehicleDisable),
            "ai_going_to_vehicle" => Some(RuntimeOperation::VehicleCount),
            "ai_attach_free" => Some(RuntimeOperation::AttachFree),
            "ai_free" => Some(RuntimeOperation::Free),
            "ai_free_units" => Some(RuntimeOperation::FreeUnits),
            "ai_migrate_by_unit" => Some(RuntimeOperation::MigrateUnits),
            "ai_teleport_to_starting_location" => Some(RuntimeOperation::Teleport),
            "ai_teleport_to_starting_location_if_unsupported" => {
                Some(RuntimeOperation::TeleportUnsupported)
            }
            _ => None,
        };
        if let Some(op) = op
            && self.has_runtime_control()
            && (op != RuntimeOperation::VehicleCount
                || self
                    .h2
                    .builtins
                    .get("ai_going_to_vehicle")
                    .is_some_and(|b| b.index == op.slot()))
        {
            return self.runtime_control(op, args, expected).map(Some);
        }
        match name {
            "ai_allow_dormant"
                if self.h2.builtins.contains_key("ai_force_active")
                    && self.h2.builtins.contains_key("not") =>
            {
                let [subject, allow] = args else {
                    return Err(invalid("ai_allow_dormant arity"));
                };
                let subject = self.expr(*subject, self.ty(types::AI)?)?;
                let allow = self.expr(*allow, h2t::BOOLEAN)?;
                let force = self.control_call("not", h2t::BOOLEAN, &[allow])?;
                return self
                    .control_call("ai_force_active", expected, &[subject, force])
                    .map(Some);
            }
            // The Xbox Halo 1 evaluator (0x000ed620) only returns void.
            // This editor operation has no runtime side effects to port.
            "ai_reconnect" if args.is_empty() && self.h2.builtins.contains_key("begin") => {
                return self.control_call("begin", expected, &[]).map(Some);
            }
            _ => {}
        }
        let Some(control) = self.ai_control else {
            return Ok(None);
        };
        let Some(&first) = args.first() else {
            return Ok(None);
        };
        let Some(request) = aicontrol::request(name, self.t.node(first)?, self.t) else {
            return Ok(None);
        };
        let Some(actions) = control.actions.get(&request) else {
            return Ok(None);
        };
        // Setting many platoons requires a scalar argument so lowering
        // through several native assignments can't repeat a side effect.
        if request.operation == h1ai::control::Operation::ManeuverEnable
            && let [_, enable] = args
            && actions.len() > 1
            && !self.t.node(*enable)?.atom()
        {
            return Ok(None);
        }
        if actions.len() < 2 || request.operation == h1ai::control::Operation::ManeuverEnable {
            return self
                .emit_actions(control, actions, args, expected)
                .map(Some);
        }
        // Repeated encounter calls share one static body. New definitions
        // are appended after the original script indices.
        let (index, name) = if let Some(&(index, ref name)) = self.ai_scripts.get(&request) {
            (index, name.clone())
        } else {
            let index = self.t.scripts.len() + self.ai_definitions.len();
            let index = u16::try_from(index)
                .ok()
                .filter(|&i| i < 0x7fff)
                .ok_or_else(|| invalid("too many scripts for encounter controls"))?;
            let name = format!("h1_ai_{}", self.ai_definitions.len());
            if self
                .t
                .scripts
                .iter()
                .any(|s| s.name.eq_ignore_ascii_case(&name))
            {
                return Err(invalid(format!("script name collision: {name}")));
            }
            let root = self.emit_actions(control, actions, args, h2t::VOID)?;
            self.ai_definitions
                .push(definition(&name, 3, h2t::VOID, root, usize::from(index))?);
            self.ai_scripts.insert(request, (index, name.clone()));
            (index, name)
        };
        Ok(Some(self.call(
            index,
            super::SCRIPT_CALL,
            expected,
            name.as_bytes(),
            &[],
        )))
    }

    fn emit_actions(
        &mut self,
        control: &Control,
        actions: &[Action],
        args: &[Datum],
        expected: u16,
    ) -> Result<Datum> {
        if matches!(
            actions.first(),
            Some(Action::Migrate {
                platoon: Some(_),
                ..
            })
        ) && self.has_runtime_control()
        {
            let mut routes = String::new();
            for &action in actions {
                let Action::Migrate {
                    from,
                    to,
                    platoon: Some(platoon),
                } = action
                else {
                    return Err(invalid("mixed maneuver and ordinary migration actions"));
                };
                ManeuverEdge {
                    from,
                    to,
                    enabled_by: self.maneuver_switches.get(&platoon).copied(),
                }
                .append_to(&mut routes);
            }
            let offset = self.intern(routes.as_bytes());
            let operand = self.literal(h2t::STRING, h2t::STRING, offset, routes.as_bytes());
            return self.runtime_control_call(RuntimeOperation::Maneuver, &[operand], expected);
        }
        let mut expressions = Vec::with_capacity(actions.len());
        let ty = if actions.len() == 1 {
            expected
        } else {
            h2t::VOID
        };
        for &action in actions {
            let expr = match action {
                Action::SetOrder { squad, order } => {
                    let squad = self.ai_literal(squad)?;
                    let order_ty = self
                        .h2
                        .type_index("ai_orders")
                        .ok_or_else(|| invalid("Halo 2 ai_orders type unknown"))?;
                    let token = format!("h1_order_{order}");
                    let order = self.literal(
                        order_ty,
                        order_ty,
                        0xffff_0000 | u32::from(order),
                        token.as_bytes(),
                    );
                    self.control_call("ai_set_orders", ty, &[squad, order])?
                }
                Action::Migrate { from, to, platoon } => {
                    let from = self.ai_literal(from)?;
                    let to = self.ai_literal(to)?;
                    // A platoon only needs storage if a script can disable it.
                    if let Some(platoon) = platoon
                        && control.switches.contains(&platoon)
                        && let Some(enabled) = self.maneuver_switch(platoon)
                    {
                        let migrate = self.control_call("ai_migrate", h2t::VOID, &[from, to])?;
                        self.control_call("if", ty, &[enabled, migrate])?
                    } else {
                        self.control_call("ai_migrate", ty, &[from, to])?
                    }
                }
                Action::Enable(platoon) => {
                    let [_, value] = args else {
                        return Err(invalid("ai_maneuver_enable arity"));
                    };
                    let flag = self
                        .maneuver_switch(platoon)
                        .ok_or_else(|| invalid("missing maneuver switch"))?;
                    let enable = self.expr(*value, h2t::BOOLEAN)?;
                    self.control_call("set", ty, &[flag, enable])?
                }
                Action::Spawn(squad) => {
                    let squad = self.ai_literal(squad)?;
                    let count = self.number(h2t::SHORT, 1);
                    self.control_call("ai_place_counted", ty, &[squad, count])?
                }
            };
            expressions.push(expr);
        }
        if let [only] = expressions.as_slice() {
            Ok(*only)
        } else {
            self.control_call("begin", expected, &expressions)
        }
    }

    fn control_call(&mut self, name: &str, ty: u16, args: &[Datum]) -> Result<Datum> {
        let function = self
            .h2
            .builtins
            .get(name)
            .ok_or_else(|| invalid(format!("Halo 2 {name} unknown")))?;
        Ok(self.call(
            function.index,
            CALL,
            ty,
            aicontrol::retail_name(name).as_bytes(),
            args,
        ))
    }

    fn has_runtime_control(&self) -> bool {
        self.h2
            .builtins
            .get("ai_magically_see_players")
            .is_some_and(|b| b.index == CONTROL)
    }

    fn runtime_control(
        &mut self,
        op: RuntimeOperation,
        args: &[Datum],
        expected: u16,
    ) -> Result<Datum> {
        let mut parameters = op.parameters();
        if op.uses_vehicle_registry() {
            parameters = &parameters[..parameters.len() - 1];
        }
        if args.len() != parameters.len() {
            return Err(invalid(format!("wrong arity for {op:?}")));
        }
        let mut compiled = Vec::with_capacity(args.len());
        for (&arg, &ty) in args.iter().zip(parameters) {
            let node = *self.t.node(arg)?;
            let literal = if ty == hs::h1_ai_slots::ACTOR_TYPE
                && node.atom()
                && !node.global()
                && self.h1.type_name(node.opcode())? == "actor_type"
            {
                // Halo 2 preserves Halo 1's actor-type enumerators.
                let text = self.t.string_bytes(node.string_offset())?.to_vec();
                self.literal(ty, ty, node.data(), &text)
            } else {
                self.expr(arg, ty)?
            };
            compiled.push(literal);
        }
        if op.uses_vehicle_registry() {
            let base = self.vehicle_state()?;
            compiled.push(self.number(h2t::LONG, i32::from(base)));
        }
        self.runtime_control_call(op, &compiled, expected)
    }

    fn vehicle_state(&mut self) -> Result<u16> {
        if let Some(base) = self.vehicle_state {
            return Ok(base);
        }
        // Calls can appear in original global initializers. Reserve names
        // after the original globals and maneuver switches in either case.
        let base = self.t.globals.len() + self.ai_control.map_or(0, |c| c.switches.len());
        let base = u16::try_from(base).map_err(|_| invalid("global count"))?;
        self.vehicle_state = Some(base);
        Ok(base)
    }

    pub(super) fn install_vehicle_tick(&mut self) -> Result<()> {
        use hs::h1_ai_slots::{VEHICLE_ROWS, VEHICLE_WIDTH};
        let Some(base) = self.vehicle_state else {
            return Ok(());
        };
        let base = usize::from(base);
        let end = base + usize::from(VEHICLE_ROWS) * usize::from(VEHICLE_WIDTH);
        if self.out.globals.len() != base || hs::vm::EXTERNAL_GLOBALS + end > 0xc00 {
            return Err(invalid("vehicle AI state exceeds the Halo 2 global pool"));
        }
        for index in base..end {
            let word = (index - base) % usize::from(VEHICLE_WIDTH);
            let init = if matches!(word, 0 | 1 | 5..=10) {
                -1
            } else {
                0
            };
            let root = self.number(h2t::LONG, init);
            let name = format!("h1_vehicle_state_{index}");
            if self
                .t
                .globals
                .iter()
                .any(|g| g.name.eq_ignore_ascii_case(&name))
            {
                return Err(invalid(format!("global name collision: {name}")));
            }
            self.out
                .globals
                .push(definition(&name, h2t::LONG, 0, root, index)?);
        }
        let name = "h1_vehicle_ai_tick";
        if self
            .t
            .scripts
            .iter()
            .any(|s| s.name.eq_ignore_ascii_case(name))
        {
            return Err(invalid(format!("script name collision: {name}")));
        }
        let base = self.number(h2t::LONG, base as i32);
        let root = self.runtime_control_call(RuntimeOperation::VehicleTick, &[base], h2t::VOID)?;
        self.out.scripts.push(definition(
            name,
            2,
            h2t::VOID,
            root,
            self.out.scripts.len(),
        )?);
        Ok(())
    }

    fn runtime_control_call(
        &mut self,
        op: RuntimeOperation,
        args: &[Datum],
        expected: u16,
    ) -> Result<Datum> {
        let call = self.call(op.slot(), CALL, expected, CONTROL_TOKEN.as_bytes(), args);
        let head = Datum(self.out.nodes[call.slot()].data());
        let mut wire = self.out.nodes[head.slot()].encode();
        wire[16..20].copy_from_slice(&op.marker().to_le_bytes());
        self.out.nodes[head.slot()] = Node::decode(&wire)?;
        Ok(call)
    }
}
