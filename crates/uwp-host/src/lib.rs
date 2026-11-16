//! The Xbox app (Dev Mode, a UWP app; docs/runtime/xbox.md).
//!
//! On Windows, [`shell`] is the app: a `CoreApplication` view with no XAML
//! whose window shows a `Windows.UI.Composition` sprite; the game
//! ([`h2host::start_in`], as on tvOS and iOS) renders through wgpu's DX12
//! `SwapChainPanel` target into a stand-in panel that hands the swap
//! chain to that sprite. Everything else here is portable and runs on the
//! Mac, where the package is built: the manifest and its images
//! ([`manifest`]), the package identity the Device Portal launches by
//! ([`identity`]), and the check that the binaries import only what an
//! app container on the console has ([`imports`]).

pub mod identity;
pub mod imports;
pub mod manifest;

#[cfg(windows)]
pub mod shell;
