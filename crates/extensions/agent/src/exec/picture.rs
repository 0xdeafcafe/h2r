//! `render.picture.open` and `render.picture.close` on the executor
//! (`render/picture.rs` holds the pictures). A ring's reply goes out
//! before its first `picture` event; a stream's reply is the last line
//! before the connection switches to binary frames, as `live.subscribe`'s.

use super::*;
use agent_proto::picture::{self as wire, CloseParams, OpenParams, Transport};
use serde_json::json;

use crate::render::picture::{EventSink, STREAM_QUEUE};

impl Agent {
    /// Replaces the pictures (a test's synthetic frame source).
    #[cfg(test)]
    pub(crate) fn set_pictures(&self, pictures: Arc<crate::render::picture::Pictures>) {
        self.lock().pictures = pictures;
    }
}

impl State {
    pub(super) fn picture_method(
        &mut self,
        conn: ConnId,
        request: Request,
    ) -> Option<Result<Value, RpcError>> {
        match request.method.as_str() {
            wire::CLOSE => Some(
                params(request.params)
                    .map(|p: CloseParams| json!({"closed": self.pictures.close(p.id)})),
            ),
            wire::OPEN => self.open_picture(conn, request).err().map(Err),
            _ => unreachable!(),
        }
    }

    fn open_picture(&mut self, conn: ConnId, request: Request) -> Result<(), RpcError> {
        let p: OpenParams = params(request.params)?;
        let id = request.id.ok_or_else(|| {
            RpcError::new(
                code::INVALID_REQUEST,
                "render.picture.open requires a request id",
            )
        })?;
        let c = self
            .conns
            .get(&conn)
            .ok_or_else(|| RpcError::new(code::GONE, "connection closed"))?;
        if c.closing || c.dead {
            return Err(RpcError::new(code::GONE, "connection closed"));
        }
        match p.transport.unwrap_or_default() {
            Transport::Ring => {
                let sink: EventSink = match &c.tx {
                    Output::Local(tx) => {
                        let tx = tx.clone();
                        Box::new(move |line| tx.try_send(line).is_ok())
                    }
                    Output::Socket(tx) => {
                        let tx = tx.clone();
                        Box::new(move |line: String| {
                            tx.try_send(crate::server::Output::Line(line.into()))
                                .is_ok()
                        })
                    }
                };
                let pictures = Arc::clone(&self.pictures);
                let opened = pictures.open_ring(conn, &p, Some(sink))?;
                let mut result = to_value(&opened);
                result["at"] = to_value(self.at());
                self.send(conn, Response::ok(id, result));
                pictures.start(opened.id);
            }
            Transport::Stream => {
                let Output::Socket(tx) = &c.tx else {
                    return Err(RpcError::new(
                        code::UNAVAILABLE,
                        "a picture stream needs a socket connection; in process, open a ring",
                    ));
                };
                let tx = tx.clone();
                if self.controller == Some(conn)
                    || !c.events.is_empty()
                    || self.waits.iter().any(|w| w.conn == conn)
                    || self.deferred.iter().any(|d| d.conn == conn)
                    || self.snap_jobs.iter().any(|j| j.conn() == conn)
                    || self.capture.as_ref().is_some_and(|c| c.conn == conn)
                    || self.pending.iter().any(|q| q.conn == conn)
                {
                    return Err(RpcError::new(
                        code::BUSY,
                        "open a picture stream on a separate connection: this one switches to binary frames",
                    ));
                }
                let (frames_tx, frames) = crate::live::Frames::channel(STREAM_QUEUE);
                let pictures = Arc::clone(&self.pictures);
                let opened = pictures.open_stream(conn, &p, frames_tx)?;
                let mut result = to_value(&opened);
                result["at"] = to_value(self.at());
                let _scope = mem_watch::scope("agent.reply");
                let line =
                    serde_json::to_string(&Response::ok(id, result)).expect("replies serialise");
                if tx
                    .try_send(crate::server::Output::Upgrade {
                        line: line.into(),
                        frames,
                    })
                    .is_err()
                {
                    pictures.close(opened.id);
                    return Err(RpcError::new(code::GONE, "stream writer unavailable"));
                }
                if let Some(c) = self.conns.get_mut(&conn) {
                    c.streaming = true;
                }
                pictures.start(opened.id);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::sync_channel;

    use super::*;
    use crate::render::picture::{FrameFeed, Pictures};
    use agent_proto::picture::{Grabbed, Opened, Ring};

    struct Still;

    impl FrameFeed for Still {
        fn ready(&self) -> Result<(), String> {
            Ok(())
        }
        fn grab(&self, after: Option<u64>) -> Result<Option<Grabbed>, String> {
            let seq = after.map_or(1, |s| s + 1);
            Ok(Some(Grabbed {
                seq,
                size: (8, 8),
                rgba: vec![seq as u8; 8 * 8 * 4],
            }))
        }
    }

    #[derive(Default)]
    struct Menu;

    impl Reader for Menu {
        fn status(&mut self) -> GameStatus {
            GameStatus {
                tick: Some(10),
                map: Some("mainmenu".into()),
                mode: Some("main_menu".into()),
                bsp: None,
            }
        }
        fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
            Ok(Vec::new())
        }
    }

    fn ask(a: &Agent, conn: ConnId, id: u64, method: &str, params: Value) {
        let r = Request::new(id, method, params);
        a.submit(conn, &serde_json::to_string(&r).unwrap());
    }

    #[test]
    fn a_ring_opens_answers_first_and_goes_with_its_connection() {
        let dir = std::env::temp_dir().join(format!("h2-picture-exec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let pictures = Arc::new(Pictures::new(Arc::new(Still), dir.clone(), false));
        let a = Agent::new();
        a.set_pictures(pictures.clone());
        let (tx, rx) = sync_channel(64);
        let conn = a.connect(tx);
        ask(&a, conn, 1, wire::OPEN, json!({"width": 8, "fps": 30}));
        a.on_tick(&mut Menu);
        let reply: Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert!(reply["error"].is_null(), "{reply}");
        let opened: Opened = serde_json::from_value(reply["result"].clone()).unwrap();
        assert_eq!(
            (opened.transport, opened.size, opened.fps),
            (Transport::Ring, [8, 8], 30)
        );
        let path = opened.path.clone().unwrap();
        assert!(path.exists());

        // A frame: in the ring, and an event after the reply.
        assert!(matches!(
            pictures.pump(Instant::now()),
            crate::render::picture::Pump::Published(1)
        ));
        let event: Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(event["method"], "event");
        assert_eq!(event["params"]["kind"], "picture");
        assert_eq!(event["params"]["id"], opened.id);
        assert_eq!(event["params"]["tick"], 10);
        let ring = Ring::open(&path).unwrap();
        let mut out = Vec::new();
        assert_eq!(ring.read_newest(&mut out).unwrap().frame, 1);

        // A stream on an in-process connection is refused.
        ask(&a, conn, 2, wire::OPEN, json!({"transport": "stream"}));
        a.on_tick(&mut Menu);
        let refused: Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(refused["error"]["code"], code::UNAVAILABLE);

        a.disconnect(conn);
        assert!(!path.exists(), "the connection took its ring with it");
        assert!(ring.closed());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn close_by_id() {
        let dir =
            std::env::temp_dir().join(format!("h2-picture-exec-close-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = Agent::new();
        a.set_pictures(Arc::new(Pictures::new(Arc::new(Still), dir.clone(), false)));
        let (tx, rx) = sync_channel(64);
        let conn = a.connect(tx);
        ask(&a, conn, 1, wire::OPEN, json!({}));
        a.on_tick(&mut Menu);
        let reply: Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        let opened: Opened = serde_json::from_value(reply["result"].clone()).unwrap();
        ask(&a, conn, 2, wire::CLOSE, json!({"id": opened.id}));
        ask(&a, conn, 3, wire::CLOSE, json!({"id": opened.id}));
        a.on_tick(&mut Menu);
        let first: Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        let second: Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(
            (
                first["result"]["closed"].clone(),
                second["result"]["closed"].clone()
            ),
            (json!(true), json!(false))
        );
        assert!(!opened.path.unwrap().exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
