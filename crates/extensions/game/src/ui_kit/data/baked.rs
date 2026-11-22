//! The docs baked into the build: `DESIGN` (`assets/ui/design.yaml`) and
//! `SCREENS`, each screen's name and its file's text, sorted by name.
//! The build script lists `assets/ui/screens/` (`build.rs`, `ui_docs`), so
//! a new file is baked in without an edit here.

include!(concat!(env!("OUT_DIR"), "/ui_baked.rs"));
