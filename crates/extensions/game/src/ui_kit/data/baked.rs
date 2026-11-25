//! The docs baked into the build: `DESIGN` (`content/ui/design.yaml`),
//! `SCREENS`, each screen's name and its file's text, sorted by name,
//! `RETAIL_SAMPLES`, the retail screens' sample states
//! (`assets/ui/retail-samples/`, `ui_kit::preview::retail_sample`), each
//! sorted by stem. The build script lists the folders (`build.rs`,
//! `ui_docs`), so a new file is baked in without an edit here.

include!(concat!(env!("OUT_DIR"), "/ui_baked.rs"));
