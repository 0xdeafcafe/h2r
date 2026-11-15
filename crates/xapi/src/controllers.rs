use crate::{Error, GuestMemory, Result, Storage, Xapi, memory};
use input::{
    PortIndex,
    device::{FeedbackRequest, Handle},
    gamepad::{Capabilities, Feedback, PollingParameters, State},
};

/// Signal a guest event. Called only from [`SignalBatch::deliver`], never
/// under the guest memory guard; implementations may take that lock.
pub trait Events {
    fn signal(&mut self, guest_handle: u32) -> Result<()>;
}

pub(crate) struct OpenInput {
    device_handle: Handle,
    last_state: State,
}

/// XPP_DEVICE_TYPE state consumed by 0x004098c5 and inspected by 0x00409884.
#[derive(Default)]
pub(crate) struct DeviceChanges {
    current: u32,
    changed: u32,
    previous: u32,
}

#[derive(Default)]
struct ChangeMasks {
    insertions: u32,
    removals: u32,
}

impl DeviceChanges {
    fn reinserted(&self) -> u32 {
        self.changed & self.previous & self.current
    }

    fn consume(&mut self) -> ChangeMasks {
        if self.changed == 0 {
            return ChangeMasks::default();
        }
        let reinserted = self.reinserted();
        let masks = ChangeMasks {
            insertions: (self.current & !self.previous) | reinserted,
            removals: (self.previous & !self.current) | reinserted,
        };
        self.changed = 0;
        self.previous = self.current;
        masks
    }
}

impl<StorageBackend: Storage> Xapi<StorageBackend> {
    pub fn init_devices(
        &mut self,
        memory: &mut impl GuestMemory,
        count: u32,
        prealloc: u32,
    ) -> Result<()> {
        if count > 64 || (count != 0 && prealloc == 0) {
            return Err(Error::INVALID_PARAMETER);
        }
        if count != 0 {
            memory.check(prealloc, count as usize * 8, false)?;
        }
        memory.check(self.config.devices.gamepad, 12, true)?;
        self.remember_states();
        self.input.sync();
        self.remember_states();
        self.publish_device_type(memory)
    }

    fn collect_changes(&mut self) {
        let (insertions, removals) = self.input.device_changes();
        self.changes.current = self.input.connected_mask();
        self.changes.changed |= insertions | removals;
    }

    /// XPeekDevices (0x00409884): observes Prev and reinserted without consuming.
    pub fn peek_devices(
        &mut self,
        memory: &mut impl GuestMemory,
        device_type: u32,
        previous_out: u32,
        reinserted_out: u32,
    ) -> Result<u32> {
        if previous_out != 0 {
            memory.check(previous_out, 4, true)?;
        }
        if reinserted_out != 0 {
            memory.check(reinserted_out, 4, true)?;
        }
        self.collect_changes();
        let (current, previous, reinserted) = if device_type == self.config.devices.gamepad {
            (
                self.changes.current,
                self.changes.previous,
                self.changes.reinserted(),
            )
        } else {
            (0, 0, 0)
        };
        if previous_out != 0 {
            memory.write(previous_out, &previous.to_le_bytes())?;
        }
        if reinserted_out != 0 {
            memory.write(reinserted_out, &reinserted.to_le_bytes())?;
        }
        Ok(current)
    }

    /// Publish Cur/Chg/Prev for generated device readers (0x00409884/0x004098c5).
    pub fn publish_device_type(&mut self, memory: &mut impl GuestMemory) -> Result<()> {
        let address = self.config.devices.gamepad;
        memory.check(address, 12, true)?;
        self.collect_changes();
        let mut bytes = [0; 12];
        bytes[..4].copy_from_slice(&self.changes.current.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.changes.changed.to_le_bytes());
        bytes[8..].copy_from_slice(&self.changes.previous.to_le_bytes());
        memory.write(address, &bytes)
    }

    pub fn get_devices(&self, device_type: u32) -> u32 {
        if device_type == self.config.devices.gamepad {
            self.input.connected_mask()
        } else {
            0
        }
    }

    /// The Cur/Prev/Chg formulas at 0x004098c5, rather than raw host edge masks.
    pub fn get_device_changes(
        &mut self,
        memory: &mut impl GuestMemory,
        device_type: u32,
        insertions_out: u32,
        removals_out: u32,
    ) -> Result<bool> {
        memory.check(self.config.devices.gamepad, 12, true)?;
        memory.check(insertions_out, 4, true)?;
        memory.check(removals_out, 4, true)?;
        self.collect_changes();
        let masks = if device_type == self.config.devices.gamepad {
            self.changes.consume()
        } else {
            ChangeMasks::default()
        };
        memory.write(insertions_out, &masks.insertions.to_le_bytes())?;
        memory.write(removals_out, &masks.removals.to_le_bytes())?;
        self.publish_device_type(memory)?;
        Ok(masks.insertions != 0 || masks.removals != 0)
    }

    pub fn input_open(
        &mut self,
        memory: &impl GuestMemory,
        device_type: u32,
        port: u32,
        slot: u32,
        polling: u32,
    ) -> Result<u32> {
        if device_type != self.config.devices.gamepad || slot != 0 {
            return Err(Error::INVALID_PARAMETER);
        }
        let port = PortIndex::new(port as usize).ok_or(Error::INVALID_PARAMETER)?;
        let parameters = if polling == 0 {
            PollingParameters::default()
        } else {
            let bytes = memory::read::<4>(memory, polling)?;
            if bytes[0] & !3 != 0 || bytes[3] != 0 {
                return Err(Error::INVALID_PARAMETER);
            }
            PollingParameters {
                auto_poll: bytes[0] & 1 != 0,
                interrupt_out: bytes[0] & 2 != 0,
                input_interval: bytes[1],
                output_interval: bytes[2],
            }
        };
        let device_handle = self
            .input
            .open(port)
            .map_err(|error| Error::Win32(error.win32()))?;
        let token = self.reserve_handle()?;
        self.input
            .set_polling_parameters(device_handle, parameters)
            .map_err(|error| Error::Win32(error.win32()))?;
        let last_state = self
            .input
            .peek_state(device_handle)
            .map_err(|error| Error::Win32(error.win32()))?;
        self.handles.insert(
            token,
            OpenInput {
                device_handle,
                last_state,
            },
        );
        Ok(token)
    }

    pub fn input_close(&mut self, handle: u32) -> Result<()> {
        let opened = self.handles.remove(&handle).ok_or(Error::INVALID_HANDLE)?;
        self.input.close(opened.device_handle);
        Ok(())
    }

    fn input_handle(&self, handle: u32) -> Result<Handle> {
        self.handles
            .get(&handle)
            .map(|opened| opened.device_handle)
            .ok_or(Error::INVALID_HANDLE)
    }

    fn remember_states(&mut self) {
        for opened in self.handles.values_mut() {
            // Bookkeeping, not a guest read: must not spend mouse motion.
            if let Ok(state) = self.input.peek_state(opened.device_handle) {
                opened.last_state = state;
            }
        }
    }

    pub fn input_poll(&mut self, handle: u32) -> Result<()> {
        self.input
            .poll(self.input_handle(handle)?)
            .map_err(|error| Error::Win32(error.win32()))?;
        self.remember_states();
        Ok(())
    }

    /// At 0x00409b6c, a disconnected handle still copies its final state before
    /// returning 1167. Halo accepts that status at 0x00124c3b.
    pub fn input_get_state(
        &mut self,
        memory: &mut impl GuestMemory,
        handle: u32,
        out: u32,
    ) -> Result<()> {
        memory.check(out, State::SIZE, true)?;
        let opened = self.handles.get_mut(&handle).ok_or(Error::INVALID_HANDLE)?;
        let result = self.input.get_state(opened.device_handle);
        if let Ok(state) = result {
            opened.last_state = state;
        }
        memory.write(out, &opened.last_state.to_bytes())?;
        result
            .map(|_| ())
            .map_err(|error| Error::Win32(error.win32()))
    }

    pub fn input_get_capabilities(
        &self,
        memory: &mut impl GuestMemory,
        handle: u32,
        out: u32,
    ) -> Result<()> {
        memory.check(out, Capabilities::SIZE, true)?;
        let capabilities = self
            .input
            .capabilities(self.input_handle(handle)?)
            .map_err(|error| Error::Win32(error.win32()))?;
        memory.write(out, &capabilities.to_bytes())
    }

    pub fn input_set_state(
        &mut self,
        memory: &mut impl GuestMemory,
        handle: u32,
        feedback_address: u32,
    ) -> Result<u32> {
        let device_handle = self.input_handle(handle)?;
        let feedback = Feedback::from_bytes(&memory::read(memory, feedback_address)?);
        memory.check(feedback_address, Feedback::SIZE, true)?;
        if self.pending_feedback.len() >= 4096 {
            return Err(Error::Win32(8));
        }
        if self.pending_feedback.contains(&feedback_address) {
            return Err(Error::Win32(170));
        }
        let request = FeedbackRequest {
            addr: feedback_address,
            h_event: feedback.h_event,
            rumble: feedback.rumble,
        };
        // Retail 0x0040a8f2 publishes immediate failures in Header.dwStatus
        // (50 at 0x0040a923, 1167 at 0x0040aa0a) and returns that status.
        // Halo's rumble loop at 0x00124f31 reads the buffer, not EAX.
        let status = match self.input.set_state(device_handle, request) {
            Ok(status) => status,
            Err(error) => {
                let status = error.win32();
                memory.write(feedback_address, &status.to_le_bytes())?;
                return Err(Error::Win32(status));
            }
        };
        memory.write(feedback_address, &status.to_le_bytes())?;
        self.pending_feedback.insert(feedback_address);
        Ok(status)
    }

    /// Write completion statuses under the memory guard. Completions without
    /// an event finish here; the rest wait in the signal queue for
    /// [`Self::take_signals`]. Each completion is attempted once per sync; a
    /// failed write is retained for retry and the first error is returned after
    /// the rest were processed, so no failure blocks later requests.
    pub fn sync_input(&mut self, memory: &mut impl GuestMemory) -> Result<()> {
        memory.check(self.config.devices.gamepad, 12, true)?;
        self.remember_states();
        self.input.sync();
        self.remember_states();
        self.publish_device_type(memory)?;
        self.completions.extend(self.input.take_completions());
        let mut first_error = None;
        for _ in 0..self.completions.len() {
            let completion = self
                .completions
                .pop_front()
                .expect("bounded by queue length");
            match memory.write(completion.addr, &completion.status.to_le_bytes()) {
                Ok(()) if completion.h_event == 0 => {
                    self.pending_feedback.remove(&completion.addr);
                }
                Ok(()) => self.signals.push_back(PendingSignal {
                    feedback_address: completion.addr,
                    guest_event: completion.h_event,
                }),
                Err(error) => {
                    first_error.get_or_insert(error);
                    self.completions.push_back(completion);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Take every completion whose status is written but whose event is not
    /// yet signalled. Deliver the batch with no guest memory or XAPI guard
    /// held: kernel event signalling takes the guest memory lock itself. The
    /// feedback buffers stay pending (reuse returns 170) until acknowledged.
    pub fn take_signals(&mut self) -> SignalBatch {
        SignalBatch(self.signals.drain(..).collect())
    }

    /// Finish delivered completions and requeue failed signals for the next
    /// batch. Returns the first delivery error after recording every result.
    pub fn acknowledge_signals(&mut self, report: SignalReport) -> Result<()> {
        for address in report.delivered {
            self.pending_feedback.remove(&address);
        }
        self.signals.extend(report.failed);
        report.first_error.map_or(Ok(()), Err)
    }
}

/// A completed feedback request whose status is already in guest memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingSignal {
    pub feedback_address: u32,
    pub guest_event: u32,
}

/// Event signals owed to the guest. Holds no guest memory or XAPI borrow, so
/// it can be delivered after both guards are released. Dropping it without
/// passing its report to [`Xapi::acknowledge_signals`] leaves the buffers
/// pending forever.
#[must_use = "deliver the signals and acknowledge the report"]
#[derive(Debug, Default)]
pub struct SignalBatch(Vec<PendingSignal>);

impl SignalBatch {
    pub fn signals(&self) -> &[PendingSignal] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Attempt each signal once. Failures are reported, not retried here.
    pub fn deliver(self, events: &mut impl Events) -> SignalReport {
        let mut report = SignalReport::default();
        for signal in self.0 {
            match events.signal(signal.guest_event) {
                Ok(()) => report.delivered.push(signal.feedback_address),
                Err(error) => {
                    report.first_error.get_or_insert(error);
                    report.failed.push(signal);
                }
            }
        }
        report
    }
}

/// Outcome of [`SignalBatch::deliver`], for [`Xapi::acknowledge_signals`].
#[must_use = "acknowledge the report so finished buffers are released"]
#[derive(Debug, Default)]
pub struct SignalReport {
    delivered: Vec<u32>,
    failed: Vec<PendingSignal>,
    first_error: Option<Error>,
}
