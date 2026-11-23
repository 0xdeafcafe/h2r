//! `h2 rules editors [status|install]`: the rules language's editor
//! extensions (docs/editor/rules-editors.md). `install` puts the VS Code
//! extension in VS Code (a `.vsix` built in `editors/vscode/`, else the
//! one on the `editors-latest` release, else one built with npm) and
//! prints the steps for Zed, which installs a dev extension from its own
//! command palette only. The one extension serves `.rules` scripts and
//! Studio's HaloScript working copies (`.hsc`). `status` says what is
//! installed and whether `h2 rules lsp` and `h2 hs lsp` start. The functions that decide and print are pure,
//! so tests read them without a process.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// The rolling release CI attaches the extensions to
/// (`.github/workflows/editors.yml`); [`VSIX_ASSET`] is the `.vsix` there.
pub const EDITORS_RELEASE: &str = "https://github.com/0xdeafcafe/h2r/releases/download/editors-latest";
pub const VSIX_ASSET: &str = "h2-rules.vsix";

/// The extension's id in VS Code (`publisher.name` in
/// `editors/vscode/package.json`) and in Zed (`editors/zed/extension.toml`).
/// Studio's "Open in editor" looks for the same ids
/// (`crates/apps/studio/src/app/rules_edit.rs`).
pub const VSCODE_EXTENSION: &str = "h2recomp.h2-rules";
pub const ZED_EXTENSION: &str = "h2-rules";

/// How long `status` waits for the language server's `initialize` answer.
const LSP_TIMEOUT: Duration = Duration::from_secs(10);

const USAGE: &str = "usage: h2 rules editors [status]
       h2 rules editors install [vscode|zed] [--build]
  status says whether VS Code and Zed have the extension and whether
  h2 rules lsp and h2 hs lsp start.
  install puts the extension in VS Code with code --install-extension: the
  newest .vsix in editors/vscode/, else the editors-latest release's, else
  one built there with npm (--build builds first). For Zed it prints the
  steps: Zed installs a dev extension from its command palette.";

pub fn run(words: &[String]) -> Result<()> {
    let mut sub = None;
    let mut which = None;
    let mut build = false;
    for w in words {
        match w.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--build" => build = true,
            "status" | "install" if sub.is_none() => sub = Some(w.as_str()),
            "vscode" | "code" | "zed" if sub == Some("install") && which.is_none() => which = Some(w.as_str()),
            _ => bail!("{USAGE}"),
        }
    }
    let env = Env::here();
    match sub.unwrap_or("status") {
        "status" => {
            let code_ext = env.code.as_deref().and_then(|c| list_extensions(c).ok());
            let exe = std::env::current_exe().map_err(|e| e.to_string());
            let lsp = exe.clone().and_then(|exe| lsp_starts(&exe, "rules"));
            print!("{}", status_text(&env, code_ext.as_deref(), &lsp));
            let hs = exe.and_then(|exe| lsp_starts(&exe, "hs"));
            print!("{}", server_line("h2 hs lsp", &hs));
            Ok(())
        }
        _ => {
            if which != Some("zed") {
                install_vscode(&env, build)?;
            }
            if which.is_none_or(|w| w == "zed") {
                print!("{}", zed_steps(&env));
            }
            Ok(())
        }
    }
}

/// Where the editors and their extensions are on this machine.
pub(crate) struct Env {
    /// `editors/` of the tree this `h2` was built from, if it is still
    /// there.
    pub editors: Option<PathBuf>,
    /// VS Code's command line.
    pub code: Option<PathBuf>,
    /// Zed's command line.
    pub zed: Option<PathBuf>,
    /// Whether Zed has the extension installed.
    pub zed_extension: bool,
    /// Where Zed keeps installed extensions.
    pub zed_installed_dir: Option<PathBuf>,
}

impl Env {
    fn here() -> Env {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let macos = cfg!(target_os = "macos");
        let path = std::env::var("PATH").ok();
        let is_file = |p: &Path| p.is_file();
        let editors = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../editors");
        let zed_installed_dir = home.as_deref().map(|h| zed_installed_dir(h, macos));
        Env {
            editors: editors.canonicalize().ok().filter(|d| d.is_dir()),
            code: find_editor("code", path.as_deref(), home.as_deref(), macos, is_file),
            zed: find_editor("zed", path.as_deref(), home.as_deref(), macos, is_file),
            zed_extension: zed_installed_dir
                .as_deref()
                .is_some_and(|d| d.join(ZED_EXTENSION).exists()),
            zed_installed_dir,
        }
    }
}

/// An editor's command line: on `PATH`, in Homebrew's or `/usr/local`'s
/// `bin` (an app started from the Finder has no `PATH`), or inside the
/// app on macOS.
pub(crate) fn find_editor(
    name: &str,
    path_var: Option<&str>,
    home: Option<&Path>,
    macos: bool,
    is_file: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = path_var
        .unwrap_or("")
        .split(':')
        .filter(|d| !d.is_empty())
        .map(|d| Path::new(d).join(name))
        .collect();
    for dir in ["/opt/homebrew/bin", "/usr/local/bin"] {
        candidates.push(Path::new(dir).join(name));
    }
    if macos {
        let inside = match name {
            "code" => Some("Visual Studio Code.app/Contents/Resources/app/bin/code"),
            "zed" => Some("Zed.app/Contents/MacOS/cli"),
            _ => None,
        };
        if let Some(inside) = inside {
            candidates.push(Path::new("/Applications").join(inside));
            if let Some(home) = home {
                candidates.push(home.join("Applications").join(inside));
            }
        }
    }
    candidates.into_iter().find(|p| is_file(p))
}

/// Where Zed keeps installed extensions, a dev extension as a link to its
/// folder: `extensions/installed` under its data directory.
pub(crate) fn zed_installed_dir(home: &Path, macos: bool) -> PathBuf {
    let data = if macos {
        home.join("Library/Application Support/Zed")
    } else {
        home.join(".local/share/zed")
    };
    data.join("extensions/installed")
}

/// The version of the extension in `code --list-extensions
/// --show-versions` output (`id@version` lines), if it is there.
pub(crate) fn extension_version(listing: &str) -> Option<String> {
    listing.lines().find_map(|line| {
        let (id, version) = line.trim().split_once('@')?;
        id.eq_ignore_ascii_case(VSCODE_EXTENSION).then(|| version.to_string())
    })
}

fn list_extensions(code: &Path) -> Result<String> {
    let out = Command::new(code)
        .args(["--list-extensions", "--show-versions"])
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {}", code.display()))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The newest `h2-rules-<version>.vsix` of a folder's file names, by
/// version.
pub(crate) fn newest_vsix<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let version = |name: &str| -> Option<Vec<u64>> {
        let v = name.strip_prefix("h2-rules-")?.strip_suffix(".vsix")?;
        v.split('.').map(|part| part.parse().ok()).collect()
    };
    names
        .into_iter()
        .filter_map(|n| version(n).map(|v| (v, n)))
        .max()
        .map(|(_, n)| n)
}

fn local_vsix(dir: &Path) -> Option<PathBuf> {
    let names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect();
    newest_vsix(names.iter().map(String::as_str)).map(|n| dir.join(n))
}

/// Builds the `.vsix` in `dir` with npm: `npm ci` (or `npm install`
/// without a lock file), then `npx vsce package`.
fn build_vsix(dir: &Path) -> Result<PathBuf> {
    let install = if dir.join("package-lock.json").is_file() { "ci" } else { "install" };
    for args in [vec![install], vec!["exec", "--", "vsce", "package"]] {
        println!("npm {} (in {})", args.join(" "), dir.display());
        let status = Command::new("npm")
            .args(&args)
            .current_dir(dir)
            .status()
            .context("running npm (is Node.js installed?)")?;
        if !status.success() {
            bail!("npm {} failed in {}", args.join(" "), dir.display());
        }
    }
    local_vsix(dir).with_context(|| format!("vsce made no .vsix in {}", dir.display()))
}

/// Downloads the release's `.vsix` into the temporary directory.
fn download_vsix() -> Result<PathBuf> {
    let url = format!("{EDITORS_RELEASE}/{VSIX_ASSET}");
    println!("downloading {url}");
    let resp = ureq::get(&url)
        .timeout(Duration::from_secs(60))
        .call()
        .with_context(|| format!("downloading {url}"))?;
    let mut bytes = Vec::new();
    resp.into_reader()
        .take(64 << 20)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {url}"))?;
    let path = std::env::temp_dir().join(VSIX_ASSET);
    std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

fn install_vscode(env: &Env, build: bool) -> Result<()> {
    let Some(code) = &env.code else {
        println!(
            "VS Code: not found (no `code` on PATH or in /Applications). Install it, or run\n  \
             \"Shell Command: Install 'code' command in PATH\" from its command palette."
        );
        return Ok(());
    };
    let dir = env.editors.as_ref().map(|e| e.join("vscode"));
    let vsix = match (&dir, build) {
        (Some(dir), true) => build_vsix(dir)?,
        _ => match dir.as_deref().and_then(local_vsix) {
            Some(v) => v,
            None => match download_vsix() {
                Ok(v) => v,
                Err(e) => match &dir {
                    Some(dir) => {
                        println!("{e:#}; building it instead");
                        build_vsix(dir)?
                    }
                    None => return Err(e),
                },
            },
        },
    };
    println!("{} --install-extension {}", code.display(), vsix.display());
    let status = Command::new(code)
        .args(["--install-extension"])
        .arg(&vsix)
        .arg("--force")
        .status()
        .with_context(|| format!("running {}", code.display()))?;
    if !status.success() {
        bail!("{} --install-extension failed", code.display());
    }
    println!("VS Code: installed {VSCODE_EXTENSION}. Reload its window to start it.");
    Ok(())
}

/// The steps that install the Zed extension, which Zed takes only from
/// its own command palette.
pub(crate) fn zed_steps(env: &Env) -> String {
    let folder = env
        .editors
        .as_ref()
        .map(|e| e.join("zed").display().to_string())
        .unwrap_or_else(|| "editors/zed in a checkout of h2recomp".to_string());
    let mut s = String::new();
    if env.zed_extension {
        s.push_str("Zed: the extension is installed. To update it, rebuild it from the Extensions page.\n");
        return s;
    }
    if env.zed.is_none() {
        s.push_str("Zed: not found. Install it from https://zed.dev, then:\n");
    } else {
        s.push_str("Zed: install the extension as a dev extension:\n");
    }
    s.push_str("  1. Install Rust with rustup (https://rustup.rs): Zed builds the extension\n");
    s.push_str("     itself, for WebAssembly.\n");
    s.push_str("  2. In Zed, open the command palette (Cmd+Shift+P) and run\n");
    s.push_str("     \"zed: install dev extension\".\n");
    s.push_str(&format!("  3. Pick the folder {folder}\n"));
    s.push_str("  4. Open a .rules file: Zed starts h2 rules lsp (`h2` on PATH, else\n");
    s.push_str("     ~/.cargo/bin/h2). A HaloScript working copy (.hsc) starts h2 hs lsp.\n");
    s
}

/// What `status` prints: each editor and its extension, and the language
/// server's start.
pub(crate) fn status_text(env: &Env, code_listing: Option<&str>, lsp: &Result<String, String>) -> String {
    let mut s = String::new();
    match (&env.code, code_listing.and_then(extension_version)) {
        (None, _) => s.push_str("VS Code: not found\n"),
        (Some(code), Some(v)) => s.push_str(&format!("VS Code: {VSCODE_EXTENSION} {v} ({})\n", code.display())),
        (Some(code), None) => s.push_str(&format!(
            "VS Code: no extension ({}); run h2 rules editors install\n",
            code.display()
        )),
    }
    match (&env.zed, env.zed_extension) {
        (_, true) => s.push_str(&format!(
            "Zed: {ZED_EXTENSION} installed ({})\n",
            env.zed_installed_dir
                .as_ref()
                .map(|d| d.join(ZED_EXTENSION).display().to_string())
                .unwrap_or_default()
        )),
        (Some(_), false) => s.push_str("Zed: no extension; run h2 rules editors install zed\n"),
        (None, false) => s.push_str("Zed: not found\n"),
    }
    s.push_str(&server_line("h2 rules lsp", lsp));
    s
}

/// Whether a language server started: its name, or why not.
pub(crate) fn server_line(command: &str, started: &Result<String, String>) -> String {
    match started {
        Ok(name) => format!("{command}: starts ({name})\n"),
        Err(why) => format!("{command}: does not start: {why}\n"),
    }
}

/// A JSON-RPC message in LSP's framing.
pub(crate) fn frame(msg: &Value) -> Vec<u8> {
    let body = msg.to_string();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

/// Reads one framed message.
pub(crate) fn read_frame(r: &mut impl BufRead) -> Result<Value> {
    let mut len = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            bail!("the server closed its output");
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            len = Some(value.trim().parse::<usize>().context("a bad Content-Length")?);
        }
    }
    let len = len.context("a message with no Content-Length")?;
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}

/// The server's name and version from its `initialize` answer.
pub(crate) fn server_name(answer: &Value) -> Result<String, String> {
    if let Some(e) = answer.get("error") {
        return Err(format!("initialize failed: {e}"));
    }
    let result = answer.get("result").ok_or("no result to initialize")?;
    if result.get("capabilities").is_none() {
        return Err("initialize gave no capabilities".to_string());
    }
    let info = &result["serverInfo"];
    Ok(format!(
        "{} {}",
        info["name"].as_str().unwrap_or("a server"),
        info["version"].as_str().unwrap_or("")
    )
    .trim_end()
    .to_string())
}

/// Starts `exe <tool> lsp` (`rules` or `hs`), asks `initialize`, and
/// shuts it down.
fn lsp_starts(exe: &Path, tool: &str) -> Result<String, String> {
    let mut child = Command::new(exe)
        .args([tool, "lsp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"processId": null, "rootUri": null, "capabilities": {}}});
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut r = BufReader::new(stdout);
        let _ = tx.send(read_frame(&mut r).map_err(|e| e.to_string()));
    });
    let answer = stdin
        .write_all(&frame(&init))
        .map_err(|e| e.to_string())
        .and_then(|()| {
            rx.recv_timeout(LSP_TIMEOUT)
                .map_err(|_| format!("no answer to initialize in {}s", LSP_TIMEOUT.as_secs()))?
        })
        .and_then(|v| server_name(&v));
    if answer.is_ok() {
        let initialized = json!({"jsonrpc": "2.0", "method": "initialized", "params": {}});
        let shutdown = json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"});
        let exit = json!({"jsonrpc": "2.0", "method": "exit"});
        for m in [initialized, shutdown, exit] {
            let _ = stdin.write_all(&frame(&m));
        }
    }
    drop(stdin);
    std::thread::sleep(Duration::from_millis(200));
    if !matches!(child.try_wait(), Ok(Some(_))) {
        let _ = child.kill();
    }
    let _ = child.wait();
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(code: bool, zed: bool, zed_extension: bool) -> Env {
        Env {
            editors: Some(PathBuf::from("/src/h2recomp/editors")),
            code: code.then(|| PathBuf::from("/usr/local/bin/code")),
            zed: zed.then(|| PathBuf::from("/usr/local/bin/zed")),
            zed_extension,
            zed_installed_dir: Some(PathBuf::from("/home/me/.local/share/zed/extensions/installed")),
        }
    }

    #[test]
    fn editors_are_found_on_path_then_in_the_usual_places() {
        let home = Path::new("/Users/me");
        let at = |want: &'static str| move |p: &Path| p == Path::new(want);
        assert_eq!(
            find_editor("code", Some("/a:/b"), Some(home), true, at("/b/code")),
            Some(PathBuf::from("/b/code"))
        );
        assert_eq!(
            find_editor("zed", None, Some(home), false, at("/opt/homebrew/bin/zed")),
            Some(PathBuf::from("/opt/homebrew/bin/zed"))
        );
        let app = "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code";
        assert_eq!(find_editor("code", Some("/a"), Some(home), true, at(app)), Some(PathBuf::from(app)));
        assert_eq!(find_editor("code", Some("/a"), Some(home), false, at(app)), None, "not macOS");
        let mine = "/Users/me/Applications/Zed.app/Contents/MacOS/cli";
        assert_eq!(find_editor("zed", None, Some(home), true, at(mine)), Some(PathBuf::from(mine)));
        assert_eq!(
            zed_installed_dir(home, true),
            Path::new("/Users/me/Library/Application Support/Zed/extensions/installed")
        );
        assert_eq!(zed_installed_dir(Path::new("/home/me"), false), Path::new("/home/me/.local/share/zed/extensions/installed"));
    }

    #[test]
    fn the_extension_and_the_newest_vsix_are_read_from_listings() {
        let listing = "rust-lang.rust-analyzer@0.3.2\nH2recomp.h2-rules@0.1.2\n";
        assert_eq!(extension_version(listing).as_deref(), Some("0.1.2"));
        assert_eq!(extension_version("rust-lang.rust-analyzer@0.3.2\n"), None);
        let names = ["h2-rules-0.1.9.vsix", "README.md", "h2-rules-0.1.10.vsix", "h2-rules-x.vsix", "other-1.0.0.vsix"];
        assert_eq!(newest_vsix(names), Some("h2-rules-0.1.10.vsix"));
        assert_eq!(newest_vsix(["package.json"]), None);
        assert!(EDITORS_RELEASE.ends_with("/releases/download/editors-latest"));
    }

    #[test]
    fn status_says_what_is_installed_and_whether_the_server_starts() {
        let ok = Ok("h2 rules lsp 0.1.0".to_string());
        let s = status_text(&env(true, true, true), Some("h2recomp.h2-rules@0.1.0\n"), &ok);
        assert_eq!(
            s,
            "VS Code: h2recomp.h2-rules 0.1.0 (/usr/local/bin/code)\n\
             Zed: h2-rules installed (/home/me/.local/share/zed/extensions/installed/h2-rules)\n\
             h2 rules lsp: starts (h2 rules lsp 0.1.0)\n"
        );
        let s = status_text(&env(true, true, false), Some(""), &Err("no answer".into()));
        assert!(s.contains("VS Code: no extension (/usr/local/bin/code); run h2 rules editors install\n"), "{s}");
        assert!(s.contains("Zed: no extension; run h2 rules editors install zed\n"), "{s}");
        assert!(s.ends_with("h2 rules lsp: does not start: no answer\n"), "{s}");
        let s = status_text(&env(false, false, false), None, &ok);
        assert!(s.starts_with("VS Code: not found\nZed: not found\n"), "{s}");
        assert_eq!(server_line("h2 hs lsp", &Ok("h2 hs lsp 0.1.0".into())), "h2 hs lsp: starts (h2 hs lsp 0.1.0)\n");
    }

    #[test]
    fn zed_steps_point_at_the_extension_folder() {
        let s = zed_steps(&env(false, true, false));
        assert!(s.contains("\"zed: install dev extension\""), "{s}");
        assert!(s.contains("Pick the folder /src/h2recomp/editors/zed\n"), "{s}");
        assert!(zed_steps(&env(false, false, false)).starts_with("Zed: not found."));
        assert!(zed_steps(&env(false, true, true)).contains("is installed"));
    }

    #[test]
    fn frames_round_trip_and_initialize_answers_name_the_server() {
        let msg = json!({"jsonrpc": "2.0", "id": 1, "result": {"capabilities": {},
            "serverInfo": {"name": "h2 rules lsp", "version": "0.1.0"}}});
        let bytes = frame(&msg);
        assert!(bytes.starts_with(b"Content-Length: "));
        let mut r = std::io::Cursor::new(bytes);
        let back = read_frame(&mut r).unwrap();
        assert_eq!(back, msg);
        assert_eq!(server_name(&back).unwrap(), "h2 rules lsp 0.1.0");
        assert!(read_frame(&mut r).is_err(), "nothing more");
        let mut bad = std::io::Cursor::new(b"X-Other: 1\r\n\r\n{}".to_vec());
        assert!(read_frame(&mut bad).is_err());
        assert!(server_name(&json!({"error": {"code": -1}})).is_err());
        assert!(server_name(&json!({"result": {}})).is_err());
    }
}
