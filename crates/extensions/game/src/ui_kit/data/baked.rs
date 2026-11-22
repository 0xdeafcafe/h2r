//! The docs baked into the build: `DESIGN` (`assets/ui/design.yaml`) and
//! `SCREENS`, each screen's name and its file's text, sorted by name.
//! The build script lists `assets/ui/screens/` (`build.rs`, `ui_docs`), so
//! a new file is baked in without an edit here. `RETAIL_SAMPLES` is the
//! same for `assets/ui/retail-samples/`: the retail screens' sample states
//! (`ui_kit::preview::retail_sample`).

include!(concat!(env!("OUT_DIR"), "/ui_baked.rs"));
