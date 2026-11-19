//! Named repository states for the disc preview and offline controller walkthrough.
use super::*;
use crate::mods_screen::Install;
use crate::ui_kit::page::{Focus, ListEffect, Live, Page, focus_line};

pub struct Preview {
    root: Sample,
    repo: Arc<Repo>,
    stack: Vec<(Sample, Focus)>,
}

impl Preview {
    pub fn new(root: Sample) -> Self {
        Self {
            root,
            repo: sample_repo(root),
            stack: vec![(root, Focus(0))],
        }
    }

    fn states(&self) -> &'static [&'static str] {
        match self.root {
            Sample::List => &[
                "focus-0",
                "focus-2",
                "focus-3",
                "loading",
                "empty",
                "offline",
                "signed-out",
                "installed",
                "downloading",
                "failed",
                "long-name",
            ],
            Sample::Pack => &[
                "focus-0",
                "focus-2",
                "focus-3",
                "partial",
                "installed",
                "downloading",
                "failed",
                "empty-pack",
            ],
            Sample::Page => &[
                "focus-0",
                "installed",
                "downloading",
                "failed",
                "confirm-remove",
                "remove-failed",
                "unavailable",
                "long-name",
            ],
        }
    }

    fn reset(&mut self, state: &str) {
        *self = Self::new(self.root);
        if let Some(row) = state.strip_prefix("focus-") {
            self.stack[0].1 = Focus(row.parse().expect("named focus row"));
            return;
        }
        if matches!(state, "loading" | "empty" | "offline" | "signed-out") {
            self.repo = Repo::new(Arc::new(SampleSource), None);
            match state {
                "empty" => self.repo.listed(Ok(Vec::new())),
                "offline" => self.repo.listed(Err(ModsError::Offline)),
                "signed-out" => self.repo.listed(Err(ModsError::NotSignedIn)),
                _ => {}
            }
            return;
        }

        if state == "long-name" {
            let mut entries = sample_entries();
            entries[0].name = "WWWWWWWW WWWWWWWW WWWWWWWW: Swords".into();
            entries[0].credit = Some("Waterworks Map Making Community".into());
            entries[0].description = Some("Flooded passages beneath the base. ".repeat(9));
            self.repo.listed(Ok(entries));
            if self.root == Sample::Page {
                self.repo.press_list(0);
            }
            return;
        }

        let maps = match self.root {
            Sample::Pack => self.repo.pack().unwrap().1,
            Sample::List | Sample::Page => self.repo.entries(),
        };
        let id = maps[0].id;
        match state {
            "empty-pack" | "unavailable" => {
                let bytes = self.repo.save_state();
                self.repo.listed(Ok(Vec::new()));
                self.repo.load_state(&bytes).unwrap();
            }
            "installed" if self.root == Sample::Pack => {
                for e in maps {
                    self.repo.install_done(e.id, Ok(()));
                }
            }
            "installed" | "partial" => self.repo.install_done(id, Ok(())),
            "confirm-remove" => {
                self.repo.install_done(id, Ok(()));
                self.repo.act();
            }
            "remove-failed" => {
                self.repo.install_done(id, Ok(()));
                self.repo
                    .lock()
                    .installs
                    .insert(id, Install::RemoveFailed("Package in use".into()));
                self.repo.touch();
            }
            "downloading" => {
                self.repo.lock().installs.insert(
                    id,
                    Install::Downloading {
                        done: 37,
                        total: 100,
                    },
                );
                self.repo.touch();
                if self.root == Sample::Pack {
                    self.stack[0].1 = Focus(1);
                }
            }
            "failed" => {
                self.repo.install_done(id, Err(ModsError::Offline));
                if self.root == Sample::Pack {
                    self.stack[0].1 = Focus(1);
                }
            }
            _ => unreachable!("unregistered Mods preview state"),
        }
    }
}

impl Live for Preview {
    fn press(&mut self, input: Input) -> String {
        let Some((screen, focus)) = self.stack.last_mut() else {
            return "closed".into();
        };
        let effect = screen.page(Arc::clone(&self.repo)).step(focus, input);
        match &effect {
            ListEffect::Action => {
                let row = Some(focus.0 as i16);
                match screen {
                    Sample::List => self.repo.x_list(row),
                    Sample::Pack => self.repo.x_pack(row),
                    Sample::Page => self.repo.act(),
                };
            }
            ListEffect::Open(name) => self.stack.push((Sample::named(name).unwrap(), Focus(0))),
            ListEffect::Close => {
                self.stack.pop();
            }
            _ => {}
        }
        format!("{effect:?}")
    }

    fn spec(&self) -> ScreenSpec {
        let (screen, focus) = self.stack.last().copied().unwrap_or((self.root, Focus(0)));
        screen.page(Arc::clone(&self.repo)).spec(&focus)
    }

    fn summary(&self) -> String {
        if self.stack.is_empty() {
            return "closed".into();
        }
        let spec = self.spec();
        format!("{} | {}", spec.name, focus_line(&spec))
    }

    fn state_names(&self) -> Vec<String> {
        self.states().iter().map(|s| (*s).into()).collect()
    }

    fn specs(&self) -> Vec<(String, ScreenSpec)> {
        let mut preview = Self::new(self.root);
        self.states()
            .iter()
            .map(|&name| {
                preview.reset(name);
                (name.into(), preview.spec())
            })
            .collect()
    }

    fn go(&mut self, name: &str) -> bool {
        if !self.states().contains(&name) {
            return false;
        }
        self.reset(name);
        true
    }
}
