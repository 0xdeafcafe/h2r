//! File > New map (docs/editor/editor.md, "New maps"): a starter map built
//! by `starter_map` from its template over the open Halo 2 source's tags,
//! off the UI thread, and written as a derived package in the data
//! directory. The window's dialog and `map.new` start the same build;
//! when it is done the Maps list refreshes and the new map opens.
use std::sync::mpsc;
use std::time::Instant;

use starter_map::build::SHARED;
use starter_map::{Kind, Request, Template};

use super::{Action, App};

/// What a finished build wrote: `starter_map::write`'s answer.
pub type Written = starter_map::Written;

/// A new map building on its own thread.
pub struct Running {
    pub title: String,
    pub kind: Kind,
    pub started: Instant,
    rx: mpsc::Receiver<Result<Written, String>>,
}

impl App {
    /// The new map in progress, if any.
    pub fn new_map_running(&self) -> Option<&Running> {
        self.new_map.as_ref()
    }
    /// Why the last new map was not written, if it was not.
    pub fn new_map_error(&self) -> Option<&str> {
        self.new_map_error.as_deref()
    }
    /// The last new map written.
    pub fn new_map_written(&self) -> Option<&Written> {
        self.new_map_written.as_ref()
    }

    /// Why a new map can't start, if it can't: it takes its tags from the
    /// open Halo 2 source, which must hold the template's base map.
    pub fn new_map_refusal(&self, kind: Kind) -> Option<String> {
        let template = match Template::builtin() {
            Ok(t) => t,
            Err(e) => return Some(e.to_string()),
        };
        let base = template.common(kind).base;
        let Some(source) = self.source.as_ref() else {
            return Some(format!(
                "Open your Halo 2 disc first: a {} map takes its tags from {base}.",
                kind.name()
            ));
        };
        let has = source.map_names().iter().any(|n| n.eq_ignore_ascii_case(base));
        (!has).then(|| format!("The open source has no {base}, which a {} map is built over.", kind.name()))
    }

    /// Start building a new map called `title`. `replace` overwrites a
    /// package of the same name; without it one is refused.
    pub fn start_new_map(&mut self, title: &str, kind: Kind, replace: bool) -> Result<(), String> {
        if self.new_map.is_some() {
            return Err("a new map is already building".into());
        }
        let mut req = Request::new(title, kind).map_err(|e| e.to_string())?;
        if let Some(why) = self.new_map_refusal(kind) {
            self.new_map_error = Some(why.clone());
            return Err(why);
        }
        let template = Template::builtin().map_err(|e| e.to_string())?;
        let source = self.source.as_ref().expect("new_map_refusal checks the source");
        req.shared = Some(source.path().to_path_buf());
        // Where each map the build may read is: the base, and the shared
        // caches the source has (a base that leans on a missing one stops
        // the build naming it).
        let names = source.map_names();
        let base = template.common(kind).base;
        let mut locations = Vec::new();
        for stem in std::iter::once(base).chain(SHARED.iter().map(|(s, _)| *s)) {
            if let Some(n) = names.iter().find(|n| n.eq_ignore_ascii_case(stem)) {
                let at = source.location(n).map_err(|e| format!("{n}: {e:#}"))?;
                locations.push((stem.to_owned(), at));
            }
        }
        let data = self.data_root();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let disc = |stem: &str| -> Result<Vec<u8>, String> {
                let (_, (path, region)) = locations
                    .iter()
                    .find(|(s, _)| s == stem)
                    .ok_or_else(|| format!("the open source has no {stem}"))?;
                scenario_edit::MapSource::region(path.clone(), *region)
                    .read_base()
                    .map_err(|e| format!("reading {stem}: {e}"))
            };
            let r = starter_map::build(&template, &req, &disc)
                .and_then(|built| starter_map::write(&built, &data, replace))
                .map_err(|e| e.to_string());
            let _ = tx.send(r);
        });
        self.new_map_error = None;
        self.info(format!("building the new {} map {title}...", kind.name()));
        self.new_map = Some(Running {
            title: title.trim().to_owned(),
            kind,
            started: Instant::now(),
            rx,
        });
        Ok(())
    }

    /// Pick up a finished new map (each frame).
    pub fn poll_new_map(&mut self) {
        let Some(r) = self.new_map.as_ref() else {
            return;
        };
        let result = match r.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("the new map's build stopped".into()),
        };
        self.finish_new_map(result);
    }

    /// Wait for the new map building (`map.new`, tests).
    pub fn wait_new_map(&mut self) {
        if let Some(r) = self.new_map.as_ref() {
            let result = r
                .rx
                .recv()
                .unwrap_or_else(|_| Err("the new map's build stopped".into()));
            self.finish_new_map(result);
        }
    }

    fn finish_new_map(&mut self, result: Result<Written, String>) {
        let Some(r) = self.new_map.take() else {
            return;
        };
        let secs = r.started.elapsed().as_secs_f32();
        match result {
            Ok(w) => {
                self.info(format!(
                    "built the {} map {} in {secs:.1} s: {}",
                    w.kind.name(),
                    w.name,
                    w.dir.display()
                ));
                self.refresh_packages();
                let key = w
                    .dir
                    .file_name()
                    .map(|d| d.to_string_lossy().into_owned())
                    .unwrap_or_else(|| w.name.clone());
                self.dispatch(Action::OpenPackageEntry(key));
                self.new_map_written = Some(w);
            }
            Err(msg) => {
                self.error(format!("{}: {msg}", r.title));
                self.new_map_error = Some(msg);
            }
        }
    }
}
