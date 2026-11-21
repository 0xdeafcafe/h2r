//! Host pacing at the audio deadline when every guest thread is blocked.
use game::{dsound_glue, run_opts::BetweenSlices};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Next {
    Retry,
    Deadlock,
    HostStop,
}

pub(super) fn wake(
    audio: &dsound_glue::Audio,
    kernel: &kernel::Kernel,
    runtime: &runtime::Runtime,
    idle: &mut u64,
    host: Option<&mut BetweenSlices>,
) -> Result<Next, String> {
    if !dsound_glue::wake_idle(audio, kernel, runtime, idle)? {
        return Ok(Next::Deadlock);
    }
    // The clock moved even though no guest thread ran. Give the shell its
    // ordinary pacing and stop boundary before retrying at the new time.
    Ok(if host.is_none_or(|run| run(kernel)) {
        Next::Retry
    } else {
        Next::HostStop
    })
}

#[cfg(all(test, feature = "local-io"))]
mod tests {
    use super::*;

    #[test]
    fn each_audio_only_advance_reaches_the_host_before_the_retry() {
        let rt = runtime::Runtime::new(runtime::Guest::new(), runtime::Dispatch::default());
        let kernel = kernel::Kernel::install(&rt, Default::default()).unwrap();
        let audio = dsound_glue::install_without_wma(
            &rt,
            &kernel,
            Box::new(|| Box::new(audio::NullSink::new(48_000, 4096))),
        )
        .unwrap();
        let mut idle = 0;
        let mut calls = 0;
        let mut host: BetweenSlices = Box::new(move |kernel| {
            calls += 1;
            assert_eq!(kernel.clock().counter(), calls * dsound_glue::QUANTUM_TICKS);
            calls < 2
        });
        assert_eq!(
            wake(&audio, &kernel, &rt, &mut idle, Some(&mut host)).unwrap(),
            Next::Deadlock
        );
        let format = audio::WaveFormat::pcm16(1, 48_000).unwrap();
        let mut buffer = audio::SoundBuffer::new(format, 512).unwrap();
        buffer.play(true);
        audio.mixer().lock().unwrap().add_buffer(buffer);
        for expected in [Next::Retry, Next::HostStop] {
            assert_eq!(
                wake(&audio, &kernel, &rt, &mut idle, Some(&mut host)).unwrap(),
                expected
            );
        }
        audio.output.halt();
    }
}
