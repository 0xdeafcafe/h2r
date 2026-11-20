//! The result of one counted flare draw sequence in a sealed scene. A
//! private draw ignores its report. Only the canonical display draw can
//! claim its sender, even if the same scene is played more than once.

use std::sync::{Mutex, mpsc};

/// The report attached to the end of a flare query. The scene owns this
/// half. Dropping an undispatched scene disconnects its pending read.
#[derive(Debug)]
pub struct FlareReport(Mutex<Option<mpsc::Sender<Progress>>>);

/// The reader retained for the guest visibility slot. A poll touches only
/// this channel; the GPU callback sends the count in guest pixel units.
pub struct FlareResult {
    events: mpsc::Receiver<Progress>,
    wait: Option<SubmissionWait>,
}

type SubmissionWait = Box<dyn FnOnce() -> Result<(), String> + Send>;

enum Progress {
    Submitted(SubmissionWait),
    Count(Result<u64, String>),
}

const DISCARDED: &str = "flare report was discarded before its count arrived";

impl FlareReport {
    pub fn channel() -> (Self, FlareResult) {
        let (tx, rx) = mpsc::channel();
        (
            Self(Mutex::new(Some(tx))),
            FlareResult { events: rx, wait: None },
        )
    }

    /// A later display draw of a scene whose query already ran carries
    /// no obligation. In particular it cannot replace an unread result.
    pub(crate) fn claim(&self) -> Option<FlareWriter> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take().map(FlareWriter)
    }
}

/// Owned by the submission once the canonical renderer accepts the query.
pub(crate) struct FlareWriter(mpsc::Sender<Progress>);

impl FlareWriter {
    #[cfg(feature = "gpu")]
    pub(crate) fn map(
        self,
        report: gfx::occlusion::PixelReport,
        device: wgpu::Device,
        index: wgpu::SubmissionIndex,
    ) {
        let callback = FlareWriter(self.0.clone());
        report.on_ready(move |count| callback.finish(count));
        self.submitted(Box::new(move || {
            device.poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: None })
                .map(|_| ()).map_err(|e| format!("waiting for the flare query: {e}"))
        }));
    }

    /// Records how a deterministic waiter can drive callbacks for this
    /// particular submission, including when no more frames are coming.
    pub(crate) fn submitted(&self, wait: SubmissionWait) {
        let _ = self.0.send(Progress::Submitted(wait));
    }

    pub(crate) fn finish(self, result: Result<u64, String>) {
        let _ = self.0.send(Progress::Count(result));
    }
}

impl FlareResult {
    /// `Ok(None)` is the hardware's incomplete reply. No device access
    /// and no GPU or render-thread wait.
    pub fn try_read(&mut self) -> Result<Option<u64>, String> {
        loop {
            match self.events.try_recv() {
                Ok(Progress::Submitted(wait)) => self.wait = Some(wait),
                Ok(Progress::Count(count)) => return count.map(Some),
                Err(mpsc::TryRecvError::Empty) => return Ok(None),
                Err(mpsc::TryRecvError::Disconnected) => return Err(DISCARDED.into()),
            }
        }
    }

    /// The waiting guest path (snapshots, deterministic runs and
    /// `Device::get_visibility_test_result`'s old tickets).
    pub fn read(mut self) -> Result<u64, String> {
        loop {
            if let Some(count) = self.try_read()? {
                return Ok(count);
            }
            if let Some(wait) = self.wait.take() {
                wait()?;
            }
            match self.events.recv().map_err(|_| DISCARDED)? {
                Progress::Submitted(wait) => self.wait = Some(wait),
                Progress::Count(count) => return count,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_poll_never_waits_and_only_one_playback_can_complete_the_query() {
        let (report, mut guest) = FlareReport::channel();
        assert_eq!(guest.try_read(), Ok(None));
        let canonical = report.claim().unwrap();
        assert!(report.claim().is_none());
        canonical.submitted(Box::new(|| panic!("a poll waited on the GPU")));
        assert_eq!(guest.try_read(), Ok(None));
        canonical.finish(Ok(37));
        assert_eq!(guest.try_read(), Ok(Some(37)));
    }

    #[test]
    fn the_waiting_guest_can_finish_the_submission_without_another_present() {
        let (report, guest) = FlareReport::channel();
        let canonical = report.claim().unwrap();
        let callback = canonical.0.clone();
        canonical.submitted(Box::new(move || {
            callback.send(Progress::Count(Ok(43))).unwrap();
            Ok(())
        }));
        assert_eq!(guest.read(), Ok(43));
    }

    #[test]
    fn discarding_an_owed_query_releases_its_reader_with_a_named_error() {
        let (report, guest) = FlareReport::channel();
        drop(report);
        assert_eq!(guest.read().unwrap_err(), DISCARDED);
        let (report, mut guest) = FlareReport::channel();
        drop(report.claim());
        assert_eq!(guest.try_read().unwrap_err(), DISCARDED);
    }
}
