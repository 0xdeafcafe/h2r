//! The Game panel (Play, pair, live edits, the player, pause and step),
//! the live objects drawn over the viewport, and the status chip. The
//! state and every command are `crate::play::Play`'s; this only draws it.

use std::path::PathBuf;

use dear_imgui_rs::{Key, Ui, WindowKey};
use scenario_edit::geometry::Vec3;

use crate::app::App;
use crate::play::{self, Conn, Play, Spawn, launch};
use crate::theme::{self, icon};

/// The panel's state around [`Play`].
pub struct PlayUi {
    pub play: Play,
    /// Sockets found by the last look.
    sockets: Vec<PathBuf>,
    step_ticks: i32,
    show_objects: bool,
}

impl Default for PlayUi {
    fn default() -> Self {
        Self {
            play: Play::new(),
            sockets: launch::discover(),
            step_ticks: 1,
            show_objects: true,
        }
    }
}

/// Live object colours by kind.
fn color(kind: &str) -> [f32; 4] {
    match kind {
        "player" => [0.35, 0.95, 0.45, 1.0],
        "actor" => [0.95, 0.45, 0.35, 1.0],
        "vehicle" => [0.4, 0.7, 1.0, 1.0],
        "machine" => [0.75, 0.75, 0.8, 0.8],
        _ => [0.55, 0.55, 0.55, 0.6],
    }
}

impl PlayUi {
    /// Once a frame, whatever tool is shown: the link's news and edits.
    pub fn update(&mut self, app: &mut App) {
        self.play.update(app);
    }

    /// F5 plays, Shift+F5 stops the game this editor started.
    pub fn shortcuts(&mut self, ui: &Ui, app: &mut App) {
        if ui.io().want_text_input() || !ui.is_key_pressed(Key::F5) {
            return;
        }
        if ui.io().key_shift() {
            self.play.stop_game(app);
        } else {
            self.play.launch(app);
        }
    }

    /// The status bar's chip, and its colour.
    pub fn chip(&self) -> (String, [f32; 4]) {
        let c = match self.play.conn {
            Conn::Connected => theme::OK,
            Conn::Connecting => theme::WARN,
            Conn::Lost(_) => theme::ERROR,
            Conn::Off => theme::DIM,
        };
        (self.play.chip(), c)
    }

    /// The Game panel.
    pub fn window(&mut self, ui: &Ui, app: &mut App, key: &WindowKey) {
        ui.window(key).build(|| self.panel(ui, app));
    }

    fn panel(&mut self, ui: &Ui, app: &mut App) {
        let (chip, c) = self.chip();
        let p = &mut self.play;
        ui.text_colored(c, chip);
        if let Conn::Lost(why) = &p.conn {
            ui.text_colored(theme::ERROR, why);
        }

        // Play.
        ui.separator();
        let can_play = app.loaded_map().is_some();
        {
            let d = ui.begin_disabled_with_cond(!can_play);
            if ui.button(format!("{} Play (F5)", icon::ZAP)) {
                p.launch(app);
            }
            d.end();
        }
        if !can_play {
            ui.set_item_tooltip("Open a map to play it.");
        }
        ui.same_line();
        let running = p.game_running();
        {
            let d = ui.begin_disabled_with_cond(!running);
            if ui.button(format!("{} Stop (Shift+F5)", icon::CLOSE)) {
                p.stop_game(app);
            }
            d.end();
        }
        ui.set_next_item_width(180.0);
        if let Some(_c) = ui.begin_combo("Spawn", p.spawn.label()) {
            for s in Spawn::ALL {
                if ui
                    .selectable_config(s.label())
                    .selected(s == p.spawn)
                    .build()
                {
                    p.spawn = s;
                }
            }
        }
        let found = launch::find_h2(Some(PathBuf::from(p.h2_path.trim())).as_deref());
        ui.set_next_item_width(-1.0);
        ui.input_text("##h2-binary", &mut p.h2_path)
            .hint(
                found
                    .as_ref()
                    .map(|f| format!("h2: {}", f.display()))
                    .unwrap_or_else(|| "h2 binary (not found: ~/.cargo/bin/h2, $H2_BIN)".into()),
            )
            .build();
        if ui.is_item_hovered() {
            ui.tooltip_text(
                "The h2 binary Play runs. Empty: $H2_BIN, else ~/.cargo/bin/h2, else h2 on the PATH.",
            );
        }
        if let Some(l) = &p.log_path {
            ui.text_disabled(format!("log: {}", l.display()));
        }
        if let Some(cmd) = &p.last_command
            && ui.is_item_hovered()
        {
            ui.tooltip_text(cmd);
        }

        // Pair.
        ui.separator();
        ui.text("Pair with a running game");
        ui.set_next_item_width(-60.0);
        ui.input_text("##agent-socket", &mut p.socket_text)
            .hint("socket path or tcp://HOST:PORT")
            .build();
        ui.same_line();
        if ui.small_button("Pair") {
            pair_text(p, app);
        }
        if ui.small_button(format!("{} Find", icon::SEARCH)) {
            self.sockets = launch::discover();
        }
        for s in &self.sockets {
            let shown = s.display().to_string();
            if ui
                .selectable_config(format!("{shown}##sock-{shown}"))
                .selected(p.endpoint().is_some_and(|e| e.to_string() == shown))
                .build()
            {
                p.socket_text = shown;
                pair_text(p, app);
            }
        }
        if p.endpoint().is_some() && ui.small_button("Unpair") {
            p.unpair(app);
        }

        if !p.is_paired() {
            return;
        }

        // While paired.
        ui.separator();
        ui.checkbox("Send edits live", &mut p.live_edits);
        match p.edits_blocked(app) {
            None => ui.text_colored(
                theme::OK,
                "Edits go to the game's scenario as you make them.",
            ),
            Some(why) => ui.text_colored(theme::WARN, format!("Edits stay here: {why}")),
        }
        ui.text_wrapped(
            "A placement edit shows when its object is made again (a respawn, a checkpoint \
             revert, or Play again); objects already in the world don't move.",
        );
        if !p.reload.is_empty() {
            ui.text_colored(
                theme::WARN,
                format!(
                    "Needs Play again (entries added or removed): {:?}",
                    p.reload
                ),
            );
        }
        ui.checkbox("Show live objects in the viewport", &mut self.show_objects);
        ui.text_disabled(format!(
            "{} objects{}",
            p.objects.len(),
            p.player()
                .map(|pl| format!(
                    ", player at ({:.1}, {:.1}, {:.1})",
                    pl.pos[0], pl.pos[1], pl.pos[2]
                ))
                .unwrap_or_default()
        ));
        if ui.button(format!("{} Go to player", icon::CROSSHAIR)) {
            p.go_to_player(app);
        }
        ui.same_line();
        if ui.button(format!("{} Teleport player here", icon::MOVE)) {
            p.teleport_to_camera(app);
        }
        if ui.is_item_hovered() {
            ui.tooltip_text(
                "Moves the player to the ground under the camera's focus. Over the viewport, \
                 T moves them to the point under the cursor. Refused in co-op and as a client.",
            );
        }

        ui.separator();
        if p.paused {
            if ui.button("Resume") {
                p.resume();
            }
        } else if ui.button("Pause") {
            p.pause();
        }
        if ui.is_item_hovered() {
            ui.tooltip_text(
                "Claims control of the game while paused; Resume gives it back. A pause \
                 resumes by itself after 10 minutes. In h2 play it also stops the window.",
            );
        }
        ui.same_line();
        if ui.button("Step") {
            p.step(self.step_ticks.max(1) as u64);
        }
        ui.same_line();
        ui.set_next_item_width(80.0);
        ui.input_int("ticks", &mut self.step_ticks);
        self.step_ticks = self.step_ticks.clamp(1, 600);
    }

    /// The live objects over the viewport image (`rect`: origin and size),
    /// and T over it to teleport the player to the point under the cursor.
    pub fn overlay(&mut self, ui: &Ui, app: &mut App, rect: Option<[f32; 4]>) {
        let Some([x, y, w, h]) = rect else { return };
        let p = &self.play;
        if !p.is_paired() {
            return;
        }
        let aspect = w / h.max(1.0);
        let cam = *app.session.camera();
        let at = |q: Vec3| {
            cam.project(q, aspect)
                .filter(|uv| (-0.1..=1.1).contains(&uv[0]) && (-0.1..=1.1).contains(&uv[1]))
                .map(|uv| [x + uv[0] * w, y + uv[1] * h])
        };
        if self.show_objects {
            let dl = ui.get_window_draw_list();
            let _clip = dl.push_clip_rect([x, y], [x + w, y + h], true);
            for o in &p.objects {
                let Some(s) = at(o.pos) else { continue };
                let c = color(&o.kind);
                let r = if o.kind == "player" { 7.0 } else { 5.0 };
                dl.add_circle(s, r, c).filled(true).build();
                dl.add_circle(s, r + 1.5, [0.0, 0.0, 0.0, 0.8])
                    .thickness(1.5)
                    .build();
                if let Some(f) = o.forward
                    && let Some(e) = at([o.pos[0] + f[0], o.pos[1] + f[1], o.pos[2] + f[2]])
                {
                    dl.add_line(s, e, c).thickness(2.0).build();
                }
                if o.kind == "player" {
                    let label = o.name.clone().unwrap_or_else(|| o.id.clone());
                    dl.add_text([s[0] + 9.0, s[1] - 8.0], c, label);
                }
            }
        }
        let m = ui.io().mouse_pos();
        let inside = m[0] >= x && m[0] < x + w && m[1] >= y && m[1] < y + h;
        if inside
            && ui.is_window_hovered()
            && !ui.io().want_text_input()
            && !ui.io().key_ctrl()
            && !ui.io().key_super()
            && ui.is_key_pressed(Key::T)
        {
            match play::surface_at(app, (m[0] - x) / w, (m[1] - y) / h, aspect) {
                Some(q) => self.play.teleport(app, q),
                None => app.error("no ground under the cursor to teleport to"),
            }
        }
    }
}

fn pair_text(p: &mut Play, app: &mut App) {
    let text = p.socket_text.trim().to_owned();
    let text = if text.is_empty() {
        agent_proto::default_socket().display().to_string()
    } else {
        text
    };
    match agent_proto::Endpoint::parse(&text) {
        Ok(e) => p.pair(app, e),
        Err(e) => app.error(format!("pair: {e}")),
    }
}

/// Draws the chip in its colour.
pub fn status_chip(ui: &Ui, chip: &(String, [f32; 4])) {
    ui.text_colored(chip.1, &chip.0);
}
