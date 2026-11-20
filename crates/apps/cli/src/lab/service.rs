//! The per-user macOS LaunchAgent for the lab host (docs/runtime/lab.md).

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{Context, Result, bail, ensure};
use serde_json::json;

use super::{host, host::HostConfig, host_config, lan};

const LABEL: &str = "com.0xdeafcafe.h2recomp.lab";
const SERVICE_ABI: &str = "h2 lab LaunchAgent 1";
const SERVE_INSTALLED: &str = "serve-installed";
const OUT_LOG: &str = "logs/host.stdout.log";
const ERR_LOG: &str = "logs/host.stderr.log";
const UPDATE_LOG: &str = "logs/host-update.log";

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .context("HOME is needed for the lab LaunchAgent")
}

fn plist_path() -> Result<PathBuf> {
    Ok(home()?
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

fn stable_binary(cfg: &HostConfig) -> PathBuf {
    cfg.dir("bin").join("h2")
}

fn built_binary(cfg: &HostConfig) -> PathBuf {
    cfg.repo.join("target/release/h2")
}

#[derive(Default)]
struct LaunchOutput {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

trait Launchctl {
    fn run(&self, args: &[&OsStr]) -> Result<LaunchOutput>;
}

struct SystemLaunchctl;
impl Launchctl for SystemLaunchctl {
    fn run(&self, args: &[&OsStr]) -> Result<LaunchOutput> {
        let output = Command::new("/bin/launchctl")
            .args(args)
            .output()
            .context("running launchctl")?;
        Ok(LaunchOutput {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

fn checked(ctl: &impl Launchctl, args: &[&OsStr]) -> Result<LaunchOutput> {
    let output = ctl.run(args)?;
    ensure!(
        output.code == Some(0),
        "launchctl {} failed ({:?}): {}",
        args[0].to_string_lossy(),
        output.code,
        output.stderr.trim()
    );
    Ok(output)
}

struct Job {
    domain: String,
    target: String,
}

impl Job {
    fn new(uid: u32) -> Self {
        let domain = format!("gui/{uid}");
        let target = format!("{domain}/{LABEL}");
        Self { domain, target }
    }

    fn current() -> Self {
        // SAFETY: geteuid has no preconditions.
        Self::new(unsafe { libc::geteuid() })
    }

    fn status(&self, ctl: &impl Launchctl) -> Result<Option<JobStatus>> {
        let output = ctl.run(&["print".as_ref(), self.target.as_ref()])?;
        match output.code {
            Some(0) => Ok(Some(JobStatus::parse(&output.stdout))),
            Some(113) => Ok(None), // launchd's "could not find service"
            _ if output.stderr.contains("Could not find domain")
                || output.stderr.contains("Could not find service") =>
            {
                Ok(None)
            }
            _ => bail!(
                "launchctl print {} failed: {}",
                self.target,
                output.stderr.trim()
            ),
        }
    }

    fn bootstrap(&self, ctl: &impl Launchctl, plist: &Path) -> Result<()> {
        checked(ctl, &["enable".as_ref(), self.target.as_ref()])?;
        checked(
            ctl,
            &[
                "bootstrap".as_ref(),
                self.domain.as_ref(),
                plist.as_os_str(),
            ],
        )?;
        checked(ctl, &["kickstart".as_ref(), self.target.as_ref()])?;
        Ok(())
    }

    fn bootout(&self, ctl: &impl Launchctl) -> Result<()> {
        checked(ctl, &["bootout".as_ref(), self.target.as_ref()])?;
        Ok(())
    }

    fn kickstart(&self, ctl: &impl Launchctl) -> Result<()> {
        checked(
            ctl,
            &["kickstart".as_ref(), "-k".as_ref(), self.target.as_ref()],
        )?;
        Ok(())
    }
}

struct JobStatus {
    pid: Option<u32>,
}

impl JobStatus {
    fn parse(output: &str) -> Self {
        let pid = output.lines().find_map(|line| {
            line.trim()
                .strip_prefix("pid = ")
                .and_then(|pid| pid.parse::<u32>().ok())
                .filter(|pid| *pid > 0)
        });
        Self { pid }
    }
}

pub(super) fn command(op: &str, args: &[String]) -> Result<()> {
    match op {
        "pair-code" => {
            println!("{}", lan::new_pairing(&pairing_root(args)?)?);
            return Ok(());
        }
        "unpair" => {
            lan::revoke(&pairing_root(args)?)?;
            println!("Lab pairing revoked.");
            return Ok(());
        }
        "service-abi" => {
            ensure!(args.is_empty(), "unexpected host service arguments");
            println!("{SERVICE_ABI}");
            return Ok(());
        }
        _ => {}
    }
    ensure!(
        cfg!(target_os = "macos"),
        "the lab LaunchAgent requires macOS"
    );

    let plist = plist_path()?;
    let job = Job::current();
    let ctl = SystemLaunchctl;
    if op == "status" {
        no_args(op, args)?;
        return status(&job, &ctl, &plist);
    }

    // A single label is shared even when two installers choose different lab
    // roots. Keep the lock file in place so all installers lock the same inode.
    let dir = plist.parent().context("LaunchAgents directory")?;
    fs::create_dir_all(dir)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join(format!(".{LABEL}.lock")))?;
    lock.lock()
        .context("locking the lab LaunchAgent configuration")?;

    match op {
        "install" => install(&job, &ctl, &plist, host_config(args)?),
        "uninstall" => {
            no_args(op, args)?;
            uninstall(&job, &ctl, &plist)
        }
        "update" => {
            no_args(op, args)?;
            ensure!(
                job.status(&ctl)?.is_some(),
                "the lab LaunchAgent is not loaded"
            );
            let socket = installed_path(&plist, "--socket")?;
            super::client::run(
                "update",
                vec![
                    "--host".into(),
                    "local".into(),
                    "--socket".into(),
                    path_str(&socket)?.into(),
                ],
            )
        }
        _ => bail!("unknown lab host command {op:?}"),
    }
}

fn no_args(op: &str, args: &[String]) -> Result<()> {
    ensure!(args.is_empty(), "usage: h2 lab host {op}");
    Ok(())
}

fn pairing_root(args: &[String]) -> Result<PathBuf> {
    match args {
        [] => {
            if cfg!(target_os = "macos") {
                let plist = plist_path()?;
                if plist.try_exists()? {
                    return installed_path(&plist, "--root");
                }
            }
            Ok(super::cache_dir().join("lab"))
        }
        [flag, path] if flag == "--root" => Ok(PathBuf::from(path)),
        _ => bail!("expected an optional --root DIR"),
    }
}

fn install(job: &Job, ctl: &impl Launchctl, path: &Path, mut cfg: HostConfig) -> Result<()> {
    ensure!(cfg.lan.is_some(), "host install requires a LAN listener");
    if cfg.cargo.is_relative() && cfg.cargo.components().count() > 1 {
        cfg.cargo = std::env::current_dir()?.join(cfg.cargo);
    }
    let args = launch_args(&cfg)?;
    let installed = path.try_exists()?;
    if installed {
        ensure!(
            args == installed_args(path)?,
            "the lab LaunchAgent has different settings; uninstall it before reinstalling"
        );
    }
    let loaded = job.status(ctl)?.is_some();
    ensure!(!loaded || installed, "{LABEL} is loaded without its plist");
    if !loaded {
        let _activity = host::activity_lock(&cfg.root)?;
        match UnixStream::connect(&cfg.socket) {
            Ok(_) => bail!(
                "a host is listening on {}; stop it before installing the LaunchAgent",
                cfg.socket.display()
            ),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) => {}
            Err(e) => return Err(e).with_context(|| format!("checking {}", cfg.socket.display())),
        }
        fs::create_dir_all(cfg.dir("logs"))?;
        if !installed || !stable_binary(&cfg).is_file() {
            install_binary(&std::env::current_exe()?, &stable_binary(&cfg))?;
        }
        let code = lan::new_pairing(&cfg.root)?;
        if !installed {
            let path_env = std::env::var("PATH").context("PATH is needed for lab builds")?;
            let xml = plist(&cfg, &args, &path_env)?;
            replace(
                path,
                0o600,
                |file| Ok(file.write_all(xml.as_bytes())?),
                |_| Ok(()),
            )?;
        }
        if let Err(error) = job.bootstrap(ctl, path) {
            if !installed {
                let cleanup = (|| {
                    if job.status(ctl)?.is_some() {
                        job.bootout(ctl)?;
                    }
                    fs::remove_file(path)?;
                    Ok::<_, anyhow::Error>(())
                })();
                if let Err(cleanup) = cleanup {
                    return Err(error).context(format!(
                        "retaining {} after failed rollback: {cleanup:#}",
                        path.display()
                    ));
                }
            }
            return Err(error);
        }
        println!("{code}");
        return Ok(());
    }
    println!("{}", lan::new_pairing(&cfg.root)?);
    Ok(())
}

fn uninstall(job: &Job, ctl: &impl Launchctl, plist: &Path) -> Result<()> {
    let loaded = job.status(ctl)?.is_some();
    if !plist.try_exists()? {
        ensure!(!loaded, "{LABEL} is loaded without its plist");
        return Ok(());
    }
    if loaded {
        let root = installed_path(plist, "--root")?;
        let _activity = host::activity_lock(&root)?;
        job.bootout(ctl)?;
        fs::remove_file(plist)?;
    } else {
        fs::remove_file(plist)?;
    }
    println!("Lab LaunchAgent uninstalled.");
    Ok(())
}

fn status(job: &Job, ctl: &impl Launchctl, plist: &Path) -> Result<()> {
    let state = job.status(ctl)?;
    let installed = plist.try_exists()?;
    let pid = state.as_ref().and_then(|state| state.pid);
    let live = if let Some(pid) = pid.filter(|_| installed) {
        host::local_status(&installed_path(plist, "--socket")?)
            .ok()
            .filter(|reply| reply["pid"].as_u64() == Some(u64::from(pid)))
    } else {
        None
    };
    let status = json!({
        "installed": installed,
        "loaded": state.is_some(),
        "running": pid.is_some(),
        "pid": pid,
        "version": live.as_ref().and_then(|reply| reply.get("build_version")),
        "build_commit": live.as_ref().and_then(|reply| reply.get("build_commit")),
    });
    println!("{}", serde_json::to_string_pretty(&status)?);
    Ok(())
}

pub(super) fn managed(args: &[String]) -> Result<()> {
    ensure!(
        cfg!(target_os = "macos"),
        "the lab LaunchAgent requires macOS"
    );
    host::serve(managed_config(args)?)
}

fn managed_config(args: &[String]) -> Result<HostConfig> {
    let mut cfg = host_config(args)?;
    cfg.managed = true;
    if !cfg.explicit_h2 {
        cfg.h2 = stable_binary(&cfg);
    }
    Ok(cfg)
}

fn installed_args(path: &Path) -> Result<Vec<String>> {
    let output = Command::new("/usr/bin/plutil")
        .args(["-extract", "ProgramArguments", "json", "-o", "-", "--"])
        .arg(path)
        .output()
        .with_context(|| format!("reading {}", path.display()))?;
    ensure!(
        output.status.success(),
        "reading {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    serde_json::from_slice(&output.stdout).with_context(|| format!("reading {}", path.display()))
}

fn installed_path(plist: &Path, flag: &str) -> Result<PathBuf> {
    let args = installed_args(plist)?;
    let [_, lab, host, entry, rest @ ..] = &args[..] else {
        bail!(
            "incomplete LaunchAgent program arguments in {}",
            plist.display()
        );
    };
    ensure!(
        lab == "lab" && host == "host" && entry == SERVE_INSTALLED,
        "unexpected LaunchAgent program arguments in {}",
        plist.display()
    );
    rest.chunks_exact(2)
        .find(|pair| pair[0] == flag)
        .map(|pair| PathBuf::from(&pair[1]))
        .with_context(|| format!("{flag} missing from {}", plist.display()))
}

fn launch_args(cfg: &HostConfig) -> Result<Vec<String>> {
    let mut args = vec![
        path_str(&stable_binary(cfg))?.to_owned(),
        "lab".into(),
        "host".into(),
        SERVE_INSTALLED.into(),
    ];
    for (flag, path) in [
        ("--root", Some(cfg.root.as_path())),
        ("--socket", Some(cfg.socket.as_path())),
        ("--repo", Some(cfg.repo.as_path())),
        ("--disc", Some(cfg.disc.as_path())),
        ("--h2", cfg.explicit_h2.then_some(cfg.h2.as_path())),
        ("--out", cfg.out.as_deref()),
        ("--data", Some(cfg.data.as_path())),
        ("--cargo", Some(cfg.cargo.as_path())),
    ] {
        if let Some(path) = path {
            args.extend([flag.into(), path_str(path)?.into()]);
        }
    }
    if let Some(lan) = &cfg.lan {
        args.extend(["--listen".into(), lan.addr.to_string()]);
        if let Some(name) = &lan.name {
            args.extend(["--name".into(), name.clone()]);
        }
    }
    Ok(args)
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("{} is not a UTF-8 path", path.display()))
}

fn plist(cfg: &HostConfig, args: &[String], path_env: &str) -> Result<String> {
    let mut xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>{LABEL}</string>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>LimitLoadToSessionType</key><string>Aqua</string>
    <key>ProgramArguments</key>
    <array>
"#
    );
    for arg in args {
        xml.push_str("        ");
        xml_string(&mut xml, arg)?;
        xml.push('\n');
    }
    xml.push_str("    </array>\n");
    for (key, path) in [
        ("WorkingDirectory", cfg.repo.clone()),
        ("StandardOutPath", cfg.dir(OUT_LOG)),
        ("StandardErrorPath", cfg.dir(ERR_LOG)),
    ] {
        xml.push_str(&format!("    <key>{key}</key>"));
        xml_string(&mut xml, path_str(&path)?)?;
        xml.push('\n');
    }
    xml.push_str("    <key>EnvironmentVariables</key>\n    <dict><key>PATH</key>");
    xml_string(&mut xml, path_env)?;
    xml.push_str("</dict>\n</dict>\n</plist>\n");
    Ok(xml)
}

fn xml_string(xml: &mut String, s: &str) -> Result<()> {
    xml.push_str("<string>");
    for c in s.chars() {
        match c {
            '&' => xml.push_str("&amp;"),
            '<' => xml.push_str("&lt;"),
            '>' => xml.push_str("&gt;"),
            '\r' => xml.push_str("&#13;"),
            '\t'
            | '\n'
            | '\u{20}'..='\u{d7ff}'
            | '\u{e000}'..='\u{fffd}'
            | '\u{10000}'..='\u{10ffff}' => xml.push(c),
            _ => bail!("launchd plist value contains an invalid XML character"),
        }
    }
    xml.push_str("</string>");
    Ok(())
}

/// Call with the lab activity lock held.
pub(super) fn stage_update(cfg: &HostConfig) -> Result<()> {
    ensure!(cfg.managed, "updates require an installed lab LaunchAgent");
    fs::create_dir_all(cfg.dir("logs"))?;
    let log_path = cfg.dir(UPDATE_LOG);
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let mut git = Command::new("git");
    git.args(["pull", "--ff-only"])
        .env("GIT_TERMINAL_PROMPT", "0");
    run_update_step(cfg, &mut git, &mut log, "git pull")?;

    let mut cargo = Command::new(&cfg.cargo);
    cargo
        .args(["build", "--release", "-p", "cli", "--bin", "h2"])
        .env("CARGO_TARGET_DIR", cfg.repo.join("target"));
    run_update_step(cfg, &mut cargo, &mut log, "cargo build")?;
    install_binary(&built_binary(cfg), &stable_binary(cfg))
}

fn run_update_step(cfg: &HostConfig, cmd: &mut Command, log: &mut File, what: &str) -> Result<()> {
    writeln!(log, "lab host update: {what} in {}", cfg.repo.display())?;
    let status = cmd
        .current_dir(&cfg.repo)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log.try_clone()?)
        .status()
        .with_context(|| format!("starting {what}"))?;
    ensure!(
        status.success(),
        "{what} failed ({status}); see {}",
        cfg.dir(UPDATE_LOG).display()
    );
    Ok(())
}

pub(super) fn restart() -> Result<()> {
    ensure!(
        cfg!(target_os = "macos"),
        "the lab LaunchAgent requires macOS"
    );
    Job::current().kickstart(&SystemLaunchctl)
}

/// Call with the lab activity lock held. Launchd restarts the host if this
/// finds and installs a newer compatible executable built in the checkout.
pub(super) fn adopt_newer(cfg: &HostConfig) -> Result<bool> {
    let source = built_binary(cfg);
    let dest = stable_binary(cfg);
    let src = match fs::metadata(&source) {
        Ok(src) if src.is_file() => src,
        Ok(_) => return Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    match fs::metadata(&dest) {
        Ok(installed) => {
            // Include ctime for tools that restore an output via a cached hard link.
            if src.modified()? <= installed.modified()?
                && (src.ctime(), src.ctime_nsec()) <= (installed.ctime(), installed.ctime_nsec())
            {
                return Ok(false);
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    install_binary(&source, &dest)?;
    Ok(true)
}

fn install_binary(source: &Path, dest: &Path) -> Result<()> {
    let mut input = File::open(source).with_context(|| format!("opening {}", source.display()))?;
    let before = input.metadata()?;
    ensure!(
        before.is_file() && before.permissions().mode() & 0o111 != 0,
        "{} is not an executable file",
        source.display()
    );
    replace(
        dest,
        0o700,
        |output| {
            io::copy(&mut input, output)?;
            output.set_modified(before.modified()?.max(std::time::SystemTime::now()))?;
            Ok(())
        },
        |temporary| {
            // Check the staged executable before replacing the program
            // launchd needs. An older branch may omit this entry point.
            let result = Command::new(temporary)
                .args(["lab", "host", "service-abi"])
                .output()
                .with_context(|| {
                    format!("checking the lab host executable at {}", source.display())
                })?;
            ensure!(
                result.status.success() && result.stdout == format!("{SERVICE_ABI}\n").as_bytes(),
                "{} does not support this lab LaunchAgent",
                source.display()
            );
            ensure!(
                binary_identity(&before) == binary_identity(&fs::metadata(source)?),
                "{} changed while installing it; retry after the build finishes",
                source.display()
            );
            Ok(())
        },
    )
}

fn binary_identity(metadata: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

fn replace(
    dest: &Path,
    mode: u32,
    write: impl FnOnce(&mut File) -> Result<()>,
    validate: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = dest.parent().context("destination directory")?;
    fs::create_dir_all(dir)?;
    let name = dest.file_name().context("destination file name")?;
    let (mut file, temporary) = loop {
        let mut file_name = OsString::from(".");
        file_name.push(name);
        file_name.push(format!(
            ".{}.{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let temporary = dir.join(file_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)
        {
            Ok(file) => break (file, temporary),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("creating {}", temporary.display())),
        }
    };
    let result = (|| {
        write(&mut file)?;
        file.sync_all()?;
        // An open writable descriptor can prevent executing the staged file.
        drop(file);
        validate(&temporary)?;
        fs::rename(&temporary, dest).with_context(|| format!("installing {}", dest.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::time::Duration;

    use super::*;

    #[test]
    fn plist_pins_the_gui_session_binary_arguments_and_logs() {
        let cfg = HostConfig {
            root: "/lab & jobs".into(),
            socket: "/run/lab.sock".into(),
            repo: "/src/<checkout>".into(),
            disc: "/disc/halo 2.iso".into(),
            h2: "/opt/current h2".into(),
            out: Some("/game/out".into()),
            data: "/game/saves".into(),
            cargo: "/opt/tool chain/cargo".into(),
            grace: Duration::from_secs(5),
            lan: Some(lan::Listen {
                addr: "0.0.0.0:29422".parse().unwrap(),
                name: Some("A & B".into()),
            }),
            managed: false,
            explicit_h2: true,
        };
        let args = launch_args(&cfg).unwrap();
        assert_eq!(
            args,
            [
                "/lab & jobs/bin/h2",
                "lab",
                "host",
                "serve-installed",
                "--root",
                "/lab & jobs",
                "--socket",
                "/run/lab.sock",
                "--repo",
                "/src/<checkout>",
                "--disc",
                "/disc/halo 2.iso",
                "--h2",
                "/opt/current h2",
                "--out",
                "/game/out",
                "--data",
                "/game/saves",
                "--cargo",
                "/opt/tool chain/cargo",
                "--listen",
                "0.0.0.0:29422",
                "--name",
                "A & B",
            ]
        );
        let xml = plist(&cfg, &args, "/usr/bin:/opt/a&b").unwrap();
        for field in [
            "<key>Label</key><string>com.0xdeafcafe.h2recomp.lab</string>",
            "<key>RunAtLoad</key><true/>",
            "<key>KeepAlive</key><true/>",
            "<key>LimitLoadToSessionType</key><string>Aqua</string>",
            "<string>/lab &amp; jobs/bin/h2</string>",
            "<string>/src/&lt;checkout&gt;</string>",
            "<string>A &amp; B</string>",
            "<key>StandardOutPath</key><string>/lab &amp; jobs/logs/host.stdout.log</string>",
            "<key>StandardErrorPath</key><string>/lab &amp; jobs/logs/host.stderr.log</string>",
            "<key>PATH</key><string>/usr/bin:/opt/a&amp;b</string>",
        ] {
            assert!(xml.contains(field), "{field} missing from {xml}");
        }
    }

    struct FakeLaunchctl {
        calls: RefCell<Vec<Vec<OsString>>>,
        outputs: RefCell<VecDeque<LaunchOutput>>,
    }

    impl Launchctl for FakeLaunchctl {
        fn run(&self, args: &[&OsStr]) -> Result<LaunchOutput> {
            self.calls
                .borrow_mut()
                .push(args.iter().map(|arg| arg.to_os_string()).collect());
            Ok(self
                .outputs
                .borrow_mut()
                .pop_front()
                .unwrap_or(LaunchOutput {
                    code: Some(0),
                    ..LaunchOutput::default()
                }))
        }
    }

    #[test]
    fn launchctl_targets_the_users_gui_job() {
        let job = Job::new(501);
        let ctl = FakeLaunchctl {
            calls: RefCell::default(),
            outputs: RefCell::new(VecDeque::from([
                LaunchOutput {
                    code: Some(113),
                    ..LaunchOutput::default()
                },
                LaunchOutput {
                    code: Some(0),
                    stdout: "gui/501/example = {\n  pid = 1234\n}".into(),
                    ..LaunchOutput::default()
                },
            ])),
        };
        assert!(job.status(&ctl).unwrap().is_none());
        assert_eq!(job.status(&ctl).unwrap().unwrap().pid, Some(1234));
        job.bootstrap(&ctl, Path::new("/my launch agent.plist"))
            .unwrap();
        job.kickstart(&ctl).unwrap();
        job.bootout(&ctl).unwrap();
        let target = "gui/501/com.0xdeafcafe.h2recomp.lab";
        let want = [
            vec!["print", target],
            vec!["print", target],
            vec!["enable", target],
            vec!["bootstrap", "gui/501", "/my launch agent.plist"],
            vec!["kickstart", target],
            vec!["kickstart", "-k", target],
            vec!["bootout", target],
        ];
        assert_eq!(
            &*ctl.calls.borrow(),
            &want.map(|args| args.into_iter().map(OsString::from).collect::<Vec<_>>())
        );
    }
}
