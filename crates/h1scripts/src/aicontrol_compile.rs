//! Lower scripted platoon actions into the level's Halo 2 orders and squads.

use h1ai::control::{Action, Control, Platoon};
use hs::h1_ai_slots::{
    CONTROL, CONTROL_TOKEN, ManeuverEdge, Operation as RuntimeOperation, PlatoonRule,
    PlatoonTrigger, Registry, encode_words,
};
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
            let index = self.declare_maneuver_state(&switch_name(platoon), true)?;
            self.maneuver_switches.insert(platoon, index);
        }
        if self.has_runtime_control() {
            for rule in &control.platoon_rules {
                if rule.can_arm {
                    let platoon = rule.platoon;
                    let name = format!("h1_latch_{}_{}", platoon.encounter, platoon.index);
                    let index = self.declare_maneuver_state(&name, false)?;
                    self.maneuver_latches.insert(platoon, index);
                }
            }
        }
        Ok(())
    }

    fn declare_maneuver_state(&mut self, name: &str, initial: bool) -> Result<u16> {
        let index = self.out.globals.len();
        if hs::vm::EXTERNAL_GLOBALS + index >= 0xc00 {
            return Err(invalid("too many globals for maneuver state"));
        }
        if self
            .t
            .globals
            .iter()
            .any(|g| g.name.eq_ignore_ascii_case(name))
        {
            return Err(invalid(format!("global name collision: {name}")));
        }
        let root = self.number(h2t::BOOLEAN, i32::from(initial));
        self.out
            .globals
            .push(definition(name, h2t::BOOLEAN, 0, root, index)?);
        Ok(index as u16)
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
        if let Some(op) = aicontrol::runtime_operation(name)
            && aicontrol::runtime_target(self.h2, op).is_some()
        {
            if op == RuntimeOperation::ExpireSquadTimers
                && let Some(control) = self.ai_control
                && let [reference] = args
                && let Some(request) = aicontrol::request(name, self.t.node(*reference)?, self.t)
                && let Some(actions) = control.actions.get(&request)
            {
                return self.emit_actions(control, actions, args, expected).map(Some);
            }
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
        if self.has_runtime_control() {
            if let Some(platoons) = control.maneuver_requests.get(&request) {
                let mut indices = String::new();
                encode_words(
                    &mut indices,
                    platoons
                        .iter()
                        .filter_map(|id| self.maneuver_latches.get(id).copied()),
                );
                let indices = self.control_string(&indices);
                let value = self.number(h2t::BOOLEAN, 1);
                return self
                    .runtime_control_call(
                        RuntimeOperation::SetManeuver,
                        &[indices, value],
                        expected,
                    )
                    .map(Some);
            }
            if request.operation == h1ai::control::Operation::ManeuverEnable
                && let Some(actions) = control.actions.get(&request)
            {
                let [_, value] = args else {
                    return Err(invalid("ai_maneuver_enable arity"));
                };
                let mut indices = String::new();
                encode_words(
                    &mut indices,
                    actions.iter().filter_map(|action| match action {
                        Action::Enable(id) => self.maneuver_switches.get(id).copied(),
                        _ => None,
                    }),
                );
                let indices = self.control_string(&indices);
                let value = self.expr(*value, h2t::BOOLEAN)?;
                return self
                    .runtime_control_call(
                        RuntimeOperation::SetManeuver,
                        &[indices, value],
                        expected,
                    )
                    .map(Some);
            }
        }
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
                Action::ExpireTimer {
                    squad,
                    reveal_players,
                } => {
                    let squad = self.ai_literal(squad)?;
                    if self.has_runtime_control() {
                        self.runtime_control_call(RuntimeOperation::ExpireSquadTimers, &[squad], ty)?
                    } else {
                        let reveal = self.number(h2t::BOOLEAN, i32::from(reveal_players));
                        self.runtime_control_call(RuntimeOperation::TimerExpire, &[squad, reveal], ty)?
                    }
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
        if op.registry().is_some() {
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
        if let Some(registry) = op.registry() {
            let base = self.registry_base(registry)?;
            compiled.push(self.number(h2t::LONG, i32::from(base)));
        }
        self.runtime_control_call(op, &compiled, expected)
    }

    fn registry_base(&mut self, registry: Registry) -> Result<u16> {
        if let Some(&(_, base)) = self.ai_registries.iter().find(|(r, _)| *r == registry) {
            return Ok(base);
        }
        // Calls can appear in original global initializers. Reserve names
        // after the original globals and maneuver switches in either case.
        let base = self.t.globals.len()
            + self.ai_control.map_or(0, |c| {
                c.switches.len() + c.platoon_rules.iter().filter(|rule| rule.can_arm).count()
            })
            + self
                .ai_registries
                .iter()
                .map(|(r, _)| usize::from(r.words()))
                .sum::<usize>();
        let base = u16::try_from(base).map_err(|_| invalid("global count"))?;
        self.ai_registries.push((registry, base));
        Ok(base)
    }

    pub(super) fn install_ai_ticks(&mut self) -> Result<()> {
        let mut hook_state = u32::MAX;
        for (registry, base) in std::mem::take(&mut self.ai_registries) {
            if let Some(shift) = registry.hook_shift() {
                hook_state = (hook_state & !(0xffff << shift)) | (u32::from(base) << shift);
            }
            let start = usize::from(base);
            let end = start + usize::from(registry.words());
            if self.out.globals.len() != start || hs::vm::EXTERNAL_GLOBALS + end > 0xc00 {
                return Err(invalid("converted AI state exceeds the Halo 2 global pool"));
            }
            for index in 0..registry.words() {
                let initial =
                    if registry == Registry::Follow && index >= hs::h1_ai_slots::FOLLOW_WORDS {
                        let word = (index - hs::h1_ai_slots::FOLLOW_WORDS)
                            % hs::h1_ai_slots::MIGRATION_WORDS;
                        self.ai_control.map_or(0, |control| {
                            control
                                .automatic_migration
                                .range(word * 32..(word + 1) * 32)
                                .fold(0u32, |bits, &id| bits | 1 << (id % 32))
                                as i32
                        })
                    } else {
                        registry.initial(index)
                    };
                let root = self.number(h2t::LONG, initial);
                let global = start + usize::from(index);
                let name = format!("h1_ai_state_{global}");
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
                    .push(definition(&name, h2t::LONG, 0, root, global)?);
            }
            let Some(operation) = registry.tick() else {
                continue;
            };
            let name = format!("h1_ai_tick_{base}");
            if self
                .t
                .scripts
                .iter()
                .any(|s| s.name.eq_ignore_ascii_case(&name))
            {
                return Err(invalid(format!("script name collision: {name}")));
            }
            let base = self.number(h2t::LONG, i32::from(base));
            let root = self.runtime_control_call(operation, &[base], h2t::VOID)?;
            self.out.scripts.push(definition(
                &name,
                2,
                h2t::VOID,
                root,
                self.out.scripts.len(),
            )?);
        }
        if hook_state != u32::MAX {
            // Native hooks can locate these registries in constant time.
            let name = hs::h1_ai_slots::HOOK_STATE;
            let index = self.out.globals.len();
            if hs::vm::EXTERNAL_GLOBALS + index >= 0xc00
                || self
                    .t
                    .globals
                    .iter()
                    .any(|g| g.name.eq_ignore_ascii_case(name))
            {
                return Err(invalid("unable to add the AI hook state index"));
            }
            let root = self.number(h2t::LONG, hook_state as i32);
            self.out
                .globals
                .push(definition(name, h2t::LONG, 0, root, index)?);
        }
        Ok(())
    }

    pub(super) fn install_combat_timers(&mut self) -> Result<()> {
        use hs::h1_ai_slots::timers;

        let Some(control) = self.ai_control else {
            return Ok(());
        };
        if !self.has_runtime_control() || control.combat_timers.is_empty() {
            return Ok(());
        }
        let rows = control.combat_timers.iter().map(|(&id, timer)| (id, timer.flags, timer.seconds));
        let words = timers::build(rows).ok_or_else(|| invalid("too many squads for combat timers"))?;
        let base = self.out.globals.len();
        if hs::vm::EXTERNAL_GLOBALS + base + words.len() >= 0xc00 {
            return Err(invalid("converted AI timers exceed the Halo 2 global pool"));
        }
        for (offset, value) in words.into_iter().enumerate() {
            self.declare_timer_word(&format!("h1_ai_timer_{offset}"), value)?;
        }
        self.declare_timer_word(timers::GLOBAL, base as u32)?;
        let name = "h1_ai_timer_tick";
        if self.t.scripts.iter().any(|s| s.name.eq_ignore_ascii_case(name)) {
            return Err(invalid(format!("script name collision: {name}")));
        }
        let base = self.number(h2t::LONG, base as i32);
        let root = self.runtime_control_call(RuntimeOperation::TimerTick, &[base], h2t::VOID)?;
        self.out.scripts.push(definition(name, 2, h2t::VOID, root, self.out.scripts.len())?);
        Ok(())
    }

    fn declare_timer_word(&mut self, name: &str, value: u32) -> Result<()> {
        if self.t.globals.iter().any(|g| g.name.eq_ignore_ascii_case(name)) {
            return Err(invalid(format!("global name collision: {name}")));
        }
        let root = self.number(h2t::LONG, value as i32);
        self.out.globals.push(definition(name, h2t::LONG, 0, root, self.out.globals.len())?);
        Ok(())
    }

    pub(super) fn install_platoon_tick(&mut self) -> Result<()> {
        let Some(control) = self.ai_control else {
            return Ok(());
        };
        if !self.has_runtime_control() || control.platoon_rules.is_empty() {
            return Ok(());
        }
        let mut rules = String::new();
        for rule in &control.platoon_rules {
            let trigger = |trigger: &h1ai::control::Trigger| PlatoonTrigger {
                condition: trigger.condition,
                squads: trigger.squads.clone(),
            };
            PlatoonRule {
                latch: self.maneuver_latches.get(&rule.platoon).copied(),
                enabled_by: self.maneuver_switches.get(&rule.platoon).copied(),
                maneuver_flags: rule.maneuver_flags,
                members: rule.members.clone(),
                change: rule.change.as_ref().map(trigger),
                maneuver: rule.maneuver.as_ref().map(trigger),
                orders: rule.orders.clone(),
                routes: rule.routes.clone(),
            }
            .append_to(&mut rules);
        }
        if rules.len() > hs::h1_ai_slots::MAX_PLATOON_TEXT {
            return Err(invalid(
                "converted AI platoon table exceeds its argument size",
            ));
        }
        let name = "h1_ai_platoon_tick";
        if self
            .t
            .scripts
            .iter()
            .any(|s| s.name.eq_ignore_ascii_case(name))
        {
            return Err(invalid(format!("script name collision: {name}")));
        }
        let argument = self.control_string(&format!("{:04x}{rules}", rules.len()));
        let root =
            self.runtime_control_call(RuntimeOperation::PlatoonTick, &[argument], h2t::VOID)?;
        self.out.scripts.push(definition(
            name,
            2,
            h2t::VOID,
            root,
            self.out.scripts.len(),
        )?);
        Ok(())
    }

    fn control_string(&mut self, literal: &str) -> Datum {
        let offset = self.intern(literal.as_bytes());
        self.literal(h2t::STRING, h2t::STRING, offset, literal.as_bytes())
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
