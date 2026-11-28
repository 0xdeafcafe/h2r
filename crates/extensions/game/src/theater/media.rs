use super::{ClipSpan, Picture, Presentation, Session, controls::params};
use agent::Access;
use agent_proto::{RpcError, code, theater as api};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

pub(super) type ScaledScreenshot =
    Box<dyn Fn((u32, u32), u64) -> Result<mpsc::Receiver<Result<Picture>>> + Send + Sync>;

type StartExport =
    Box<dyn Fn(api::ClipExport, Presentation, u64) -> Result<api::ClipJob> + Send + Sync>;
type ExportStatus = Box<dyn Fn(api::ClipJob) -> Result<api::ClipStatus> + Send + Sync>;

pub struct Media {
    pub screenshots: PathBuf,
    pub latest: Box<dyn Fn() -> Result<Picture> + Send + Sync>,
    pub raster: crate::run_opts::NativeSize,
    pub start_export: StartExport,
    pub export_status: ExportStatus,
}

impl Session {
    pub fn attach_media(&self, media: Media, rt: &Arc<runtime::Runtime>) -> Result<()> {
        self.media
            .set((media, Arc::downgrade(rt)))
            .map_err(|_| anyhow::anyhow!("Theater media already attached"))
    }

    /// The paused guest redraw enqueues one temporary full-raster size and
    /// replies with its captured picture after restoring the pause point.
    pub fn attach_scaled_screenshot(
        &self,
        capture: impl Fn((u32, u32), u64) -> Result<mpsc::Receiver<Result<Picture>>>
        + Send
        + Sync
        + 'static,
    ) -> Result<()> {
        self.scaled_screenshot
            .set(Box::new(capture))
            .map_err(|_| anyhow::anyhow!("scaled screenshot renderer already attached"))
    }

    pub(super) fn add_media_methods(self: &Arc<Self>, agent: &agent::Agent) {
        let session = self.clone();
        agent.add_game_method(api::SCREENSHOT, Access::Act, move |value, ctx| {
            let request = params::<api::Screenshot>(value)?;
            let scale = request.scale.unwrap_or(1);
            if !(1..=4).contains(&scale) {
                return Err(RpcError::new(
                    code::INVALID_PARAMS,
                    "screenshot scale must be 1 to 4",
                ));
            }
            let (media, runtime) = session.media.get().ok_or_else(media_unavailable)?;
            let (tick, paused) = {
                let state = session.lock();
                if state.seeking.is_some() || state.restart.is_some() {
                    return Err(RpcError::new(
                        code::UNAVAILABLE,
                        "screenshot needs a settled replay tick",
                    ));
                }
                (state.tick, state.paused)
            };
            let path = request.path.unwrap_or_else(|| {
                media
                    .screenshots
                    .join(format!("theater-{}.png", live_proto::sign::random_id()))
            });
            if scale == 1 {
                let picture = (media.latest)().map_err(unavailable)?;
                return save(picture, &path, tick);
            }

            let size = scaled_size((media.raster)(), scale).map_err(unavailable)?;
            #[cfg(feature = "gpu")]
            if session.view.get().is_some() {
                let rt = runtime.upgrade().ok_or_else(media_unavailable)?;
                let picture = session
                    .capture_scaled_host_view(&rt, size)
                    .map_err(unavailable)?;
                return save(picture, &path, tick);
            }
            #[cfg(not(feature = "gpu"))]
            let _ = runtime;
            if !paused {
                return Err(RpcError::new(
                    code::UNAVAILABLE,
                    "pause the film for a scaled screenshot",
                ));
            }
            let render = session.scaled_screenshot.get().ok_or_else(|| {
                RpcError::new(
                    code::UNAVAILABLE,
                    "scaled screenshots require paused guest redraw",
                )
            })?;
            let reply = render(size, tick).map_err(unavailable)?;
            ctx.defer_at_boundary(Box::new(move |_| match reply.try_recv() {
                Ok(result) => Some(
                    result
                        .map_err(unavailable)
                        .and_then(|picture| save(picture, &path, tick)),
                ),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(RpcError::new(
                    code::UNAVAILABLE,
                    "paused screenshot redraw stopped",
                ))),
            }));
            Ok(Value::Null)
        });

        let session = self.clone();
        agent.add_method(api::CLIP_MARK, Access::Act, move |p| {
            let request = params::<api::ClipMark>(p)?;
            let mut state = session.lock();
            let marks = update_marks(
                state.presentation.clip_marks,
                request,
                state.header.meta.length_ticks,
            )
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
            state.presentation.clip_marks = marks;
            Ok(json!(api::ClipMarks {
                start: marks[0],
                end: marks[1]
            }))
        });

        let session = self.clone();
        agent.add_method(api::CLIP_EXPORT, Access::Act, move |p| {
            let clip = params::<api::ClipExport>(p)?;
            let (media, _) = session.media.get().ok_or_else(media_unavailable)?;
            let (length, presentation) = {
                let state = session.lock();
                let length = state.header.meta.length_ticks;
                ClipSpan::new(clip.from, clip.to, clip.fps, length)
                    .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
                let mut presentation = state.presentation.clone();
                if let super::Mode::Replay(replay) = &state.mode
                    && state.header.version >= 2
                {
                    replay.camera.save(&mut presentation);
                }
                (length, presentation)
            };
            let job = (media.start_export)(clip, presentation, length).map_err(unavailable)?;
            Ok(json!(job))
        });

        let session = self.clone();
        agent.add_method(api::CLIP_STATUS, Access::Read, move |p| {
            let job = params::<api::ClipJob>(p)?;
            let (media, _) = session.media.get().ok_or_else(media_unavailable)?;
            let status = (media.export_status)(job).map_err(unavailable)?;
            Ok(json!(status))
        });
    }
}

fn scaled_size((width, height): (u32, u32), scale: u8) -> Result<(u32, u32)> {
    let scale = u32::from(scale);
    let axis = |size: u32| {
        size.checked_mul(scale)
            .filter(|n| (1..=settings::Resolution::MAX_DIMENSION).contains(n))
            .context("scaled screenshot exceeds the render size limit")
    };
    Ok((axis(width)?, axis(height)?))
}

fn update_marks(
    current: [Option<u64>; 2],
    request: api::ClipMark,
    length: u64,
) -> Result<[Option<u64>; 2]> {
    if request.clear {
        ensure!(
            request.start.is_none() && request.end.is_none(),
            "cannot set and clear clip marks together"
        );
        return Ok([None, None]);
    }
    let [start, end] = [request.start.or(current[0]), request.end.or(current[1])];
    ensure!(
        start.is_none_or(|tick| tick <= length)
            && end
                .is_none_or(|tick| tick <= length.saturating_add(1)
                    && start.is_some_and(|from| from < tick)),
        "invalid clip marks"
    );
    Ok([start, end])
}

fn save(picture: Picture, path: &Path, tick: u64) -> Result<Value, RpcError> {
    let path = std::path::absolute(path).map_err(|e| unavailable(e.into()))?;
    let (width, height) = picture.size;
    let write = || -> Result<()> {
        let parent = path.parent().context("screenshot path needs a file name")?;
        fs::create_dir_all(parent)?;
        let file = File::options().write(true).create_new(true).open(&path)?;
        if let Err(error) =
            crate::render_glue::encode_png(BufWriter::new(file), &picture.rgba, width, height)
        {
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        Ok(())
    };
    write().map_err(unavailable)?;
    Ok(json!(api::ScreenshotSaved {
        path,
        width,
        height,
        tick
    }))
}

fn unavailable(error: anyhow::Error) -> RpcError {
    RpcError::new(code::UNAVAILABLE, format!("{error:#}"))
}

fn media_unavailable() -> RpcError {
    RpcError::new(
        code::UNAVAILABLE,
        "Theater media requires a rendered playback session",
    )
}
