//! Phantom peers (docs/game/bots.md, section 5 and stage 6): bots that are
//! remote players of a machine that does not exist, made up in memory on
//! the host. Not built yet: [`add`] says so. The signature is the one stage
//! 6 fills in, so callers can be written against it now.

use crate::local::PadSource;

/// Who a phantom is (docs/game/bots.md, section 9.1): the server's account
/// and how the lobby shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub xuid: u64,
    pub gamertag: String,
    /// Emblem foreground, background and three colours, primary and
    /// secondary armour colours, as the roster gives them.
    pub emblem: [u8; 5],
    pub colors: [u8; 2],
}

/// A phantom on the books.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PhantomHandle(pub u32);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Phantom peers are stage 6.
    NotBuilt,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotBuilt => write!(
                f,
                "phantom peers are not built yet (docs/game/bots.md, stage 6)"
            ),
        }
    }
}
impl std::error::Error for Error {}

/// Adds a phantom peer with `identity` on `team` (`None`: the host
/// balances), driven by `source`.
pub fn add(
    identity: Identity,
    team: Option<u8>,
    source: Box<dyn PadSource>,
) -> Result<PhantomHandle, Error> {
    let _ = (identity, team, source);
    Err(Error::NotBuilt)
}

/// Removes a phantom peer.
pub fn remove(handle: PhantomHandle) -> Result<(), Error> {
    let _ = handle;
    Err(Error::NotBuilt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phantoms_say_they_are_not_built() {
        let src: Box<dyn PadSource> = Box::new(|_: &crate::local::Frame<'_>| Default::default());
        assert_eq!(add(Identity::default(), None, src), Err(Error::NotBuilt));
        assert_eq!(remove(PhantomHandle(0)), Err(Error::NotBuilt));
    }
}
