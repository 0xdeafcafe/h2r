//! The Game panel and the controls shared with the bar and Game menu.

mod toasts;

use std::path::PathBuf;

use agent_proto::Endpoint;
use dear_imgui_rs::{Key, TreeNodeFlags, Ui, WindowKey};

use crate::app::App;
use crate::play::pair::{Request, relation_text};
use crate::play::{self, LinkState, LinkTone, LinkView, Play, Spawn, launch, phase_label};
use crate::theme::{self, icon};

const LINK_POPUP: &str = "##game-link-popup";

/// The panel's state around [`Play`].
pub struct PlayUi {
    pub play: Play,
    sockets: Vec<PathBuf>,
    step_ticks: i32,
    open_link_popup: bool,
    picking_game: bool,
}

impl Default for PlayUi {
    fn default() -> Self {
        Self {
            play: Play::discovering(),
            sockets: launch::discover(),
            step_ticks: 1,
            open_link_popup: false,
            picking_game: false,
        }
    }
}

impl PlayUi {
    pub fn update(&mut self, app: &mut App) {
        self.play.update(app);
    }

    /// F5 plays, Shift+F5 stops the game this editor started.
    pub fn shortcuts(&mut self, ui: &Ui) {
        if ui.io().want_text_input() || !ui.is_key_pressed_with_repeat(Key::F5, false) {
            return;
        }
        self.play.request(if ui.io().key_shift() {
            Request::Stop
        } else {
            Request::Play
        });
    }

    pub fn menu(&mut self, ui: &Ui, app: &mut App) {
        let Some(_menu) = ui.begin_menu("Game") else {
            return;
        };
        let view = self.play.displayed_link(app);
        for (label, key, enabled, request) in [
            ("Play", "F5", view.can_play, Request::Play),
            ("Stop", "Shift+F5", view.can_stop, Request::Stop),
        ] {
            if ui.menu_item_enabled_selected_with_shortcut(label, key, false, enabled) {
                self.play.request(request);
            }
        }
        ui.separator();
        let found = self.play.notice(app);
        let can_pair = found.is_some() || self.play.retry_endpoint().is_some();
        if ui.menu_item_enabled_selected_no_shortcut("Pair with the running game", false, can_pair)
            && !self.pair_offer(found)
        {
            self.open_link_popup = true;
        }
        if ui.menu_item_enabled_selected_no_shortcut("Unpair", false, view.paired) {
            self.play.request(Request::Unpair);
        }
        ui.separator();
        if ui.menu_item_enabled_selected_no_shortcut(
            "Pause the game while I edit",
            app.pairing.pause_while_editing,
            true,
        ) {
            app.pairing.pause_while_editing = !app.pairing.pause_while_editing;
        }
        let aim = app.pairing.selection.enabled;
        if ui.menu_item_enabled_selected_no_shortcut("Aim select", aim, Play::can_aim_select(app))
            && let Err(why) = Play::set_aim_select(app, !aim)
        {
            app.error(why);
        }
        let (label, request) = if view.paused {
            ("Resume", Request::Resume)
        } else {
            ("Pause", Request::Pause)
        };
        if ui.menu_item_enabled_selected_no_shortcut(
            label,
            false,
            view.controls.can_toggle_pause(view.paused),
        ) {
            self.play.request(request);
        }
        if ui.menu_item_enabled_selected_no_shortcut(
            "Step",
            false,
            view.paused && view.controls.step.is_none(),
        ) {
            self.play.request(Request::Step(self.step_ticks as u64));
        }
        if ui.menu_item_enabled_selected_no_shortcut(
            "Edit",
            self.play.live_edits,
            view.state.can_edit(),
        ) {
            self.play.request(Request::Edit(!self.play.live_edits));
        }
    }

    /// Pair immediately when there is a single offer. With several games
    /// the popover lets the user choose.
    fn pair_offer(&mut self, offer: Option<play::pair::Notice>) -> bool {
        if let Some(socket) = offer.and_then(|n| n.socket) {
            self.play.request(Request::Pair(Endpoint::Unix(socket)));
            true
        } else {
            false
        }
    }

    pub fn bar_pill(&mut self, ui: &Ui, app: &mut App, view: &LinkView, clicked: bool) {
        let pick = self.play.picking_game();
        let open = std::mem::take(&mut self.open_link_popup) || (pick && !self.picking_game);
        self.picking_game = pick;
        self.link_popup(ui, app, view, open, clicked);
        if pick && !open && !ui.is_popup_open(LINK_POPUP) {
            self.play.cancel_picker();
        }
        const ASK: &str = "Playtest";
        if app.pairing.prompt.is_some() && !ui.is_popup_open(ASK) {
            ui.open_popup(ASK);
        }
        if let Some(_modal) = ui.begin_modal_popup_config(ASK).begin() {
            let Some(prompt) = app.pairing.prompt.clone() else {
                ui.close_current_popup();
                return;
            };
            let (message, yes, no, alternative) = match prompt.ask {
                play::playtest::Ask::Send => (
                    "Send your edits to this game?", "Send edits".into(), "Cancel".into(), "cancel",
                ),
                play::playtest::Ask::Load => (
                    "The game is running another map.",
                    format!("Load {} in the game", app.loaded_map().unwrap_or("map")),
                    format!("Open {} here", self.play.map().unwrap_or("map")),
                    "open",
                ),
            };
            ui.text(message);
            let choice = if ui.button(yes) {
                Some(prompt.ask.name())
            } else {
                ui.same_line();
                ui.button(no).then_some(alternative)
            };
            if let Some(choice) = choice {
                self.play.request(Request::PlayAnswer(prompt, choice.into()));
                ui.close_current_popup();
            }
        }
    }

    fn pill(&mut self, ui: &Ui, app: &mut App, view: &LinkView, open: bool) {
        let clicked = theme::pill(
            ui,
            theme::link_colour(view.tone),
            link_icon(view.tone),
            &view.text,
        );
        if let Some(why) = &view.why {
            ui.set_item_tooltip(why);
        }
        self.link_popup(ui, app, view, open, clicked);
    }

    fn link_popup(&mut self, ui: &Ui, app: &mut App, view: &LinkView, open: bool, clicked: bool) {
        let pair_now = clicked
            && matches!(view.state, LinkState::Found { .. })
            && self.pair_offer(self.play.notice(app));
        if open || (clicked && !pair_now) {
            ui.open_popup(LINK_POPUP);
        }
        let Some(_popup) = ui.begin_popup(LINK_POPUP) else {
            return;
        };

        match &view.state {
            LinkState::Connecting | LinkState::Reconnecting { .. } if ui.button("Cancel") => {
                self.play.request(Request::Unpair);
                ui.close_current_popup();
            }
            LinkState::Lost { .. } => {
                if let Some(endpoint) = self.play.retry_endpoint().cloned()
                    && ui.button("Pair again")
                {
                    self.play.request(Request::PairSocket(endpoint));
                    ui.close_current_popup();
                }
            }
            LinkState::OtherMap { game, .. } if ui.button(format!("Open {game}")) => {
                self.play.open_game_map(app);
                ui.close_current_popup();
            }
            _ => {}
        }

        for reason in &view.unavailable {
            ui.text_wrapped(reason);
        }

        let games = self.play.finder.running();
        if games.is_empty() && self.play.endpoint().is_none() {
            ui.text_disabled("No games found");
        }
        let mut selected = None;
        for game in games {
            let Some(status) = &game.status else { continue };
            let socket = game.path.to_string_lossy();
            let _id = ui.push_id(socket.as_ref());
            let endpoint = game.endpoint();
            let current = self.play.endpoint() == Some(&endpoint);
            let map = status.at.map.as_deref().unwrap_or("No map yet");
            ui.text(format!(
                "{map} · {}",
                phase_label(status.at.phase, status.at.paused)
            ));
            ui.set_item_tooltip(format!(
                "{socket}\n{}",
                relation_text(
                    play::pair::map_match(app, game.map()),
                    game.map(),
                    app.loaded_map()
                ),
            ));
            ui.same_line();
            if current {
                ui.text_disabled(if view.paired {
                    "Paired"
                } else {
                    "Connecting"
                });
            } else if ui.small_button("Pair") {
                selected = Some(endpoint);
            }
        }
        if let Some(endpoint) = selected {
            self.play.request(Request::Pair(endpoint));
            ui.close_current_popup();
        }
        ui.separator();
        if view.paired && ui.button("Unpair") {
            self.play.request(Request::Unpair);
            ui.close_current_popup();
        }
        ui.checkbox("Auto-pair", &mut app.pairing.auto_pair);
        ui.set_item_tooltip("Pairs in View mode with the only game running or the only game on the open map.");
    }

    pub fn edit_switch(&mut self, ui: &Ui, view: &LinkView) {
        let _disabled = ui.begin_disabled_with_cond(!view.state.can_edit());
        let on = self.play.live_edits;
        if theme::edit_switch(ui, on) {
            self.play.request(Request::Edit(!on));
        }
        ui.set_item_tooltip(if on {
            "Return to View mode"
        } else {
            "Send tag and placement edits to the paired game"
        });
    }

    pub fn step(&mut self) {
        self.play.request(Request::Step(self.step_ticks as u64));
    }

    pub fn window(&mut self, ui: &Ui, app: &mut App, key: &WindowKey) {
        ui.window(key).build(|| {
            let view = self.play.displayed_link(app);
            self.pill(ui, app, &view, false);
            theme::section(ui, "Session");
            self.session(ui, app, &view);
            theme::section(ui, "Player");
            self.player(ui, app, &view);
            if ui.collapsing_header("Advanced", TreeNodeFlags::NONE) {
                self.advanced(ui, app);
            }
        });
    }

    fn session(&mut self, ui: &Ui, app: &mut App, view: &LinkView) {
        if view.paired {
            self.edit_switch(ui, view);
        }
        let p = &self.play;
        if view.paired {
            if let Some(at) = p.status.as_ref().map(|s| &s.at) {
                let kind = p
                    .session
                    .map_or("Waiting for session kind", play::session_label);
                ui.text(format!(
                    "{} · {} · tick {} · {kind}",
                    at.map.as_deref().unwrap_or("No map yet"),
                    phase_label(at.phase, at.paused),
                    at.tick.unwrap_or(at.ticks),
                ));
            }
            if let Some(stream) = &app.pairing.mirror {
                let meta = stream.status(app.pairing.latest || view.paused);
                if let (Some(fps), Some(ms)) = (meta["fps"].as_f64(), meta["delay_ms"].as_f64()) {
                    let counts = app.pairing.counters.as_ref().map_or_else(String::new, |c| {
                        format!(" · {} dropped, {} skipped", c["dropped"], c["skipped"])
                    });
                    ui.text(format!("{fps:.0} Hz · {ms:.0} ms delay{counts}"));
                }
                if let Some(error) = stream.error() {
                    ui.text_colored(theme::WARN, error);
                } else if !view.paused && app.pairing.shown.as_ref().is_some_and(|s| s.stale) {
                    ui.text_disabled("Waiting for live frames");
                }
            }
            let mut live = app.layers.visible(scenario_edit::layers::LayerId::Live);
            if ui.checkbox("Show live objects in the viewport", &mut live) {
                let mut layers = app.layers.clone();
                layers.set(scenario_edit::layers::LayerId::Live, live);
                app.dispatch(crate::app::Action::SetLayers(layers));
            }
            ui.checkbox("Latest (no interpolation)", &mut app.pairing.latest);
            if !p.reload.is_empty() {
                ui.text_colored(theme::WARN, format!("Needs Play again: {:?}", p.reload));
            }
        }
    }

    fn player(&mut self, ui: &Ui, app: &mut App, view: &LinkView) {
        if view.paired {
            if ui.button(format!("{} Go to player", icon::CROSSHAIR)) {
                self.play.go_to_player(app);
            }
            ui.same_line();
            if ui.button(format!("{} Teleport player here", icon::MOVE)) {
                self.play.teleport_to_camera(app);
            }
            ui.set_item_tooltip("In Edit, move to the ground under the camera focus. T uses the cursor. Shift+T also matches the editor camera’s facing.");
        }
        ui.set_next_item_width(180.0);
        if let Some(_combo) = ui.begin_combo("Spawn", self.play.spawn_choice(app).label()) {
            for spawn in Spawn::ALL {
                if ui
                    .selectable_config(spawn.label())
                    .selected(spawn == self.play.spawn_choice(app))
                    .build()
                {
                    app.pairing.spawn = Some(spawn);
                }
            }
        }
        ui.set_item_tooltip("Where Play starts the player.");
    }

    fn advanced(&mut self, ui: &Ui, app: &mut App) {
        let p = &mut self.play;
        ui.set_next_item_width(-1.0);
        ui.input_text("##agent-socket", &mut p.socket_text)
            .hint("socket path or tcp://HOST:PORT")
            .build();
        if ui.button("Pair") {
            p.pair_socket_text(app);
        }
        ui.same_line();
        if ui.button(format!("{} Find", icon::SEARCH)) {
            self.sockets = launch::discover();
            p.finder.look_again();
        }
        for socket in &self.sockets {
            let path = socket.to_string_lossy();
            if ui
                .selectable_config(path.as_ref())
                .selected(p.socket_text == path.as_ref())
                .build()
            {
                p.socket_text = path.into_owned();
            }
        }

        let found = launch::find_h2(Some(PathBuf::from(p.h2_path.trim())).as_deref());
        ui.set_next_item_width(-1.0);
        ui.input_text("##h2-binary", &mut p.h2_path)
            .hint(found.map_or_else(
                || "h2 binary".into(),
                |path| format!("h2: {}", path.display()),
            ))
            .build();
        ui.set_item_tooltip(
            "The h2 binary Play runs. Empty uses $H2_BIN, the h2 beside the editor (target/<profile>/h2), ~/.cargo/bin/h2, h2 on the PATH, or the lab host's copy.",
        );
        if let Some(log) = &p.log_path {
            ui.text_wrapped(format!("Launch log: {}", log.display()));
            if let Some(cmd) = &p.last_command {
                ui.set_item_tooltip(cmd);
            }
        }
        ui.set_next_item_width(100.0);
        ui.input_int("Step ticks", &mut self.step_ticks);
        self.step_ticks = self.step_ticks.clamp(1, 600);
    }

    /// T over the viewport teleports the player to the point under the cursor.
    pub fn overlay(&mut self, ui: &Ui, app: &mut App, rect: Option<[f32; 4]>) {
        let Some([x, y, w, h]) = rect else { return };
        toasts::draw(ui, &self.play, [x, y, w, h]);
        let view = self.play.displayed_link(app);
        if matches!(
            view.state,
            LinkState::Editing { .. } | LinkState::Blocked { .. }
        ) && w > theme::LIVE_FRAME
            && h > theme::LIVE_FRAME
        {
            let inset = theme::LIVE_FRAME * 0.5;
            ui.get_window_draw_list()
                .add_rect(
                    [x + inset, y + inset],
                    [x + w - inset, y + h - inset],
                    theme::link_colour(view.tone),
                )
                .thickness(theme::LIVE_FRAME)
                .build();
        }
        let p = &self.play;
        if !p.is_paired() {
            return;
        }
        let m = ui.io().mouse_pos();
        let inside = m[0] >= x && m[0] < x + w && m[1] >= y && m[1] < y + h;
        if inside
            && ui.is_window_hovered()
            && !ui.io().want_text_input()
            && !ui.io().key_ctrl()
            && !ui.io().key_super()
            && ui.is_key_pressed_with_repeat(Key::T, false)
        {
            match Play::cursor_destination(app) {
                Some(q) => self.play.player_here(app, q, ui.io().key_shift()),
                None => self.play.game_refusal(app, "no ground under the cursor to teleport to"),
            }
        }
    }
}

pub fn link_icon(tone: LinkTone) -> &'static str {
    match tone {
        LinkTone::View => icon::EYE,
        LinkTone::Live => icon::ZAP,
        LinkTone::Warn | LinkTone::Error => icon::ALERT,
        LinkTone::Idle | LinkTone::Notice => icon::INFO,
    }
}

impl PlayUi {
    pub fn pending_window(&mut self, ui: &Ui, app: &mut App, key: &WindowKey) {
        use crate::play::Apply;
        let p = &mut self.play;
        let mut action = None;
        let focus = std::mem::take(&mut app.pairing.open_pending);
        ui.window(key).focused(focus).build(|| {
            if !app.pairing.journal.is_empty() {
                ui.separator_with_text("Already in the game");
                for c in &app.pairing.journal {
                    ui.text(format!("{}  {}  {}", c["tag"], c["path"], c["holds"]));
                }
                if ui.button("Adopt game baseline") {
                    action = Some(Apply::Adopt);
                }
                ui.same_line();
                if ui.button("Revert game journal") {
                    action = Some(Apply::Revert);
                }
            }
            if let Some(scope) = app.pairing.peer_confirmation {
                if ui.button(format!(
                    "Confirm edits visible only on this host ({} other machines)",
                    scope.peers - 1
                )) {
                    action = Some(Apply::Confirm(scope));
                }
            }
            let move_refusal = p.refused_move_at(std::time::Instant::now());
            if let Some(why) = move_refusal {
                ui.separator_with_text("Player move refused");
                ui.text_wrapped(why);
            }
            // Grouped under the effect sentences.
            let mut heading = "";
            let mut has_reload = false;
            for r in app.pairing.pending.sorted() {
                let effect = crate::play::tag_sync::Effect::of_row(r);
                let next = effect.map_or("Waiting.", |e| e.sentence());
                has_reload |= effect == Some(crate::play::tag_sync::Effect::Reload);
                if heading != next {
                    ui.separator_with_text(next);
                    heading = next;
                }
                ui.text_wrapped(format!(
                    "{} · {}",
                    r["label"].as_str().or(r["path"].as_str()).unwrap_or("?"),
                    r["why"].as_str().unwrap_or("")
                ));
                if r["conflict"] == true && r["resolvable"] != false
                    && crate::play::tag_sync::check_conflict_path(&app.tags, r).is_ok()
                    && let Some(at) = r["at"].as_u64()
                {
                    let _id = ui.push_id(at as usize);
                    if ui.button("Take the game's value") {
                        action = Some(Apply::Take(at as u32));
                    }
                    ui.same_line();
                    if ui.button("Overwrite") {
                        action = Some(Apply::Overwrite(at as u32));
                    }
                }
            }
            if app.pairing.pending.is_empty() {
                if move_refusal.is_none() {
                    ui.text_disabled("No pending edits");
                }
            } else {
                if let Some(why) = p.edits_blocked(app) {
                    ui.text_wrapped(why);
                }
                if ui.button("Send") {
                    action = Some(Apply::Send);
                }
                if has_reload {
                    ui.same_line();
                    if ui.button("Reload map") {
                        action = Some(Apply::Reload);
                    }
                }
            }
            if p.can_respawn() && ui.button("Respawn now") {
                action = Some(Apply::Respawn);
            }
        });
        const LEAVE: &str = "Unpair from the game?";
        if app.pairing.leaving && !ui.is_popup_open(LEAVE) {
            ui.open_popup(LEAVE);
        }
        if let Some(_modal) = ui.begin_modal_popup_config(LEAVE).begin() {
            if ui.button("Keep the game as it is") {
                action = Some(Apply::Keep);
            }
            if ui.button("Restore journalled fields") {
                action = Some(Apply::Restore);
            }
            if action.is_some() || !app.pairing.leaving {
                ui.close_current_popup();
            }
        }
        if let Some(action) = action {
            p.request(Request::Apply(action));
        }
    }
}
