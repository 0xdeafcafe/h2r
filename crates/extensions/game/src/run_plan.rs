//! What decides a run's hook plan besides its switches: the render hooks
//! this binary was built with, and the telemetry hooks, plain or replaying a
//! film's plan. `registration` builds the plan with them; the D3D glue and
//! the theater ask them which profile a run or a film has.

use std::sync::Arc;

use crate::telemetry;

/// Group availability follows boot's Cargo features. A desktop playback binary
/// can select the smaller registration plan recorded by a dedicated host.
pub fn compiled_render_hooks() -> film::RenderHooks {
    if cfg!(feature = "gpu") {
        film::RenderHooks::Gpu
    } else if cfg!(feature = "desktop-runtime") {
        film::RenderHooks::Desktop
    } else {
        film::RenderHooks::Slim
    }
}

/// Live telemetry's hooks: the plain install, or, with a film recording or
/// playing, the plan its header records.
pub fn register_telemetry(
    features: &mut hooks::Registry,
    config: &telemetry::TelemetryConfig,
    film: Option<&Arc<crate::theater::Session>>,
) -> anyhow::Result<Option<Arc<telemetry::Telemetry>>> {
    let Some(film) = film else {
        return Ok(
            telemetry::install(features, config).unwrap_or_else(|error| {
                eprintln!("live telemetry disabled: {error}");
                None
            }),
        );
    };
    // Guest transfers depend on the installed hook addresses. The recorder
    // and player consume the same header plan even when their sinks differ.
    let hooks = film.telemetry_plan();
    let installed = telemetry::install_film(features, config, film.film_observer(), hooks)
        .map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        installed.is_some() == hooks.enabled(),
        "film telemetry hook plan {hooks:?} could not be installed"
    );
    Ok(installed)
}
