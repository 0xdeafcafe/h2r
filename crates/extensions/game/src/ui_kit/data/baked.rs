//! The docs baked into the build: `DESIGN` (`assets/ui/design.yaml`),
//! `SCREENS`, each screen's name and its file's text, sorted by name,
//! `RETAIL_EDITS`, each retail edit file's stem and text
//! (`assets/ui/retail-edits/`, `ui_kit::retail_edits`), and
//! `RETAIL_SAMPLES`, the retail screens' sample states
//! (`assets/ui/retail-samples/`, `ui_kit::preview::retail_sample`), each
//! sorted by stem. The build script lists the folders (`build.rs`,
//! `ui_docs`), so a new file is baked in without an edit here.

include!(concat!(env!("OUT_DIR"), "/ui_baked.rs"));
