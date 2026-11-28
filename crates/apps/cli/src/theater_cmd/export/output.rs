use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};

use agent_proto::theater::ClipFormat;
use anyhow::{Context, Result, ensure};

use crate::movies_cmd::{self, Ffmpeg, VideoEncoder};

struct Scratch(PathBuf);

impl Scratch {
    fn new(parent: &Path) -> Result<Self> {
        let path = parent.join(format!(".h2-export-{}", live_proto::sign::random_id()));
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut dir = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            dir.mode(0o700);
        }
        dir.create(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) struct Output {
    mp4: Option<Mp4>,
    path: PathBuf,
    fps: u32,
    size: Option<(u32, u32)>,
    frames: u64,
    scratch: Scratch,
}

impl Output {
    pub(super) fn open(
        path: &Path,
        format: ClipFormat,
        fps: u32,
        path_env: Option<OsString>,
    ) -> Result<Self> {
        ensure!(matches!(fps, 30 | 60), "clip fps must be 30 or 60");
        let ffmpeg = match format {
            ClipFormat::Png => None,
            ClipFormat::Mp4 => {
                let ffmpeg = movies_cmd::discover_encoding(path_env).map_err(|error| {
                    let message = format!(
                        "{error} (h2 movies exit {}). Export as a PNG sequence or install an ffmpeg with H.264 and AAC encoders.",
                        error.exit_code()
                    );
                    anyhow::Error::new(error).context(message)
                })?;
                Some(ffmpeg)
            }
        };
        ensure!(path.file_name().is_some(), "export needs an output name");
        let path = std::path::absolute(path)?;
        match fs::symlink_metadata(&path) {
            Ok(_) => anyhow::bail!("export output already exists: {}", path.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let parent = path
            .parent()
            .context("export output needs a parent directory")?;
        fs::create_dir_all(parent)?;
        let scratch = Scratch::new(parent)?;
        let mp4 = ffmpeg
            .map(|ffmpeg| Mp4::new(ffmpeg, &scratch.0))
            .transpose()?;
        Ok(Self {
            mp4,
            path,
            fps,
            size: None,
            frames: 0,
            scratch,
        })
    }

    pub(super) fn write_frame(&mut self, size: (u32, u32), rgba: &[u8]) -> Result<()> {
        let (width, height) = size;
        let len = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4));
        ensure!(
            width > 0 && height > 0 && len == Some(rgba.len()),
            "invalid RGBA export frame"
        );
        ensure!(
            self.size.is_none_or(|first| first == size),
            "export frame dimensions changed"
        );
        self.size = Some(size);
        if let Some(mp4) = &mut self.mp4 {
            mp4.write_frame(size, self.fps, rgba, &self.scratch.0)?;
        } else {
            let path = self.scratch.0.join(frame_name(self.frames));
            boot::render_glue::write_png(&path, rgba, width, height)?;
        }
        self.frames += 1;
        Ok(())
    }

    pub(super) fn write_audio(&mut self, pcm: &[f32]) -> Result<()> {
        if let Some(mp4) = &mut self.mp4 {
            ensure!(pcm.len().is_multiple_of(2), "export audio must be stereo");
            for sample in pcm {
                mp4.audio.write_all(&sample.to_le_bytes())?;
            }
            mp4.has_audio |= !pcm.is_empty();
        }
        Ok(())
    }

    pub(super) fn finish(self) -> Result<()> {
        ensure!(self.frames != 0, "clip produced no frames");
        let Self {
            mp4,
            path,
            frames,
            scratch,
            ..
        } = self;
        if let Some(mp4) = mp4 {
            let file = mp4.finish(&scratch.0)?;
            ensure!(file.metadata()?.len() != 0, "ffmpeg produced an empty file");
            fs::hard_link(file, &path).with_context(|| format!("publishing {}", path.display()))?;
        } else {
            publish_pngs(&scratch.0, &path, frames)?;
        }
        Ok(())
    }
}

fn frame_name(index: u64) -> String {
    format!("frame-{index:06}.png")
}

fn publish_pngs(source: &Path, path: &Path, frames: u64) -> Result<()> {
    fs::create_dir(path).with_context(|| format!("creating {}", path.display()))?;
    for index in 0..frames {
        let name = frame_name(index);
        if let Err(error) = fs::hard_link(source.join(&name), path.join(name)) {
            for written in 0..index {
                let _ = fs::remove_file(path.join(frame_name(written)));
            }
            let _ = fs::remove_dir(path);
            return Err(error).with_context(|| format!("publishing {}", path.display()));
        }
    }
    Ok(())
}

struct Mp4 {
    ffmpeg: Ffmpeg,
    encoder: Option<Encoder>,
    audio: BufWriter<File>,
    has_audio: bool,
}

impl Mp4 {
    fn new(ffmpeg: Ffmpeg, dir: &Path) -> Result<Self> {
        Ok(Self {
            ffmpeg,
            encoder: None,
            audio: BufWriter::new(File::create(dir.join("audio.f32"))?),
            has_audio: false,
        })
    }

    fn write_frame(&mut self, size: (u32, u32), fps: u32, rgba: &[u8], dir: &Path) -> Result<()> {
        if self.encoder.is_none() {
            let args = video_args(self.ffmpeg.video, size, fps, &dir.join("video.mp4"));
            let log = dir.join("ffmpeg.log");
            let child = command(&self.ffmpeg, args, &log)?
                .stdin(Stdio::piped())
                .spawn()
                .with_context(|| format!("starting {}", self.ffmpeg.path.display()))?;
            self.encoder = Some(Encoder { child, log });
        }
        let encoder = self.encoder.as_mut().context("starting the clip encoder")?;
        let result = encoder
            .child
            .stdin
            .as_mut()
            .context("opening the clip encoder input")?
            .write_all(rgba);
        if let Err(error) = result {
            // Closing the pipe and waiting preserves ffmpeg's diagnostic when
            // the write failed because the encoder exited early.
            encoder.wait()?;
            return Err(error).context("writing an RGBA frame to ffmpeg");
        }
        Ok(())
    }

    fn finish(self, dir: &Path) -> Result<PathBuf> {
        let Self {
            ffmpeg,
            encoder,
            mut audio,
            has_audio,
        } = self;
        encoder.context("clip encoder received no frames")?.wait()?;
        audio.flush()?;
        drop(audio);
        let video = dir.join("video.mp4");
        if !has_audio {
            return Ok(video);
        }
        let log = dir.join("mux.log");
        let output = dir.join("clip.mp4");
        let args = mux_args(&video, &dir.join("audio.f32"), &output);
        let status = command(&ffmpeg, args, &log)?
            .stdin(Stdio::null())
            .status()
            .with_context(|| format!("starting {}", ffmpeg.path.display()))?;
        check_status(status, &log)?;
        Ok(output)
    }
}

const COMMON_ARGS: &[&str] = &["-hide_banner", "-loglevel", "error", "-nostdin", "-n"];
const MP4_ARGS: &[&str] = &["-movflags", "+faststart", "-f", "mp4"];

fn video_args(
    encoder: VideoEncoder,
    (width, height): (u32, u32),
    fps: u32,
    out: &Path,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = COMMON_ARGS.iter().map(OsString::from).collect();
    args.extend(["-f", "rawvideo", "-pixel_format", "rgba", "-video_size"].map(OsString::from));
    args.push(format!("{width}x{height}").into());
    args.push("-framerate".into());
    args.push(fps.to_string().into());
    args.extend(["-i", "pipe:0", "-map", "0:v:0", "-an"].map(OsString::from));
    args.extend(encoder.args().iter().map(OsString::from));
    args.extend(
        [
            "-vf",
            "pad=ceil(iw/2)*2:ceil(ih/2)*2",
            "-pix_fmt",
            "yuv420p",
        ]
        .map(OsString::from),
    );
    args.extend(MP4_ARGS.iter().map(OsString::from));
    args.push(out.into());
    args
}

fn mux_args(video: &Path, audio: &Path, out: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = COMMON_ARGS.iter().map(OsString::from).collect();
    args.push("-i".into());
    args.push(video.into());
    args.extend(["-f", "f32le", "-ar", "48000", "-ac", "2", "-i"].map(OsString::from));
    args.push(audio.into());
    args.extend(
        [
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-af",
            "apad",
            "-shortest",
        ]
        .map(OsString::from),
    );
    args.extend(MP4_ARGS.iter().map(OsString::from));
    args.push(out.into());
    args
}

fn command(ffmpeg: &Ffmpeg, args: Vec<OsString>, log: &Path) -> Result<Command> {
    let mut command = Command::new(&ffmpeg.path);
    command
        .args(args)
        .stdout(Stdio::null())
        .stderr(File::create(log)?);
    Ok(command)
}

struct Encoder {
    child: Child,
    log: PathBuf,
}

impl Encoder {
    fn wait(&mut self) -> Result<()> {
        drop(self.child.stdin.take());
        check_status(self.child.wait()?, &self.log)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn check_status(status: ExitStatus, log: &Path) -> Result<()> {
    if status.success() {
        return Ok(());
    }
    let mut tail = Vec::new();
    if let Ok(file) = File::open(log).and_then(|mut file| {
        let offset = file.metadata()?.len().saturating_sub(8192);
        file.seek(SeekFrom::Start(offset))?;
        Ok(file)
    }) {
        let _ = file.take(8192).read_to_end(&mut tail);
    }
    let message = String::from_utf8_lossy(&tail);
    anyhow::bail!("ffmpeg failed ({status}): {}", message.trim_end());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    #[test]
    fn ffmpeg_arguments_are_explicit_for_raw_clip_streams() {
        let video = Path::new("video part.mp4");
        let audio = Path::new("audio part.f32");
        let out = Path::new("a clip.mp4");
        assert_eq!(
            video_args(VideoEncoder::X264, (641, 479), 60, video),
            [
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-n",
                "-f",
                "rawvideo",
                "-pixel_format",
                "rgba",
                "-video_size",
                "641x479",
                "-framerate",
                "60",
                "-i",
                "pipe:0",
                "-map",
                "0:v:0",
                "-an",
                "-c:v",
                "libx264",
                "-preset",
                "slow",
                "-crf",
                "18",
                "-profile:v",
                "high",
                "-vf",
                "pad=ceil(iw/2)*2:ceil(ih/2)*2",
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "+faststart",
                "-f",
                "mp4",
                "video part.mp4",
            ]
            .map(OsString::from)
            .to_vec(),
        );
        assert_eq!(
            mux_args(video, audio, out),
            [
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-n",
                "-i",
                "video part.mp4",
                "-f",
                "f32le",
                "-ar",
                "48000",
                "-ac",
                "2",
                "-i",
                "audio part.f32",
                "-map",
                "0:v:0",
                "-map",
                "1:a:0",
                "-c:v",
                "copy",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
                "-af",
                "apad",
                "-shortest",
                "-movflags",
                "+faststart",
                "-f",
                "mp4",
                "a clip.mp4",
            ]
            .map(OsString::from)
            .to_vec(),
        );
    }

    #[test]
    fn missing_ffmpeg_preserves_the_movies_exit_reason_and_png_output() {
        let temp = Scratch::new(&std::env::temp_dir()).unwrap();
        let mp4 = temp.0.join("no-encoder.mp4");
        let error = Output::open(&mp4, ClipFormat::Mp4, 30, None).err().unwrap();
        let reason = error.downcast_ref::<movies_cmd::DiscoverError>().unwrap();
        assert!(matches!(reason, movies_cmd::DiscoverError::Missing(_)));
        assert_eq!(reason.exit_code(), movies_cmd::EXIT_NO_FFMPEG);
        assert!(error.to_string().contains("Export as a PNG sequence"));
        assert!(!mp4.exists());

        let sequence = temp.0.join("pngs");
        let mut output = Output::open(&sequence, ClipFormat::Png, 30, None).unwrap();
        let first = [255, 0, 0, 255, 0, 255, 0, 255];
        let second = [0, 0, 255, 255, 255, 255, 0, 255];
        output.write_frame((2, 1), &first).unwrap();
        output.write_frame((2, 1), &second).unwrap();
        assert!(!sequence.exists());
        output.finish().unwrap();
        assert_eq!(fs::read_dir(&sequence).unwrap().count(), 2);
        for (index, expected) in [first, second].into_iter().enumerate() {
            let file = File::open(sequence.join(frame_name(index as u64))).unwrap();
            let mut reader = png::Decoder::new(BufReader::new(file)).read_info().unwrap();
            let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
            let info = reader.next_frame(&mut pixels).unwrap();
            assert_eq!((info.width, info.height), (2, 1));
            assert_eq!(&pixels[..info.buffer_size()], &expected);
        }
    }

    #[test]
    fn publish_preserves_a_destination_created_during_export() {
        let temp = Scratch::new(&std::env::temp_dir()).unwrap();
        let path = temp.0.join("frames");
        let mut output = Output::open(&path, ClipFormat::Png, 60, None).unwrap();
        let stage = output.scratch.0.clone();
        output.write_frame((1, 1), &[0, 0, 0, 255]).unwrap();
        fs::create_dir(&path).unwrap();
        let original = path.join(frame_name(0));
        fs::write(&original, b"keep this file").unwrap();
        assert!(output.finish().is_err());
        assert_eq!(fs::read(original).unwrap(), b"keep this file");
        assert!(!stage.exists());
    }
}
