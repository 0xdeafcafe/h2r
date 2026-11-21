//! Sample guest colours at slice boundaries and own output restoration.

use std::sync::MutexGuard;
use std::time::{Duration, Instant};

use input::lightbar::Lightbars;
use settings::{Setting, Store};

use crate::{pad_lightbar, setting_defs::PAD_LIGHTBAR, xapi_glue};

pub(crate) struct ControllerLights<'a> {
    xapi: &'a xapi_glue::Shared,
    lightbars: Lightbars,
    enabled: Option<Setting<bool>>,
    next_sample: Instant,
}

const SAMPLE_PERIOD: Duration = Duration::from_millis(250);

impl<'a> ControllerLights<'a> {
    pub(crate) fn new(xapi: &'a xapi_glue::Shared, settings: Option<&Store>) -> Self {
        let enabled = settings
            .filter(|s| s.def(PAD_LIGHTBAR.key).is_some())
            .map(|s| s.handle::<bool>(PAD_LIGHTBAR.key));
        let lightbars = lock(xapi).input.lightbars();
        lightbars.update_game(enabled.as_ref().is_none_or(Setting::get), [None; 4]);
        Self {
            xapi,
            lightbars,
            enabled,
            next_sample: Instant::now(),
        }
    }

    pub(crate) fn install(&self, agent: &agent::Agent) {
        agent::lightbar::install(agent, self.lightbars.clone());
    }

    pub(crate) fn sample(&mut self, rt: &runtime::Runtime) {
        let now = Instant::now();
        if now >= self.next_sample {
            self.next_sample = now + SAMPLE_PERIOD;
            let colours = pad_lightbar::colours(&*rt.memory());
            self.lightbars
                .update_game(self.enabled.as_ref().is_none_or(Setting::get), colours);
        }
    }
}

impl Drop for ControllerLights<'_> {
    fn drop(&mut self) {
        lock(self.xapi).input.restore_lightbars();
    }
}

fn lock(xapi: &xapi_glue::Shared) -> MutexGuard<'_, xapi::Xapi<xapi::VfsStorage>> {
    xapi.lock().unwrap_or_else(|e| e.into_inner())
}
