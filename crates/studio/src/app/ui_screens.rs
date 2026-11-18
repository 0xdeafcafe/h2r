//! The UI Screens tool's state, no UI toolkit: the screen picked (one of
//! ours by name and state, or a retail screen by tag), its page driven
//! live by pad presses ([`boot::ui_kit::page::Live`]), the picture drawn
//! from the editor's disc (or the synthetic source without one), the
//! widgets' rectangles and the layout checks' findings, and the export.
//! docs/game/ui-builder.md, docs/tools/editor.md.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use boot::ui_kit::input::Input;
use boot::ui_kit::layout::{self, Rules};
use boot::ui_kit::page::Live;
use boot::ui_kit::preview::{self, Canvas, Disc, Retail};
use boot::ui_kit::screens;
use boot::ui_kit::spec::ScreenSpec;
use boot::ui_kit::view::{View, view_of};

/// What the tool shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// One of our screens ([`screens::NAMES`]) in a named state.
    Native { screen: String, state: String },
    /// A retail screen of `mainmenu.map`, by tag name, as the disc has it.
    Retail(String),
}

/// One finding of the layout checks, with the widgets it names.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub text: String,
    pub ids: Vec<String>,
}

/// The picture of the current pick and what was found in it.
pub struct Shot {
    pub canvas: Canvas,
    /// The view (none for a retail screen: only its picture is drawn).
    pub view: Option<View>,
    /// The checks' findings, the kit's then the layout's.
    pub findings: Vec<Finding>,
    /// Checks the source could not run (the art checks without a disc).
    pub skipped: Vec<String>,
    /// The rules the view was held to (safe and reserved areas).
    pub rules: Rules,
    /// Drawn from the disc, else the synthetic source.
    pub from_disc: bool,
    /// Bumped on every redraw, for the texture.
    pub rev: u64,
}

/// The tool's state.
pub struct UiScreens {
    pub pick: Option<Pick>,
    live: Option<Box<dyn Live>>,
    /// The disc the pictures come from, by its path.
    disc: Option<(PathBuf, Disc)>,
    /// Why the disc did not open, once per path.
    disc_error: Option<(PathBuf, String)>,
    disc_path: Option<PathBuf>,
    shot: Option<Shot>,
    rev: u64,
    /// The last press and what the page did with it.
    pub last: Option<String>,
    pub zoom: f32,
    pub show_rects: bool,
    pub show_safe: bool,
    pub show_reserved: bool,
}

impl Default for UiScreens {
    fn default() -> Self {
        UiScreens {
            pick: None,
            live: None,
            disc: None,
            disc_error: None,
            disc_path: None,
            shot: None,
            rev: 0,
            last: None,
            zoom: 1.0,
            show_rects: true,
            show_safe: true,
            show_reserved: true,
        }
    }
}

impl UiScreens {
    /// Our screens and their states, in order.
    pub fn tree() -> Vec<(&'static str, Vec<String>)> {
        screens::NAMES
            .iter()
            .map(|n| {
                let states = screens::live(n)
                    .map(|l| l.state_names())
                    .unwrap_or_default();
                (*n, states)
            })
            .collect()
    }

    /// The disc image the pictures come from (`None`: the synthetic
    /// source). Set from the editor's open source; a change redraws.
    pub fn set_disc(&mut self, path: Option<&Path>) {
        let path = path.map(Path::to_path_buf);
        if path == self.disc_path {
            return;
        }
        self.disc_path = path;
        self.redraw();
    }

    /// The retail screens of the disc's `mainmenu.map` (empty without one).
    pub fn retail(&mut self) -> Vec<String> {
        self.disc().map(Disc::retail_screens).unwrap_or_default()
    }

    /// Whether the pictures come from the disc.
    pub fn has_disc(&mut self) -> bool {
        self.disc().is_some()
    }

    /// Why the disc did not open, when it didn't.
    pub fn disc_error(&self) -> Option<&str> {
        self.disc_error.as_ref().map(|(_, e)| e.as_str())
    }

    fn disc(&mut self) -> Option<&Disc> {
        let path = self.disc_path.clone()?;
        if self.disc.as_ref().is_none_or(|(p, _)| *p != path) {
            self.disc = None;
            if self.disc_error.as_ref().is_some_and(|(p, _)| *p == path) {
                return None;
            }
            match Disc::open(&path) {
                Ok(d) => self.disc = Some((path, d)),
                Err(e) => {
                    self.disc_error = Some((path, format!("{e:#}")));
                    return None;
                }
            }
        }
        self.disc.as_ref().map(|(_, d)| d)
    }

    /// Shows `pick`: a native screen's page goes to its state.
    pub fn select(&mut self, pick: Pick) -> Result<()> {
        match &pick {
            Pick::Native { screen, state } => {
                let mut live =
                    screens::live(screen).with_context(|| format!("no screen {screen:?}"))?;
                if !live.go(state) {
                    bail!("{screen} has no state {state:?}");
                }
                self.live = Some(live);
            }
            Pick::Retail(_) => self.live = None,
        }
        self.pick = Some(pick);
        self.last = None;
        self.redraw();
        Ok(())
    }

    /// A pad press on the page shown: what it did. `None` for a retail
    /// screen (nothing to step).
    pub fn press(&mut self, input: Input) -> Option<String> {
        let live = self.live.as_mut()?;
        let effect = live.press(input);
        let line = format!("{input:?}: {effect}");
        self.last = Some(line.clone());
        self.redraw();
        Some(line)
    }

    /// The page's summary line.
    pub fn summary(&self) -> Option<String> {
        self.live.as_ref().map(|l| l.summary())
    }

    /// The spec shown, for a native screen.
    pub fn spec(&self) -> Option<ScreenSpec> {
        self.live.as_ref().map(|l| l.spec())
    }

    fn redraw(&mut self) {
        self.shot = None;
    }

    /// The picture of the pick, drawn when it changed.
    pub fn shot(&mut self) -> Option<&Shot> {
        if self.shot.is_none() {
            let pick = self.pick.clone()?;
            self.rev += 1;
            let shot = match pick {
                Pick::Native { state, .. } => self.draw_native(&state),
                Pick::Retail(tag) => self.draw_retail(&tag),
            };
            match shot {
                Ok(s) => self.shot = Some(s),
                Err(e) => {
                    self.last = Some(format!("draw failed: {e:#}"));
                    return None;
                }
            }
        }
        self.shot.as_ref()
    }

    fn draw_native(&mut self, state: &str) -> Result<Shot> {
        let spec = self.spec().context("no page")?;
        let rev = self.rev;
        let disc = self.disc();
        let from = match disc {
            Some(d) => preview::From::Disc(d),
            None => preview::From::Synthetic,
        };
        let src = from.source();
        let view = view_of(&spec, &from.geometry(spec.list)?, src, state);
        let canvas = preview::render_view(&view, src);
        let rules = Rules::for_spec(&spec);
        let mut findings = Vec::new();
        if let Some(d) = disc {
            for f in d.check(&spec)? {
                findings.push(Finding {
                    text: format!("kit: {f}"),
                    ids: Vec::new(),
                });
            }
        }
        let report = layout::check(&view, src, &rules);
        findings.extend(report.issues.iter().map(|i| Finding {
            text: i.to_string(),
            ids: i.ids().into_iter().map(String::from).collect(),
        }));
        Ok(Shot {
            canvas,
            view: Some(view),
            findings,
            skipped: report.skipped.iter().map(|s| s.to_string()).collect(),
            rules,
            from_disc: disc.is_some(),
            rev,
        })
    }

    fn draw_retail(&mut self, tag: &str) -> Result<Shot> {
        let rev = self.rev;
        let d = self.disc().context("retail screens need the disc")?;
        let canvas = d.render_retail(&Retail {
            tag,
            pane: 0,
            text: &|_| None,
            cells: &[],
            footer: None,
        })?;
        Ok(Shot {
            canvas,
            view: None,
            findings: Vec::new(),
            skipped: Vec::new(),
            rules: Rules::default(),
            from_disc: true,
            rev,
        })
    }

    /// The topmost widget at UI pixel `px` (640 by 480, top left origin).
    pub fn widget_at(&mut self, px: [f32; 2]) -> Option<String> {
        let view = self.shot()?.view.as_ref()?;
        view.widgets
            .iter()
            .rev()
            .find(|w| {
                let [x, y, wd, h] = w.pixels().map(|v| v as f32);
                px[0] >= x && px[0] < x + wd && px[1] >= y && px[1] < y + h
            })
            .map(|w| w.id.clone())
    }

    /// Widget `id`'s JSON, as the rects file has it.
    pub fn widget_json(&mut self, id: &str) -> Option<String> {
        let view = self.shot()?.view.as_ref()?;
        let json = view.json();
        let w = json["widgets"]
            .as_array()?
            .iter()
            .find(|w| w["id"] == id)?
            .clone();
        serde_json::to_string_pretty(&w).ok()
    }

    /// Writes the pick's PNG and JSON (`<screen>-<state>`), and the sheet
    /// of every screen in every state, into `dir`. Never into a Git
    /// checkout: what is drawn from the disc stays out of repositories.
    pub fn export(&mut self, dir: &Path) -> Result<Vec<PathBuf>> {
        if let Some(repo) = git_checkout(dir) {
            bail!(
                "{} is inside the Git checkout {}; pick a folder outside it",
                dir.display(),
                repo.display()
            );
        }
        std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
        let stem = match self.pick.clone().context("nothing picked")? {
            Pick::Native { screen, state } => match &self.last {
                Some(_) => format!("{screen}-{state}-pressed"),
                None => format!("{screen}-{state}"),
            },
            Pick::Retail(tag) => tag.replace(['\\', '/'], "_"),
        };
        let mut written = Vec::new();
        let shot = self.shot().context("nothing drawn")?;
        let png = dir.join(format!("{stem}.png"));
        shot.canvas.write_png(&png)?;
        written.push(png);
        if let Some(v) = &shot.view {
            let json = dir.join(format!("{stem}.json"));
            std::fs::write(&json, serde_json::to_string_pretty(&v.json())?)?;
            written.push(json);
        }
        let from = match self.disc() {
            Some(d) => preview::From::Disc(d),
            None => preview::From::Synthetic,
        };
        written.push(preview::write_sheet(&from, dir)?);
        Ok(written)
    }
}

/// The Git checkout `dir` is in (it, or a folder above it, holds `.git`).
pub fn git_checkout(dir: &Path) -> Option<PathBuf> {
    let abs = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(dir)
    };
    abs.ancestors()
        .find(|a| a.join(".git").exists())
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn native(screen: &str, state: &str) -> Pick {
        Pick::Native {
            screen: screen.into(),
            state: state.into(),
        }
    }

    /// The tree lists every screen of ours with its states.
    #[test]
    fn the_tree_has_every_screen_and_state() {
        let t = UiScreens::tree();
        assert_eq!(t.len(), screens::NAMES.len());
        let signin = t.iter().find(|(n, _)| *n == "signin-code").unwrap();
        assert!(signin.1.contains(&"code-wrong".to_string()));
        assert!(t.iter().all(|(_, s)| !s.is_empty()));
    }

    /// Without a disc a native screen draws from the synthetic source,
    /// with its widgets, and the art checks are reported as skipped.
    #[test]
    fn a_screen_draws_without_a_disc() {
        let mut s = UiScreens::default();
        s.select(native("skulls", "focus-0")).unwrap();
        let shot = s.shot().unwrap();
        assert!(!shot.from_disc);
        assert_eq!([shot.canvas.width, shot.canvas.height], [640, 480]);
        assert!(!shot.view.as_ref().unwrap().widgets.is_empty());
        assert!(!shot.skipped.is_empty());
        assert!(s.select(native("skulls", "nope")).is_err());
        assert!(s.select(native("nope", "focus-0")).is_err());
    }

    /// Pad presses step the page and redraw; the summary follows.
    #[test]
    fn presses_step_the_page() {
        let mut s = UiScreens::default();
        s.select(native("signin-code", "code")).unwrap();
        let rev = s.shot().unwrap().rev;
        assert_eq!(s.press(Input::Up).as_deref(), Some("Up: Redraw"));
        assert_eq!(
            s.summary().as_deref(),
            Some("Sign in as Chief | [1] 0 0 - 0 0 0 | -")
        );
        assert!(s.shot().unwrap().rev > rev, "redrawn");
        s.select(native("skulls", "focus-0")).unwrap();
        assert_eq!(s.press(Input::Down).as_deref(), Some("Down: Focus(1)"));
        assert_eq!(s.spec().unwrap().focus, 1);
    }

    /// The widget under a point, and its JSON.
    #[test]
    fn hovering_finds_the_widget_and_its_json() {
        let mut s = UiScreens::default();
        s.select(native("signin-code", "code")).unwrap();
        let title = {
            let v = s.shot().unwrap().view.as_ref().unwrap();
            v.widgets.iter().find(|w| w.id == "title").unwrap().pixels()
        };
        let at = [title[0] as f32 + 2.0, title[1] as f32 + 2.0];
        let id = s.widget_at(at).unwrap();
        let json = s.widget_json(&id).unwrap();
        assert!(json.contains(&format!("\"id\": \"{id}\"")), "{json}");
        assert!(s.widget_at([-5.0, -5.0]).is_none());
    }

    /// The findings name their widgets: sign-in has no legend yet.
    #[test]
    fn findings_come_with_their_widgets() {
        let mut s = UiScreens::default();
        s.select(native("signin-code", "code")).unwrap();
        let f = &s.shot().unwrap().findings;
        assert!(f.iter().any(|f| f.text.contains("no legend")), "{f:?}");
        let issue = layout::Issue::Fit { id: "row.0".into() };
        assert_eq!(issue.ids(), ["row.0"]);
    }

    /// Export writes the picture, its JSON and the sheet, and refuses a
    /// folder inside a Git checkout.
    #[test]
    fn export_writes_outside_git_only() {
        let mut s = UiScreens::default();
        s.select(native("account", "focus-1")).unwrap();
        let dir = std::env::temp_dir().join(format!("h2-editor-ui-{}", std::process::id()));
        let written = s.export(&dir).unwrap();
        let names: Vec<String> = written
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            ["account-focus-1.png", "account-focus-1.json", "sheet.png"]
        );
        assert!(dir.join("skulls-focus-0.png").exists(), "the sheet's tiles");
        std::fs::remove_dir_all(&dir).unwrap();
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui-out");
        let e = s.export(&here).unwrap_err().to_string();
        assert!(e.contains("Git checkout"), "{e}");
        assert!(!here.exists());
    }

    /// With the owner's disc: the pictures come from it, retail screens
    /// list and draw, and every native state is checked with nothing
    /// skipped.
    #[test]
    #[ignore = "needs the owner's disc image (H2_DISC)"]
    fn with_the_disc() {
        let iso = std::env::var_os("H2_DISC")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../disc/halo2.iso")
            });
        let mut s = UiScreens::default();
        s.set_disc(Some(&iso));
        assert!(s.has_disc(), "{:?}", s.disc_error());
        s.select(native("skulls", "focus-0")).unwrap();
        let shot = s.shot().unwrap();
        assert!(shot.from_disc && shot.skipped.is_empty());
        let retail = s.retail();
        let main = retail
            .iter()
            .find(|n| n.ends_with("main_menu"))
            .cloned()
            .unwrap_or_else(|| retail[0].clone());
        s.select(Pick::Retail(main)).unwrap();
        assert!(s.shot().unwrap().view.is_none());
        assert!(s.press(Input::A).is_none(), "nothing to step");
    }
}
