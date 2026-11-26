//! The agent while a film is paused between slices.
use super::*;

impl Agent {
    /// A film pause holds the machine between slices. Only its host controls
    /// need service.
    pub fn poll_theater(&self, reader: &mut dyn Reader) {
        let mut s = self.lock();
        let mut keep = VecDeque::new();
        while let Some(queued) = s.pending.pop_front() {
            let conn = queued.conn;
            let req = &queued.request;
            if agent_proto::theater::METHODS.contains(&req.method.as_str())
                || matches!(req.method.as_str(), method::METHODS | method::UI_SCREEN | method::UI_INPUT)
                || (req.method == debug_menu::sources::method::CHEAT_SET
                    && req.params["name"].as_str() == Some("hide_hud"))
            {
                let id = req.id.clone();
                if let (Some(id), Some(result)) = (id, s.handle(conn, queued.request, reader)) {
                    let reply = match result {
                        Ok(v) => Response::ok(id, v),
                        Err(e) => Response::err(id, e),
                    };
                    s.send(conn, reply);
                }
            } else {
                keep.push_back(queued);
            }
        }
        s.pending = keep;
        s.settle_deferred_at(reader, true);
    }
}
