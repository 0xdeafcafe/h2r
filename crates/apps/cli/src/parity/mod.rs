//! Behavioural parity uses the controller connection for the entire case.
mod biped;
mod weapon;
use agent_proto::Client;
use anyhow::{Context, Result, bail, ensure};
use h1map::{H1Map, Tag};
use h1objects::parity::Rules;
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{BufReader, Cursor},
    path::Path,
    time::Duration,
};

const RIFLE: &str = r"weapons\assault rifle\assault rifle";

struct Table {
    failed: usize,
    rows: usize,
}
impl Table {
    fn row(
        &mut self,
        check: &str,
        source: impl std::fmt::Display,
        observed: impl std::fmt::Display,
        pass: bool,
    ) {
        self.rows += 1;
        self.failed += usize::from(!pass);
        println!(
            "{}\t{}\t{}\t{}",
            check,
            source,
            observed,
            if pass { "PASS" } else { "FAIL" }
        );
    }
    fn error(&mut self, check: &str, e: anyhow::Error) {
        self.row(check, "required", format!("{e:#}"), false);
    }
}

struct Case {
    client: Client,
    h2: map::Map<File>,
    h1: H1Map,
    rules: Rules,
    h2_sandbox: bool,
    mp: bool,
    table: Table,
    rifle_projectile: Option<u32>,
}
impl Case {
    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.client
            .call(method, params)
            .with_context(|| method.to_owned())
    }
    fn wait(&mut self, ticks: u64) -> Result<()> {
        let now = self.call(agent_proto::method::STATUS, json!({}))?["at"]["tick"]
            .as_u64()
            .context("no game tick")?;
        self.call(
            agent_proto::method::WAIT,
            json!({"until":{"tick":now+ticks},"timeout_ms":60000}),
        )?;
        Ok(())
    }
    fn player(&mut self) -> Result<Value> {
        let v = self.call(agent_proto::method::WORLD_QUERY, json!({"kind":["player"]}))?;
        let p = v["objects"]
            .as_array()
            .context("world.query objects")?
            .iter()
            .find(|p| p["id"] == "player.0")
            .context("player.0 missing")?;
        ensure!(p["unit"].is_string(), "player.0 has no live unit");
        Ok(p.clone())
    }

    fn weapon_name(&self, path: &str) -> Result<String> {
        if !self.h2_sandbox {
            return Ok(path.to_owned());
        }
        use h1convert::sandbox::{H2Item, h2_equivalent};
        let Some(H2Item::Weapon(role)) = h2_equivalent(path) else {
            bail!("{path}: no Halo 2 sandbox weapon role")
        };
        let mut candidates = self
            .h2
            .tags()
            .iter()
            .filter(|t| t.group == map::FourCC(*b"weap"))
            .filter_map(|t| t.name.as_deref())
            .filter(|n| n.rsplit('\\').next() == Some(role));
        let name = candidates
            .next()
            .with_context(|| format!("no weap for sandbox role {role}"))?;
        ensure!(
            candidates.next().is_none(),
            "ambiguous weap for role {role}"
        );
        Ok(name.to_owned())
    }

    fn round(&mut self) -> Result<()> {
        let bots = self.call(agent_proto::sim::method::PLAYER_LIST, json!({}))?;
        let round = self.call(agent_proto::probe::method::ROUND, json!({}))?;
        ensure!(round["error"].is_null(), "{}", round["error"]);
        let rows = round["players"].as_array().context("round players")?;
        let first = rows
            .iter()
            .find(|r| r["player"] == 0)
            .and_then(|r| r["first_control"].as_u64())
            .context("no applied control packet for player.0 since round start")?;
        let bots = bots["clankers"]
            .as_array()
            .context("clanker roster missing")?;
        ensure!(!bots.is_empty(), "no bots joined at round start");
        let mut opponents = 0;
        for bot in bots {
            let id = bot["player"]
                .as_str()
                .context("bot has no player")?
                .strip_prefix("player.")
                .context("bot player id")?
                .parse::<u64>()?;
            if id == 0 {
                continue;
            }
            opponents += 1;
            let acted = rows
                .iter()
                .find(|r| r["player"] == id)
                .and_then(|r| r["first_action"].as_u64());
            self.table.row(
                &format!("round player.{id} first movement/fire"),
                format!("at or after player control {first}"),
                json!(acted),
                acted.is_some_and(|t| t >= first),
            );
        }
        ensure!(opponents > 0, "no second player under bot control");
        Ok(())
    }

    fn checks(&mut self, base: &str, round: bool) -> Result<()> {
        if self.mp
            && round
            && let Err(e) = self.round()
        {
            self.table.error("round control", e);
        }
        let guns: Vec<_> = self
            .h1
            .tags()
            .iter()
            .filter(|t| t.group == h1map::Group::new(b"weap"))
            .cloned()
            .collect();
        ensure!(!guns.is_empty(), "no source weapons");
        for gun in guns {
            self.call(agent_proto::state::method::LOAD, json!({"name":base}))?;
            if let Err(e) = self.gun(&gun) {
                self.table.error(&gun.name, e);
            }
        }
        let bipeds: Vec<_> = self
            .h1
            .tags()
            .iter()
            .filter(|t| t.group == h1map::Group::new(b"bipd"))
            .cloned()
            .collect();
        ensure!(!bipeds.is_empty(), "no source bipeds");
        for biped in bipeds {
            self.call(agent_proto::state::method::LOAD, json!({"name":base}))?;
            if let Err(e) = self.biped(&biped) {
                self.table.error(&biped.name, e);
            }
        }
        Ok(())
    }
}

pub fn run(args: Vec<String>) -> Result<()> {
    let (args, phase) = match args.split_last() {
        Some((s, rest)) if matches!(s.as_str(), "--round-only" | "--objects-only") => {
            (rest, s.as_str())
        }
        _ => (args.as_slice(), "all"),
    };
    let [iso_path, map_path, case, socket] = args else {
        bail!(
            "usage: h2 parity <halo1.iso> <converted.map> <bloodgulch-h1|bloodgulch-h2|a30> <serve socket> [--round-only|--objects-only]"
        )
    };
    let (mp, h2_sandbox, name) = match case.as_str() {
        "bloodgulch-h1" => (true, false, "bloodgulch"),
        "bloodgulch-h2" => (true, true, "bloodgulch"),
        "a30" => (false, false, "a30"),
        _ => bail!("unknown parity case {case}"),
    };
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(iso_path)?))?;
    let disc = h1map::disc::Disc::open(&mut iso)?;
    let entry = iso.find("default.xbe")?;
    let rules = Rules::read(&iso.read(&entry)?)?;
    let h1 = H1Map::from_reader(Cursor::new(
        iso.read(disc.map(name).context("source map absent")?)?,
    ))?;
    let h2 = map::Map::open(map_path)?;
    ensure!(
        h2.header().cache_type
            == if mp {
                map::CacheType::Multiplayer
            } else {
                map::CacheType::Campaign
            },
        "wrong map type"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let client = loop {
        match Client::connect(Path::new(socket)) {
            Ok(client) => break client,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(e) => return Err(e).with_context(|| format!("connect to {socket}")),
        }
    };
    client.set_timeout(Some(Duration::from_secs(310)))?;
    let mut c = Case {
        client,
        h1,
        h2,
        rules,
        h2_sandbox,
        mp,
        rifle_projectile: None,
        table: Table { rows: 0, failed: 0 },
    };
    c.call(agent_proto::method::CLAIM, json!({}))?;
    c.call(
        agent_proto::method::WAIT,
        json!({"until":{"phase":"gameplay","map_ticks":150},"timeout_ms":300000}),
    )?;
    c.client.set_timeout(Some(Duration::from_secs(90)))?;
    let status = c.call(agent_proto::method::STATUS, json!({}))?;
    let actual = status["at"]["map"]
        .as_str()
        .context("loaded map name")?
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("");
    ensure!(
        actual.eq_ignore_ascii_case(&c.h2.header().name),
        "serve has {actual}, supplied cache is {}",
        c.h2.header().name
    );
    ensure!(
        actual == if mp { "h1_bloodgulch" } else { "a30" },
        "wrong map for {case}: {actual}"
    );
    // An override keeps the same map header in both sandboxes. Verify the
    // complete named, resident weapon inventory against the selected file.
    use std::collections::BTreeSet;
    let wanted: BTreeSet<_> =
        c.h2.tags()
            .iter()
            .filter(|t| t.group == map::FourCC(*b"weap") && t.location.is_some())
            .map(|t| (format!("0x{:08x}", t.id.0), t.name.clone()))
            .collect();
    let live = c.call(agent_proto::hot::method::EDIT_TAGS, json!({"group":"weap","limit":500}))?;
    let seen: BTreeSet<_> = live["tags"]
        .as_array()
        .context("live weapons")?
        .iter()
        .map(|t| {
            Ok((
                t["tag"].as_str().context("tag datum")?.to_owned(),
                t["name"].as_str().map(str::to_owned),
            ))
        })
        .collect::<Result<_>>()?;
    ensure!(
        !wanted.is_empty() && wanted.is_subset(&seen),
        "loaded weapon datums/names differ from the supplied cache"
    );
    if mp {
        let local = c.player()?;
        let name = local["tag_name"]
            .as_str()
            .context("local biped name is missing")?;
        ensure!(
            (name == r"characters\cyborg_mp\cyborg_mp") != h2_sandbox,
            "unexpected player biped {name} for {case}"
        );
    }
    println!("check ({case})\tHalo 1\tconverted\tresult");
    if phase == "--round-only" {
        ensure!(mp, "round timing requires multiplayer");
        if let Err(e) = c.round() {
            c.table.error("round control", e);
        }
        ensure!(c.table.failed == 0, "round timing failed ({case})");
        return Ok(());
    }
    let snap = format!("parity-{}-{}", std::process::id(), case);
    c.call(agent_proto::state::method::SAVE, json!({"name":snap}))?;
    let verdict = c.checks(&snap, phase == "all");
    c.call(agent_proto::state::method::LOAD, json!({"name":snap}))
        .with_context(|| format!("restore {snap}"))?;
    c.call(agent_proto::state::method::DROP, json!({"name":snap}))?;
    verdict?;
    ensure!(
        c.table.failed == 0,
        "{}/{} parity checks failed ({case})",
        c.table.failed,
        c.table.rows
    );
    Ok(())
}
