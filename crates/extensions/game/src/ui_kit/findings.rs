//! Every finding for one screen state in one list: the spec checks against
//! the disc, the content checks, the layout checks (those on what was
//! drawn included) and the consistency rules that read one view
//! (docs/game/ui-builder.md "The checks"). `h2 ui preview` prints it, and
//! the editor's UI Screens counts and lists it.

use std::fmt;

use anyhow::Result;
use serde::Serialize;

use crate::ui_kit::layout::{self, Rules};
use crate::ui_kit::preview;
use crate::ui_kit::spec::ScreenSpec;
use crate::ui_kit::view::View;

/// One finding.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Found {
    /// Which checks found it: `check` (the spec against the disc),
    /// `content`, `layout` or `consistency`.
    pub check: &'static str,
    /// Its kind or rule (`panel_shared`, `button`, `value_arrows`).
    pub kind: String,
    /// The widgets it names.
    pub ids: Vec<String>,
    pub text: String,
}

impl fmt::Display for Found {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.check, self.text)
    }
}

/// What one state's checks found.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct StateReport {
    pub findings: Vec<Found>,
    /// Flagged, never failed ([`layout::Note`]).
    pub notes: Vec<String>,
    /// Layout checks the source could not run (no art without the disc).
    pub skipped: Vec<&'static str>,
}

/// Spec `spec` drawn as `view` from `from`, checked.
pub fn of_state(from: &preview::From<'_>, spec: &ScreenSpec, view: &View) -> Result<StateReport> {
    let src = from.source();
    let mut findings = Vec::new();
    if let preview::From::Disc(d) = from {
        findings.extend(d.check(spec)?.into_iter().map(|f| Found {
            check: "check",
            kind: "spec".into(),
            ids: Vec::new(),
            text: f.to_string(),
        }));
    }
    findings.extend(
        crate::ui_kit::content::check_with_source(spec, view, src)
            .into_iter()
            .map(|f| Found {
                check: "content",
                kind: f.kind.into(),
                text: f.to_string(),
                ids: vec![f.id],
            }),
    );
    let r = layout::check(view, src, &Rules::for_spec(spec));
    findings.extend(r.issues.iter().map(|i| Found {
        check: "layout",
        kind: i.kind(),
        ids: i.ids().into_iter().map(String::from).collect(),
        text: i.to_string(),
    }));
    findings.extend(
        crate::ui_kit::consistency::view_findings(spec, view)
            .into_iter()
            .map(|f| Found {
                check: "consistency",
                kind: f.rule.into(),
                ids: f.ids,
                text: f.text,
            }),
    );
    Ok(StateReport {
        findings,
        notes: r.notes.iter().map(ToString::to_string).collect(),
        skipped: r.skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clean screen has nothing; the Quick Options dialogs, whose rows
    /// the synthetic source (no `fs_dialog` art) leaves with too little
    /// contrast, have layout findings named by check and kind.
    #[test]
    fn a_states_findings_come_in_one_list() {
        let from = preview::From::Synthetic;
        let clean = crate::ui_kit::screens::states("kit.example").unwrap();
        let (name, spec) = &clean[0];
        let v = from.view(spec, name).unwrap();
        let r = of_state(&from, spec, &v).unwrap();
        assert!(r.findings.is_empty(), "{:?}", r.findings);
        assert_eq!(r.skipped, ["covered", "off screen"]);

        let mut found = Vec::new();
        for (state, spec) in crate::ui_kit::screens::states("clankers").unwrap() {
            let v = from.view(&spec, &state).unwrap();
            found.extend(of_state(&from, &spec, &v).unwrap().findings);
        }
        assert!(found.iter().any(|x| x.check == "layout" && !x.kind.is_empty()), "{found:?}");
        assert!(found.iter().all(|x| x.to_string().starts_with(x.check)));
    }
}
