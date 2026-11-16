//! The app on the console: a `CoreApplication` view (no XAML; the
//! `windows` crate has no XAML projection) presenting through
//! `Windows.UI.Composition`.
//!
//! wgpu 30's DX12 backend has no `CoreWindow` target. Its `SwapChainPanel`
//! target makes a composition swap chain and hands it to
//! `ISwapChainPanelNative::SetSwapChain`; [`Panel`] implements that
//! interface itself and shows the swap chain on a sprite covering the
//! window (`ICompositorInterop::CreateCompositionSurfaceForSwapChain`),
//! which is what a XAML `SwapChainPanel` does inside. So `app::embedded`
//! keeps the one `host_surface` seam on Windows.
//!
//! The view's `Run` is the render loop on the UI thread: it pumps the
//! window's events, then draws the newest guest frame (the present waits
//! for vblank). Suspending pauses the guest and flushes the hard disk's
//! metadata before the deferral completes; resuming unpauses it. Logs
//! (stderr) go to `LocalState\h2uwp.log`, readable through the Device
//! Portal's file explorer.
#![allow(unsafe_code, non_snake_case)]

use std::ffi::c_void;
use std::os::windows::io::IntoRawHandle;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::ApplicationModel::Activation::IActivatedEventArgs;
use windows::ApplicationModel::Core::{
    CoreApplication, CoreApplicationView, IFrameworkView, IFrameworkView_Impl, IFrameworkViewSource,
    IFrameworkViewSource_Impl,
};
use windows::ApplicationModel::{Package, SuspendingDeferral, SuspendingEventArgs};
use windows::Foundation::{EventHandler, TypedEventHandler};
use windows::Graphics::Display::DisplayInformation;
use windows::Storage::ApplicationData;
use windows::UI::Composition::{CompositionStretch, Compositor, SpriteVisual};
use windows::UI::Core::{
    BackRequestedEventArgs, CoreProcessEventsOption, CoreWindow, CoreWindowEventArgs, SystemNavigationManager,
    VisibilityChangedEventArgs, WindowSizeChangedEventArgs,
};
use windows::UI::ViewManagement::{ApplicationView, ApplicationViewBoundsMode};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Console::{STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};
use windows::Win32::System::WinRT::Composition::ICompositorInterop;
use windows_core::{HRESULT, HSTRING, IInspectable, IUnknown, IUnknown_Vtbl, Interface, Ref, Result, implement, interface};

/// XAML's `ISwapChainPanelNative` (windows.ui.xaml.media.dxinterop.h),
/// which wgpu's `SwapChainPanel` target calls.
#[interface("63aad0b8-7c24-40ff-85a8-640d944cc325")]
unsafe trait ISwapChainPanelNative: IUnknown {
    fn SetSwapChain(&self, swap_chain: *mut c_void) -> HRESULT;
}

/// The stand-in `SwapChainPanel`: whatever swap chain wgpu sets becomes
/// the content of `visual`, stretched over the window.
#[implement(ISwapChainPanelNative)]
struct Panel {
    compositor: Compositor,
    visual: SpriteVisual,
}

impl Panel {
    fn set(&self, swap_chain: *mut c_void) -> Result<()> {
        // SAFETY: wgpu passes a live IDXGISwapChain1 for the call.
        let swap_chain = unsafe { IUnknown::from_raw_borrowed(&swap_chain) }.ok_or_else(|| windows_core::Error::from_hresult(HRESULT(0x8000_4003u32 as i32)))?;
        let interop: ICompositorInterop = self.compositor.cast()?;
        // SAFETY: a composition swap chain (CreateSwapChainForComposition).
        let surface = unsafe { interop.CreateCompositionSurfaceForSwapChain(swap_chain) }?;
        let brush = self.compositor.CreateSurfaceBrushWithSurface(&surface)?;
        brush.SetStretch(CompositionStretch::Fill)?;
        self.visual.SetBrush(&brush)
    }
}

impl ISwapChainPanelNative_Impl for Panel_Impl {
    unsafe fn SetSwapChain(&self, swap_chain: *mut c_void) -> HRESULT {
        match self.set(swap_chain) {
            Ok(()) => HRESULT(0),
            Err(e) => {
                eprintln!("h2uwp: SetSwapChain: {e}");
                e.code()
            }
        }
    }
}

/// What the window's and the app's event handlers (which must be `Send`)
/// leave for the render loop.
static VISIBLE: AtomicBool = AtomicBool::new(true);
static CLOSED: AtomicBool = AtomicBool::new(false);
static RESIZED: AtomicBool = AtomicBool::new(false);
static RESUMED: AtomicBool = AtomicBool::new(false);
static SUSPENDING: Mutex<Option<SuspendingDeferral>> = Mutex::new(None);

#[implement(IFrameworkViewSource)]
struct Source;

impl IFrameworkViewSource_Impl for Source_Impl {
    fn CreateView(&self) -> Result<IFrameworkView> {
        Ok(View.into())
    }
}

#[implement(IFrameworkView)]
struct View;

impl IFrameworkView_Impl for View_Impl {
    fn Initialize(&self, view: Ref<CoreApplicationView>) -> Result<()> {
        view.ok()?.Activated(&TypedEventHandler::<CoreApplicationView, IActivatedEventArgs>::new(|_, _| {
            CoreWindow::GetForCurrentThread()?.Activate()
        }))?;
        CoreApplication::Suspending(&EventHandler::<SuspendingEventArgs>::new(|_, args| {
            let deferral = args.ok()?.SuspendingOperation()?.GetDeferral()?;
            *SUSPENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(deferral);
            Ok(())
        }))?;
        CoreApplication::Resuming(&EventHandler::<IInspectable>::new(|_, _| {
            RESUMED.store(true, Ordering::Relaxed);
            Ok(())
        }))?;
        Ok(())
    }

    fn SetWindow(&self, window: Ref<CoreWindow>) -> Result<()> {
        let window = window.ok()?;
        window.VisibilityChanged(&TypedEventHandler::<CoreWindow, VisibilityChangedEventArgs>::new(|_, args| {
            VISIBLE.store(args.ok()?.Visible()?, Ordering::Relaxed);
            Ok(())
        }))?;
        window.SizeChanged(&TypedEventHandler::<CoreWindow, WindowSizeChangedEventArgs>::new(|_, _| {
            RESIZED.store(true, Ordering::Relaxed);
            Ok(())
        }))?;
        window.Closed(&TypedEventHandler::<CoreWindow, CoreWindowEventArgs>::new(|_, _| {
            CLOSED.store(true, Ordering::Relaxed);
            Ok(())
        }))?;
        // B on the pad is the game's Back, not the system's: keep the app.
        SystemNavigationManager::GetForCurrentView()?.BackRequested(&EventHandler::<BackRequestedEventArgs>::new(
            |_, args| args.ok()?.SetHandled(true),
        ))?;
        // The whole screen, not the TV-safe area the Xbox gives by default.
        let _ = ApplicationView::GetForCurrentView()?.SetDesiredBoundsMode(ApplicationViewBoundsMode::UseCoreWindow);
        Ok(())
    }

    fn Load(&self, _entry_point: &HSTRING) -> Result<()> {
        Ok(())
    }

    fn Run(&self) -> Result<()> {
        run()
    }

    fn Uninitialize(&self) -> Result<()> {
        Ok(())
    }
}

fn hstring_path(s: HSTRING) -> PathBuf {
    PathBuf::from(s.to_os_string())
}

/// The window's size in pixels.
fn pixel_size(window: &CoreWindow) -> Result<(u32, u32)> {
    let b = window.Bounds()?;
    let scale = DisplayInformation::GetForCurrentView().and_then(|d| d.RawPixelsPerViewPixel()).unwrap_or(1.0);
    Ok((((b.Width as f64) * scale).round().max(1.0) as u32, ((b.Height as f64) * scale).round().max(1.0) as u32))
}

/// Sends stderr and stdout to `<local>\h2uwp.log`: a UWP process has no
/// console, and std writes to the standard handle on every call.
fn log_to(local: &Path) {
    if let Ok(f) = std::fs::File::create(local.join("h2uwp.log")) {
        let h = HANDLE(f.into_raw_handle());
        // SAFETY: the handle stays open for the process's life.
        unsafe {
            let _ = SetStdHandle(STD_ERROR_HANDLE, h);
            let _ = SetStdHandle(STD_OUTPUT_HANDLE, h);
        }
    }
}

fn run() -> Result<()> {
    let window = CoreWindow::GetForCurrentThread()?;
    let package = hstring_path(Package::Current()?.InstalledLocation()?.Path()?);
    let local = hstring_path(ApplicationData::Current()?.LocalFolder()?.Path()?);
    log_to(&local);
    eprintln!("h2uwp: package {}, local state {}", package.display(), local.display());

    let compositor = Compositor::new()?;
    let target = compositor.CreateTargetForCurrentView()?;
    let visual = compositor.CreateSpriteVisual()?;
    visual.SetRelativeSizeAdjustment(windows_numerics::Vector2 { X: 1.0, Y: 1.0 })?;
    target.SetRoot(&visual)?;
    let panel: ISwapChainPanelNative = Panel { compositor: compositor.clone(), visual: visual.clone() }.into();

    if std::env::var_os("H2_PIPELINE_CACHE").is_none() {
        // SAFETY: before the guest, render and audio threads exist.
        unsafe { std::env::set_var("H2_PIPELINE_CACHE", local.join("cache").join(d3d::pipeline_store::FILE_NAME)) };
    }
    let layout = h2host::Layout::for_package(&package, &local);
    let size = pixel_size(&window)?;
    // SAFETY: `panel` lives until the end of this function, past the shell;
    // this is the UI thread.
    let mut shell = match unsafe { h2host::start_in(panel.as_raw(), size, layout) } {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("h2uwp: the game did not start: {e:#}");
            None
        }
    };
    let dispatcher = window.Dispatcher()?;
    let mut reported = false;
    while !CLOSED.load(Ordering::Relaxed) {
        let visible = VISIBLE.load(Ordering::Relaxed) && shell.is_some();
        dispatcher.ProcessEvents(if visible {
            CoreProcessEventsOption::ProcessAllIfPresent
        } else {
            CoreProcessEventsOption::ProcessOneAndAllPending
        })?;
        let Some(s) = shell.as_mut() else { continue };
        if let Some(deferral) = SUSPENDING.lock().unwrap_or_else(|e| e.into_inner()).take() {
            s.set_paused(true);
            vfs::flush_pending_metadata();
            eprintln!("h2uwp: suspended");
            deferral.Complete()?;
            continue;
        }
        if RESUMED.swap(false, Ordering::Relaxed) {
            s.set_paused(false);
            eprintln!("h2uwp: resumed");
        }
        if RESIZED.swap(false, Ordering::Relaxed) {
            s.resize(pixel_size(&window)?);
        }
        if !reported && let Some(exit) = s.guest_exit() {
            eprintln!("h2uwp: the game stopped: {exit:?}");
            reported = true;
        }
        if VISIBLE.load(Ordering::Relaxed) {
            s.redraw();
        }
    }
    if let Some(s) = shell.take() {
        let report = s.stop();
        eprintln!("h2uwp: exit {:?}", report.guest);
    }
    vfs::flush_pending_metadata();
    drop(panel);
    Ok(())
}

/// The app's entry point: runs the view until the window closes.
pub fn main() -> Result<()> {
    let source: IFrameworkViewSource = Source.into();
    CoreApplication::Run(&source)
}
