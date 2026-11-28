//! Deliver one menu action through the agent's XAPI tap. Check the same
//! dispatcher owner and focused row as `ui.audit` at the request boundary
//! and again at the first guest pad read. Answer after the neutral release.

use agent::{Access, Agent};
use agent_proto::sim::menu::{MODE_LIVE, PRESS_MS, PRESS_READS};
use agent_proto::{Boundary, PadStep, Phase, RpcError, code, method};
use guest_layout::memory::{GuestMemory, MemFault};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::ui_audit::Target;

#[derive(Copy, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Action {
    Up,
    Down,
    Left,
    Right,
    Choose,
    Back,
    Friends,
    /// A on the Live players screen opens the Live menu (0x25286d). In
    /// other modes the same press can join or start a game.
    OpenLiveMenu,
    /// X in the pregame lobby: the GAME MODE picker, for the party leader
    /// (`crate::lobby_tabs::input`).
    ChangeMode,
}

impl Action {
    fn button(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
            Self::Choose | Self::OpenLiveMenu => "a",
            Self::Back => "b",
            Self::Friends => "y",
            Self::ChangeMode => "x",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Press {
    action: Action,
    target: String,
    row: Option<u32>,
    phase: Option<Phase>,
}

impl Press {
    fn check_phase(&self, current: Phase) -> Result<(), RpcError> {
        if current == Phase::Loading {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "menu input unavailable while loading",
            ));
        }
        if self.phase.is_some_and(|wanted| current != wanted) {
            return Err(RpcError::new(code::GONE, "game phase changed"));
        }
        Ok(())
    }

    fn check(&self, owner: &Target, join_mode: Option<u32>) -> Result<(), RpcError> {
        if self.target != owner.token {
            return Err(RpcError::new(code::GONE, "menu input target changed"));
        }
        if self.action == Action::Choose && self.row.is_none() {
            return Err(RpcError::new(code::INVALID_PARAMS, "choose requires a row"));
        }
        if self.row.is_some() && self.row != owner.focused_row {
            return Err(RpcError::new(
                code::GONE,
                "menu focus changed or is unreadable",
            ));
        }
        if self.action == Action::OpenLiveMenu
            && (owner.id != crate::ui_watch::JOIN_SCREEN || join_mode != Some(MODE_LIVE))
        {
            return Err(RpcError::new(
                code::GONE,
                "expected the Xbox Live players screen",
            ));
        }
        if self.action == Action::ChangeMode && owner.id != crate::lobby_options::SCREEN
        {
            return Err(RpcError::new(code::GONE, "expected the pregame lobby"));
        }
        Ok(())
    }
}

// XInputGetState hands its tap the very same guest view the game is using.
struct PadMemory<'a>(&'a dyn xapi::GuestMemory);

impl GuestMemory for PadMemory<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        self.0.read(addr, buf).map_err(|_| MemFault {
            addr,
            len: buf.len() as u32,
            write: false,
        })
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        Err(MemFault {
            addr,
            len: data.len() as u32,
            write: true,
        })
    }
}

impl Press {
    fn check_owner(&self, mem: &dyn GuestMemory) -> Result<(), RpcError> {
        let owner = crate::ui_audit::target(mem)
            .ok_or_else(|| RpcError::new(code::UNAVAILABLE, "no menu owns controller zero"))?;
        let join_mode = if owner.id == crate::ui_watch::JOIN_SCREEN {
            owner
                .screen
                .checked_add(crate::ui_watch::JOIN_MODE)
                .and_then(|addr| mem.u32(addr).ok())
        } else {
            None
        };
        self.check(&owner, join_mode)
    }

    fn still_receives(&self, mem: &dyn GuestMemory) -> bool {
        // A crawl may only deliver into the main-menu scenario, even if
        // a new map reused a screen address between the tick and read.
        if self.phase == Some(Phase::Menu) {
            use live_telemetry::layout::{GAME_GLOBALS_PTR, OPTIONS, OPTIONS_TYPE};
            let mode = mem
                .u32(GAME_GLOBALS_PTR)
                .ok()
                .filter(|p| *p != 0)
                .and_then(|p| p.checked_add(OPTIONS + OPTIONS_TYPE))
                .and_then(|addr| mem.u32(addr).ok());
            if mode != Some(3) {
                return false;
            }
        }
        self.check_owner(mem).is_ok()
    }
}

pub(crate) fn install(agent: &Agent, pad: agent::MenuPad) {
    agent.add_game_method(method::UI_PRESS, Access::Act, move |params, ctx| {
        let p: Press = serde_json::from_value(params)
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
        if ctx.at.boundary != Boundary::Tick || ctx.at.paused {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "requires a running game tick",
            ));
        }
        p.check_phase(ctx.at.phase)?;
        ctx.read_guest(|mem| p.check_owner(mem))??;
        let release = PadStep {
            ms: Some(PRESS_MS),
            reads: Some(PRESS_READS),
            ..Default::default()
        };
        let press = PadStep {
            buttons: vec![p.action.button().into()],
            ..release.clone()
        };
        let result = json!({
            "action": p.action, "target": p.target, "row": p.row, "player": 0, "delivered": true,
        });
        let receipt = pad.queue(ctx.conn, &[press, release], move |memory| {
            p.still_receives(&PadMemory(memory))
        })?;
        ctx.defer_for(
            agent::DEFER_TICKS_MAX,
            agent::MenuPad::TIMEOUT,
            Box::new(move |_| receipt.result().map(|done| done.map(|()| result.clone()))),
        );
        Ok(serde_json::Value::Null)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(row: Option<u32>) -> Target {
        Target {
            token: "synthetic-menu".into(),
            id: 9,
            screen: 0x2000,
            focused_row: row,
        }
    }

    #[test]
    fn choose_requires_the_observed_target_and_focused_row() {
        let mut p = Press {
            action: Action::Choose,
            target: "synthetic-menu".into(),
            row: None,
            phase: None,
        };
        assert_eq!(
            p.check(&owner(Some(2)), None).unwrap_err().code,
            code::INVALID_PARAMS
        );
        p.row = Some(2);
        assert!(p.check(&owner(Some(2)), None).is_ok());
        assert_eq!(p.check(&owner(Some(3)), None).unwrap_err().code, code::GONE);
        assert_eq!(p.check(&owner(None), None).unwrap_err().code, code::GONE);
        p.target = "other-menu".into();
        assert_eq!(p.check(&owner(Some(2)), None).unwrap_err().code, code::GONE);
    }

    #[test]
    fn back_needs_no_list_and_live_continue_checks_the_join_mode() {
        let mut p = Press {
            action: Action::Back,
            target: "synthetic-menu".into(),
            row: None,
            phase: None,
        };
        let mut t = owner(None);
        assert!(p.check(&t, None).is_ok());
        assert_eq!(p.action.button(), "b");
        p.action = Action::OpenLiveMenu;
        assert!(p.check(&t, Some(MODE_LIVE)).is_err());
        t.id = crate::ui_watch::JOIN_SCREEN;
        assert!(p.check(&t, Some(4)).is_err());
        assert!(p.check(&t, None).is_err());
        assert!(p.check(&t, Some(MODE_LIVE)).is_ok());
    }

    #[test]
    fn change_mode_is_x_and_needs_the_lobby() {
        let p: Press = serde_json::from_value(json!({
            "action": "change_mode", "target": "synthetic-menu"
        }))
        .unwrap();
        assert_eq!(p.action.button(), "x");
        let mut t = owner(None);
        assert_eq!(p.check(&t, None).unwrap_err().code, code::GONE);
        t.id = crate::lobby_options::SCREEN;
        assert!(p.check(&t, None).is_ok(), "no row: X is the lobby's, not a row's");
        // The bumper actions are gone with the strip.
        for gone in ["tab_prev", "tab_next"] {
            assert!(
                serde_json::from_value::<Press>(json!({"action": gone, "target": "x"})).is_err(),
                "{gone}"
            );
        }
    }

    #[test]
    fn menu_actions_reject_raw_buttons_unknown_fields_and_missing_targets() {
        for params in [
            json!({"action":"start", "target":"example"}),
            json!({"action":"back", "target":"example", "player":1}),
            json!({"action":"back"}),
        ] {
            assert!(serde_json::from_value::<Press>(params).is_err());
        }
    }

    #[test]
    fn crawl_requires_menu_but_unguarded_owners_can_be_in_game() {
        let mut p: Press = serde_json::from_value(json!({
            "action": "back", "target": "synthetic-menu", "phase": "menu"
        }))
        .unwrap();
        assert!(p.check_phase(Phase::Menu).is_ok());
        assert_eq!(p.check_phase(Phase::Gameplay).unwrap_err().code, code::GONE);
        p.phase = None;
        assert!(p.check_phase(Phase::Gameplay).is_ok());
        assert_eq!(
            p.check_phase(Phase::Loading).unwrap_err().code,
            code::UNAVAILABLE
        );
    }
}
