//! Optional game features advertised by the current pairing's `session.methods`.

use std::collections::HashSet;

use agent_proto::{RpcError, editor, hot, live, sim};
use serde_json::Value;

use super::{App, Conn, Play};

const FEATURES: &[(&str, &str)] = &[
    (hot::method::EDIT_MAP, "Checking the game's map identity"),
    (hot::method::EDIT_CHANGES, "Checking the edit journal"),
    (hot::method::EDIT_BATCH, "Sending field edits"),
    (hot::method::EDIT_REVERT, "Reverting edits in the game"),
    (hot::method::EDIT_GET, "Taking a placement value from the game"),
    (hot::method::EDIT_READ, "Taking tag bytes from the game"),
    (live::SUBSCRIBE, "Showing live objects and players"),
    (live::STATUS, "Refreshing stream counters and map identity"),
    (sim::method::STATE_SET, "Moving the player to an editor position"),
    (sim::method::MAP_RELOAD, "Loading a play copy in the running game"),
    (sim::method::OBJECT_RESPAWN, "Respawning an edited placement"),
    (editor::method::FOCUS, "Focusing the game window"),
];

impl Play {
    pub(super) fn has_method(&self, name: &str) -> bool {
        self.methods
            .as_ref()
            .is_some_and(|methods| methods.contains(name))
    }

    pub(super) fn require_method(&self, name: &str) -> Result<(), String> {
        if self.methods.is_none() {
            return Err("waiting for the game's method list".into());
        }
        if !self.has_method(name) {
            return Err(format!("the game does not offer {name}"));
        }
        Ok(())
    }

    pub(super) fn unavailable_features(&self) -> Vec<String> {
        if !self.is_paired() || self.methods.is_none() {
            return Vec::new();
        }
        FEATURES
            .iter()
            .filter(|(method, _)| !self.has_method(method))
            .map(|(method, feature)| format!("{feature} requires {method}."))
            .collect()
    }

    pub(super) fn received_methods(&mut self, app: &mut App, result: Result<Value, RpcError>) {
        let methods = match result {
            Ok(value) => serde_json::from_value(value).unwrap_or_else(|_| {
                app.error("session.methods returned an unreadable method list");
                HashSet::new()
            }),
            Err(e) => {
                app.error(format!("session.methods: {e}"));
                HashSet::new()
            }
        };
        self.methods = Some(methods);
        if self.has_method(live::SUBSCRIBE) {
            if app.pairing.mirror.is_none() {
                app.pairing.mirror = self.endpoint().cloned().map(live_mirror::Stream::open);
            }
        } else {
            app.pairing.drop_stream();
        }
        // A status-only host can reconnect, but cannot verify retained edits.
        if !self.has_method(hot::method::EDIT_MAP)
            && matches!(self.conn, Conn::Reconnecting { .. })
        {
            self.conn = Conn::Connected;
            self.live_edits = false;
            self.playtest_consent = false;
            self.edits.own_start = false;
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use agent_proto::{Endpoint, method, sim::SessionKind};
    use serde_json::json;

    use super::*;
    use crate::app::Action;
    use crate::play::link::{Event, Purpose};
    use crate::play::testing::{Temp, fake};
    use crate::play::Apply;

    #[test]
    fn mirror_waits_for_an_advertised_subscription() {
        let game = fake::Game::start("advertised-stream", "example", SessionKind::Local);
        let mut play = Play::new();
        let mut app = App::new();
        play.retry = Some(Endpoint::Unix(game.path.clone()));
        play.on_event(&mut app, Event::Connected);
        assert!(app.pairing.mirror.is_none());

        play.received_methods(&mut app, Ok(json!([live::STATUS])));
        assert!(app.pairing.mirror.is_none());
        assert!(
            play.link_view(&app)
                .unavailable
                .iter()
                .any(|reason| reason.contains(live::SUBSCRIBE))
        );

        play.received_methods(&mut app, Ok(json!([live::SUBSCRIBE])));
        assert!(app.pairing.mirror.is_some());
        assert!(!play.has_method(live::STATUS));
    }

    #[test]
    fn missing_methods_disable_only_the_features_that_need_them() {
        let temp = Temp::new("partial-methods");
        let game = fake::Game::start("partial-methods", "example", SessionKind::Local);
        let mut app = App::new();
        app.set_data_root(temp.0.join("data"));
        app.dispatch(Action::OpenSource(temp.map("example")));
        let mut play = Play::new();
        fake::pair_for_view(&mut play, &mut app, &game);

        let mut methods = play.methods.clone().unwrap();
        assert!(methods.remove(hot::method::EDIT_BATCH));
        play.received_methods(&mut app, Ok(json!(methods)));
        assert!(play.map_refusal(&app).is_none());
        assert!(play.session_refusal(&app).is_none());
        let why = play.apply_edits(&mut app, Apply::Send).unwrap_err();
        assert!(why.contains(hot::method::EDIT_BATCH));
        assert!(play.has_method(sim::method::MAP_RELOAD));

        let journals = Arc::new(AtomicUsize::new(0));
        let requests = Arc::clone(&journals);
        game.agent
            .add_method(hot::method::EDIT_CHANGES, agent::Access::Read, move |_| {
                requests.fetch_add(1, Ordering::Relaxed);
                Ok(json!({"changes":[]}))
            });
        // The identity is available on this host, the journal and teleport are not.
        play.received_methods(&mut app, Ok(json!([hot::method::EDIT_MAP, "fixture.ping"])));
        assert!(play.has_method("fixture.ping"));
        assert!(play.map_refusal(&app).is_none());
        for (action, method) in [
            (Apply::Send, hot::method::EDIT_CHANGES),
            (Apply::Revert, hot::method::EDIT_REVERT),
        ] {
            let why = play.apply_edits(&mut app, action).unwrap_err();
            assert!(why.contains(method), "{why}");
        }
        play.edits.journal_ready = false;
        play.teleport(&mut app, [1., 2., 3.]);
        assert!(app.status().unwrap().text().contains(sim::method::STATE_SET));

        // One poll with edit.map available exercises the journal gate.
        let poll = serde_json::to_value(play.status.as_ref().unwrap()).unwrap();
        play.on_event(&mut app, Event::Poll(poll.clone()));
        let epoch = play.edits.epoch.unwrap();
        let identity = json!({
            "name":"example", "epoch":epoch, "checksum":app.session.source_checksum(),
            "scenario":fake::SCNR, "session":{"kind":"local","writes":true}
        });
        play.edit_map(&mut app, Ok(identity));
        assert!(!play.edits.journal_ready);
        play.received_methods(&mut app, Ok(json!([])));
        play.on_event(&mut app, Event::Poll(poll));
        assert!(!play.edits.map_requested);
        assert!(play.map_refusal(&app).unwrap().contains(hot::method::EDIT_MAP));

        // The reply on the same control connection follows every earlier call.
        play.call(method::STATUS, Value::Null, Purpose::Other);
        fake::until(&mut play, &mut app, "control connection barrier", |_, app| {
            app.log()
                .iter()
                .any(|line| line.status.text().starts_with("session.status:"))
        });
        assert_eq!(journals.load(Ordering::Relaxed), 0);
        assert!(game.calls(sim::method::STATE_SET).is_empty());
    }
}
