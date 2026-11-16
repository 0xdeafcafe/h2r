//! File dialogs behind a small trait, so the shell and tools can ask for a
//! path without a window in tests. [`NativeDialog`] is the system's own
//! (the `rfd` crate: NSOpenPanel / NSSavePanel on macOS); a tool or the
//! shell without one, or a dialog that answers [`Picked::Unavailable`],
//! falls back to the typed-path prompt.

use std::path::{Path, PathBuf};

/// One file-type filter: a label and extensions without the dot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub name: String,
    pub extensions: Vec<String>,
}

impl Filter {
    pub fn new(name: impl Into<String>, extensions: &[&str]) -> Self {
        Filter {
            name: name.into(),
            extensions: extensions.iter().map(|e| e.to_string()).collect(),
        }
    }
}

/// The filters for a tool's [`crate::Tool::extensions`]: one filter with
/// all of them, or none when the tool lists none.
pub fn filters_for(label: &str, extensions: &[&str]) -> Vec<Filter> {
    if extensions.is_empty() {
        Vec::new()
    } else {
        vec![Filter::new(label, extensions)]
    }
}

/// What a dialog answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    Path(PathBuf),
    /// The user closed it without choosing.
    Cancelled,
    /// No dialog could be shown; ask for a typed path instead.
    Unavailable,
}

/// A modal file dialog. Both calls block until the user answers.
pub trait FileDialog {
    /// Choose an existing file. `start` is a file or directory to start in.
    fn open_file(&mut self, title: &str, filters: &[Filter], start: Option<&Path>) -> Picked;

    /// Choose where to save; `suggested` is the default file (its
    /// directory, when it has one, is where the dialog starts).
    fn save_file(&mut self, title: &str, filters: &[Filter], suggested: &Path) -> Picked;
}

/// The system's file dialog.
#[derive(Debug, Default, Clone, Copy)]
pub struct NativeDialog;

impl NativeDialog {
    fn base(title: &str, filters: &[Filter]) -> rfd::FileDialog {
        let mut d = rfd::FileDialog::new().set_title(title);
        for f in filters {
            d = d.add_filter(&f.name, &f.extensions);
        }
        d
    }
}

/// Splits `start` into the directory a dialog starts in and the file name
/// it suggests: a directory starts there, a file in its parent.
pub fn start_dir_and_name(start: &Path) -> (Option<PathBuf>, Option<String>) {
    if start.as_os_str().is_empty() {
        return (None, None);
    }
    if start.is_dir() {
        return (Some(start.to_owned()), None);
    }
    let dir = start
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_owned);
    let name = start.file_name().map(|n| n.to_string_lossy().into_owned());
    (dir, name)
}

impl FileDialog for NativeDialog {
    fn open_file(&mut self, title: &str, filters: &[Filter], start: Option<&Path>) -> Picked {
        let mut d = Self::base(title, filters);
        if let Some((Some(dir), _)) = start.map(start_dir_and_name) {
            d = d.set_directory(dir);
        }
        d.pick_file().map_or(Picked::Cancelled, Picked::Path)
    }

    fn save_file(&mut self, title: &str, filters: &[Filter], suggested: &Path) -> Picked {
        let mut d = Self::base(title, filters);
        let (dir, name) = start_dir_and_name(suggested);
        if let Some(dir) = dir {
            d = d.set_directory(dir);
        }
        if let Some(name) = name {
            d = d.set_file_name(name);
        }
        d.save_file().map_or(Picked::Cancelled, Picked::Path)
    }
}

/// A scripted dialog for tests: answers from a queue (then
/// [`Picked::Unavailable`]) and records every request.
#[derive(Debug, Default)]
pub struct ScriptedDialog {
    pub answers: std::collections::VecDeque<Picked>,
    pub asked: std::rc::Rc<std::cell::RefCell<Vec<Request>>>,
}

/// One request a [`ScriptedDialog`] saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub save: bool,
    pub title: String,
    pub filters: Vec<Filter>,
    pub start: Option<PathBuf>,
}

impl ScriptedDialog {
    pub fn new(answers: impl IntoIterator<Item = Picked>) -> Self {
        ScriptedDialog {
            answers: answers.into_iter().collect(),
            asked: Default::default(),
        }
    }

    fn answer(&mut self, r: Request) -> Picked {
        self.asked.borrow_mut().push(r);
        self.answers.pop_front().unwrap_or(Picked::Unavailable)
    }
}

impl FileDialog for ScriptedDialog {
    fn open_file(&mut self, title: &str, filters: &[Filter], start: Option<&Path>) -> Picked {
        self.answer(Request {
            save: false,
            title: title.into(),
            filters: filters.to_vec(),
            start: start.map(Path::to_owned),
        })
    }

    fn save_file(&mut self, title: &str, filters: &[Filter], suggested: &Path) -> Picked {
        self.answer(Request {
            save: true,
            title: title.into(),
            filters: filters.to_vec(),
            start: Some(suggested.to_owned()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_splits_into_directory_and_name() {
        assert_eq!(start_dir_and_name(Path::new("")), (None, None));
        assert_eq!(
            start_dir_and_name(Path::new("untitled.dat")),
            (None, Some("untitled.dat".into()))
        );
        assert_eq!(
            start_dir_and_name(Path::new("/nonexistent/dir/a.dat")),
            (Some("/nonexistent/dir".into()), Some("a.dat".into()))
        );
        let tmp = std::env::temp_dir();
        assert_eq!(start_dir_and_name(&tmp), (Some(tmp.clone()), None));
    }

    #[test]
    fn filters_follow_the_tools_extensions() {
        assert!(filters_for("Files", &[]).is_empty());
        assert_eq!(
            filters_for("Playlists", &["dat"]),
            [Filter {
                name: "Playlists".into(),
                extensions: vec!["dat".into()]
            }]
        );
    }

    #[test]
    fn the_script_answers_in_order_then_is_unavailable() {
        let mut d = ScriptedDialog::new([Picked::Cancelled, Picked::Path("a".into())]);
        let f = filters_for("x", &["x"]);
        assert_eq!(d.open_file("one", &f, None), Picked::Cancelled);
        assert_eq!(
            d.save_file("two", &f, Path::new("b")),
            Picked::Path("a".into())
        );
        assert_eq!(d.open_file("three", &[], None), Picked::Unavailable);
        let asked = d.asked.borrow();
        assert_eq!(asked.len(), 3);
        assert!(asked[1].save && asked[1].start == Some("b".into()));
    }
}
