//! The stream switches exactly once, in the same FIFO as JSON replies. The
//! output thread writes the acknowledgement before reading any binary frame.

use super::*;
use agent_proto::live::{self, Layers, SubscribeParams};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Close {
    stream: ConnId,
}

impl Agent {
    /// Boot installs this on an external Live service. It reads the
    /// active game session's network class at the same boundary as capture.
    pub fn set_live_session_probe(&self, probe: fn(&dyn guest_layout::GuestMemory) -> bool) {
        self.lock().live_session = Some(probe);
    }

    /// Boot installs this beside [`Agent::set_live_session_probe`]: the
    /// live stream still refuses every Live game, edits refuse only what
    /// the probe names (matchmaking, a client, an unreadable session).
    pub fn set_live_edit_probe(&self, probe: crate::exec::LiveEditProbe) {
        self.lock().live_edit = Some(probe);
    }

    /// The `camera` layer's source: the views of the last sealed frame
    /// scene (the game's scene glue). Without one, `live.subscribe` lists
    /// `camera` under `refused`.
    pub fn set_camera_source(&self, source: crate::live::CameraSource) {
        self.lock().cameras = Some(source);
    }

    /// Why this run seals no frame scene (no window, native rendering or
    /// the frame scene off). The first reason given is kept, and
    /// `live.subscribe` refuses `camera` with it.
    pub fn set_camera_unavailable(&self, why: impl Into<String>) {
        let mut s = self.lock();
        if s.camera_off.is_none() {
            s.camera_off = Some(why.into());
        }
    }
}

/// Without a source the run draws no frame scene to take a camera from.
pub(crate) const NO_CAMERA: &str =
    "no sealed frame scene: this run does not draw with the native renderer";

impl State {
    fn external_live_game(&self, reader: &mut dyn Reader) -> bool {
        let Some(probe) = self.live_session else {
            return false;
        };
        if self.game.mode.as_deref() == Some("main_menu") {
            return false;
        }
        // An unavailable reader on a protected host can't certify a local game.
        let mut remote = true;
        reader.with_guest(&mut |m| remote = probe(m));
        remote
    }

    pub(super) fn capture_live(
        &mut self,
        reader: &mut dyn Reader,
        only: Option<ConnId>,
        stepping: bool,
    ) {
        if !self.live.active() {
            return;
        }
        if self.external_live_game(reader) {
            self.live.close_all();
            return;
        }
        if let Some(tick) = self.game.tick {
            self.live.capture(
                reader,
                crate::live::Stamp {
                    tick,
                    clock: self.ticks,
                    epoch: self.live_epoch,
                    paused: self.paused.is_some(),
                    stepping,
                },
                only,
            );
        }
    }

    pub(super) fn live_method(
        &mut self,
        conn: ConnId,
        request: Request,
        reader: &mut dyn Reader,
    ) -> Option<Result<Value, RpcError>> {
        match request.method.as_str() {
            live::STATUS => {
                let mut status = self.live.status();
                let (_, checksum) = reader.live_metadata();
                status["map"] = json!({"name": self.game.map, "checksum": checksum,
                    "epoch": self.live_epoch, "activation": self.activation});
                Some(Ok(status))
            }
            live::CLOSE => Some(
                params(request.params).map(|p: Close| json!({"closed": self.live.close(p.stream)})),
            ),
            live::SUBSCRIBE => self.subscribe_live(conn, request, reader).err().map(Err),
            _ => unreachable!(),
        }
    }

    fn subscribe_live(
        &mut self,
        conn: ConnId,
        request: Request,
        reader: &mut dyn Reader,
    ) -> Result<(), RpcError> {
        let p: SubscribeParams = params(request.params)?;
        if !(1..=30).contains(&p.every) || p.key_every == 0 {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "every must be 1 to 30 and key_every positive",
            ));
        }
        let id = request.id.ok_or_else(|| {
            RpcError::new(code::INVALID_REQUEST, "subscribe requires a request id")
        })?;
        if self.no_tick_hook {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "the agent tick observer is disabled",
            ));
        }
        if self.external_live_game(reader) {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "the mirror is unavailable during a Live game",
            ));
        }
        let c = self
            .conns
            .get(&conn)
            .ok_or_else(|| RpcError::new(code::GONE, "connection closed"))?;
        let Output::Socket(tx) = &c.tx else {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "live.subscribe requires a stream socket",
            ));
        };
        if c.closing
            || c.dead
            || self.controller == Some(conn)
            || !c.events.is_empty()
            || self.waits.iter().any(|w| w.conn == conn)
            || self.deferred.iter().any(|d| d.conn == conn)
            || self.snap_jobs.iter().any(|j| j.conn() == conn)
            || self.capture.as_ref().is_some_and(|c| c.conn == conn)
        {
            return Err(RpcError::new(
                code::BUSY,
                "subscribe on a separate open connection",
            ));
        }
        let camera_refused = match (&self.camera_off, &self.cameras) {
            (Some(why), _) => Some(why.as_str()),
            (None, None) => Some(NO_CAMERA),
            (None, Some(_)) => None,
        };
        let (layers, refused) = Layers::negotiate(&p.layers, camera_refused);
        let cameras = self.cameras.clone().filter(|_| layers.camera);
        let (tick_rate, checksum) = reader.live_metadata();
        let at = self.at();
        let frames = self
            .live
            .subscribe(conn, layers, p.every, p.key_every, self.ticks, cameras)
            .map_err(|e| {
                RpcError::new(
                    code::UNAVAILABLE,
                    format!("starting the mirror worker: {e}"),
                )
            })?;
        let result = json!({"at": at, "stream": conn, "protocol": live::PROTOCOL, "tick_rate": tick_rate,
            "map": {"name": self.game.map, "checksum": checksum, "epoch": self.live_epoch,
                "activation": self.activation},
            "layers": layers.names(), "refused": refused});
        let _scope = mem_watch::scope("agent.reply");
        let line = serde_json::to_string(&Response::ok(id, result)).expect("subscribe serialises");
        if tx
            .try_send(crate::server::Output::Upgrade { line: line.into(), frames })
            .is_err()
        {
            self.live.close(conn);
            return Err(RpcError::new(code::GONE, "stream writer unavailable"));
        }
        self.conns
            .get_mut(&conn)
            .expect("connected subscriber")
            .streaming = true;
        // on_tick captures after dispatch. A subscriber arriving in its
        // pause loop would otherwise see nothing until resume.
        if self.holding {
            self.capture_live(reader, Some(conn), false);
        }
        Ok(())
    }
}
