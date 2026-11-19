//! Match files, bot tuning files and batch files (docs/runtime/sim.md §5,
//! §7). TOML, or JSON when the file name ends in `.json`. Every file is
//! read whole and checked before anything runs: [`load_match`] and
//! [`load_batch`] answer a list of problems, not the first one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::clock::Pace;

/// Halo 2 has four local players; the direct path is four bots
/// (docs/runtime/sim.md §5, "More than four").
pub const MAX_BOTS: usize = 4;

/// Variant settings are numbered `0..0x70` (`variants::record`).
pub const MAX_SETTING: u32 = 0x70;

/// The variant setting that holds the time limit, in seconds.
pub const TIME_LIMIT_SETTING: u32 = 10;
/// The variant setting that turns team play on.
pub const TEAM_PLAY_SETTING: u32 = 91;

/// A game type, by the name a match file gives it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GameType {
    #[default]
    Slayer,
    Oddball,
    Juggernaut,
    King,
    Ctf,
    Assault,
    Territories,
}

impl GameType {
    /// In the game's built-in list order (`variants::GameType::ALL`), which
    /// is the type number `VARIANT_DEFAULTS` (`0x19d220`) takes.
    pub const ALL: [GameType; 7] = [
        GameType::Slayer,
        GameType::Oddball,
        GameType::Juggernaut,
        GameType::King,
        GameType::Ctf,
        GameType::Assault,
        GameType::Territories,
    ];

    /// The built-in list index (0 Slayer .. 6 Territories).
    pub fn list_index(self) -> u32 {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0) as u32
    }

    pub fn name(self) -> &'static str {
        match self {
            GameType::Slayer => "slayer",
            GameType::Oddball => "oddball",
            GameType::Juggernaut => "juggernaut",
            GameType::King => "king",
            GameType::Ctf => "ctf",
            GameType::Assault => "assault",
            GameType::Territories => "territories",
        }
    }

    pub fn parse(s: &str) -> Result<GameType, String> {
        let k: String = s
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        Ok(match k.as_str() {
            "slayer" => GameType::Slayer,
            "oddball" | "ball" => GameType::Oddball,
            "juggernaut" => GameType::Juggernaut,
            "king" | "koth" | "kingofthehill" => GameType::King,
            "ctf" | "capturetheflag" => GameType::Ctf,
            "assault" => GameType::Assault,
            "territories" => GameType::Territories,
            _ => {
                return Err(format!(
                    "unknown game type {s:?} (slayer, oddball, juggernaut, king, ctf, assault, territories)"
                ));
            }
        })
    }

    /// The game is won by something other than kills.
    pub fn objective(self) -> bool {
        !matches!(self, GameType::Slayer | GameType::Juggernaut)
    }

    /// The variant setting that holds this type's score to win (all write
    /// `+0x50`; `variants::GameType::score_setting`).
    pub fn score_setting(self) -> u32 {
        match self {
            GameType::Ctf => 1,
            GameType::Slayer => 2,
            GameType::Oddball => 3,
            GameType::King => 4,
            GameType::Juggernaut => 7,
            GameType::Territories => 8,
            GameType::Assault => 9,
        }
    }
}

/// A bot's skill (`bots::mind::Skill`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Skill {
    Easy,
    #[default]
    Normal,
    Heroic,
    Legendary,
}

impl Skill {
    pub fn parse(s: &str) -> Result<Skill, String> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "easy" => Skill::Easy,
            "normal" => Skill::Normal,
            "heroic" => Skill::Heroic,
            "legendary" => Skill::Legendary,
            _ => {
                return Err(format!(
                    "unknown skill {s:?} (easy, normal, heroic, legendary)"
                ));
            }
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Skill::Easy => "easy",
            Skill::Normal => "normal",
            Skill::Heroic => "heroic",
            Skill::Legendary => "legendary",
        }
    }
}

/// What a bot is tuned by (docs/runtime/sim.md, "Bot parameters from a
/// file"). Every field is optional: one left out keeps the built-in value
/// for the bot's skill.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotParams {
    /// easy, normal, heroic or legendary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    /// Nominal reaction, ticks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reaction: Option<u32>,
    /// The tracking error's spread, radians.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<f32>,
    /// How much of the needed lead it gives a moving target, 0..1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<f32>,
    /// Fraction of full turn rate per radian of aim error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain: Option<f32>,
    /// Fires while the aim is this close to the target, degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fire_cone_deg: Option<f32>,
    /// `false`: hold the trigger. `true` with `burst_on`/`burst_off`: fire
    /// in bursts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burst: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burst_on: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burst_off: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grenades_per_minute: Option<f32>,
    /// The decisions' sense of the fight, 0..1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awareness: Option<f32>,
    /// The personality, each 0..1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggression: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caution: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teamwork: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective_focus: Option<f32>,
}

impl BotParams {
    /// `self` with every field `over` sets replaced by `over`'s.
    pub fn overlay(&self, over: &BotParams) -> BotParams {
        fn pick<T: Clone>(a: &Option<T>, b: &Option<T>) -> Option<T> {
            b.clone().or_else(|| a.clone())
        }
        BotParams {
            skill: pick(&self.skill, &over.skill),
            reaction: pick(&self.reaction, &over.reaction),
            error: pick(&self.error, &over.error),
            lead: pick(&self.lead, &over.lead),
            gain: pick(&self.gain, &over.gain),
            fire_cone_deg: pick(&self.fire_cone_deg, &over.fire_cone_deg),
            burst: pick(&self.burst, &over.burst),
            burst_on: pick(&self.burst_on, &over.burst_on),
            burst_off: pick(&self.burst_off, &over.burst_off),
            grenades_per_minute: pick(&self.grenades_per_minute, &over.grenades_per_minute),
            awareness: pick(&self.awareness, &over.awareness),
            aggression: pick(&self.aggression, &over.aggression),
            caution: pick(&self.caution, &over.caution),
            teamwork: pick(&self.teamwork, &over.teamwork),
            objective_focus: pick(&self.objective_focus, &over.objective_focus),
        }
    }

    /// What is wrong with it, if anything (`at` names it in the messages).
    pub fn problems(&self, at: &str) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(s) = &self.skill
            && let Err(e) = Skill::parse(s)
        {
            out.push(format!("{at}: {e}"));
        }
        let unit = [
            ("lead", self.lead),
            ("awareness", self.awareness),
            ("aggression", self.aggression),
            ("caution", self.caution),
            ("teamwork", self.teamwork),
            ("objective_focus", self.objective_focus),
        ];
        for (name, v) in unit {
            if let Some(v) = v
                && !(0.0..=1.0).contains(&v)
            {
                out.push(format!("{at}: {name} = {v}: 0 to 1"));
            }
        }
        let positive = [
            ("error", self.error),
            ("gain", self.gain),
            ("fire_cone_deg", self.fire_cone_deg),
            ("grenades_per_minute", self.grenades_per_minute),
        ];
        for (name, v) in positive {
            if let Some(v) = v
                && !(v.is_finite() && v >= 0.0)
            {
                out.push(format!("{at}: {name} = {v}: a number, 0 or more"));
            }
        }
        if let Some(c) = self.fire_cone_deg
            && c > 180.0
        {
            out.push(format!("{at}: fire_cone_deg = {c}: at most 180"));
        }
        if self.reaction.is_some_and(|r| r > 300) {
            out.push(format!("{at}: reaction: at most 300 ticks"));
        }
        let on_off = self.burst_on.is_some() || self.burst_off.is_some();
        if self.burst == Some(false) && on_off {
            out.push(format!(
                "{at}: burst = false holds the trigger; burst_on/burst_off make no sense with it"
            ));
        }
        if self.burst_on == Some(0) {
            out.push(format!("{at}: burst_on: at least 1 tick"));
        }
        out
    }

    /// The skill, `normal` when unset (call after [`problems`](Self::problems)).
    pub fn skill(&self) -> Skill {
        self.skill
            .as_deref()
            .and_then(|s| Skill::parse(s).ok())
            .unwrap_or_default()
    }

    /// Reads a tuning file (TOML or JSON).
    pub fn load(path: &Path) -> Result<BotParams, String> {
        parse_file(path)
    }
}

/// One `[[bot]]` of a match file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotSpec {
    /// Shown in the report (default `Clanker <port>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Overrides the tuning's skill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    /// The team in a team game (0 red, 1 blue, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<u8>,
    /// A tuning file, relative to the match file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<PathBuf>,
    /// Tuning inline; over the file's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tuning: Option<BotParams>,
    /// Which arm of an A/B batch the bot plays for (set by the batch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arm: Option<String>,
}

fn default_game_type() -> String {
    "slayer".into()
}

fn default_pace() -> String {
    "unpaced".into()
}

/// A match file (docs/runtime/sim.md, "Match file").
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchConfig {
    /// A multiplayer map on the disc, by file name (`lockout`).
    pub map: String,
    #[serde(default = "default_game_type")]
    pub game_type: String,
    /// Seconds; 0 or unset: the type's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_limit_s: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_play: Option<bool>,
    #[serde(default)]
    pub seed: u64,
    /// Stop after this many game ticks (30 a second).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ticks: Option<u64>,
    /// Stop after this long on the wall clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wall_s: Option<u64>,
    /// Run the world render every Nth world frame (0: never).
    #[serde(default)]
    pub render_every: u32,
    /// `unpaced`, `realtime` or `x<factor>`.
    #[serde(default = "default_pace")]
    pub pace: String,
    /// More variant settings, by id (`"83" = 1`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, i32>,
    #[serde(default, rename = "bot")]
    pub bots: Vec<BotSpec>,
}

/// A bot as a run uses it: everything resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct Bot {
    /// Its controller port, which is its place in the match file.
    pub port: u8,
    pub name: String,
    pub skill: Skill,
    pub team: Option<u8>,
    /// The tuning file and the inline tuning, the skill folded in.
    pub params: BotParams,
    pub arm: Option<String>,
    /// The brain's random seed: from the match seed and the port.
    pub seed: u64,
}

/// A match file, read and checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub config: MatchConfig,
    pub game_type: GameType,
    pub pace: Pace,
    pub bots: Vec<Bot>,
    /// Every variant setting the match writes, by id: the time limit, the
    /// score limit and team play first, then `settings`.
    pub variant: Vec<(u32, i32)>,
    pub team_play: bool,
}

impl Match {
    /// The match as JSON for the report: the file with every bot's tuning
    /// resolved inline (no paths), so a report says what was played.
    pub fn resolved_json(&self) -> serde_json::Value {
        let mut c = self.config.clone();
        c.game_type = self.game_type.name().into();
        c.team_play = Some(self.team_play);
        c.bots = self
            .bots
            .iter()
            .map(|b| BotSpec {
                name: Some(b.name.clone()),
                skill: Some(b.skill.name().into()),
                team: b.team,
                params: None,
                tuning: Some(b.params.clone()),
                arm: b.arm.clone(),
            })
            .collect();
        serde_json::to_value(c).unwrap_or(serde_json::Value::Null)
    }
}

/// The brain seed of the bot on `port` in a match with `seed`
/// (SplitMix64 of the two, so neighbouring seeds and ports differ).
pub fn bot_seed(seed: u64, port: u8) -> u64 {
    let mut x = seed ^ (u64::from(port) + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn parse_text<T: serde::de::DeserializeOwned>(text: &str, json: bool) -> Result<T, String> {
    if json {
        serde_json::from_str(text).map_err(|e| e.to_string())
    } else {
        toml::from_str(text).map_err(|e| e.to_string())
    }
}

fn is_json(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
}

fn parse_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_text(&text, is_json(path)).map_err(|e| format!("{}: {e}", path.display()))
}

/// Parses a match file's text (TOML, or JSON with `json`).
pub fn parse_match(text: &str, json: bool) -> Result<MatchConfig, String> {
    parse_text(text, json)
}

/// Reads and checks a match file; tuning files are read relative to it.
pub fn load_match(path: &Path) -> Result<Match, String> {
    let config: MatchConfig = parse_file(path)?;
    let base = path.parent().unwrap_or(Path::new("."));
    resolve(config, &|p: &Path| BotParams::load(&base.join(p)))
}

/// Checks `config` and resolves its bots; `read` loads a tuning file by
/// the path the match file gives. Every problem is listed.
pub fn resolve(
    config: MatchConfig,
    read: &dyn Fn(&Path) -> Result<BotParams, String>,
) -> Result<Match, String> {
    let mut problems = Vec::new();
    if config.map.trim().is_empty() {
        problems.push("map: empty".to_string());
    }
    let game_type = GameType::parse(&config.game_type).unwrap_or_else(|e| {
        problems.push(format!("game_type: {e}"));
        GameType::Slayer
    });
    let pace = Pace::parse(&config.pace).unwrap_or_else(|e| {
        problems.push(format!("pace: {e}"));
        Pace::Unpaced
    });
    if config.bots.is_empty() {
        problems.push("no [[bot]]: a match needs 1 to 4 bots".to_string());
    }
    if config.bots.len() > MAX_BOTS {
        problems.push(format!(
            "{} bots: the direct path has {MAX_BOTS} local players at most; 5 to 16 need a System Link game, which sim mode does not host yet (docs/runtime/sim.md §5)",
            config.bots.len()
        ));
    }
    if config.max_ticks == Some(0) {
        problems.push("max_ticks = 0: leave it out for no limit".to_string());
    }
    let team_play = config.team_play.unwrap_or(false);
    let mut variant = Vec::new();
    if let Some(t) = config.time_limit_s {
        if t > 0x7fff {
            problems.push(format!("time_limit_s = {t}: at most 32767"));
        }
        variant.push((TIME_LIMIT_SETTING, t.min(0x7fff) as i32));
    }
    if let Some(s) = config.score_limit {
        variant.push((game_type.score_setting(), s.min(i32::MAX as u32) as i32));
    }
    if config.team_play.is_some() {
        variant.push((TEAM_PLAY_SETTING, i32::from(team_play)));
    }
    for (k, v) in &config.settings {
        match k.trim().parse::<u32>() {
            Ok(id) if id < MAX_SETTING => {
                if variant.iter().any(|(i, _)| *i == id) {
                    problems.push(format!("settings.{k}: set by the match's own fields too"));
                }
                variant.push((id, *v));
            }
            _ => problems.push(format!(
                "settings.{k}: a setting id, 0 to {}",
                MAX_SETTING - 1
            )),
        }
    }
    let mut bots = Vec::new();
    for (i, spec) in config.bots.iter().enumerate().take(MAX_BOTS) {
        let at = format!("bot {}", i + 1);
        let port = i as u8;
        let file = match &spec.params {
            Some(p) => read(p).unwrap_or_else(|e| {
                problems.push(format!("{at}: params: {e}"));
                BotParams::default()
            }),
            None => BotParams::default(),
        };
        let mut params = match &spec.tuning {
            Some(t) => file.overlay(t),
            None => file,
        };
        if let Some(s) = &spec.skill {
            params.skill = Some(s.clone());
        }
        problems.extend(params.problems(&at));
        let skill = params.skill();
        params.skill = Some(skill.name().into());
        if let Some(t) = spec.team
            && t >= 8
        {
            problems.push(format!("{at}: team {t}: 0 to 7"));
        }
        let team = if team_play {
            Some(spec.team.unwrap_or(port % 2))
        } else {
            spec.team
        };
        bots.push(Bot {
            port,
            name: spec
                .name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| format!("Clanker {port}")),
            skill,
            team,
            params,
            arm: spec.arm.clone(),
            seed: bot_seed(config.seed, port),
        });
    }
    if team_play && bots.len() >= 2 {
        let first = bots[0].team;
        if bots.iter().all(|b| b.team == first) {
            problems.push("team_play: every bot is on one team; nobody to fight".to_string());
        }
    }
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    Ok(Match {
        config,
        game_type,
        pace,
        bots,
        variant,
        team_play,
    })
}

// ---- batches ----

/// One arm of an A/B batch.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Arm {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A tuning file, relative to the batch file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tuning: Option<BotParams>,
}

/// How a batch splits the bots between its arms.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// A and B in the same match: teams A and B in a team game, every
    /// other bot in free-for-all.
    #[default]
    Versus,
    /// Every match twice: all bots A, then all bots B.
    Separate,
}

impl Mode {
    pub fn parse(s: &str) -> Result<Mode, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "versus" | "vs" => Ok(Mode::Versus),
            "separate" => Ok(Mode::Separate),
            _ => Err(format!("unknown mode {s:?} (versus, separate)")),
        }
    }
}

/// A batch file (docs/runtime/sim.md §7).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchConfig {
    /// The results directory, relative to the batch file.
    pub out: PathBuf,
    /// Processes at once (default: half the cores).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel: Option<usize>,
    /// The match file every run starts from, relative to the batch file.
    #[serde(rename = "match")]
    pub match_file: PathBuf,
    /// One run per map (default: the match's).
    #[serde(default)]
    pub maps: Vec<String>,
    /// One run per seed (default: the match's).
    #[serde(default)]
    pub seeds: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Fork every run from a snapshot at spawn (docs/runtime/sim.md, "Forking
    /// from spawn"), falling back to a fresh boot.
    #[serde(default)]
    pub fork: bool,
    pub a: Arm,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b: Option<Arm>,
}

/// An arm with its tuning read.
#[derive(Clone, Debug, PartialEq)]
pub struct ArmParams {
    pub name: String,
    pub params: BotParams,
}

/// One run of a batch.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    /// `<map>-s<seed>` plus `-a`/`-b` in separate mode: the run's
    /// directory under `<out>/runs/`.
    pub id: String,
    pub map: String,
    pub seed: u64,
    /// The match file the run plays: self-contained (tunings inline).
    pub config: MatchConfig,
}

/// A batch, read and planned.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    pub config: BatchConfig,
    pub out: PathBuf,
    pub mode: Mode,
    pub arms: Vec<ArmParams>,
    pub runs: Vec<Run>,
}

/// Reads a batch file, its match file and its arms' tuning files, and plans
/// its runs.
pub fn load_batch(path: &Path) -> Result<Batch, String> {
    let config: BatchConfig = parse_file(path)?;
    let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let match_path = base.join(&config.match_file);
    let base_match: MatchConfig = parse_file(&match_path)?;
    let match_dir = match_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let read_arm = |p: &Path| BotParams::load(&base.join(p));
    let read_bot = |p: &Path| BotParams::load(&match_dir.join(p));
    plan(config, base_match, &read_arm, &read_bot, &base)
}

/// Plans a batch's runs: maps × seeds (× arms in separate mode). Every
/// run's match file has its bots' tunings inline, the arm's over the
/// bot's own, so a run directory is self-contained.
pub fn plan(
    config: BatchConfig,
    base_match: MatchConfig,
    read_arm: &dyn Fn(&Path) -> Result<BotParams, String>,
    read_bot: &dyn Fn(&Path) -> Result<BotParams, String>,
    base: &Path,
) -> Result<Batch, String> {
    let mut problems = Vec::new();
    let mode = match config.mode.as_deref() {
        Some(m) => Mode::parse(m).unwrap_or_else(|e| {
            problems.push(e);
            Mode::Versus
        }),
        None => Mode::Versus,
    };
    let arm = |a: &Arm, default: &str, problems: &mut Vec<String>| {
        let file = match &a.params {
            Some(p) => read_arm(p).unwrap_or_else(|e| {
                problems.push(format!("arm {default}: params: {e}"));
                BotParams::default()
            }),
            None => BotParams::default(),
        };
        let params = match &a.tuning {
            Some(t) => file.overlay(t),
            None => file,
        };
        problems.extend(params.problems(&format!("arm {default}")));
        ArmParams {
            name: a.name.clone().unwrap_or_else(|| default.to_string()),
            params,
        }
    };
    let mut arms = vec![arm(&config.a, "a", &mut problems)];
    if let Some(b) = &config.b {
        arms.push(arm(b, "b", &mut problems));
    }
    if arms.len() == 2 && arms[0].name == arms[1].name {
        problems.push(format!("arms a and b are both named {:?}", arms[0].name));
    }
    if base_match.bots.is_empty() {
        problems.push("the match file has no [[bot]]".to_string());
    }
    if mode == Mode::Versus && arms.len() == 2 && base_match.bots.len() < 2 {
        problems.push("versus needs at least two bots, one per arm".to_string());
    }
    if config.parallel == Some(0) {
        problems.push("parallel = 0: at least 1".to_string());
    }
    // The bots' own tuning files, read once.
    let mut own = Vec::new();
    for (i, b) in base_match.bots.iter().enumerate() {
        let file = match &b.params {
            Some(p) => read_bot(p).unwrap_or_else(|e| {
                problems.push(format!("bot {}: params: {e}", i + 1));
                BotParams::default()
            }),
            None => BotParams::default(),
        };
        own.push(match &b.tuning {
            Some(t) => file.overlay(t),
            None => file,
        });
    }
    let maps = if config.maps.is_empty() {
        vec![base_match.map.clone()]
    } else {
        config.maps.clone()
    };
    let seeds = if config.seeds.is_empty() {
        vec![base_match.seed]
    } else {
        config.seeds.clone()
    };
    let team_play = base_match.team_play.unwrap_or(false);
    // Which arm each bot plays for, per run kind.
    let versus = |i: usize, b: &BotSpec| -> usize {
        if arms.len() < 2 {
            return 0;
        }
        if team_play {
            usize::from(b.team.unwrap_or((i % 2) as u8) % 2)
        } else {
            i % 2
        }
    };
    let mut runs = Vec::new();
    let kinds: Vec<Option<usize>> = match (mode, arms.len()) {
        (Mode::Separate, 2) => vec![Some(0), Some(1)],
        _ => vec![None],
    };
    for map in &maps {
        for &seed in &seeds {
            for kind in &kinds {
                let mut c = base_match.clone();
                c.map = map.clone();
                c.seed = seed;
                c.bots = base_match
                    .bots
                    .iter()
                    .enumerate()
                    .map(|(i, b)| {
                        let a = kind.unwrap_or_else(|| versus(i, b));
                        let arm = &arms[a];
                        let mut spec = b.clone();
                        spec.params = None;
                        spec.tuning = Some(own[i].overlay(&arm.params));
                        if let Some(s) = &arm.params.skill {
                            spec.skill = Some(s.clone());
                        }
                        if team_play && spec.team.is_none() {
                            spec.team = Some((i % 2) as u8);
                        }
                        spec.arm = Some(arm.name.clone());
                        spec
                    })
                    .collect();
                let id = match kind {
                    Some(a) => format!("{}-s{seed}-{}", safe(map), safe(&arms[*a].name)),
                    None => format!("{}-s{seed}", safe(map)),
                };
                // Each planned match must itself check.
                if let Err(e) = resolve(c.clone(), &|_| Ok(BotParams::default())) {
                    problems.push(format!("run {id}: {}", e.replace('\n', "; ")));
                }
                runs.push(Run {
                    id,
                    map: map.clone(),
                    seed,
                    config: c,
                });
            }
        }
    }
    let mut ids: Vec<&str> = runs.iter().map(|r| r.id.as_str()).collect();
    ids.sort_unstable();
    if ids.windows(2).any(|w| w[0] == w[1]) {
        problems.push("two runs share an id: repeated maps or seeds".to_string());
    }
    if !problems.is_empty() {
        problems.dedup();
        return Err(problems.join("\n"));
    }
    let out = base.join(&config.out);
    Ok(Batch {
        config,
        out,
        mode,
        arms,
        runs,
    })
}

/// `[A-Za-z0-9_-]` of a name, for directory names.
pub fn safe(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if t.is_empty() { "_".into() } else { t }
}

#[cfg(test)]
mod tests;
