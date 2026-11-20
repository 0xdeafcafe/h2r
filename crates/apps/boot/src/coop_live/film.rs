//! Asynchronous lookups affect the lobby on the game thread at a defined tick.
//! Playback supplies those results without spawning a watcher or pinging a peer.
use super::{Guest, Inner, Lobby, TEXT_SLOT_LEN, TEXT_SLOTS};
use ::film::tape::{Call, Tap};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::sync::{Arc, Mutex};

const MAX_OBSERVATION: usize = 128 << 10;

#[derive(Serialize, Deserialize)]
pub(super) enum Change {
    Ping(Option<u16>),
    Joined(Option<Guest>),
}

pub(super) struct LobbyFilm {
    tap: Arc<dyn Tap>,
    pending: Mutex<Vec<Change>>,
}

impl LobbyFilm {
    pub(super) fn new(tap: Arc<dyn Tap>) -> Self {
        Self {
            tap,
            pending: Mutex::default(),
        }
    }

    pub(super) fn replaying(&self) -> bool {
        self.tap.replaying()
    }

    /// A paused guest need only see the latest result from each poller.
    pub(super) fn stage(&self, next: Change) {
        let mut queue = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        queue.retain(|old| std::mem::discriminant(old) != std::mem::discriminant(&next));
        queue.push(next);
    }

    pub(super) fn read<T: Serialize + DeserializeOwned>(
        &self,
        op: &str,
        args: &[u32],
        live: impl FnOnce() -> T,
    ) -> Option<T> {
        if self.tap.replaying() {
            let c = self.tap.next(op, args, 0)?;
            if c.result == Ok(0)
                && let [bytes] = c.outputs.as_slice()
                && bytes.len() <= MAX_OBSERVATION
                && let Ok(value) = serde_json::from_slice(bytes)
            {
                return Some(value);
            }
            self.tap.fault(format!("invalid {op} observation"));
            return None;
        }
        let value = live();
        match serde_json::to_vec(&value) {
            Ok(bytes) if bytes.len() <= MAX_OBSERVATION => self.tap.record(
                ::film::NETR,
                Call {
                    index: 0,
                    op: op.into(),
                    args: args.into(),
                    thread: 0,
                    result: Ok(0),
                    outputs: vec![bytes],
                },
            ),
            _ => self
                .tap
                .fault(format!("{op} exceeds film observation budget")),
        }
        Some(value)
    }

    pub(super) fn pump(&self, lobby: &Lobby) {
        if !lobby.in_coop()
            && !lobby
                .session()
                .is_some_and(|s| s.peers >= 2 && !s.is_host())
        {
            self.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
            return;
        }
        let due = || std::mem::take(&mut *self.pending.lock().unwrap_or_else(|e| e.into_inner()));
        if let Some(changes) = self.read("Coop.Poll", &[], due) {
            if changes.len() > 2 {
                self.tap.fault("invalid co-op poll count".into());
                return;
            }
            for c in changes {
                match c {
                    Change::Joined(guest) => {
                        lobby.apply_joined(guest);
                    }
                    Change::Ping(ms) => {
                        lobby.sync_retail();
                        lobby.apply_ping(ms);
                    }
                }
            }
        }
    }

    pub(super) fn mode(&self, input: u32, pick: impl FnOnce() -> u32) -> u32 {
        let value = self.read("Coop.LobbyMode", &[input], pick).unwrap_or(input);
        if self.replaying() && input == super::MODE_CUSTOM {
            super::drop_coop_mode();
        }
        if value == input || (input == super::MODE_CUSTOM && value == super::MODE_COOP) {
            value
        } else {
            self.tap.fault("invalid co-op lobby mode".into());
            input
        }
    }
}

pub(super) fn policy(lobby: Arc<Lobby>) -> hooks::SnapshotPolicy {
    let check = lobby.clone();
    let save = lobby.clone();
    hooks::SnapshotPolicy::section_forbid_while(
        move || {
            if !crate::theater::in_keyframe() {
                return super::screens_live(&check);
            }
            let pending = *super::COOP_MODE_PENDING
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if super::coop_mode_waits(pending, std::time::Instant::now()) {
                return Some("co-op lobby mode setter is pending".into());
            }
            None
        },
        move || {
            let inner = save.lock();
            let slots = save.texts.slots.lock().unwrap_or_else(|e| e.into_inner());
            serde_json::to_vec(&(&*inner, &*slots)).expect("serialisable co-op state")
        },
        move |bytes, _| {
            if !crate::theater::in_keyframe()
                || !lobby.film.as_ref().is_some_and(LobbyFilm::replaying)
            {
                return Err("co-op film state requires isolated playback".into());
            }
            restore_lobby(&lobby, bytes)?;
            super::drop_coop_mode();
            Ok(())
        },
    )
}

fn restore_lobby(lobby: &Lobby, bytes: &[u8]) -> Result<(), String> {
    type Slots = Vec<(Vec<u8>, bool)>;
    if bytes.len() > 1 << 20 {
        return Err("co-op lobby checkpoint too large".into());
    }
    let (inner, slots): (Inner, Slots) =
        serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if inner.difficulty > 3
        || inner.level >= lobby.levels.len().max(1)
        || inner.friends.len() > super::MAX_FRIENDS
        || slots.len() > TEXT_SLOTS as usize
        || slots
            .iter()
            .any(|(v, _)| v.len() > TEXT_SLOT_LEN as usize || v.last() != Some(&0))
    {
        return Err("invalid co-op lobby checkpoint".into());
    }
    *lobby.lock() = inner;
    *lobby.texts.slots.lock().unwrap_or_else(|e| e.into_inner()) = slots;
    Ok(())
}

/// Guest feature selection differs between the System Link probe and the
/// retail co-op lobby. Restore only these recognised booleans/level facts.
#[derive(Serialize, Deserialize, PartialEq)]
pub(crate) struct Options {
    row: bool,
    list_ui: bool,
    auto_join: bool,
    lift: bool,
    log: bool,
    latency: bool,
    look: Option<bool>,
    probe: Option<(String, String)>,
}

impl Options {
    pub(crate) fn capture() -> Self {
        let env = |k: &str| std::env::var(k).ok();
        let jip = crate::coop_jip::Stages::parse(env);
        let probe = crate::coop_probe::parse(env).ok().flatten();
        Self {
            row: std::env::var_os("H2_NO_COOP_ROW").is_none(),
            list_ui: super::list_ui(),
            auto_join: super::auto_join_screen(),
            lift: jip.lift,
            log: jip.log,
            latency: crate::coop_latency::wanted(env("H2_COOP_LATENCY").as_deref()),
            look: env("H2_COOP_LOOK_PREDICTION")
                .and_then(|s| crate::setting_defs::COOP_LOOK_PREDICTION.parse(&s).ok())
                .and_then(|v| v.as_bool()),
            probe: probe.map(|p| (p.map, p.difficulty.to_string())),
        }
    }

    pub(crate) fn configure(&self, cmd: &mut std::process::Command) -> anyhow::Result<()> {
        if self.row {
            cmd.env_remove("H2_NO_COOP_ROW");
        } else {
            cmd.env("H2_NO_COOP_ROW", "1");
        }
        for (k, on) in [
            ("H2_COOP_LIST_UI", self.list_ui),
            ("H2_COOP_JOIN_SCREEN", self.auto_join),
            ("H2_COOP_JIP_LIFT", self.lift),
            ("H2_COOP_JIP_LOG", self.log),
            ("H2_COOP_LATENCY", self.latency),
        ] {
            cmd.env(k, if on { "1" } else { "0" });
        }
        if let Some(look) = self.look {
            cmd.env("H2_COOP_LOOK_PREDICTION", if look { "1" } else { "0" });
        }
        if let Some((map, difficulty)) = &self.probe {
            anyhow::ensure!(
                !map.is_empty()
                    && map.len() <= 64
                    && map
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b' ')),
                "invalid film co-op probe level"
            );
            crate::level_boot::Difficulty::parse(difficulty)?;
            cmd.env("H2_COOP_PROBE", "1")
                .env("H2_COOP_PROBE_MAP", map)
                .env("H2_COOP_PROBE_DIFFICULTY", difficulty);
        } else {
            cmd.env_remove("H2_COOP_PROBE")
                .env_remove("H2_COOP_PROBE_MAP")
                .env_remove("H2_COOP_PROBE_DIFFICULTY");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::Role;
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct Tape {
        playback: bool,
        calls: Mutex<VecDeque<Call>>,
    }
    impl Tap for Tape {
        fn replaying(&self) -> bool {
            self.playback
        }
        fn record(&self, tag: ::film::Tag, c: Call) {
            assert_eq!(tag, ::film::NETR);
            self.calls.lock().unwrap().push_back(c);
        }
        fn next(&self, op: &str, args: &[u32], thread: u32) -> Option<Call> {
            let c = self.calls.lock().unwrap().pop_front()?;
            assert_eq!((c.op.as_str(), &c.args[..], c.thread), (op, args, thread));
            Some(c)
        }
        fn fault(&self, why: String) {
            panic!("{why}")
        }
    }

    #[test]
    fn established_host_lobby_restores_text_and_validates_before_mutation() {
        let lobby = Lobby::new(vec![], None, None, Box::new(|_| vec![]));
        let session = super::super::coop_probe::Session {
            peers: 2,
            players: 2,
            ..Default::default()
        };
        let mut saved = Inner {
            role: Role::Host,
            session: Some(session),
            difficulty: 3,
            advertised: Some((1, 101, 3)),
            ..Default::default()
        };
        let text = vec![(b"Ready\0".to_vec(), true)];
        restore_lobby(&lobby, &serde_json::to_vec(&(&saved, &text)).unwrap()).unwrap();
        assert_eq!(lobby.role(), Role::Host);
        assert_eq!(lobby.session(), Some(session));
        assert_eq!(lobby.difficulty(), 3);
        assert_eq!(lobby.texts.get(0).as_deref(), Some("Ready"));

        saved.role = Role::Idle;
        saved.level = 88;
        assert!(restore_lobby(&lobby, &serde_json::to_vec(&(saved, text)).unwrap()).is_err());
        assert_eq!(lobby.role(), Role::Host);
        assert_eq!(lobby.session(), Some(session));
    }

    #[test]
    fn asynchronous_ping_crosses_only_the_recorded_game_tick() {
        fn host(film: LobbyFilm) -> Lobby {
            let mut lobby = Lobby::new(vec![], None, None, Box::new(|_| vec![]));
            lobby.lock().role = Role::Host;
            lobby.film = Some(film);
            lobby
        }
        let tape = Arc::new(Tape::default());
        let record = host(LobbyFilm::new(tape.clone()));
        let f = record.film.as_ref().unwrap();
        f.stage(Change::Ping(Some(12)));
        f.stage(Change::Ping(Some(23)));
        assert_eq!(record.ping(), None);
        f.pump(&record);
        assert_eq!(record.ping(), Some(23));
        f.stage(Change::Ping(None));
        f.pump(&record);
        let replay = host(LobbyFilm::new(Arc::new(Tape {
            playback: true,
            calls: Mutex::new(std::mem::take(&mut *tape.calls.lock().unwrap())),
        })));
        let f = replay.film.as_ref().unwrap();
        f.pump(&replay);
        assert_eq!(replay.ping(), Some(23));
        f.pump(&replay);
        assert_eq!(replay.ping(), None);
    }
}
