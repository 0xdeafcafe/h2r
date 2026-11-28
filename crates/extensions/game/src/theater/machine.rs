//! A client's film is its machine (docs/game/film-format.md, "The machine
//! tape"; docs/game/theater-design-recording-playback.md, "Kinds of
//! game").
//!
//! A client of a System Link or Live game another console hosts is sent
//! state, not every player's updates, so its film cannot be played from
//! the simulation's inputs alone. A recording session keeps the machine's
//! outside inputs from boot instead: every Live HLE call (`live_hle::tape`,
//! the network's receives among them) and every pad read (`xapi::tap`).
//! When a client's game is sealed, its film takes that tape. A replay boots
//! the same build, answers every Live call and pad read from the tape and
//! never opens a socket, so the client's game runs again as it ran.
use anyhow::{Context, Result, bail};
use film::tape::{PadRead, Role, Tape};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

const _: () = assert!(xapi::tap::PACKET == film::tape::PACKET);

/// The outside inputs a recording session keeps for its client films.
pub struct Machine {
    role: Box<dyn Fn() -> Role + Send + Sync>,
    calls: Arc<live_hle::tape::Tape>,
    pads: Arc<Mutex<xapi::tap::InputLog>>,
}

impl Machine {
    /// Start keeping the machine tape: a recorder on the Live HLE and a pad
    /// recorder as XAPI's innermost tap, so the taps installed after it
    /// (the superbounce assist, the agent's) see the pad it logged, as a
    /// replay feeds it to them. Call before the guest runs and before any
    /// other tap is installed.
    pub fn record<S: xapi::Storage>(
        hle: &Arc<live_hle::Hle>,
        xapi: &Mutex<xapi::Xapi<S>>,
    ) -> Result<Self> {
        let mut x = xapi.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(other) = x.take_input_tap() {
            x.set_input_tap(Some(other));
            bail!("another input tap owns the pad reads");
        }
        let calls = live_hle::tape::Tape::recorder();
        if hle.set_tape(calls.clone()).is_err() {
            bail!("the Live HLE already has a machine tape");
        }
        let (recorder, pads) = xapi::tap::Recorder::new();
        x.set_input_tap(Some(Box::new(recorder)));
        let hle = Arc::clone(hle);
        Ok(Self {
            role: Box::new(move || hle.film_role()),
            calls,
            pads,
        })
    }

    /// A machine whose role is `role` and whose tape is `tape`, for tests.
    #[cfg(test)]
    pub(crate) fn fixed(role: Role, tape: &Tape) -> Self {
        let calls = live_hle::tape::Tape::recorder();
        for call in &tape.calls {
            calls.record(call.clone());
        }
        Self {
            role: Box::new(move || role),
            calls,
            pads: Arc::new(Mutex::new(input_log(&tape.pads))),
        }
    }

    /// Whose game this console is in now (`Hle::film_role`).
    pub fn role(&self) -> Role {
        (self.role)()
    }

    /// The tape from boot to now, or why there is none.
    pub fn tape(&self) -> std::result::Result<Tape, String> {
        if let Some(why) = self.calls.spoiled() {
            return Err(why);
        }
        let pads = self.pads.lock().unwrap_or_else(|e| e.into_inner());
        Ok(Tape {
            calls: self.calls.calls(),
            pads: pad_reads(&pads),
        })
    }
}

pub fn pad_reads(log: &xapi::tap::InputLog) -> Vec<PadRead> {
    log.reads
        .iter()
        .map(|r| PadRead {
            handle: r.handle,
            status: r.status,
            packet: r.packet,
        })
        .collect()
}

pub fn input_log(pads: &[PadRead]) -> xapi::tap::InputLog {
    xapi::tap::InputLog {
        reads: pads
            .iter()
            .map(|p| xapi::tap::Read {
                tick: None,
                handle: p.handle,
                status: p.status,
                packet: p.packet,
            })
            .collect(),
    }
}

/// A client film, ready to replay.
pub struct ClientFilm {
    pub role: Role,
    pub tape: Tape,
}

/// The film at `path` when it is a client's: its role and its tape. `None`
/// for every other film (they play from their game start). A client film
/// whose tape does not read is an error, never an engine replay.
pub fn client_film(path: &Path) -> Result<Option<ClientFilm>> {
    let file = std::fs::File::open(path).with_context(|| path.display().to_string())?;
    let mut reader = film::file::FilmReader::open(file)?;
    let role = reader.role();
    if !role.is_client() {
        return Ok(None);
    }
    let tape = reader
        .tape()
        .with_context(|| format!("the {} film's machine tape", role.label()))?;
    if tape.is_empty() {
        bail!("this {} film holds no machine tape", role.label());
    }
    Ok(Some(ClientFilm { role, tape }))
}

/// Answer the guest from `film`: every Live call from its tape, with no
/// socket and no server, and every pad read from it. Call before the
/// guest runs and before any other tap is installed.
pub fn replay<S: xapi::Storage>(
    hle: &Arc<live_hle::Hle>,
    xapi: &Mutex<xapi::Xapi<S>>,
    film: ClientFilm,
) -> Result<Arc<live_hle::tape::Tape>> {
    let mut x = xapi.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(other) = x.take_input_tap() {
        x.set_input_tap(Some(other));
        bail!("another input tap owns the pad reads");
    }
    let calls = live_hle::tape::Tape::replayer(film.tape.calls);
    if hle.set_tape(calls.clone()).is_err() {
        bail!("the Live HLE already has a machine tape");
    }
    let (replayer, _stats) = xapi::tap::Replayer::new(input_log(&film.tape.pads));
    x.set_input_tap(Some(Box::new(replayer)));
    Ok(calls)
}

#[cfg(test)]
pub(crate) mod tests;
