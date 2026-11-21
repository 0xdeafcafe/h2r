//! Names for the builtins no scenario calls, from a script documentation
//! listing the user supplies (the `hs_doc` text the retail editing kit's
//! script documentation is), read at run time and never stored.
//! docs/formats/haloscript.md, "Builtin names from a documentation listing".
//!
//! A listing entry is a signature line, `(<return> name <param> ...)`,
//! then lines of description up to a blank line. A `;` line names a
//! section; entries after one that names globals are external globals.
//! An entry's name is given to a builtin only when the builtin's
//! descriptor has the same return type and the same parameter types in
//! order, and the pairing is not ambiguous:
//!
//! - Builtins a scenario names (`Catalog::observe`) whose listing entry
//!   has the same signature are anchors. The longest run of anchors in
//!   the same order in both is kept.
//! - Between two anchors, the unnamed builtins and the unused entries are
//!   paired in order when they are as many and every pair matches.
//!   Otherwise a builtin takes an entry only when that entry is the one
//!   unused entry there with its signature and the builtin is the one
//!   unnamed builtin there with it.
//!
//! Everything else stays unnamed and shows as `#builtin:<index>`. An
//! entry with optional, repeated or grouped parameters (`[<x>]`,
//! `<x(s)>`) has no fixed signature and names nothing; nor does a builtin
//! that reads its own arguments (flags bit 1), since its parameter list is
//! not its argument count.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{Catalog, Result, invalid};

/// One signature and its description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocEntry {
    pub name: String,
    /// The return type's name as the listing spells it.
    pub returns: String,
    /// Parameter type names, or `None` for a signature with optional,
    /// repeated or grouped parameters.
    pub parameters: Option<Vec<String>>,
    /// The parameter text as written.
    pub written: String,
    pub description: String,
    /// 1-based line of the signature.
    pub line: usize,
}

/// A parsed listing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptDoc {
    pub functions: Vec<DocEntry>,
    pub globals: Vec<DocEntry>,
    /// Signature lines that did not parse (unbalanced, no return type).
    pub unreadable: Vec<usize>,
}

impl ScriptDoc {
    /// Parse a listing. A text with no entry at all is an error: it is not
    /// a script documentation listing.
    pub fn parse(text: &str) -> Result<Self> {
        let mut doc = Self::default();
        let mut globals = false;
        let mut current: Option<(DocEntry, bool)> = None;
        let finish = |doc: &mut Self, entry: Option<(DocEntry, bool)>| {
            if let Some((mut e, global)) = entry {
                e.description = e.description.trim_end().to_owned();
                if global {
                    doc.globals.push(e);
                } else {
                    doc.functions.push(e);
                }
            }
        };
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if let Some(comment) = line.strip_prefix(';') {
                finish(&mut doc, current.take());
                let upper = comment.to_ascii_uppercase();
                if upper.contains("GLOBAL") {
                    globals = true;
                } else if upper.contains("FUNCTION") {
                    globals = false;
                }
                continue;
            }
            if line.starts_with("(<") {
                finish(&mut doc, current.take());
                match signature(line) {
                    Some(mut e) => {
                        e.line = i + 1;
                        current = Some((e, globals));
                    }
                    None => doc.unreadable.push(i + 1),
                }
                continue;
            }
            if line.is_empty() {
                finish(&mut doc, current.take());
                continue;
            }
            if let Some((e, _)) = current.as_mut() {
                if !e.description.is_empty() {
                    e.description.push('\n');
                }
                e.description.push_str(line);
            }
        }
        finish(&mut doc, current.take());
        if doc.functions.is_empty() && doc.globals.is_empty() {
            return Err(invalid(
                "no script documentation entries: a listing has lines like `(<void> name <short>)`",
            ));
        }
        Ok(doc)
    }
}

/// `(<return> name params...)`, balanced on one line.
fn signature(line: &str) -> Option<DocEntry> {
    let mut depth = 0i32;
    let mut end = None;
    for (i, c) in line.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i);
                    break;
                }
                if depth < 0 {
                    return None;
                }
            }
            _ => {}
        }
    }
    let inner = &line[1..end?];
    let rest = inner.strip_prefix('<')?;
    let close = rest.find('>')?;
    let returns = rest[..close].trim();
    if !is_word(returns) {
        return None;
    }
    let rest = rest[close + 1..].trim_start();
    let name_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    let name = &rest[..name_end];
    if name.is_empty() || name.contains(['<', '>', '(', ')', '[', ']', '\u{fffd}']) {
        return None;
    }
    let written = rest[name_end..].trim().to_owned();
    let parameters = written
        .split_whitespace()
        .map(|t| {
            t.strip_prefix('<')
                .and_then(|t| t.strip_suffix('>'))
                .filter(|w| is_word(w))
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>();
    Some(DocEntry {
        name: name.to_owned(),
        returns: returns.to_owned(),
        parameters,
        written,
        description: String::new(),
        line: 0,
    })
}

fn is_word(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Where a builtin's name comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameFrom {
    /// A scenario calls it by this name.
    Map,
    /// The documentation listing's entry with this index in
    /// [`ScriptDoc::functions`].
    Doc(usize),
}

/// What the listing named.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocReport {
    pub functions: usize,
    /// Entries with no fixed signature.
    pub without_signature: usize,
    /// Map-named builtins whose entry agrees, in order.
    pub anchors: usize,
    /// Map-named builtins whose entry has another signature: the listing
    /// describes another build there.
    pub disagree: Vec<String>,
    pub named: usize,
}

/// Every builtin's name, if it has one, and where it came from.
#[derive(Debug, Clone)]
pub struct Naming {
    names: Vec<Option<(String, NameFrom)>>,
    pub report: Option<DocReport>,
}

impl Naming {
    /// The catalog's own names (from scenarios), and with a listing the
    /// names it gives by signature as the module comment says.
    pub fn new(catalog: &Catalog, doc: Option<&ScriptDoc>) -> Self {
        let mut names: Vec<Option<(String, NameFrom)>> = catalog
            .functions()
            .iter()
            .map(|f| f.name().map(|n| (n.to_owned(), NameFrom::Map)))
            .collect();
        let report = doc.map(|doc| name_from_doc(catalog, doc, &mut names));
        Self { names, report }
    }

    pub fn name(&self, index: usize) -> Option<&str> {
        self.names.get(index)?.as_ref().map(|(n, _)| n.as_str())
    }
    pub fn from(&self, index: usize) -> Option<NameFrom> {
        self.names.get(index)?.as_ref().map(|(_, f)| *f)
    }
    /// The name, or `#builtin:<index>`.
    pub fn label(&self, index: usize) -> String {
        self.name(index)
            .map_or_else(|| format!("#builtin:{index}"), str::to_owned)
    }
    /// The call form with parameter types: `(name <short> <object>)`, or
    /// `(#builtin:<index> ...)` unnamed.
    pub fn call_form(&self, catalog: &Catalog, index: usize) -> Result<String> {
        let f = catalog.function(
            u16::try_from(index).map_err(|_| invalid(format!("unknown builtin index {index}")))?,
        )?;
        let mut out = format!("({}", self.label(index));
        for &p in f.parameters() {
            out.push_str(&format!(" <{}>", catalog.type_name(p)?));
        }
        out.push(')');
        Ok(out)
    }
    pub fn len(&self) -> usize {
        self.names.len()
    }
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// The builtin's (return, parameters) by type name; `None` when a type
/// index has no name, or when the builtin reads its own arguments (flags
/// bit 1): its parameter list is not its argument count.
fn builtin_signature(catalog: &Catalog, index: usize) -> Option<(String, Vec<String>)> {
    let f = catalog.functions().get(index)?;
    if f.flags() & 2 != 0 {
        return None;
    }
    let returns = catalog.type_name(f.return_type()).ok()?.to_owned();
    let parameters = f
        .parameters()
        .iter()
        .map(|&p| catalog.type_name(p).ok().map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    Some((returns, parameters))
}

fn matches(signature: &Option<(String, Vec<String>)>, entry: &DocEntry) -> bool {
    match (signature, &entry.parameters) {
        (Some((returns, parameters)), Some(theirs)) => {
            *returns == entry.returns && parameters == theirs
        }
        _ => false,
    }
}

fn name_from_doc(
    catalog: &Catalog,
    doc: &ScriptDoc,
    names: &mut [Option<(String, NameFrom)>],
) -> DocReport {
    let entries = &doc.functions;
    let signatures: Vec<_> = (0..names.len())
        .map(|i| builtin_signature(catalog, i))
        .collect();
    let mut count: HashMap<&str, usize> = HashMap::new();
    for e in entries {
        *count.entry(e.name.as_str()).or_default() += 1;
    }
    let unique = |e: &DocEntry| count[e.name.as_str()] == 1;
    let taken: HashSet<String> = names.iter().flatten().map(|(n, _)| n.clone()).collect();
    let by_name: BTreeMap<&str, usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| unique(e))
        .map(|(d, e)| (e.name.as_str(), d))
        .collect();

    // Anchors, by builtin index, then the longest run increasing in both.
    let mut report = DocReport {
        functions: entries.len(),
        without_signature: entries.iter().filter(|e| e.parameters.is_none()).count(),
        ..DocReport::default()
    };
    let mut anchors = Vec::new();
    for (i, n) in names.iter().enumerate() {
        let Some((name, _)) = n else { continue };
        let Some(&d) = by_name.get(name.as_str()) else {
            continue;
        };
        if signatures[i].is_none() {
            // Its own arguments, or an unnamed type: nothing to compare.
            continue;
        }
        if matches(&signatures[i], &entries[d]) {
            anchors.push((i, d));
        } else {
            report.disagree.push(name.clone());
        }
    }
    let chain = longest_increasing(&anchors);
    report.anchors = chain.len();

    let pool = |d: usize| {
        let e = &entries[d];
        e.parameters.is_some() && unique(e) && !taken.contains(&e.name)
    };
    let mut bounds = vec![(None, None)];
    bounds.extend(chain.iter().map(|&(i, d)| (Some(i), Some(d))));
    bounds.push((None, None));
    let mut given = Vec::new();
    for pair in bounds.windows(2) {
        let (i_lo, d_lo) = pair[0];
        let (i_hi, d_hi) = pair[1];
        let after = |x: usize, lo: Option<usize>| lo.is_none_or(|lo| x > lo);
        let before = |x: usize, hi: Option<usize>| hi.is_none_or(|hi| x < hi);
        let builtins: Vec<usize> = (0..names.len())
            .filter(|&i| names[i].is_none() && after(i, i_lo) && before(i, i_hi))
            .collect();
        let docs: Vec<usize> = (0..entries.len())
            .filter(|&d| pool(d) && after(d, d_lo) && before(d, d_hi))
            .collect();
        if builtins.is_empty() || docs.is_empty() {
            continue;
        }
        if builtins.len() == docs.len()
            && builtins
                .iter()
                .zip(&docs)
                .all(|(&i, &d)| matches(&signatures[i], &entries[d]))
        {
            given.extend(builtins.iter().copied().zip(docs.iter().copied()));
            continue;
        }
        for &i in &builtins {
            let mut fits = docs
                .iter()
                .filter(|&&d| matches(&signatures[i], &entries[d]));
            let (Some(&d), None) = (fits.next(), fits.next()) else {
                continue;
            };
            let rivals = builtins
                .iter()
                .filter(|&&j| matches(&signatures[j], &entries[d]))
                .count();
            if rivals == 1 {
                given.push((i, d));
            }
        }
    }
    for (i, d) in given {
        names[i] = Some((entries[d].name.clone(), NameFrom::Doc(d)));
        report.named += 1;
    }
    report
}

/// The longest subsequence of `pairs` (sorted by the first) increasing in
/// the second.
fn longest_increasing(pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let n = pairs.len();
    let mut length = vec![1usize; n];
    let mut previous = vec![usize::MAX; n];
    for k in 0..n {
        for j in 0..k {
            if pairs[j].1 < pairs[k].1 && length[j] + 1 > length[k] {
                length[k] = length[j] + 1;
                previous[k] = j;
            }
        }
    }
    let Some(mut k) = (0..n).max_by_key(|&k| (length[k], usize::MAX - k)) else {
        return Vec::new();
    };
    let mut out = vec![pairs[k]];
    while previous[k] != usize::MAX {
        k = previous[k];
        out.push(pairs[k]);
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{Builtin, Exe};

    /// Invented types: 0 `void`, 1 `boolean`, 2 `short`, 3 `real`.
    fn catalog(builtins: &[(u16, &[u16])], named: &[(usize, &str)]) -> Catalog {
        catalog_with_flags(builtins, named, &[])
    }

    /// As `catalog`, with flags 2 (own arguments) on the builtins in `own`.
    fn catalog_with_flags(
        builtins: &[(u16, &[u16])],
        named: &[(usize, &str)],
        own: &[usize],
    ) -> Catalog {
        let exe = Exe {
            types: ["void", "boolean", "short", "real"]
                .map(str::to_owned)
                .to_vec(),
            builtins: builtins
                .iter()
                .enumerate()
                .map(|(i, &(returns, parameters))| Builtin {
                    returns,
                    flags: if own.contains(&i) { 2 } else { 0 },
                    parameters: parameters.to_vec(),
                    ..Builtin::default()
                })
                .collect(),
            ..Exe::default()
        };
        let mut c = exe.catalog();
        for &(i, n) in named {
            c.functions[i].name = Some(n.to_owned());
        }
        c
    }

    fn listing(entries: &[&str]) -> ScriptDoc {
        let mut text = String::from("; invented functions\n\n");
        for e in entries {
            text.push_str(e);
            text.push_str("\ninvented description\n\n");
        }
        ScriptDoc::parse(&text).unwrap()
    }

    #[test]
    fn a_listing_parses_signatures_sections_and_descriptions() {
        let doc = ScriptDoc::parse(
            "; FUNCTIONS\n\n(<void> alpha <short> <real>)\nfirst line\nsecond line\n\n\
             (<boolean> beta <boolean(s)>)\nmany\n\n(<short> gamma [<real>])\n\n\
             (<void> broken <short>\n\n; GLOBALS\n\n(<real> delta)\na global\n",
        )
        .unwrap();
        let names: Vec<_> = doc.functions.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["alpha", "beta", "gamma"]);
        assert_eq!(
            doc.functions[0].parameters,
            Some(vec!["short".into(), "real".into()])
        );
        assert_eq!(doc.functions[0].description, "first line\nsecond line");
        assert_eq!(doc.functions[0].line, 3);
        assert_eq!(doc.functions[1].parameters, None);
        assert_eq!(doc.functions[2].parameters, None);
        assert_eq!(doc.unreadable, [12]);
        assert_eq!(doc.globals.len(), 1);
        assert_eq!(doc.globals[0].name, "delta");
        assert!(ScriptDoc::parse("not a listing\n").is_err());
    }

    #[test]
    fn names_go_only_where_the_signature_matches() {
        // 0 is named by a map; 1 takes `beta` (in order between anchors);
        // 2 has a `real` parameter the listing's `gamma` does not.
        let c = catalog(&[(0, &[2]), (1, &[]), (0, &[3])], &[(0, "alpha")]);
        let doc = listing(&[
            "(<void> alpha <short>)",
            "(<boolean> beta)",
            "(<void> gamma <short>)",
        ]);
        let n = Naming::new(&c, Some(&doc));
        assert_eq!(n.name(0), Some("alpha"));
        assert_eq!(n.from(0), Some(NameFrom::Map));
        assert_eq!(n.name(1), Some("beta"));
        assert_eq!(n.from(1), Some(NameFrom::Doc(1)));
        assert_eq!(n.name(2), None);
        assert_eq!(n.label(2), "#builtin:2");
        assert_eq!(n.call_form(&c, 2).unwrap(), "(#builtin:2 <real>)");
        let r = n.report.unwrap();
        assert_eq!((r.anchors, r.named), (1, 1));
    }

    #[test]
    fn an_ambiguous_signature_names_nothing() {
        // Two unnamed `(<void>)` builtins and two `(<void>)` entries in a
        // different count from the gap: neither pairing is certain.
        let c = catalog(&[(0, &[]), (0, &[]), (1, &[2])], &[]);
        let doc = listing(&[
            "(<void> one)",
            "(<void> two)",
            "(<void> three)",
            "(<boolean> four <short>)",
        ]);
        let n = Naming::new(&c, Some(&doc));
        assert_eq!((n.name(0), n.name(1)), (None, None));
        assert_eq!(n.name(2), Some("four"));
    }

    #[test]
    fn equal_runs_pair_in_order_and_anchors_out_of_order_are_dropped() {
        let c = catalog(
            &[(0, &[]), (0, &[]), (1, &[]), (0, &[]), (0, &[2])],
            &[(2, "middle"), (4, "last")],
        );
        // `last` comes before `middle` in this listing: one anchor stays.
        let doc = listing(&[
            "(<void> a)",
            "(<void> b)",
            "(<boolean> middle)",
            "(<void> c)",
            "(<void> last <short>)",
        ]);
        let n = Naming::new(&c, Some(&doc));
        assert_eq!(
            (0..5).map(|i| n.label(i)).collect::<Vec<_>>(),
            ["a", "b", "middle", "c", "last"]
        );
        let shuffled = listing(&["(<void> last <short>)", "(<boolean> middle)", "(<void> a)"]);
        let n = Naming::new(&c, Some(&shuffled));
        assert_eq!(n.report.as_ref().unwrap().anchors, 1);
    }

    #[test]
    fn a_disagreeing_entry_is_reported_and_its_name_never_moves() {
        let c = catalog(&[(0, &[2]), (0, &[3])], &[(0, "alpha")]);
        // The listing's alpha takes a real: it describes another build.
        let doc = listing(&["(<void> alpha <real>)"]);
        let n = Naming::new(&c, Some(&doc));
        assert_eq!(n.name(1), None);
        assert_eq!(n.report.unwrap().disagree, ["alpha"]);
    }

    #[test]
    fn repeated_names_and_open_signatures_name_nothing() {
        let c = catalog(&[(0, &[2]), (1, &[])], &[]);
        let doc = listing(&[
            "(<void> twice <short>)",
            "(<void> twice <short>)",
            "(<boolean> open [<short>])",
        ]);
        let n = Naming::new(&c, Some(&doc));
        assert_eq!((n.name(0), n.name(1)), (None, None));
        assert_eq!(n.report.unwrap().without_signature, 1);
    }

    #[test]
    fn a_builtin_that_reads_its_own_arguments_takes_no_listing_name() {
        // Builtin 0 reads its own arguments; its empty parameter list says
        // nothing about its arguments, so `(<void> zero)` cannot be checked.
        // Builtin 1, named by a map, is neither an anchor nor a disagreement.
        let c = catalog_with_flags(&[(0, &[]), (0, &[])], &[(1, "own")], &[0, 1]);
        let doc = listing(&["(<void> zero)", "(<void> own <short>)"]);
        let n = Naming::new(&c, Some(&doc));
        assert_eq!(n.name(0), None);
        let r = n.report.unwrap();
        assert_eq!((r.anchors, r.named), (0, 0));
        assert!(r.disagree.is_empty());
    }
}
