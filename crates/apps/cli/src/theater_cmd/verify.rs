//! Replay verification reuses the CLI child's launch configuration. Each
//! request gets its own initially restored data directory and agent socket.
use super::{Options, Temp};
use agent_proto::theater::{VerificationReport, Verify};
use anyhow::{Context, Result, ensure};
use boot::theater::{Presentation, VerificationMode};
use std::{fs::File, path::Path, process::Command, sync::Arc};

const REQUEST: &str = "verify-request.json";
const REPORT: &str = "verify-report.json";

pub(super) fn mode(opts: &Options, work: &Path) -> Result<VerificationMode> {
    let input = work.join(REQUEST);
    if input.try_exists()? {
        let (request, baseline) = serde_json::from_reader(File::open(input)?)?;
        return Ok(VerificationMode::Worker {
            request,
            baseline,
            report: work.join(REPORT),
        });
    }
    let opts = opts.clone();
    Ok(VerificationMode::Player(Arc::new(move |request| {
        run(&opts, request)
    })))
}

fn run(opts: &Options, request: Verify) -> Result<VerificationReport> {
    if !request.ring {
        return pass(opts, request, None);
    }
    let session = Temp::new()?;
    let baseline = session.0.join("capture-hashes");
    let reference = pass(
        opts,
        Verify {
            ring: false,
            ..request
        },
        Some(&baseline),
    )?;
    if reference.first_divergence.is_some() {
        return Ok(reference);
    }
    pass(opts, request, Some(&baseline))
}

fn pass(opts: &Options, request: Verify, baseline: Option<&Path>) -> Result<VerificationReport> {
    let work = Temp::new()?;
    serde_json::to_writer(File::create(work.0.join(REQUEST))?, &(request, baseline))?;
    serde_json::to_writer(
        File::create(work.0.join("view.json"))?,
        &Presentation::default(),
    )?;
    let socket = work.0.join("agent.sock");
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args(["theater", "play"])
        .arg(&opts.film)
        .arg("--disc")
        .arg(&opts.disc)
        .arg("--out")
        .arg(&opts.out)
        .args(["--render-every", "0"])
        .arg("--socket")
        .arg(&socket)
        .env("H2_AGENT", &socket)
        .env("H2_THEATER_WORK", &work.0)
        .env("H2_THEATER_TARGET", "0")
        .env("H2_THEATER_PAUSED", "0")
        .env("H2_THEATER_SPEED", "1")
        .env("H2_THEATER_RING", if request.ring { "1" } else { "0" })
        .env_remove("H2_THEATER_RANGE");
    if let Some(data) = &opts.data {
        cmd.arg("--data").arg(data);
    }
    if let Some(movies) = &opts.movies {
        cmd.arg("--movies").arg(movies);
    }
    let status = cmd
        .status()
        .context("starting Theater verification player")?;
    let result = work.0.join(REPORT);
    ensure!(
        result.try_exists()?,
        "verification player exited {status} without a report"
    );
    let report: VerificationReport = serde_json::from_reader(File::open(result)?)?;
    ensure!(
        status.success() || report.first_divergence.is_some(),
        "verification player exited {status}"
    );
    Ok(report)
}
