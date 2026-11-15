//! Routing chosen game sounds to a controller speaker.
//!
//! DirectSound sees sample bytes, not sound tags. Halo 2 hands the shim the
//! encoded permutation bytes it loaded from the map (`SetBufferData`, or
//! stream packets out of its sound cache), so a sound can be recognised by
//! the bytes it starts with. [`SoundIndex`] maps a fingerprint of a
//! permutation's first [`PREFIX_BYTES`] to the tag that owns it, built at run
//! time from the player's own map files (`tags::sound`), and [`RouteTable`]
//! turns tag names into a [`VoiceOutput`]. Nothing here holds game data:
//! names and bytes come from the player's disc when the game runs.
//!
//! A stream is a reusable voice, so its label follows the packets
//! ([`next_output`]): a packet that starts a known sound labels the stream,
//! the packets after it (the rest of that sound) keep the label, and an
//! unknown packet on an idle stream, or a flush, clears it.
//!
//! See `docs/playstation-audio.md`.

use std::collections::HashMap;

use audio::VoiceOutput;
use serde::Deserialize;

/// Bytes of a permutation's start that identify it: 16 mono or 8 stereo
/// Xbox ADPCM blocks (36 bytes per channel), about 23 ms at 44.1 kHz. A
/// permutation shorter than this is identified by all of its bytes.
pub const PREFIX_BYTES: usize = 576;

/// FNV-1a 64 over the identifying prefix of `bytes`, mixed with its length.
/// A packet shorter than [`PREFIX_BYTES`] matches only a permutation of
/// exactly that length.
pub fn fingerprint(bytes: &[u8]) -> u64 {
    let p = &bytes[..bytes.len().min(PREFIX_BYTES)];
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in p.iter().chain(&(p.len() as u32).to_le_bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Which tag a fingerprint belongs to: one tag, or several that share the
/// same opening bytes (silence, a shared attack).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Owner {
    One(u32),
    Many(Vec<u32>),
}

/// Fingerprints of every permutation of the loaded maps, to tag names.
#[derive(Clone, Debug, Default)]
pub struct SoundIndex {
    names: Vec<String>,
    by_name: HashMap<String, u32>,
    by_print: HashMap<u64, Owner>,
}

impl SoundIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that tag `name` has a permutation whose encoded bytes begin
    /// with `prefix` (at least [`PREFIX_BYTES`] of it, or all of a shorter
    /// permutation). The same tag may be added from several maps.
    pub fn insert(&mut self, name: &str, prefix: &[u8]) {
        let id = match self.by_name.get(name) {
            Some(&id) => id,
            None => {
                let id = self.names.len() as u32;
                self.names.push(name.to_string());
                self.by_name.insert(name.to_string(), id);
                id
            }
        };
        let print = fingerprint(prefix);
        match self.by_print.get_mut(&print) {
            None => {
                self.by_print.insert(print, Owner::One(id));
            }
            Some(Owner::One(o)) if *o == id => {}
            Some(Owner::One(o)) => {
                let o = *o;
                self.by_print.insert(print, Owner::Many(vec![o, id]));
            }
            Some(Owner::Many(v)) => {
                if !v.contains(&id) {
                    v.push(id);
                }
            }
        }
    }

    /// Distinct tags indexed.
    pub fn tag_count(&self) -> usize {
        self.names.len()
    }

    /// The tag names whose permutations start with these bytes (empty if
    /// none; several if they share an opening).
    pub fn lookup(&self, bytes: &[u8]) -> Vec<&str> {
        match self.by_print.get(&fingerprint(bytes)) {
            None => Vec::new(),
            Some(Owner::One(id)) => vec![self.names[*id as usize].as_str()],
            Some(Owner::Many(v)) => v
                .iter()
                .map(|&id| self.names[id as usize].as_str())
                .collect(),
        }
    }
}

/// A `*` glob on tag names, ignoring ASCII case and treating `/` as `\`
/// (tag paths use backslashes). Without a `*` the pattern must match the
/// whole name.
fn glob(pattern: &str, name: &str) -> bool {
    let norm = |s: &str| s.to_ascii_lowercase().replace('/', "\\");
    let (p, n) = (norm(pattern), norm(name));
    let parts: Vec<&str> = p.split('*').collect();
    if parts.len() == 1 {
        return p == n;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !n.starts_with(first) || n.len() < first.len() + last.len() || !n.ends_with(last) {
        return false;
    }
    let mut rest = &n[first.len()..n.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(i) => rest = &rest[i + mid.len()..],
            None => return false,
        }
    }
    true
}

/// The controller-speaker settings: `[pad_speaker]` in the settings file, or
/// a file of its own (`H2_PAD_SPEAKER=<path>`). Unknown keys are errors, as
/// in `docs/settings-menu.md`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PadSpeakerConfig {
    /// On unless turned off (`H2_PAD_SPEAKER=0`). With no PlayStation
    /// controller attached every sound stays on the main output.
    pub enabled: bool,
    /// Output device name pattern; `auto` finds a DualSense, then a DS4.
    pub device: String,
    /// Routed sounds play only on the controller (true) or on both.
    pub exclusive: bool,
    /// Tag name globs to send to the controller.
    pub routes: Vec<String>,
    /// Tag name globs never sent, even if a route matches.
    pub exclude: Vec<String>,
}

/// Starting patterns: the player's own sounds. Shields, reloads, the
/// empty-magazine click, skulls and the round-start cues; the player
/// character's own sounds (pain, death, boarding, melee), jumps and
/// landings; and first-person weapon handling: melee, raising a weapon
/// (the sword opening is its raise), inspection poses, dry fire, grenade
/// throws, zoom and overheating. Walking and running footsteps and gunfire
/// stay on the main output. Routing is by sound name, not by who plays it,
/// so an enemy playing one of these sounds is routed too. These are generic
/// words, not tag paths read from the disc; check them against your own
/// maps with `h2 tags <map> snd!` and adjust (`docs/playstation-audio.md`).
pub const DEFAULT_ROUTES: &[&str] = &[
    "*shield*",
    "*reload*",
    "*empty*",
    "*skull*",
    "*countdown*",
    "*game_start*",
    "*masterchief*",
    "*chief*jump*",
    "*chief*land*",
    "*weapons*melee*",
    "*weapons*ready*",
    "*weapons*pos*",
    "*sword*melee*",
    "*dryfire*",
    "*throwgren*",
    "*weapons*zoom*",
    "*overheat*",
];

impl Default for PadSpeakerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            device: "auto".to_string(),
            exclusive: true,
            routes: DEFAULT_ROUTES.iter().map(|s| s.to_string()).collect(),
            exclude: Vec::new(),
        }
    }
}

/// A bad settings file or environment value.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("pad speaker settings: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("pad speaker settings: cannot read {path}: {err}")]
    Read { path: String, err: std::io::Error },
    #[error("{var}: expected 0 or 1, got {value:?}")]
    Bool { var: &'static str, value: String },
}

impl PadSpeakerConfig {
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        Ok(toml::from_str(text)?)
    }

    /// The settings from the environment, through `var` (so tests need not
    /// touch the process environment):
    ///
    /// - `H2_PAD_SPEAKER`: unset or `1` is on with the defaults, `0` off,
    ///   any other value is a TOML file of these settings (on unless the
    ///   file says `enabled = false`);
    /// - `H2_PAD_SPEAKER_DEVICE`: the device pattern;
    /// - `H2_PAD_SPEAKER_EXCLUSIVE`: `0` plays routed sounds on both outputs.
    ///
    /// Environment beats file, file beats default.
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let mut c = match var("H2_PAD_SPEAKER").as_deref().map(str::trim) {
            None | Some("" | "1") => Self::default(),
            Some("0") => Self {
                enabled: false,
                ..Self::default()
            },
            Some(path) => {
                let text = std::fs::read_to_string(path).map_err(|err| ConfigError::Read {
                    path: path.to_string(),
                    err,
                })?;
                Self::from_toml(&text)?
            }
        };
        if let Some(d) = var("H2_PAD_SPEAKER_DEVICE").filter(|d| !d.trim().is_empty()) {
            c.device = d;
        }
        if let Some(v) = var("H2_PAD_SPEAKER_EXCLUSIVE") {
            c.exclusive = match v.trim() {
                "1" => true,
                "0" => false,
                _ => {
                    return Err(ConfigError::Bool {
                        var: "H2_PAD_SPEAKER_EXCLUSIVE",
                        value: v,
                    });
                }
            };
        }
        Ok(c)
    }
}

/// Tag name globs to outputs.
#[derive(Clone, Debug)]
pub struct RouteTable {
    routes: Vec<String>,
    exclude: Vec<String>,
    routed: VoiceOutput,
}

impl RouteTable {
    pub fn new(config: &PadSpeakerConfig) -> Self {
        Self {
            routes: config.routes.clone(),
            exclude: config.exclude.clone(),
            routed: if config.exclusive {
                VoiceOutput::Side
            } else {
                VoiceOutput::Both
            },
        }
    }

    /// Where the tag `name` is heard.
    pub fn output_for(&self, name: &str) -> VoiceOutput {
        if self.exclude.iter().any(|p| glob(p, name)) || !self.routes.iter().any(|p| glob(p, name))
        {
            VoiceOutput::Main
        } else {
            self.routed
        }
    }
}

/// What a packet or buffer's bytes say about the sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Classified {
    /// The start of a known sound, heard on this output. Sounds whose
    /// opening several tags share are routed only if every one of them
    /// routes the same way; otherwise they stay on the main mix.
    Start(VoiceOutput),
    /// Not the start of any indexed sound (a continuation, or unknown).
    Unknown,
}

/// A [`SoundIndex`] and a [`RouteTable`], resolved once per fingerprint.
#[derive(Clone, Debug)]
pub struct PadRouter {
    by_print: HashMap<u64, VoiceOutput>,
}

impl PadRouter {
    pub fn new(index: &SoundIndex, table: &RouteTable) -> Self {
        let by_print = index
            .by_print
            .iter()
            .map(|(&print, owner)| {
                let ids: &[u32] = match owner {
                    Owner::One(id) => std::slice::from_ref(id),
                    Owner::Many(v) => v,
                };
                let mut outs = ids
                    .iter()
                    .map(|&id| table.output_for(&index.names[id as usize]));
                let first = outs.next().unwrap_or(VoiceOutput::Main);
                let out = if outs.all(|o| o == first) {
                    first
                } else {
                    VoiceOutput::Main
                };
                (print, out)
            })
            .collect();
        Self { by_print }
    }

    pub fn classify(&self, bytes: &[u8]) -> Classified {
        if bytes.is_empty() {
            return Classified::Unknown;
        }
        self.by_print
            .get(&fingerprint(bytes))
            .map_or(Classified::Unknown, |&o| Classified::Start(o))
    }

    /// Fingerprints that route somewhere other than the main mix.
    pub fn routed_count(&self) -> usize {
        self.by_print
            .values()
            .filter(|&&o| o != VoiceOutput::Main)
            .count()
    }
}

/// A stream's output after a packet: a recognised start sets it; anything
/// else keeps the current label while the stream is still playing earlier
/// packets of that sound, and clears it on an idle stream.
pub fn next_output(current: VoiceOutput, packet: Classified, stream_idle: bool) -> VoiceOutput {
    match packet {
        Classified::Start(o) => o,
        Classified::Unknown if stream_idle => VoiceOutput::Main,
        Classified::Unknown => current,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic "encoded sound": deterministic bytes from a seed.
    fn sound(seed: u8, len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| {
                (i as u8)
                    .wrapping_mul(31)
                    .wrapping_add(seed.wrapping_mul(97))
            })
            .collect()
    }

    fn router(index: &SoundIndex, config: &PadSpeakerConfig) -> PadRouter {
        PadRouter::new(index, &RouteTable::new(config))
    }

    #[test]
    fn fingerprint_uses_the_prefix_and_short_lengths() {
        let a = sound(1, 4000);
        let mut b = a.clone();
        b[PREFIX_BYTES + 10] ^= 0xff; // past the prefix: same sound
        assert_eq!(fingerprint(&a), fingerprint(&b));
        b[3] ^= 1;
        assert_ne!(fingerprint(&a), fingerprint(&b));
        // A short sound is all of its bytes, and its length counts.
        let short = sound(2, 72);
        assert_ne!(fingerprint(&short), fingerprint(&short[..36]));
        assert_eq!(fingerprint(&a[..PREFIX_BYTES]), fingerprint(&a));
    }

    #[test]
    fn globs() {
        assert!(glob(
            "*shield*",
            r"sound\characters\masterchief\shield_down"
        ));
        assert!(glob("*SHIELD*", r"Sound\Shield"));
        assert!(glob(
            "sound/weapons/*reload*",
            r"sound\weapons\smg\reload_full"
        ));
        assert!(!glob("sound/weapons/*reload*", r"sound\vehicles\reload"));
        assert!(glob("exact", "EXACT"));
        assert!(!glob("exact", "exactly"));
        assert!(glob("a*b*c", "aXbYc"));
        assert!(!glob("a*b*c", "aXcYb"));
        assert!(!glob("ab*ba", "aba"));
        assert!(glob("*", "anything"));
    }

    #[test]
    fn packets_find_their_tag_and_route() {
        let mut idx = SoundIndex::new();
        let (reload, music, click) = (sound(1, 9000), sound(2, 9000), sound(3, 108));
        idx.insert(r"fx\weapons\rifle\reload", &reload[..PREFIX_BYTES]);
        idx.insert(r"music\level\loop", &music[..PREFIX_BYTES]);
        idx.insert(r"fx\weapons\rifle\empty", &click);
        assert_eq!(idx.tag_count(), 3);
        let cfg = PadSpeakerConfig {
            enabled: true,
            ..PadSpeakerConfig::default()
        };
        let r = router(&idx, &cfg);
        // A packet carrying the whole permutation, or its first 0x7ff8 chunk.
        assert_eq!(r.classify(&reload), Classified::Start(VoiceOutput::Side));
        assert_eq!(
            r.classify(&reload[..PREFIX_BYTES * 3]),
            Classified::Start(VoiceOutput::Side)
        );
        assert_eq!(r.classify(&music), Classified::Start(VoiceOutput::Main));
        assert_eq!(r.classify(&click), Classified::Start(VoiceOutput::Side));
        // The middle of a sound, a padded short sound and nothing at all.
        assert_eq!(r.classify(&reload[PREFIX_BYTES..]), Classified::Unknown);
        assert_eq!(
            r.classify(&[click.clone(), vec![0; 36]].concat()),
            Classified::Unknown
        );
        assert_eq!(r.classify(&[]), Classified::Unknown);
        assert_eq!(r.routed_count(), 2);
        // Both, when not exclusive; exclusions win over routes.
        let both = PadSpeakerConfig {
            exclusive: false,
            exclude: vec!["*empty".into()],
            ..cfg
        };
        let r = router(&idx, &both);
        assert_eq!(r.classify(&reload), Classified::Start(VoiceOutput::Both));
        assert_eq!(r.classify(&click), Classified::Start(VoiceOutput::Main));
    }

    #[test]
    fn shared_openings_route_only_when_every_owner_agrees() {
        let silence = vec![0u8; PREFIX_BYTES];
        let mut idx = SoundIndex::new();
        idx.insert(r"fx\shield_recharge", &silence);
        idx.insert(r"fx\shield_depleted", &silence);
        idx.insert(r"fx\shield_recharge", &silence); // again, from another map
        let cfg = PadSpeakerConfig::default();
        assert_eq!(idx.lookup(&silence).len(), 2);
        assert_eq!(
            router(&idx, &cfg).classify(&silence),
            Classified::Start(VoiceOutput::Side)
        );
        idx.insert(r"dialog\cortana\line", &silence);
        assert_eq!(idx.lookup(&silence).len(), 3);
        assert_eq!(
            router(&idx, &cfg).classify(&silence),
            Classified::Start(VoiceOutput::Main)
        );
    }

    #[test]
    fn stream_label_follows_the_sound() {
        use Classified::*;
        use VoiceOutput::*;
        // A routed sound starts, its continuation keeps the label...
        let o = next_output(Main, Start(Side), true);
        let o = next_output(o, Unknown, false);
        assert_eq!(o, Side);
        // ...and an unknown sound on the idle stream clears it.
        assert_eq!(next_output(o, Unknown, true), Main);
        // A known unrouted sound clears it at once, even queued behind.
        assert_eq!(next_output(Side, Start(Main), false), Main);
    }

    #[test]
    fn config_from_env_and_toml() {
        let none = |_: &str| None;
        assert_eq!(
            PadSpeakerConfig::from_env(none).unwrap(),
            PadSpeakerConfig::default()
        );
        assert!(PadSpeakerConfig::default().enabled, "on by default");
        let off = PadSpeakerConfig::from_env(|k| (k == "H2_PAD_SPEAKER").then(|| "0".into())).unwrap();
        assert!(!off.enabled);
        let on = PadSpeakerConfig::from_env(|k| match k {
            "H2_PAD_SPEAKER" => Some("1".into()),
            "H2_PAD_SPEAKER_DEVICE" => Some("Wireless Controller".into()),
            "H2_PAD_SPEAKER_EXCLUSIVE" => Some("0".into()),
            _ => None,
        })
        .unwrap();
        assert!(on.enabled && !on.exclusive);
        assert_eq!(on.device, "Wireless Controller");
        assert!(matches!(
            PadSpeakerConfig::from_env(|k| (k == "H2_PAD_SPEAKER_EXCLUSIVE").then(|| "yes".into())),
            Err(ConfigError::Bool { .. })
        ));

        let c = PadSpeakerConfig::from_toml("enabled = true\nroutes = ['*grenade*']\n").unwrap();
        assert!(c.enabled && c.exclusive);
        assert_eq!(c.routes, vec!["*grenade*"]);
        assert!(PadSpeakerConfig::from_toml("enabld = true").is_err());

        let dir = std::env::temp_dir().join(format!("padroute-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pad.toml");
        std::fs::write(&path, "exclude = ['*music*']\n").unwrap();
        let p = path.to_str().unwrap().to_string();
        let f = PadSpeakerConfig::from_env(|k| (k == "H2_PAD_SPEAKER").then(|| p.clone())).unwrap();
        assert!(
            f.enabled,
            "a settings file turns it on unless it says otherwise"
        );
        assert_eq!(f.exclude, vec!["*music*"]);
        std::fs::write(&path, "enabled = false\n").unwrap();
        let f = PadSpeakerConfig::from_env(|k| (k == "H2_PAD_SPEAKER").then(|| p.clone())).unwrap();
        assert!(!f.enabled);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
