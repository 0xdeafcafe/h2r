//! Scripted input for the headless screenshots (`h2-editor --do`): mouse
//! moves, clicks, drags, typing and keys aimed at widgets by label, then
//! crops and numbered callouts over the frame. Widgets and windows are
//! found through [`crate::probe`].
//!
//! A target is one of:
//! - `x,y`: a point, in points from the top left;
//! - `win:<name>`: the drawn window whose name ends with `<name>`, else
//!   the front-most one containing it (tooltips are `##Tooltip`, menus
//!   `##Menu`, combo lists `##Combo`, context menus `##Popup`, docked
//!   panels their key, `###inspector`);
//! - `[<window>>]<label>[#<n>]`: the `n`th (from 0) widget labelled
//!   `<label>` (the `##id` part included), else whose label contains it,
//!   in a window whose name contains `<window>`.
//!
//! Any target may end in `@<dx>,<dy>`: that far from its top left
//! corner, rather than its centre.
use anyhow::{Result, anyhow, bail};

use crate::probe::{Item, Window};

/// One step of a script.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Rest the mouse on a target long enough for its tooltip.
    Hover(String),
    /// Click (left, right, or double).
    Click(String, Button),
    /// Glide the mouse to a target over a few frames (drags).
    Move(String),
    /// Press or release the left button where the mouse is.
    Down,
    Up,
    /// Type text into the focused field.
    Type(String),
    /// Press and release a key `n` times.
    Key(dear_imgui_rs::Key, usize),
    /// Run `n` frames.
    Wait(usize),
    /// Let `ms` milliseconds of wall time pass (background work), then
    /// run a few frames.
    Sleep(u64),
    /// Scroll the mouse wheel (positive is up).
    Wheel(f32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Double,
}

/// Steps separated by `;`: `hover <t>`, `click <t>`, `rclick <t>`,
/// `dclick <t>`, `move <t>`, `down`, `up`, `type <text>`,
/// `key <name>[*n]`, `wait <frames>`, `sleep <ms>`, `wheel <dy>`.
pub fn parse(text: &str) -> Result<Vec<Step>> {
    let mut out = Vec::new();
    for s in text.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let (verb, rest) = s.split_once(' ').unwrap_or((s, ""));
        let rest = rest.trim().to_owned();
        let need = |what: &str| -> Result<String> {
            if rest.is_empty() {
                bail!("--do: {verb} needs {what}")
            }
            Ok(rest.clone())
        };
        out.push(match verb {
            "hover" => Step::Hover(need("a target")?),
            "click" => Step::Click(need("a target")?, Button::Left),
            "rclick" => Step::Click(need("a target")?, Button::Right),
            "dclick" => Step::Click(need("a target")?, Button::Double),
            "move" => Step::Move(need("a target")?),
            "down" => Step::Down,
            "up" => Step::Up,
            "type" => Step::Type(need("text")?),
            "key" => {
                let v = need("a key")?;
                let (name, n) = match v.split_once('*') {
                    Some((k, n)) => (k.to_owned(), n.parse()?),
                    None => (v, 1),
                };
                Step::Key(key(&name)?, n)
            }
            "wait" => Step::Wait(need("frames")?.parse()?),
            "sleep" => Step::Sleep(need("milliseconds")?.parse()?),
            "wheel" => Step::Wheel(need("an amount")?.parse()?),
            v => bail!("--do: unknown step {v:?}"),
        });
    }
    Ok(out)
}

fn key(name: &str) -> Result<dear_imgui_rs::Key> {
    use dear_imgui_rs::Key as K;
    Ok(match name.to_ascii_lowercase().as_str() {
        "backspace" => K::Backspace,
        "delete" => K::Delete,
        "enter" => K::Enter,
        "escape" => K::Escape,
        "tab" => K::Tab,
        "down" => K::DownArrow,
        "up" => K::UpArrow,
        "e" => K::E,
        "w" => K::W,
        "f" => K::F,
        "end" => K::End,
        _ => bail!("--do: unknown key {name:?}"),
    })
}

/// What one frame drew, to aim targets at.
#[derive(Default, Clone)]
pub struct Scene {
    pub items: Vec<Item>,
    pub windows: Vec<Window>,
}

impl Scene {
    /// A target's rectangle (`[x0, y0, x1, y1]` in points) and the point
    /// in it to aim at.
    pub fn find(&self, target: &str) -> Result<([f32; 4], [f32; 2])> {
        let (what, offset) = match target.rsplit_once('@') {
            Some((w, o)) if point(o).is_some() => (w, point(o)),
            _ => (target, None),
        };
        let rect = if let Some([x, y]) = point(what) {
            [x, y, x, y]
        } else if let Some(name) = what.strip_prefix("win:") {
            // A docked panel (`###log`) rather than its child windows
            // (`###log/##log-lines`); else the front-most.
            let mut found = self.windows.iter().rev().filter(|w| w.name.contains(name));
            let first = found.clone().next();
            found
                .find(|w| w.name.ends_with(name))
                .or(first)
                .map(|w| w.rect)
                .ok_or_else(|| anyhow!("no window {name:?} drawn"))?
        } else {
            let (window, label) = match what.split_once('>') {
                Some((w, l)) => (w, l),
                None => ("", what),
            };
            let (label, n) = match label.rsplit_once('#') {
                Some((l, n)) if !l.ends_with('#') && n.parse::<usize>().is_ok() => {
                    (l, n.parse().unwrap())
                }
                _ => (label, 0),
            };
            // Exact labels first (`##map` is not `x##map-x`), else parts.
            let inside = || self.items.iter().filter(|i| i.window.contains(window));
            let exact = inside().any(|i| i.label == label);
            inside()
                .filter(|i| if exact { i.label == label } else { i.label.contains(label) })
                .nth(n)
                .map(|i| i.rect)
                .ok_or_else(|| anyhow!("no widget {what:?} drawn"))?
        };
        let at = match offset {
            Some([dx, dy]) => [rect[0] + dx, rect[1] + dy],
            None => [(rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0],
        };
        Ok((rect, at))
    }

    /// The union of the targets' rectangles.
    pub fn union(&self, targets: &[String]) -> Result<[f32; 4]> {
        let mut r = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for t in targets {
            let (b, _) = self.find(t)?;
            r = [r[0].min(b[0]), r[1].min(b[1]), r[2].max(b[2]), r[3].max(b[3])];
        }
        Ok(r)
    }
}

/// The widget labels the targets name (to [`crate::probe::want`]).
pub fn labels<'a>(targets: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out = Vec::new();
    for t in targets {
        let what = match t.rsplit_once('@') {
            Some((w, o)) if point(o).is_some() => w,
            _ => t,
        };
        if point(what).is_some() || what.starts_with("win:") {
            continue;
        }
        let label = what.split_once('>').map_or(what, |(_, l)| l);
        let label = match label.rsplit_once('#') {
            Some((l, n)) if !l.ends_with('#') && n.parse::<usize>().is_ok() => l,
            _ => label,
        };
        out.push(label.to_owned());
    }
    out
}

/// The targets a script's steps aim at.
pub fn targets(steps: &[Step]) -> impl Iterator<Item = &str> {
    steps.iter().filter_map(|s| match s {
        Step::Hover(t) | Step::Click(t, _) | Step::Move(t) => Some(t.as_str()),
        _ => None,
    })
}

/// `x,y` as numbers.
fn point(s: &str) -> Option<[f32; 2]> {
    let (x, y) = s.split_once(',')?;
    Some([x.trim().parse().ok()?, y.trim().parse().ok()?])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> Scene {
        let item = |label: &str, window: &str, rect| Item {
            label: label.into(),
            window: window.into(),
            rect,
        };
        Scene {
            items: vec![
                item("File", "##MainMenuBar", [0.0, 0.0, 30.0, 20.0]),
                item("Move (W)", "###viewport", [100.0, 40.0, 160.0, 60.0]),
                item("Slayer##a", "##Combo_00", [10.0, 100.0, 90.0, 120.0]),
                item("Slayer##b", "##Combo_00", [10.0, 120.0, 90.0, 140.0]),
            ],
            windows: vec![Window {
                name: "Inspector###inspector".into(),
                rect: [1000.0, 30.0, 1400.0, 900.0],
            }],
        }
    }

    #[test]
    fn targets_find_widgets_windows_and_points() {
        let s = scene();
        assert_eq!(s.find("File").unwrap().1, [15.0, 10.0]);
        assert_eq!(s.find("Combo>Slayer#1").unwrap().0[1], 120.0);
        assert_eq!(s.find("Move (W)@4,2").unwrap().1, [104.0, 42.0]);
        assert_eq!(s.find("win:inspector").unwrap().0[2], 1400.0);
        assert_eq!(s.find("5,6").unwrap().1, [5.0, 6.0]);
        assert!(s.find("Rotate").is_err());
        let u = s
            .union(&["File".into(), "win:inspector".into()])
            .unwrap();
        assert_eq!(u, [0.0, 0.0, 1400.0, 900.0]);
    }

    #[test]
    fn scripts_parse() {
        let steps = parse("click View; hover Reset layout ; key backspace*3; wait 4").unwrap();
        assert_eq!(steps[0], Step::Click("View".into(), Button::Left));
        assert_eq!(steps[1], Step::Hover("Reset layout".into()));
        assert_eq!(steps[2], Step::Key(dear_imgui_rs::Key::Backspace, 3));
        assert_eq!(steps[3], Step::Wait(4));
        assert!(parse("jump x").is_err());
        let wanted = labels(targets(&steps).chain(["win:x", "Combo>##map#2@3,4", "1,2"]));
        assert_eq!(wanted, ["View", "Reset layout", "##map"]);
    }
}
