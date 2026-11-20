//! Owner-side screenshots for Live mods. All game control uses the same
//! agent protocol as `h2 agent`. One private game boots for each member,
//! keeping packages on a common base independent of the owner's lobby.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_proto::{CaptureReply, Client as Agent, method};
use anyhow::{Context, Result, bail, ensure};
use content::{MapId, Sha256};
use live_hle::maps::{Fetch, Got, HttpFetch};
use live_proto::maps::{MapAnswer, MapSummary, ModPackSummary, pack_id_ok};
use live_proto::screenshots::{CameraPose, CapturePermit, CaptureProof, PROOF_HEADER};
use live_proto::{Xuid, lxm};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::live_cmd::{Args, Client, accounts_path, call, pick_account, status_of};
use crate::live_maps::Signer;

const USAGE: &str = "usage: h2 live mods capture PACK_ID|MAP_ID [--server URL] [--as GAMERTAG] [--disc ISO] [--out IMAGE_DIR]";
const SHOTS: usize = 3;
const DEADLINE: Duration = Duration::from_secs(180);

struct Owner<'a> {
    client: &'a Client,
    signer: Signer,
    download: HttpFetch<'a>,
}

impl Fetch for Owner<'_> {
    fn get(&self, path: &str, cap: u64) -> Result<Got, String> {
        let token = self
            .signer
            .token(self.client, lxm::READ)
            .map_err(|e| e.to_string())?;
        self.download.get_authenticated(path, cap, token)
    }
}

impl Owner<'_> {
    fn read<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let bytes = match self.get(path, 4 << 20).map_err(anyhow::Error::msg)? {
            Got::Ok(bytes) => bytes,
            Got::NotFound => bail!("no published content at {path}"),
            Got::Gone(reason) => bail!("{path} was taken down: {reason}"),
        };
        serde_json::from_slice(&bytes).with_context(|| format!("reading {path}"))
    }

    fn members(&self, target: &str, uploader: Xuid) -> Result<(Option<String>, Vec<MapSummary>)> {
        if let Ok(id) = target.parse::<MapId>() {
            let answer: MapAnswer = self.read(&format!("/maps/{id}"))?;
            ensure!(
                answer.map.manifest.map_id() == id && answer.map.summary.map_id == id,
                "the server returned another map for {id}"
            );
            ensure!(
                answer.map.summary.author.xuid == uploader,
                "sign in as the uploader of {id}"
            );
            return Ok((None, vec![answer.map.summary]));
        }
        ensure!(pack_id_ok(target), "expected a pack ID or a full MapId");
        #[derive(Deserialize)]
        struct Answer {
            pack: ModPackSummary,
        }
        let Answer { pack } = self.read(&format!("/mod-packs/{target}"))?;
        ensure!(pack.id == target, "the server returned a different pack");
        ensure!(
            pack.uploader.xuid == uploader,
            "sign in as the uploader of {}",
            pack.info.name
        );
        ensure!(
            !pack.members.is_empty(),
            "pack {target} has no playable maps"
        );
        ensure!(
            pack.members.iter().all(|map| map.author.xuid == uploader),
            "the pack contains a map owned by another uploader"
        );
        Ok((Some(pack.id), pack.members))
    }

    fn capture_session(&self, map: MapId) -> Result<CapturePermit> {
        let url = self.client.url(&format!("/maps/{map}/screenshot-capture-session"));
        let answer = self
            .write(|| self.client.agent.post(&url), None)
            .with_context(|| format!("opening a Live capture for {map}"))?;
        serde_json::from_str(&answer).context("reading the Live capture permit")
    }

    fn upload(&self, target: &str, slot: usize, proof: &CaptureProof, bytes: &[u8]) -> Result<()> {
        #[derive(Deserialize)]
        struct Receipt {
            status: Status,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum Status {
            Pending,
            Approved,
        }

        let url = self.client.url(&format!("{target}/screenshots/capture/{slot}"));
        let proof = serde_json::to_string(proof)?;
        let answer = self
            .write(
                || {
                    self.client.agent.put(&url)
                        .set("content-type", "image/png")
                        .set(PROOF_HEADER, &proof)
                },
                Some(bytes),
            )
            .with_context(|| format!("uploading {target} capture {slot}"))?;
        let receipt: Receipt =
            serde_json::from_str(&answer).context("reading the Live capture receipt")?;
        if matches!(receipt.status, Status::Pending) {
            eprintln!("{target} capture {} is pending review", slot + 1);
        }
        Ok(())
    }

    fn write(&self, request: impl Fn() -> ureq::Request, body: Option<&[u8]>) -> Result<String> {
        for retry in 0..=4 {
            let req = self
                .client
                .authorized(request(), self.signer.token(self.client, lxm::CONTENT_WRITE)?)?;
            match call(req, body) {
                Err(e) if status_of(&e) == Some(429) && retry < 4 => {
                    std::thread::sleep(Duration::from_secs(2 << retry));
                }
                result => return result,
            }
        }
        unreachable!()
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn create() -> Result<Self> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos() as u64;
        let path = std::env::temp_dir().join(format!("h2c-{nonce:x}"));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Game(Child);

impl Drop for Game {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn serve(dir: &Path, data: &Path, disc: &Path, image: Option<&Path>) -> Result<(Game, Agent)> {
    #[cfg(unix)]
    let socket = dir.join("s");
    #[cfg(not(unix))]
    let socket = {
        let addr = std::net::TcpListener::bind(("127.0.0.1", 0))?.local_addr()?;
        PathBuf::from(format!("tcp://{addr}"))
    };
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("serve")
        .arg(disc)
        .arg("--data")
        .arg(data)
        .arg("--socket")
        .arg(&socket)
        .args(["--no-halo1", "--no-mappack", "--no-movies"])
        .env("H2_BOOT_GFX", dir.join("gpu"))
        .env("H2_BOOT_GFX_EVERY", "never")
        .env("H2_RENDER_SCALE", "1")
        .env_remove("H2_BOOT_GFX_LIGHTING_AB")
        .env_remove("H2_LEVEL")
        .env_remove("H2_MAP_DIR")
        .stdin(Stdio::null());
    if let Some(image) = image {
        cmd.arg("--out").arg(image);
    }
    let mut game = Game(
        cmd.spawn()
            .context("starting the local headless h2 serve")?,
    );
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(status) = game.0.try_wait()? {
            bail!("h2 serve exited with {status} before its agent was available");
        }
        match Agent::connect(&socket) {
            Ok(agent) => {
                agent.set_timeout(Some(DEADLINE + Duration::from_secs(5)))?;
                return Ok((game, agent));
            }
            Err(e) if Instant::now() >= deadline => {
                return Err(e).context("connecting to h2 serve");
            }
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn until(condition: Value) -> Value {
    json!({"until":condition, "timeout_ms":DEADLINE.as_millis() as u64})
}

/// Prefer distinct spawn points spaced through the scenario's starts. On
/// a small map use distinct angles at a repeated position.
fn view(slot: usize, count: usize) -> (usize, f64) {
    let index = if count >= SHOTS {
        slot * count / SHOTS
    } else {
        slot % count
    };
    let yaw = if slot >= count {
        (slot / count) as f64 * std::f64::consts::TAU / SHOTS.div_ceil(count) as f64
    } else {
        0.0
    };
    (index, yaw)
}

fn capture(
    mut rpc: impl FnMut(&str, Value) -> Result<Value>,
    id: MapId,
    base: &str,
    permit: &CapturePermit,
    frames: &Path,
    process: u32,
    mut image: impl FnMut(usize, &CaptureProof, &[u8]) -> Result<()>,
) -> Result<()> {
    rpc(method::CLAIM, json!({}))?;
    rpc(method::WAIT, until(json!({"phase":"menu", "map_ticks":10})))?;
    let loaded = rpc(method::MAPS_LOAD, json!({"map_id":id}))?;
    ensure!(
        loaded["queued"] == true && loaded["check"] == true,
        "the game rejected {id}"
    );
    let path = loaded["map"]["path"]
        .as_str()
        .context("maps.load returned no map path")?;
    let map = path
        .rsplit('\\')
        .next()
        .context("empty map path")?
        .to_lowercase();
    rpc(
        method::WAIT,
        until(json!({"phase":"gameplay", "map":map, "map_ticks":45})),
    )?;

    let mut count = 1;
    for slot in 0..SHOTS {
        let (index, yaw) = view(slot, count);
        let pose = rpc(
            method::RENDER_SPAWN_VIEW,
            json!({"index":index, "yaw_offset":yaw}),
        )?;
        count = pose["count"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n > 0)
            .context("no readable spawn points")?;
        let shot = rpc(method::RENDER_CAPTURE, json!({"game_id":permit.game_id}))?;
        let file = Path::new(
            shot["path"]
                .as_str()
                .context("render.capture returned no PNG")?,
        );
        ensure!(
            file.parent() == Some(frames)
                && file
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&format!("capture-{process}-"))
                        && name.ends_with(".png")),
            "render.capture returned a path outside this game’s frame files"
        );
        let result = (|| {
            ensure!(
                shot["at"]["phase"] == "gameplay" && shot["at"]["map"] == map,
                "the game left {map} before capture {slot}"
            );
            let capture = CaptureReply::deserialize(&shot).context("reading the frame capture")?;
            let source = capture.source.context("the game returned no capture provenance")?;
            let map_id = source.map_id.parse::<MapId>().context("invalid captured map ID")?;
            ensure!(
                source.game_id == permit.game_id && map_id == id && source.map.eq_ignore_ascii_case(base),
                "the game returned a frame for a different capture session or map"
            );
            let bytes = fs::read(file).with_context(|| format!("reading {}", file.display()))?;
            let digest = source.digest.parse::<Sha256>().context("invalid capture digest")?;
            ensure!(digest == Sha256::of(&bytes), "the captured PNG changed before upload");
            let proof = CaptureProof {
                game_id: source.game_id,
                tick: capture.tick,
                map_id,
                map: source.map,
                camera: CameraPose {
                    position: source.camera.position,
                    forward: source.camera.forward,
                    up: source.camera.up,
                },
                digest,
                permit: Some(permit.token.clone()),
            };
            image(slot, &proof, &bytes)
        })();
        let _ = fs::remove_file(file);
        result?;
    }
    Ok(())
}

/// Each of the pack's three slots takes a different map when possible.
/// A one-map pack gets its three different views.
fn representative(slot: usize, maps: usize) -> (usize, usize) {
    (slot * maps / SHOTS, slot)
}

/// Make an already installed Map Pack base available to the private game.
fn map_pack_base(map: &str, data: &Path, disc: &Path) -> Result<()> {
    if !content::map_pack::is_map(map) || boot::map_bases::map_pack_file(data, map).is_some() {
        return Ok(());
    }
    let local = content::store::default_data_root();
    if let Some((offer, name, src)) = boot::map_bases::map_pack_file(&local, map) {
        let dst = vfs::Config::new(data, boot::new_maps::HALO2_TITLE)
            .title_data_dir()
            .join(vfs::CONTENT_DIR)
            .join(offer);
        fs::create_dir_all(&dst)?;
        fs::copy(src, dst.join(name))?;
    } else {
        crate::mappack_cmd::auto_install(disc, data);
    }
    Ok(())
}

pub(crate) fn run(mut args: Args) -> Result<()> {
    let server = args.take("--server")?;
    let as_tag = args.take("--as")?;
    let image = args.take("--out")?.map(PathBuf::from);
    let disc = args.take("--disc")?.map(PathBuf::from);
    let [target] = args
        .positional_for(USAGE)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("{USAGE}"))?;
    let disc = match disc {
        Some(disc) => disc,
        None => crate::default_disc()?,
    };
    let client = Client::connect(server)?;
    let account = pick_account(&client, &accounts_path()?, as_tag.as_deref())?;
    let xuid = account.xuid;
    let download_agent = live_hle::maps::download_agent();
    let owner = Owner {
        client: &client,
        signer: Signer::Account(account),
        download: HttpFetch {
            agent: &download_agent,
            api: &client.api,
            reader: None,
        },
    };
    let (pack, maps) = owner.members(&target, xuid)?;
    let scratch = Scratch::create()?;
    let data = scratch.0.join("data");
    let gate = boot::map_bases::gate(&disc, &data, content::DiscHashes::default());
    for (member, map) in maps.iter().enumerate() {
        map_pack_base(&map.base.map, &data, &disc)?;
        gate.install(&owner, map.map_id, &mut |_, _| {})?;
        eprintln!("Capturing {} ({})", map.name, map.map_id);
        {
            let (game, mut agent) = serve(&scratch.0, &data, &disc, image.as_deref())?;
            let permit = owner.capture_session(map.map_id)?;
            capture(
                |method, params| Ok(agent.call(method, params)?),
                map.map_id,
                &map.base.map,
                &permit,
                &agent_proto::cache_dir().join("frames"),
                game.0.id(),
                |shot, proof, png| {
                    owner.upload(&format!("/maps/{}", map.map_id), shot, proof, png)?;
                    if let Some(id) = &pack {
                        for slot in 0..SHOTS {
                            if representative(slot, maps.len()) == (member, shot) {
                                owner.upload(&format!("/mod-packs/{id}"), slot, proof, png)?;
                            }
                        }
                    }
                    Ok(())
                },
            )
            .with_context(|| format!("capturing {} ({})", map.name, map.map_id))?;
        }
        gate.remove(&map.map_id).map_err(anyhow::Error::msg)?;
    }
    println!("Submitted {SHOTS} captured views for each of {} maps.", maps.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_verified_gameplay_through_the_agent_for_each_image_slot() {
        let scratch = Scratch::create().unwrap();
        let mut poses = Vec::new();
        let mut slots = Vec::new();
        let id = MapId([0x2b; 32]);
        let permit = CapturePermit {
            game_id: "capture-123".into(),
            token: "test-permit".into(),
        };
        let rpc = |method: &str, params: Value| -> Result<Value> {
            Ok(match method {
                method::CLAIM => json!({}),
                method::WAIT => {
                    if params["until"]["phase"] == "gameplay" {
                        assert_eq!(params["until"]["map"], "package-alias");
                    }
                    json!({})
                }
                method::MAPS_LOAD => {
                    assert_eq!(params["map_id"], json!(id));
                    json!({"queued":true, "check":true, "map":{"path":r"t:\$c\offer\package-alias"}})
                }
                method::RENDER_SPAWN_VIEW => {
                    poses.push(params["index"].as_u64().unwrap());
                    json!({"count":9})
                }
                method::RENDER_CAPTURE => {
                    assert_eq!(params["game_id"], permit.game_id);
                    let path = scratch.0.join(format!("capture-123-{}.png", poses.len()));
                    let bytes = [poses.len() as u8];
                    fs::write(&path, bytes)?;
                    json!({
                        "path": path,
                        "at": {"map":"package-alias", "phase":"gameplay"},
                        "tick": 42 + poses.len(),
                        "present": 25 + poses.len(),
                        "source": {
                            "game_id": permit.game_id,
                            "map_id": id,
                            "map": "lockout",
                            "camera": {
                                "position": [3.0, 4.0, 5.0],
                                "forward": [1.0, 0.0, 0.0],
                                "up": [0.0, 0.0, 1.0]
                            },
                            "digest": Sha256::of(&bytes)
                        }
                    })
                }
                other => panic!("unexpected call {other}"),
            })
        };
        capture(rpc, id, "lockout", &permit, &scratch.0, 123, |slot, proof, bytes| {
            assert_eq!(bytes, [(slot + 1) as u8]);
            assert_eq!(proof.game_id, permit.game_id);
            assert_eq!(proof.map_id, id);
            assert_eq!(proof.map, "lockout");
            assert_eq!(proof.tick, 43 + slot as u64);
            assert_eq!(proof.camera.position, [3.0, 4.0, 5.0]);
            assert_eq!(proof.digest, Sha256::of(bytes));
            assert_eq!(proof.permit.as_deref(), Some(permit.token.as_str()));
            slots.push(slot);
            Ok(())
        })
        .unwrap();
        assert_eq!(poses, [0, 3, 6]);
        assert_eq!(slots, [0, 1, 2]);
        assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 0);
    }

    #[test]
    fn three_views_cover_small_and_large_spawn_lists_without_duplicates() {
        for count in 1..=128 {
            let views: Vec<_> = (0..SHOTS).map(|slot| view(slot, count)).collect();
            for (i, pose) in views.iter().enumerate() {
                assert!(pose.0 < count);
                assert!(!views[..i].contains(pose));
            }
        }
        assert_eq!(view(2, 12), (8, 0.0));
    }

    #[test]
    fn a_pack_samples_across_its_members_even_with_only_one_map() {
        assert_eq!(
            (0..3).map(|i| representative(i, 12)).collect::<Vec<_>>(),
            [(0, 0), (4, 1), (8, 2)]
        );
        assert_eq!(
            (0..3).map(|i| representative(i, 1)).collect::<Vec<_>>(),
            [(0, 0), (0, 1), (0, 2)]
        );
    }
}
