//! Identify statically linked Xbox SDK (XDK) library functions in an XBE.
//!
//! Matching is by OOVPA signatures, the byte-pattern format of Cxbx-Reloaded's
//! XbSymbolDatabase. The signature data is ODbL-1.0 licensed, which cannot be
//! shipped under this repo's MIT OR Apache-2.0, so it is never vendored: it is
//! read at run time from a checkout fetched by `scripts/setup-xbsymboldb.sh`
//! (default `.tools/XbSymbolDatabase`, override with `H2_XBSYMDB`). Only the
//! matcher lives here.
//!
//! The XBE header lists each linked library and its build number. Each library
//! selects one or more signature databases and the code sections to search;
//! every symbol is tried against all its revisions up to that build, and the
//! highest revision that matches wins. Signatures may refer to other symbols
//! by address (call targets, data pointers), so matching runs in waves until no
//! new symbol resolves.

mod parse;
mod scan;
mod solve;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use xbe::Xbe;

pub use parse::Kind;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "XbSymbolDatabase sources not found at {0}\nrun scripts/setup-xbsymboldb.sh (or set H2_XBSYMDB to a checkout)"
    )]
    NoDatabase(PathBuf),
    #[error("reading {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0}: {1}")]
    Parse(PathBuf, String),
    #[error("{0}")]
    Scan(String),
    #[error(transparent)]
    Xbe(#[from] xbe::Error),
}

/// One identified function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub va: u32,
    /// Signature database it came from: `D3D8`, `D3D8LTCG`, `DSound`,
    /// `Xapi`, `XGraphic`, `XOnline`, `XNet`, `XActEng`, `JVS`.
    pub library: &'static str,
    pub name: String,
    /// The signature revision (XDK build) that matched.
    pub xdk_version: u16,
    /// Link-time-code-generated variant (custom calling convention).
    pub ltcg: bool,
    pub kind: Kind,
    /// The same signature matched at more than one address.
    pub ambiguous: bool,
    /// Another signature named a different function at the same address
    /// (short signatures from different library flavours collide).
    pub shared: bool,
}

#[derive(Debug, Default)]
pub struct Report {
    /// Sorted by address, then name.
    pub matches: Vec<Match>,
    /// Signature tasks still waiting on a symbol that never resolved.
    pub unresolved_tasks: usize,
    /// Distinct symbols with at least one signature runnable for this build.
    pub candidate_symbols: BTreeMap<&'static str, usize>,
    /// Table entries whose signature variable could not be found.
    pub missing_sigs: usize,
    /// Signature runs thrown out for matching many places once their
    /// data-symbol references were left unchecked.
    pub dropped_ambiguous: usize,
    /// Symbols that stalled tasks were waiting on, with how many tasks each.
    pub missing_refs: BTreeMap<String, usize>,
    /// Libraries from the XBE header that no database covers.
    pub unscanned_libraries: Vec<String>,
}

impl Report {
    /// Per database: (functions, ambiguous, ltcg).
    pub fn counts(&self) -> BTreeMap<&'static str, (usize, usize, usize)> {
        let mut m: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
        for x in self.matches.iter().filter(|x| x.kind == Kind::Function) {
            let e = m.entry(x.library).or_default();
            e.0 += 1;
            e.1 += x.ambiguous as usize;
            e.2 += x.ltcg as usize;
        }
        m
    }
}

/// How an XBE library maps to signature databases and where to search.
struct Db {
    /// Our name for the database; also its source file stem.
    name: &'static str,
    file: &'static str,
    /// XBE library names that enable it.
    libs: &'static [&'static str],
    /// Sections to search (Cxbx's lists, minus the flash ROM).
    sections: &'static [&'static str],
}

const DBS: &[Db] = &[
    Db { name: "D3D8", file: "D3D8_OOVPA.c", libs: &["D3D8", "D3D8LTCG"], sections: &[".text", "D3D"] },
    Db { name: "D3D8LTCG", file: "D3D8LTCG_OOVPA.c", libs: &["D3D8LTCG"], sections: &[".text", "D3D"] },
    Db { name: "DSound", file: "DSound_OOVPA.c", libs: &["DSOUND"], sections: &["DSOUND", ".rdata"] },
    Db { name: "JVS", file: "JVS_OOVPA.c", libs: &["JVS"], sections: &[".text", "XPP"] },
    Db { name: "XActEng", file: "XActEng_OOVPA.c", libs: &["XACTENG"], sections: &["XACTENG"] },
    Db { name: "Xapi", file: "Xapi_OOVPA.c", libs: &["XAPILIB"], sections: &[".text", "XPP"] },
    Db { name: "XGraphic", file: "XGraphic_OOVPA.c", libs: &["XGRAPHC"], sections: &[".text", "XGRPH"] },
    Db {
        name: "XOnline",
        file: "XOnline_OOVPA.c",
        libs: &["XONLINE", "XONLINEN", "XONLINES", "XONLINLS"],
        sections: &[".text", "XONLINE", "XNET"],
    },
    Db {
        name: "XNet",
        file: "XNet_OOVPA.c",
        libs: &["XNET", "XNETS", "XNETN", "XONLINE", "XONLINEN", "XONLINES", "XONLINLS"],
        sections: &[".text", "XNET"],
    },
];

/// Where the signature sources are expected.
pub fn default_db_dir(repo_root: &Path) -> PathBuf {
    match std::env::var_os("H2_XBSYMDB") {
        Some(p) => PathBuf::from(p),
        None => repo_root.join(".tools/XbSymbolDatabase"),
    }
}

struct LoadedDb {
    db: &'static Db,
    build: u16,
    entries: Vec<parse::Entry>,
}

fn read(p: &Path) -> Result<String, Error> {
    std::fs::read_to_string(p).map_err(|e| Error::Io(p.to_path_buf(), e))
}

/// Identify library functions in `xbe` using the sources under `db_dir`
/// (the root of an XbSymbolDatabase checkout).
pub fn identify(xbe: &Xbe, db_dir: &Path) -> Result<Report, Error> {
    let src = db_dir.join("src/OOVPADatabase");
    if !src.join("D3D8_OOVPA.c").is_file() {
        return Err(Error::NoDatabase(db_dir.to_path_buf()));
    }
    let mut report = Report::default();
    let mut store = parse::SigStore::default();
    let mut loaded: Vec<LoadedDb> = Vec::new();
    for lib in &xbe.libraries {
        let lname = lib.name.trim();
        if !DBS.iter().any(|d| d.libs.contains(&lname)) {
            report.unscanned_libraries.push(format!("{lname} {}", lib.build));
        }
    }
    for db in DBS {
        // The newest build among the libraries that enable this database.
        let Some(build) = xbe.libraries.iter().filter(|l| db.libs.contains(&l.name.trim())).map(|l| l.build).max()
        else {
            continue;
        };
        let raw = read(&src.join(db.file))?;
        let mut includes = Vec::new();
        let text = parse::preprocess(&raw, &mut store.aliases, &mut includes);
        for inc in includes.iter().filter(|i| i.ends_with(".inl")) {
            let t = read(&src.join(inc))?;
            let mut inc2 = Vec::new();
            let p = parse::preprocess(&t, &mut store.aliases, &mut inc2);
            parse::parse_sigs(&p, &mut store).map_err(|e| Error::Parse(src.join(inc), e))?;
        }
        let entries = parse::parse_table(&raw, &text).map_err(|e| Error::Parse(src.join(db.file), e))?;
        loaded.push(LoadedDb { db, build, entries });
    }

    // Symbols that other signatures can refer to by address: functions (and
    // internals) that have at least one real signature.
    let mut resolvable: HashSet<&str> = HashSet::new();
    for l in &loaded {
        for e in l.entries.iter().filter(|e| e.kind != Kind::Variable && !e.manual) {
            if e.versions.iter().any(|&v| store.resolve(&e.var, v).is_some()) {
                resolvable.insert(e.xref.as_str());
            }
        }
    }

    // One task per (entry, revision at or below the library build).
    let mut tasks: Vec<scan::Task> = Vec::new();
    for (di, l) in loaded.iter().enumerate() {
        let mut seen: HashSet<&str> = HashSet::new();
        for (ei, e) in l.entries.iter().enumerate() {
            if e.manual || e.versions.is_empty() {
                continue;
            }
            let mut any = false;
            for &v in &e.versions {
                if v > l.build {
                    continue;
                }
                match store.resolve(&e.var, v) {
                    Some(sig) => {
                        any = true;
                        tasks.push(scan::Task::new(di, ei, v, sig));
                    }
                    None => report.missing_sigs += 1,
                }
            }
            if any && e.kind == Kind::Function && seen.insert(e.var.as_str()) {
                *report.candidate_symbols.entry(l.db.name).or_default() += 1;
            }
        }
    }

    // Sections per database.
    let mut sections: HashMap<&'static str, Vec<(u32, &[u8])>> = HashMap::new();
    for db in DBS {
        let mut v = Vec::new();
        for s in xbe.sections.iter().filter(|s| db.sections.contains(&s.name.as_str())) {
            v.push((s.va, xbe.section_bytes(s)?));
        }
        sections.insert(db.name, v);
    }

    // Resolve addresses; see solve.rs for when an address becomes authoritative.
    let section_list: Vec<Vec<(u32, &[u8])>> = loaded.iter().map(|l| sections[l.db.name].clone()).collect();
    let symbol_of = |db: usize, entry: usize| loaded[db].entries[entry].xref.as_str();
    let read_u32 = |va: u32| xbe.read_u32(va).ok();
    let outcome = solve::solve(
        &mut tasks,
        &solve::Input { sections: &section_list, symbol_of: &symbol_of, resolvable: &resolvable, read_u32: &read_u32 },
    )
    .map_err(Error::Scan)?;
    let done = outcome.done;
    report.dropped_ambiguous = outcome.dropped_ambiguous;
    report.unresolved_tasks = done.iter().filter(|d| !**d).count();
    for (t, _) in tasks.iter().zip(&done).filter(|(_, d)| !**d) {
        for (_, n) in t.xrefs.iter().filter(|(_, n)| !outcome.known.contains_key(n.as_str())) {
            *report.missing_refs.entry(n.clone()).or_default() += 1;
        }
    }

    // Final answer: per entry, the highest revision that matched.
    let mut best: BTreeMap<(usize, usize), (u16, Vec<u32>)> = BTreeMap::new();
    for (i, t) in tasks.iter().enumerate() {
        if done[i] && !t.hits.is_empty() {
            let e = best.entry((t.db, t.entry)).or_insert((t.version, t.hits.clone()));
            if t.version >= e.0 {
                *e = (t.version, t.hits.clone());
            }
        }
    }
    for ((di, ei), (ver, hits)) in best {
        let l = &loaded[di];
        let e = &l.entries[ei];
        for &va in &hits {
            report.matches.push(Match {
                va,
                library: l.db.name,
                name: e.name.clone(),
                xdk_version: ver,
                ltcg: e.ltcg,
                kind: e.kind,
                ambiguous: hits.len() > 1,
                shared: false,
            });
        }
    }
    report.matches.sort_by(|a, b| (a.va, &a.name, a.library).cmp(&(b.va, &b.name, b.library)));
    for i in 0..report.matches.len() {
        let va = report.matches[i].va;
        let name = report.matches[i].name.clone();
        let shared = report.matches.iter().any(|m| m.va == va && m.name != name && m.kind == Kind::Function);
        report.matches[i].shared = shared;
    }
    Ok(report)
}
