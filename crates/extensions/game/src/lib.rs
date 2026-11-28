//! Our additions to the game: menus and settings screens, skulls, variants,
//! the campaign picker, gameplay hooks and the glue to the platform crates.
//!
//! `boot` is the composition root above this crate and re-exports it.
#![recursion_limit = "256"]

// Build and load: `crate::fallback`, `crate::tier`, `stamp_path` and the
// rest are `recomp`'s.
pub use recomp::*;
pub use live_telemetry as telemetry;
pub use live_hle::{HostPublisher, LiveConfig};
pub use run_opts::{BetweenSlices, Host, RunOpts, RunReport, VfsReady};

/// Prints `line` to stderr the first time this process says it. Install
/// notes repeat on every feature-plan build; this keeps them to one line.
pub fn say_once(line: String) {
    static SAID: std::sync::Mutex<Option<std::collections::HashSet<String>>> =
        std::sync::Mutex::new(None);
    let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
    if said.get_or_insert_with(Default::default).insert(line.clone()) {
        eprintln!("{line}");
    }
}

pub mod account_screen;
pub mod actor_net;
pub mod agent_frames;
mod agent_spawn_view;
pub mod spectator_view;
pub mod tag_arena;
pub mod agent_glue;
pub mod forge_controls;
pub mod forge_glue;
pub mod forge_hud;
pub mod forge_readout;
pub mod forge_screens;
pub mod forge_place_at_start;
pub mod forge_record;
pub mod forge_spare_clips;
pub mod forge_spawn_weight;
mod forge_map;
mod agent_route;
pub mod aim_assist;
pub mod attract;
mod audio_loop_agent;
mod audio_loops;
pub mod campaign_cache;
pub mod campaign_cards;
pub mod campaign_info;
pub mod campaign_menu;
pub mod campaigns;
pub mod cheats;
pub mod community_variants;
// Game variants as bundles: the store, the list, porting (docs/game/rules-engine.md).
pub mod game_variants;
pub mod controls_text;
pub mod coop_jip;
pub mod coop_latency;
pub mod coop_live;
pub mod coop_look;
pub mod coop_players;
pub mod coop_probe;
pub mod custom_games;
pub mod d3d_diag;
pub mod d3d_glue;
pub mod debug_menu_glue;
pub mod debug_readout;
pub mod debug_views;
pub mod dsound_glue;
mod editor_glue;
pub mod firefight;
// Rules scripts on the authoritative machine (docs/game/rules-engine.md).
#[cfg(feature = "rules")]
pub mod rules;
pub mod fov_row;
pub mod render_counters;
pub mod render_rng;
pub mod render_scratch;
// The flare state held by the render between frames (docs/engine/sim-render-split.md, slice 6).
pub mod flare_store;
#[cfg(feature = "gpu")]
pub mod render_aspect;
/// Without a window (the headless host) the UI lays out at the guest's
/// own 640x480.
#[cfg(not(feature = "gpu"))]
pub mod render_aspect {
    pub fn layout_output() -> (u32, u32) {
        (640, 480)
    }
}
pub mod game_code;
pub mod game_load;
pub mod game_menu;
pub mod game_setup_help;
pub mod h1_recordings;
pub mod h1_conversation;
pub mod h1_options;
pub mod h1_options_row;
pub mod h1_particle_physics;
pub mod h1_flag;
pub mod h1_glow;
pub mod h1_lightning;
pub mod h1_sound_promotion;
pub mod h1_cinematics;
pub mod h1_script_builtins;
pub mod h1_squad_storage;
pub mod h1_variants;
pub mod h1_ai_control;
pub mod h1_command_atoms;
pub mod health;
pub mod host_audit;
pub mod host_lobby;
pub mod host_metrics;
pub mod host_mem;
pub mod host_perf;
pub mod hot_glue;
pub mod hud;
#[cfg(feature = "gpu")]
pub mod hud_native;
#[cfg(feature = "gpu")]
pub mod hud_overlay;
pub mod hud_kit;
pub mod hud_kit_world;
pub mod hud_swap;
pub mod h1_hud_text;
pub mod icons;
pub mod join_link;
pub mod replication_admission;
pub mod keyboard_glue;
pub mod lab_takeover;
pub mod layout_glue;
pub mod level_boot;
pub mod lobby_mode;
pub mod lobby_start;
pub mod lobby_tabs;
pub mod map_variant;
pub mod list_class;
#[cfg(feature = "gpu")]
pub mod lod_distance;
mod lobby_quality;
mod lobby_status;
pub mod live_glue;
pub mod live_hopper;
pub mod live_notice;
pub mod lobby_options;
mod menu_music;
pub mod loaded_map;
pub mod lockstep_glue;
pub mod map_bases;
pub mod map_copy;
pub mod map_override;
pub mod map_modes;
pub mod map_select;
pub mod map_areas;
pub mod map_slots;
pub mod medal_announcer;
pub mod mod_slots;
pub mod monitor_model;
pub mod mods_screen;
pub mod movies_glue;
#[cfg(all(feature = "gpu", feature = "movie-playback"))]
pub mod movie_display_glue;
#[cfg(feature = "gpu")]
pub mod native_decal_decorators_glue;
#[cfg(feature = "gpu")]
pub mod native_decals_glue;
#[cfg(feature = "gpu")]
pub mod native_weather_glue;
#[cfg(feature = "gpu")]
pub mod native_decorators_glue;
#[cfg(feature = "gpu")]
pub mod native_sky_glue;
#[cfg(feature = "gpu")]
pub mod native_first_person_glue;
#[cfg(feature = "gpu")]
pub mod native_first_person_load;
#[cfg(feature = "gpu")]
pub mod native_fog_glue;
#[cfg(feature = "gpu")]
pub mod native_gels_glue;
#[cfg(feature = "gpu")]
pub mod native_gels_scene;
#[cfg(feature = "gpu")]
pub mod native_layers_glue;
#[cfg(feature = "gpu")]
pub mod native_gels_replace;
#[cfg(feature = "gpu")]
pub mod native_hud_glue;
#[cfg(feature = "gpu")]
pub mod modal_blur_glue;
#[cfg(feature = "gpu")]
pub mod native_cinematic_glue;
#[cfg(feature = "gpu")]
pub mod native_lights_glue;
#[cfg(feature = "gpu")]
pub mod native_map;
#[cfg(feature = "gpu")]
pub mod native_shadow_chain;
#[cfg(feature = "gpu")]
pub mod native_shadows_draw;
#[cfg(feature = "gpu")]
pub mod native_shadows_scope;
#[cfg(feature = "gpu")]
pub mod native_shadows_glue;
#[cfg(feature = "gpu")]
pub mod native_objects_glue;
#[cfg(feature = "gpu")]
pub mod native_particles_glue;
#[cfg(feature = "gpu")]
pub mod native_particles_admission;
#[cfg(feature = "gpu")]
pub mod native_particle_systems;
#[cfg(feature = "gpu")]
pub mod native_post_glue;
#[cfg(feature = "gpu")]
pub mod native_post_load;
#[cfg(feature = "gpu")]
mod native_refusals;
#[cfg(feature = "gpu")]
pub mod native_residual_glue;
#[cfg(feature = "gpu")]
pub mod native_sky_cut_glue;
#[cfg(feature = "gpu")]
pub mod native_targets_glue;
#[cfg(feature = "gpu")]
pub mod native_flare_draw_glue;
#[cfg(feature = "gpu")]
pub mod native_fog_quad_glue;
#[cfg(feature = "gpu")]
pub mod native_fog_fans_glue;
#[cfg(feature = "gpu")]
pub mod native_flare_query_glue;
#[cfg(feature = "gpu")]
pub mod native_transparents_glue;
#[cfg(feature = "gpu")]
pub mod native_ocean_glue;
#[cfg(feature = "gpu")]
pub mod native_water_glue;
#[cfg(feature = "gpu")]
pub mod native_world_glue;
#[cfg(feature = "gpu")]
pub(crate) mod relight;
#[cfg(feature = "gpu")]
pub mod native_world_layers_glue;
#[cfg(feature = "gpu")]
pub mod opaque_streams;
#[cfg(feature = "gpu")]
pub mod frame_cutover;
#[cfg(feature = "gpu")]
pub mod frame_owner;
#[cfg(feature = "gpu")]
pub mod frame_parity;
#[cfg(feature = "gpu")]
pub mod scene_glue;
pub mod new_maps;
#[cfg(feature = "test-maps")]
pub mod test_maps;
pub mod pad_lightbar;
pub mod pad_speaker;
pub mod data_salts;
pub mod precision;
#[cfg(feature = "desktop-runtime")]
pub mod render_probe;
pub mod retail_rows;
#[cfg(feature = "gpu")]
pub mod return_to_menu;
pub mod run_plan;
pub mod sampling;
pub mod bot_accuracy;
pub mod bot_glue;
pub mod bot_mark;
pub mod bots_nav_auto;
pub mod clanker_card;
pub mod clankermatch;
pub mod clankers;
pub mod pause_change_map;
pub mod pause_menu;
pub mod player_bot;
pub mod player_goto;
pub mod setting_defs;
pub mod sha;
pub mod signin_keyboard;
pub mod signin_screen;
pub mod sim_bots;
pub mod sim_bounces;
pub mod sim_census;
pub mod sim_drops;
pub mod sim_glue;
pub mod sim_rounds;
pub mod sim_rules;
mod sim_pages;
pub mod skull_glyphs;
pub mod skulls;
pub mod skulls_custom;
pub mod skulls_menu;
pub mod skulls_model;
pub mod skulls_mp;
pub mod skulls_mp_row;
pub mod skulls_screen;
pub mod snapshot_glue;
pub mod snapshot_probe;
pub mod speaker_row;
pub mod speaker_setting;
#[cfg(feature = "local-io")]
pub mod steam_speaker;
pub mod stopgaps;
pub mod pogo;
pub mod pogo_agent;
pub mod pogo_camera;
pub mod pogo_loadout;
pub mod pogo_mode;
pub mod pogo_pose;
pub mod pogo_pose_glue;
pub mod pogo_ride;
pub mod pogo_sound;
#[cfg(feature = "gpu")]
pub mod sound_feed;
// `audio.meter`: the host mixer's output level (docs/audio/audio.md).
pub mod mixer_meter;
pub mod pogo_skeleton;
pub mod superbounce_assist;
pub mod superbounce_capsule;
pub mod superbounce_chain;
pub mod superbounce_charge;
pub mod superbounce_font;
pub mod superbounce_flight;
pub mod superbounce_ghost;
pub mod superbounce_guide;
pub mod objective_hud;
pub mod superbounce_hud;
pub mod superbounce_options;
pub mod superbounce_prompts;
pub mod superbounce_readout;
pub mod superbounce_reload;
pub mod superbounce_seams;
pub mod superbounce_second;
pub mod superbounce_targets;
pub mod superbounce_world;
pub mod superbounce_mode;
pub mod superbounce_verify;
pub mod superbounce_view;
pub mod teabag;
pub mod theater;
pub mod theater_ui;
pub mod touch_glue;
pub mod trace;
pub mod tweakers;
pub mod ui;
mod ui_audit;
pub mod ui_back;
pub mod ui_kit;
mod ui_press;
pub mod ui_stack;
mod ui_transition;
pub mod ui_watch;
pub mod variant_ext;
pub mod variant_options;
pub mod variant_page;
pub mod variant_quick;
pub mod variant_weapons;
pub mod video_screen;
#[cfg(feature = "local-io")]
pub mod voice_glue;
pub mod voice_settings;
pub mod world_lines;
pub mod xapi_glue;
#[cfg(test)]
pub mod string_ids;
pub mod run_opts;

#[path = "actor_net/link.rs"]
pub mod actor_net_link;
