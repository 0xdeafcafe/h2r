//! The message of the day the embedded server publishes when none has
//! ever been published (docs/live-admin.md §3): a header for the Live main
//! menu and tips the matchmaking screen picks from at random every 15 s,
//! as the original service's rotating "did you know" lines did.
//!
//! The text is original to this project. It stays ASCII (the game's font
//! has every ASCII glyph) and well under the 255 UTF-16 unit limit, so it
//! fits the screen's text box. Edit it from the web admin's MOTD editor or
//! `h2 live motd set`; this default is never written over a published one.

use live_proto::motd::Motd;
use live_proto::{ErrorCode, HALO_2_MOTD_FILE, HALO_2_TITLE_ID};

use crate::app::Live;

pub const HEADER: &str = "Welcome back to Xbox Live for Halo 2.";

pub const TIPS: &[&str] = &[
    "Did you know? Every game you play is on the stats site at /stats: carnage reports, medals and a map of where each kill happened.",
    "Did you know? `h2 live q` asks the service anything from a terminal: who is online, recent games, a player's record. Try `h2 live q --help`.",
    "Did you know? The web dashboard shows who is online right now. Open the address `h2 play --live` printed when it started.",
    "Did you know? Friends live on this server too: `h2 live friend add <gamertag>` sends a request, and they accept it from their side.",
    "Did you know? An admin can change this message from the web dashboard. The new one shows the next time you sign in.",
    "Did you know? Playlists come from multiplayer_hopper.dat. Admins can rebuild it in the web admin's hopper editor.",
    "Did you know? Everything here is a signed event in one log. Your account's key signs what you do; the server cannot forge it.",
    "Did you know? Your account and its key are kept in accounts.json. `h2 live account export` copies one to another machine.",
    "Did you know? `h2 live team create <name>` starts a Team. Recruit members by gamertag; they join when they accept.",
    "Did you know? Your service record counts kills, deaths, assists and medals across every game this server has seen.",
    "Tip: a melee from behind is an instant kill. Patience pays more than a full clip.",
    "Tip: hold the reload button over a dropped weapon to swap for it. Tap it to reload instead.",
    "Tip: two weapons at once leaves no hand free for grenades. Drop the second gun when you need to throw.",
    "Tip: a fully charged plasma pistol shot strips a shield in one hit. Follow it with anything.",
    "Tip: the Battle Rifle fires a burst of three. Aim for the head and pace your trigger pulls.",
    "Tip: boarding a vehicle from the side takes it from its driver. Watch your flanks when you drive.",
    "Tip: shields recharge only when you stop taking damage. Break line of sight before you push again.",
    "Tip: in Capture the Flag, the carrier is slow. Escort them; do not race them home.",
    "Tip: crouching keeps you off the motion tracker. Moving fast lights you up.",
    "Tip: sticky grenades cling to players and vehicles. Lead a moving target.",
    "Did you know? This service runs on your own machine. Nothing here is Microsoft's or Bungie's; it is a recreation for this project.",
];

/// The default as the game file's text.
pub fn motd() -> Motd {
    Motd {
        header: HEADER.into(),
        messages: TIPS.iter().map(|s| (*s).into()).collect(),
    }
}

/// Publishes [`motd`] for Halo 2 unless a message of the day was ever
/// published. Returns whether it published.
pub async fn ensure(live: &Live) -> anyhow::Result<bool> {
    match live.file(HALO_2_TITLE_ID, HALO_2_MOTD_FILE).await {
        Ok(_) => Ok(false),
        Err(e) if e.code == ErrorCode::NotFound => {
            let bytes = motd().build()?;
            crate::admin::publish(live, HALO_2_TITLE_ID, HALO_2_MOTD_FILE, &bytes)
                .await
                .map_err(|e| anyhow::anyhow!("publishing the default MOTD: {}", e.message))?;
            Ok(true)
        }
        Err(e) => Err(anyhow::anyhow!("reading the MOTD: {}", e.message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_a_file_the_game_takes() {
        let m = motd();
        assert!((16..=live_proto::motd::MAX_MESSAGES).contains(&m.messages.len()));
        for s in std::iter::once(&m.header).chain(&m.messages) {
            assert!(s.is_ascii(), "{s}");
            assert!(s.encode_utf16().count() <= 200, "{s}");
        }
        let file = m.build().unwrap();
        // It fits the game's one attachment buffer (docs/live-motd-fetch.md §5).
        assert!(file.len() <= live_proto::motd::MAX_FILE, "{}", file.len());
        assert_eq!(Motd::parse(&file).unwrap(), m);
    }
}
