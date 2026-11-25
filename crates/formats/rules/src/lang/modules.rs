//! Modules: `use "waves.rules"` reads another file's declarations in at
//! its line, before checking.
//!
//! [`expand`] turns a root file and the files it names into one text and
//! a line map. The text is what the same source would be in one file,
//! so the checker and lowering see no module and the blob is the blob of
//! that text. Diagnostics are mapped back through the line map to a file
//! and a line in it (`lang::diagnose_files`). See
//! `docs/game/rules-engine.md`, "Modules".
//!
//! The rules:
//!
//! - `use "name.rules"` is alone on a line at the top level (a trailing
//!   `;` is allowed). The line is replaced by the module's lines.
//! - A name is a plain file name of 1 to [`MAX_NAME`] letters, digits,
//!   `_`, `-` and `.`, ending `.rules`, with no directory part. The
//!   files of one script are one flat set.
//! - A module has no `rules` header. It may hold anything else a script
//!   holds, and a header line (`settings`, `requires`) counts as the
//!   script's own, so naming it in two files is the "given twice" error.
//! - A file is read once however many files name it. A file that names
//!   one that is reading it, directly or through others, is a cycle.
//! - At most [`MAX_FILES`] files, [`MAX_DEPTH`] deep, and
//!   [`super::MAX_SOURCE_BYTES`] bytes in all.

use super::lex::{self, Tok};
use super::{MAX_SOURCE_BYTES, Pos};

/// The most files in one script, the root included.
pub const MAX_FILES: usize = 32;
/// The deepest chain of `use` from the root.
pub const MAX_DEPTH: usize = 8;
/// The longest module name, in bytes.
pub const MAX_NAME: usize = 64;

/// Whether `name` is a module name: see the module's rules.
pub fn is_module_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name.len() > ".rules".len()
        && name.ends_with(".rules")
        && !name.starts_with('.')
        && !name.contains("..")
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// Whether `src` might name a module: a cheap test before [`expand`]
/// lexes it.
pub fn mentions_use(src: &str) -> bool {
    src.contains("use")
}

/// Where a line of the expanded text came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Origin {
    /// An index into [`Expanded::files`]; 0 is the root.
    pub file: u16,
    /// The line in that file, from 1.
    pub line: u32,
    /// The line of the root file's `use` that read the file in (the
    /// file's own line, for the root).
    pub root_line: u32,
}

/// The files read as one text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expanded {
    pub src: String,
    /// Each file's name, the root first, in the order they were read.
    pub files: Vec<String>,
    /// The origin of each line of `src`; line `n` (from 1) is `lines[n - 1]`.
    pub lines: Vec<Origin>,
}

impl Expanded {
    /// The file and line of line `line` (from 1) of the text.
    pub fn origin(&self, line: u32) -> Option<Origin> {
        self.lines.get((line as usize).checked_sub(1)?).copied()
    }

    pub fn file_name(&self, o: Origin) -> &str {
        &self.files[o.file as usize]
    }
}

/// A problem with a `use`, in the file that has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UseError {
    /// The file the problem is in: a name from [`Expanded::files`], or the
    /// root's.
    pub file: String,
    /// Its place in that file.
    pub pos: Pos,
    /// The line of the root file's `use` the problem came through (the
    /// problem's own line when it is in the root).
    pub root_line: u32,
    /// `use`, or `limit` for a size.
    pub code: &'static str,
    pub message: String,
}

/// One `use` line found in a file.
struct Found {
    line: u32,
    name: String,
    pos: Pos,
}

/// What a file's lexing says about modules.
struct Scan {
    uses: Vec<Found>,
    /// Problems with the `use` lines themselves.
    errors: Vec<(Pos, String)>,
    /// The file opens with a `rules` header.
    header: Option<Pos>,
}

fn scan(text: &str) -> Scan {
    let (tokens, _) = lex::lex_tolerant(text);
    let mut out = Scan {
        uses: Vec::new(),
        errors: Vec::new(),
        header: None,
    };
    if let Some(Tok::Ident(w)) = tokens.first().map(|t| &t.tok)
        && w == "rules"
    {
        out.header = Some(tokens[0].pos);
    }
    let mut depth = 0i32;
    let mut i = 0;
    while i < tokens.len() {
        let t = &tokens[i];
        match &t.tok {
            Tok::Punct("{" | "(" | "[") => depth += 1,
            Tok::Punct("}" | ")" | "]") => depth = (depth - 1).max(0),
            Tok::Ident(w) if w == "use" && depth == 0 && t.nl => {
                if let Some(Tok::Str(name)) = tokens.get(i + 1).map(|n| &n.tok) {
                    let mut end = i + 2;
                    if matches!(tokens.get(end).map(|n| &n.tok), Some(Tok::Punct(";"))) {
                        end += 1;
                    }
                    let after = tokens.get(end);
                    let alone = !tokens[i + 1].nl && after.is_none_or(|n| n.nl || n.tok == Tok::Eof);
                    if alone {
                        out.uses.push(Found {
                            line: t.pos.line,
                            name: name.clone(),
                            pos: t.pos,
                        });
                    } else {
                        out.errors.push((t.pos, "`use \"file\"` is alone on its line".to_string()));
                    }
                    i = end;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

struct Ctx<'a> {
    read: &'a dyn Fn(&str) -> Option<String>,
    files: Vec<String>,
    /// Every file taken in, so a second `use` of it adds nothing.
    seen: Vec<String>,
    /// The files being read, outermost first.
    stack: Vec<String>,
    out: String,
    lines: Vec<Origin>,
    errors: Vec<UseError>,
    bytes: usize,
    over: bool,
    said: bool,
}

impl Ctx<'_> {
    fn fail(&mut self, file: &str, pos: Pos, root_line: u32, code: &'static str, message: String) {
        self.errors.push(UseError {
            file: file.to_string(),
            pos,
            root_line,
            code,
            message,
        });
    }

    fn push_line(&mut self, text: &str, file: u16, line: u32, root_line: u32) {
        self.bytes += text.len() + 1;
        if self.bytes > MAX_SOURCE_BYTES {
            self.over = true;
            return;
        }
        self.out.push_str(text);
        self.out.push('\n');
        self.lines.push(Origin { file, line, root_line });
    }

    /// Takes in one file's text: its lines, with each `use` line replaced
    /// by the module it names. `root_line` is the root's `use` line that
    /// led here, or 0 for the root.
    fn file(&mut self, idx: u16, text: &str, root_line: u32) {
        let name = self.files[idx as usize].clone();
        let sc = scan(text);
        if idx != 0
            && let Some(pos) = sc.header
        {
            self.fail(&name, pos, root_line, "use", "a module has no `rules` header".to_string());
        }
        for (pos, m) in sc.errors {
            let at = if idx == 0 { pos.line } else { root_line };
            self.fail(&name, pos, at, "use", m);
        }
        let mut lines: Vec<&str> = text.split('\n').collect();
        if text.ends_with('\n') {
            lines.pop();
        }
        let mut uses = sc.uses.iter().peekable();
        for (i, l) in lines.iter().enumerate() {
            let line = i as u32 + 1;
            let at = if idx == 0 { line } else { root_line };
            if self.over {
                break;
            }
            match uses.peek() {
                Some(f) if f.line == line => {
                    let f = uses.next().expect("peeked");
                    self.module(&name, f, at);
                }
                _ => self.push_line(l, idx, line, at),
            }
        }
        if self.over && !self.said {
            self.said = true;
            let m = format!("the files are over the {} KiB cap in all", MAX_SOURCE_BYTES / 1024);
            self.fail(&name, Pos { line: 1, col: 1 }, root_line.max(1), "limit", m);
        }
    }

    fn module(&mut self, from_name: &str, f: &Found, root_line: u32) {
        let name = f.name.as_str();
        if !is_module_name(name) {
            let m = format!("`{name}` is not a module name: a plain file name ending `.rules`, with no directory");
            return self.fail(from_name, f.pos, root_line, "use", m);
        }
        if let Some(i) = self.stack.iter().position(|s| s == name) {
            let chain: Vec<&str> = self.stack[i..].iter().map(String::as_str).chain([name]).collect();
            let m = format!("a cycle: {}", chain.join(" uses "));
            return self.fail(from_name, f.pos, root_line, "use", m);
        }
        if self.seen.iter().any(|s| s == name) {
            return;
        }
        if self.stack.len() > MAX_DEPTH {
            let m = format!("files nest at most {MAX_DEPTH} deep; `{name}` is one more");
            return self.fail(from_name, f.pos, root_line, "limit", m);
        }
        if self.files.len() >= MAX_FILES {
            let m = format!("a script has at most {MAX_FILES} files");
            return self.fail(from_name, f.pos, root_line, "limit", m);
        }
        let Some(text) = (self.read)(name) else {
            let m = format!("no file named `{name}`");
            return self.fail(from_name, f.pos, root_line, "use", m);
        };
        self.seen.push(name.to_string());
        self.files.push(name.to_string());
        let idx = (self.files.len() - 1) as u16;
        self.stack.push(name.to_string());
        self.file(idx, &text, root_line);
        self.stack.pop();
    }
}

/// Reads `root` (named `root_name` in messages) and the files it names
/// through `read`, which gives a file's text by name. The text and line
/// map, or every problem with a `use`, in the order they were met.
pub fn expand(root_name: &str, root: &str, read: &dyn Fn(&str) -> Option<String>) -> Result<Expanded, Vec<UseError>> {
    let mut cx = Ctx {
        read,
        files: vec![root_name.to_string()],
        seen: vec![root_name.to_string()],
        stack: vec![root_name.to_string()],
        out: String::new(),
        lines: Vec::new(),
        errors: Vec::new(),
        bytes: 0,
        over: false,
        said: false,
    };
    cx.file(0, root, 0);
    if cx.errors.is_empty() {
        Ok(Expanded {
            src: cx.out,
            files: cx.files,
            lines: cx.lines,
        })
    } else {
        cx.errors.truncate(super::MAX_ERRORS);
        Err(cx.errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let list: Vec<(String, String)> = list.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        move |n| list.iter().find(|(a, _)| a == n).map(|(_, b)| b.clone())
    }

    #[test]
    fn names_are_plain_files() {
        for ok in ["waves.rules", "a-b_c.1.rules"] {
            assert!(is_module_name(ok), "{ok}");
        }
        for bad in ["", ".rules", "a", "a.txt", "../a.rules", "d/a.rules", "d\\a.rules", ".a.rules", "a..b.rules", "a b.rules"] {
            assert!(!is_module_name(bad), "{bad}");
        }
        assert!(!is_module_name(&format!("{}.rules", "x".repeat(MAX_NAME))));
    }

    #[test]
    fn a_use_line_is_replaced_by_the_modules_lines() {
        let read = files(&[("w.rules", "global var a: int = 0\nglobal var b: int = 0\n")]);
        let x = expand("main.rules", "rules \"T\" 1\nuse \"w.rules\"\non tick { }\n", &read).unwrap();
        assert_eq!(x.src, "rules \"T\" 1\nglobal var a: int = 0\nglobal var b: int = 0\non tick { }\n");
        let at = |n| x.origin(n).map(|o| (x.file_name(o).to_string(), o.line, o.root_line));
        assert_eq!(at(1), Some(("main.rules".into(), 1, 1)));
        assert_eq!(at(3), Some(("w.rules".into(), 2, 2)));
        assert_eq!(at(4), Some(("main.rules".into(), 3, 3)));
        assert_eq!(at(5), None);
    }

    #[test]
    fn a_file_is_read_once_and_a_cycle_names_both_files() {
        let read = files(&[
            ("a.rules", "use \"c.rules\"\n"),
            ("b.rules", "use \"c.rules\"\n"),
            ("c.rules", "global var c: int = 0\n"),
        ]);
        let x = expand("m.rules", "use \"a.rules\"\nuse \"b.rules\"\nuse \"c.rules\"\n", &read).unwrap();
        assert_eq!(x.src, "global var c: int = 0\n");
        let read = files(&[("a.rules", "use \"b.rules\"\n"), ("b.rules", "use \"a.rules\"\n")]);
        let e = expand("m.rules", "use \"a.rules\"\n", &read).unwrap_err();
        assert_eq!(e.len(), 1);
        assert_eq!((e[0].file.as_str(), e[0].root_line), ("b.rules", 1));
        assert_eq!(e[0].message, "a cycle: a.rules uses b.rules uses a.rules");
        let e = expand("m.rules", "use \"m.rules\"\n", &read).unwrap_err();
        assert_eq!(e[0].message, "a cycle: m.rules uses m.rules");
    }

    #[test]
    fn a_bad_use_is_an_error_at_its_line() {
        let read = files(&[("h.rules", "rules \"X\" 1\n")]);
        let e = |src: &str| expand("m.rules", src, &read).unwrap_err().remove(0);
        assert!(e("\nuse \"nope.rules\"\n").message.contains("no file named"));
        assert_eq!(e("\nuse \"nope.rules\"\n").pos, Pos { line: 2, col: 1 });
        assert!(e("use \"../x.rules\"\n").message.contains("not a module name"));
        assert!(e("use \"h.rules\"\n").message.contains("no `rules` header"));
        assert!(e("use \"h.rules\" on tick { }\n").message.contains("alone on its line"));
        // Inside a block it is not a `use` line.
        assert!(expand("m.rules", "on tick {\nuse \"x.rules\"\n}\n", &read).is_ok());
    }

    #[test]
    fn the_depth_the_count_and_the_size_are_capped() {
        let chain = |n: usize| {
            move |name: &str| {
                let i: usize = name.strip_prefix('f')?.strip_suffix(".rules")?.parse().ok()?;
                Some(if i < n { format!("use \"f{}.rules\"\n", i + 1) } else { String::new() })
            }
        };
        assert!(expand("m.rules", "use \"f1.rules\"\n", &chain(MAX_DEPTH)).is_ok());
        assert_eq!(expand("m.rules", "use \"f1.rules\"\n", &chain(MAX_DEPTH + 2)).unwrap_err()[0].code, "limit");
        let wide: String = (1..=MAX_FILES).map(|i| format!("use \"f{i}.rules\"\n")).collect();
        assert_eq!(expand("m.rules", &wide, &chain(0)).unwrap_err()[0].code, "limit");
        let big = |_: &str| Some("x\n".repeat(MAX_SOURCE_BYTES / 2));
        let e = expand("m.rules", "use \"a.rules\"\nuse \"b.rules\"\n", &big).unwrap_err();
        assert!(e.iter().any(|x| x.code == "limit" && x.message.contains("cap")));
    }
}
