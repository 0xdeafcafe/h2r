//! Local export jobs live in their own replay processes. Job status
//! outlives the watching player.
mod output;

use super::{Options, Temp, child_with_start};
use agent_proto::theater::{ClipExport, ClipFormat, ClipJob, ClipStatus};
use anyhow::{Context, Result, ensure};
use boot::theater::{ClipCapture, ClipSink, ClipSpan, Launch, Media, Picture, Presentation};
use output::Output;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const SIZE: (u32, u32) = boot::setting_defs::GUEST_FRAME;

#[derive(serde::Serialize, serde::Deserialize)]
struct Request {
    clip: ClipExport,
    presentation: Presentation,
    length: u64,
}

fn jobs() -> PathBuf {
    agent_proto::cache_dir().join("theater-exports")
}

fn job_dir(id: &str) -> Result<PathBuf> {
    ensure!(
        id.len() == 22
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')),
        "invalid export job ID"
    );
    Ok(jobs().join(id))
}

pub(super) fn media(
    options: Options,
    dir: PathBuf,
    frames: app::FrameSink,
    raster: boot::run_opts::NativeSize,
) -> Media {
    // FrameSink is Send but not Sync (its editor mark source is FnMut); the
    // lock lets the shared capture closure read from it.
    let frames = std::sync::Mutex::new(frames);
    Media {
        screenshots: dir.join("screenshots"),
        latest: Box::new(move || {
            let (size, rgba) = frames
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .read_latest_published_rgba()
                .context("no game frame to capture yet")?;
            Ok(Picture { size, rgba })
        }),
        raster,
        start_export: Box::new(move |clip, presentation, length| {
            launch(&options, &dir, clip, presentation, length)
        }),
        export_status: Box::new(status),
    }
}

fn launch(
    opts: &Options,
    dir: &Path,
    clip: ClipExport,
    presentation: Presentation,
    length: u64,
) -> Result<ClipJob> {
    let span = ClipSpan::new(clip.from, clip.to, clip.fps, length)?;
    let job = live_proto::sign::random_id();
    let work = job_dir(&job)?;
    let name = match clip.format {
        ClipFormat::Png => job.clone(),
        ClipFormat::Mp4 => format!("{job}.mp4"),
    };
    let path = std::path::absolute(clip.path.unwrap_or_else(|| dir.join("exports").join(name)))?;
    let request = Request {
        clip: ClipExport {
            path: Some(path.clone()),
            ..clip
        },
        presentation,
        length,
    };
    fs::create_dir_all(jobs())?;
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&work)?;
    let initial = ClipStatus {
        frames_done: 0,
        frames_total: span.frames(),
        path,
        error: None,
        finished: false,
    };
    let launched = (|| -> Result<()> {
        serde_json::to_writer(File::create(work.join("request.json"))?, &request)?;
        write_status(&work, &initial)?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(["theater", "play"])
            .arg(&opts.film)
            .arg("--disc")
            .arg(&opts.disc)
            .arg("--out")
            .arg(&opts.out)
            .env("H2_THEATER_EXPORT_JOB", &work)
            .env("H2_AGENT", "off")
            .env_remove("H2_THEATER_WORK")
            .env_remove("H2_THEATER_RANGE")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(data) = &opts.data {
            command.arg("--data").arg(data);
        }
        if let Some(movies) = &opts.movies {
            command.arg("--movies").arg(movies);
        }
        // This player inherits the execution switches validated at Watch.
        let mut child = command
            .spawn()
            .context("starting the offline clip player")?;
        let work = work.clone();
        std::thread::spawn(move || {
            let exit = child.wait();
            if let Ok(mut status) = read_status(&work)
                && !status.finished
            {
                status.finished = true;
                status.error = Some(match exit {
                    Ok(code) => format!("offline clip player stopped ({code})"),
                    Err(error) => format!("waiting for offline clip player: {error}"),
                });
                let _ = write_status(&work, &status);
            }
        });
        Ok(())
    })();
    if let Err(error) = launched {
        let _ = fs::remove_dir_all(&work);
        return Err(error);
    }
    Ok(ClipJob { job })
}

fn status(job: ClipJob) -> Result<ClipStatus> {
    let work = job_dir(&job.job)?;
    read_status(&work)
}

fn read_status(work: &Path) -> Result<ClipStatus> {
    Ok(serde_json::from_reader(
        File::open(work.join("status.json")).context("no such export job")?,
    )?)
}

fn write_status(work: &Path, status: &ClipStatus) -> Result<()> {
    let temporary = work.join("status.pending");
    serde_json::to_writer(File::create(&temporary)?, status)?;
    fs::rename(&temporary, work.join("status.json"))?;
    Ok(())
}

pub(super) fn run(mut opts: Options, job: &Path) -> Result<()> {
    let initial = read_status(job)?;
    let result = std::panic::catch_unwind(move || -> Result<()> {
        let request: Request = serde_json::from_reader(File::open(job.join("request.json"))?)?;
        let clip = request.clip;
        let span = ClipSpan::new(clip.from, clip.to, clip.fps, request.length)?;
        let status = read_status(job)?;
        let output = Output::open(
            &status.path,
            clip.format,
            clip.fps,
            std::env::var_os("PATH"),
        )?;
        let sink = Box::new(JobSink {
            work: job.to_owned(),
            output: Some(output),
            status,
            updated: Instant::now(),
        });
        let mut blank = vec![0; SIZE.0 as usize * SIZE.1 as usize * 4];
        for pixel in blank.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        let capture = Arc::new(ClipCapture::new(
            span,
            Picture {
                size: SIZE,
                rgba: blank,
            },
            clip.format == ClipFormat::Mp4,
            sink,
        ));
        let start = Launch {
            target: clip.from.saturating_sub(1),
            speed: 1.0,
            paused: false,
            presentation: request.presentation,
        };
        opts.every = 1;
        let temp = Temp::new()?;
        child_with_start(opts, &temp.0, start, Some(capture.clone()))?;
        capture.finish()
    });
    let result = result.unwrap_or_else(|_| Err(anyhow::anyhow!("offline clip player panicked")));
    if let Err(error) = &result {
        let mut status = read_status(job).unwrap_or(initial);
        status.error = Some(format!("{error:#}"));
        status.finished = true;
        write_status(job, &status)?;
    }
    let _ = fs::remove_file(job.join("request.json"));
    result
}

struct JobSink {
    work: PathBuf,
    output: Option<Output>,
    status: ClipStatus,
    updated: Instant,
}

impl ClipSink for JobSink {
    fn frame(&mut self, picture: &Picture) -> Result<()> {
        let output = self.output.as_mut().context("clip already finished")?;
        if picture.size == SIZE {
            output.write_frame(SIZE, &picture.rgba)?;
        } else {
            output.write_frame(SIZE, &fit(picture))?;
        }
        self.status.frames_done += 1;
        if self.updated.elapsed() >= Duration::from_millis(150) {
            write_status(&self.work, &self.status)?;
            self.updated = Instant::now();
        }
        Ok(())
    }

    fn audio(&mut self, pcm: &[f32]) -> Result<()> {
        self.output
            .as_mut()
            .context("clip already finished")?
            .write_audio(pcm)
    }

    fn finish(&mut self) -> Result<()> {
        ensure!(
            self.status.frames_done == self.status.frames_total,
            "clip frame count differs"
        );
        write_status(&self.work, &self.status)?;
        self.output
            .take()
            .context("clip already finished")?
            .finish()?;
        self.status.finished = true;
        write_status(&self.work, &self.status)
    }
}

/// Keep a single clip canvas even if a selected split-screen viewport
/// appears or disappears. Fit its aspect ratio inside the canvas.
fn fit(picture: &Picture) -> Vec<u8> {
    let (w, h) = picture.size;
    let (dw, dh) = SIZE;
    let width = dw.min((dh * w / h).max(1));
    let height = dh.min((dw * h / w).max(1));
    let (left, top) = ((dw - width) / 2, (dh - height) / 2);
    let mut rgba = vec![0; (dw * dh * 4) as usize];
    for pixel in rgba.chunks_exact_mut(4) {
        pixel[3] = 255;
    }
    for y in 0..height {
        for x in 0..width {
            let src = ((y * h / height * w + x * w / width) * 4) as usize;
            let dst = (((y + top) * dw + x + left) * 4) as usize;
            rgba[dst..dst + 4].copy_from_slice(&picture.rgba[src..src + 4]);
        }
    }
    rgba
}

/// The headless player's host: the film's view drawn offscreen, each
/// presented picture handed to the clip.
pub(super) fn host(capture: Arc<ClipCapture>, host_bots: bool) -> Result<boot::Host> {
    let mut frames = app::FrameSink::headless().context("creating the offline clip GPU")?;
    let raster: boot::run_opts::NativeSize = Arc::new(|| SIZE);
    let writer = capture.clone();
    let view = boot::theater::HostView::new(
        boot::render_glue::share_context(frames.context()),
        raster,
        |size| size,
        move |view, size| {
            writer
                .present(|| {
                    frames.submit_view(view, size, true)?;
                    read_picture(&frames)
                })
                .map_err(|error| format!("{error:#}"))
        },
    );
    Ok(boot::Host {
        film_view: Some(view),
        host_bots,
        host_light: true,
        film_capture: Some(capture),
        ..Default::default()
    })
}

fn read_picture(frames: &app::FrameSink) -> Result<Picture> {
    let (size, rgba) = frames
        .read_latest_published_rgba()
        .context("no clip picture was published")?;
    Ok(Picture { size, rgba })
}
