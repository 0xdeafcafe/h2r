//! The `halo1` converter of `convert` items (docs/editor/studio-redesign.md,
//! "Conversions"): the import above as a [`workspace::Converter`]. A unit
//! is one job ([`Job::key`]: `a10`, `bloodgulch/h2`), and the recipe's
//! settings pick the jobs ([`Picks`]). The import's own stamp
//! ([`STAMP_FILE`]) is kept in step, so `h2 import halo1` and
//! `h2 package prepare` share what either converted.
use std::collections::BTreeSet;

use workspace::{ConvertSpec, Converter, Input, Unit};

use super::*;

/// The Halo 1 disc beside `game_disc`, converted into `data`.
pub struct Halo1 {
    pub game_disc: PathBuf,
    pub data: PathBuf,
    /// Convert again whatever the import's stamp says.
    pub force: bool,
}

/// What a recipe's `[settings]` pick: `campaign`, `halo2_assets` (each
/// level's Halo 2-assets variant), `multiplayer` and `sandboxes` (`"h1"`,
/// `"h2"`). Each left out is all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picks {
    pub campaign: bool,
    pub halo2_assets: bool,
    pub multiplayer: bool,
    pub sandboxes: Vec<Sandbox>,
}

impl Default for Picks {
    fn default() -> Self {
        Self {
            campaign: true,
            halo2_assets: true,
            multiplayer: true,
            sandboxes: SANDBOXES.to_vec(),
        }
    }
}

fn flag(key: &str, v: &toml::Value) -> Result<bool, String> {
    v.as_bool()
        .ok_or_else(|| format!("settings: {key} is true or false"))
}

impl Picks {
    pub fn from_settings(t: &toml::Table) -> Result<Self, String> {
        let mut p = Self::default();
        for (k, v) in t {
            match k.as_str() {
                "campaign" => p.campaign = flag(k, v)?,
                "halo2_assets" => p.halo2_assets = flag(k, v)?,
                "multiplayer" => p.multiplayer = flag(k, v)?,
                "sandboxes" => {
                    let list = v
                        .as_array()
                        .ok_or("settings: sandboxes is a list of \"h1\" and \"h2\"")?;
                    p.sandboxes = list
                        .iter()
                        .map(|s| match s.as_str() {
                            Some("h1") => Ok(Sandbox::Halo1),
                            Some("h2") => Ok(Sandbox::Halo2),
                            _ => Err(format!("settings: sandbox {s} is not \"h1\" or \"h2\"")),
                        })
                        .collect::<Result<_, _>>()?;
                }
                other => {
                    return Err(format!(
                        "settings: the halo1 converter has no setting {other:?}"
                    ));
                }
            }
        }
        Ok(p)
    }

    /// Whether the recipe wants `job`.
    pub fn takes(&self, job: &Job) -> bool {
        match job {
            Job::Campaign(_) => self.campaign,
            Job::CampaignHalo2(_) => self.campaign && self.halo2_assets,
            Job::Multiplayer(_, s) => self.multiplayer && self.sandboxes.contains(s),
        }
    }
}

/// The jobs of a disc whose `maps/` holds `stems` that `picks` takes, in
/// [`jobs`]' order.
pub fn picked(stems: &[String], picks: &Picks) -> Vec<Job<'static>> {
    jobs(stems).into_iter().filter(|j| picks.takes(j)).collect()
}

fn open_h1(h1: &Path) -> Result<(Xiso<BufReader<File>>, Disc)> {
    let f = File::open(h1).with_context(|| format!("opening {}", h1.display()))?;
    let mut iso = Xiso::open(BufReader::new(f)).with_context(|| h1.display().to_string())?;
    let disc = Disc::open(&mut iso)?;
    Ok((iso, disc))
}

fn stems_of(disc: &Disc) -> Vec<String> {
    disc.maps.iter().map(|e| map_stem(e).to_owned()).collect()
}

impl Converter for Halo1 {
    fn name(&self) -> &str {
        "halo1"
    }

    fn version(&self) -> String {
        CONVERTER_VERSION.to_string()
    }

    fn input(&self, _: &ConvertSpec) -> Result<Input, String> {
        let h1 = discover(&self.game_disc).ok_or_else(|| {
            format!(
                "no Halo 1 disc beside {} (put {AUTO_NAME} there)",
                self.game_disc.display()
            )
        })?;
        // The bases come from the Halo 2 disc image; a bare default.xbe has
        // none.
        let is_iso = File::open(&self.game_disc)
            .ok()
            .is_some_and(|f| Xiso::open(BufReader::new(f)).is_ok());
        if !is_iso {
            return Err(format!(
                "{} is not a disc image, and the conversion needs the Halo 2 disc's maps",
                self.game_disc.display()
            ));
        }
        let (size, mtime) = size_mtime(&h1).map_err(|e| format!("{e:#}"))?;
        Ok(Input {
            label: h1.display().to_string(),
            identity: format!("size={size} mtime={mtime}"),
            path: h1,
        })
    }

    /// What `h2 import halo1` (or the import before packages) converted.
    fn kept(&self, _: &ConvertSpec) -> bool {
        let root = boot::store_status::root(&self.data);
        workspace::converted::levels(&root).iter().any(|mark| {
            workspace::converted::verified(&root, mark, "halo1").is_some()
        })
    }

    fn units(&self, input: &Input, spec: &ConvertSpec) -> Result<Vec<Unit>, String> {
        let picks = Picks::from_settings(&spec.settings)?;
        let (_, disc) = open_h1(&input.path).map_err(|e| format!("{e:#}"))?;
        let all = picked(&stems_of(&disc), &picks);
        if all.is_empty() {
            return Err(format!(
                "{} has no Halo 1 campaign or multiplayer maps the recipe picks",
                input.label
            ));
        }
        Ok(all
            .iter()
            .map(|j| Unit {
                name: j.key(),
                key: String::new(),
                output: Some(j.output(&self.data)),
            })
            .collect())
    }

    fn convert(
        &self,
        input: &Input,
        spec: &ConvertSpec,
        todo: &[Unit],
        done: &mut dyn FnMut(&str, Result<(), String>),
    ) {
        let r = catch_unwind(AssertUnwindSafe(|| self.run(input, spec, todo, done)))
            .unwrap_or_else(|p| Err(anyhow!("panicked: {}", panic_text(&p))));
        // A unit not said done converts at the next prepare.
        if let Err(e) = r {
            eprintln!("halo1: not imported: {e:#}");
        }
    }
}

impl Halo1 {
    fn run(
        &self,
        input: &Input,
        spec: &ConvertSpec,
        todo: &[Unit],
        done: &mut dyn FnMut(&str, Result<(), String>),
    ) -> Result<()> {
        let h1 = &input.path;
        crate::safe_out::protect(h1);
        crate::safe_out::protect(&self.game_disc);
        let picks = Picks::from_settings(&spec.settings).map_err(|e| anyhow!(e))?;
        let (iso, disc) = open_h1(h1)?;
        let names: BTreeSet<&str> = todo.iter().map(|u| u.name.as_str()).collect();
        let wanted: Vec<Job<'static>> = picked(&stems_of(&disc), &picks)
            .into_iter()
            .filter(|j| names.contains(j.key().as_str()))
            .collect();
        let (size, mtime) = size_mtime(h1)?;
        let old = Stamp::read(&self.data);
        let mut stamp = fresh_stamp(old.as_ref(), size, mtime, || sha256_file(h1))?;
        if self.force {
            stamp.done.clear();
        }
        let todo_jobs = pending(&wanted, &stamp, &self.data);
        // What the import's own stamp holds for this disc and converter.
        for j in wanted.iter().filter(|j| !todo_jobs.contains(*j)) {
            if let Some(r) = stamp.done.get(&j.key()) {
                done(&j.key(), r.clone());
            }
        }
        if todo_jobs.is_empty() {
            return stamp.write(&self.data);
        }
        eprintln!(
            "halo1: converting {} maps from {} (converter version {CONVERTER_VERSION})",
            todo_jobs.len(),
            h1.display()
        );
        let conv = DiscConverter::new(
            h1.clone(),
            iso,
            disc,
            self.game_disc.clone(),
            self.data.clone(),
        );
        let order = schedule(&todo_jobs, |m| {
            conv.disc.map(m).map_or(0, |e| u64::from(e.size))
        });
        conv.expect(&order);
        let running: BTreeSet<String> = todo_jobs.iter().map(Job::key).collect();
        let mut told: BTreeSet<String> = BTreeSet::new();
        let data = self.data.clone();
        let mut save = |s: &Stamp| -> Result<()> {
            for k in &running {
                if told.contains(k) {
                    continue;
                }
                if let Some(r) = s.done.get(k) {
                    told.insert(k.clone());
                    done(k, r.clone());
                }
            }
            s.write(&data)
        };
        let mut say = |s: String| eprintln!("halo1: {s}");
        run_jobs(&order, workers(), &mut stamp, &conv, &mut save, &mut say);
        Ok(())
    }
}
