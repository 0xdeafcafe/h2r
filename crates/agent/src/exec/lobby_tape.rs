//! The playerless supervisor acts through these lobby methods. Record the
//! boundary that actually ran them, independent of the socket's arrival time.
use super::*;

pub trait LobbyTape: Send + Sync {
    fn record_lobby(&self, boundary: u64, name: &str, params: &Value);
    fn lobby_due(&self, boundary: u64) -> Vec<(String, Value)>;
    fn lobby_error(&self, message: String);
}
pub(super) fn is_lobby(name: &str) -> bool {
    matches!(
        name,
        "host.lobby.state" | "host.lobby.open" | "host.lobby.start" | "host.lobby.close"
    )
}
impl Agent {
    pub fn set_lobby_tape(&self, tape: Arc<dyn LobbyTape>) {
        self.lock().lobby_tape = Some(tape);
    }
    /// A film pause holds the machine between slices. Only its host controls
    /// need service, and polling them must not advance the lobby tape cursor.
    pub fn poll_theater(&self, reader: &mut dyn Reader) {
        let mut s = self.lock();
        let mut keep = VecDeque::new();
        while let Some((conn, req)) = s.pending.pop_front() {
            if agent_proto::theater::METHODS.contains(&req.method.as_str())
                || req.method == method::METHODS
                || (req.method == debug_menu::sources::method::CHEAT_SET
                    && req.params["name"].as_str() == Some("hide_hud"))
            {
                let id = req.id.clone();
                if let (Some(id), Some(result)) = (id, s.handle(conn, req, reader)) {
                    let reply = match result {
                        Ok(v) => Response::ok(id, v),
                        Err(e) => Response::err(id, e),
                    };
                    s.send(conn, reply);
                }
            } else {
                keep.push_back((conn, req));
            }
        }
        s.pending = keep;
    }
}
impl State {
    pub(super) fn lobby_boundary(&mut self, reader: &mut dyn Reader) {
        let Some(tape) = self.lobby_tape.clone() else {
            return;
        };
        self.lobby_boundary += 1;
        for (name, params) in tape.lobby_due(self.lobby_boundary) {
            if !is_lobby(&name) {
                tape.lobby_error("invalid recorded lobby action".into());
                return;
            }
            // Call the same registered method, on the game thread. A deferred
            // reply is still polled. Its socket connection need not exist.
            let conn = self.controller.unwrap_or(u64::MAX);
            let mut req = Request::new(0, &name, params);
            req.id = None; // Tape actions have no client waiting for a reply.
            if let Some(Err(e)) = self.handle(conn, req, reader) {
                tape.lobby_error(format!("{name}: {}", e.message));
                return;
            }
        }
    }
    pub(super) fn lobby_record(
        &self,
        name: &str,
        params: &Value,
    ) -> Option<(Arc<dyn LobbyTape>, u64, Value)> {
        if !is_lobby(name) {
            return None;
        }
        Some((
            self.lobby_tape.clone()?,
            self.lobby_boundary,
            params.clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Film {
        recorded: Mutex<Vec<(u64, String, Value)>>,
        playback: Mutex<VecDeque<(u64, String, Value)>>,
        errors: Mutex<Vec<String>>,
    }
    impl LobbyTape for Film {
        fn record_lobby(&self, b: u64, n: &str, p: &Value) {
            self.recorded.lock().unwrap().push((b, n.into(), p.clone()));
        }
        fn lobby_due(&self, b: u64) -> Vec<(String, Value)> {
            let mut q = self.playback.lock().unwrap();
            let mut ready = vec![];
            while q.front().is_some_and(|(i, _, _)| *i == b) {
                let (_, m, p) = q.pop_front().unwrap();
                ready.push((m, p));
            }
            ready
        }
        fn lobby_error(&self, msg: String) {
            self.errors.lock().unwrap().push(msg)
        }
    }
    struct Empty;
    impl Reader for Empty {
        fn status(&mut self) -> GameStatus {
            GameStatus::default()
        }
        fn objects(&mut self) -> Result<Vec<Object>, String> {
            Ok(vec![])
        }
    }
    #[test]
    fn film_lobby_actions_keep_the_boundary_and_only_run_registered_lobby_methods() {
        let name = "host.lobby.open";
        let tape = Arc::new(Film::default());
        let shared = Arc::new(Mutex::new(vec![]));
        let install = || {
            let a = Agent::new();
            let calls = shared.clone();
            a.add_game_method(name, Access::Act, move |p, _| {
                calls.lock().unwrap().push(p);
                Ok(Value::Null)
            });
            a.set_lobby_tape(tape.clone());
            a
        };
        let a = install();
        let (tx, _rx) = std::sync::mpsc::sync_channel(8);
        let c = a.connect(tx);
        a.between_slices(&mut Empty, 0);
        a.submit(
            c,
            &serde_json::json!({"id":1,"jsonrpc":"2.0","method":name,"params":{"map":"fixture"}})
                .to_string(),
        );
        a.between_slices(&mut Empty, 0);
        let actions = tape.recorded.lock().unwrap().clone();
        assert_eq!(
            actions,
            vec![(2, name.into(), serde_json::json!({"map":"fixture"}))]
        );
        *tape.playback.lock().unwrap() = actions.into();
        let replay = install();
        replay.between_slices(&mut Empty, 0);
        assert_eq!(shared.lock().unwrap().len(), 1);
        replay.between_slices(&mut Empty, 0);
        assert_eq!(shared.lock().unwrap().len(), 2);
        tape.playback
            .lock()
            .unwrap()
            .push_back((3, "state.load".into(), Value::Null));
        replay.between_slices(&mut Empty, 0);
        assert_eq!(
            tape.errors.lock().unwrap().as_slice(),
            ["invalid recorded lobby action"]
        );
    }
}
