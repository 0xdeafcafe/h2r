//! `h2-editor [--disc <iso> | --map <file.map>] [--hopper <multiplayer_hopper.dat>] [--data <dir>]`
//!
//! Opens the editor window. `--data` is the runtime's data root, as `h2
//! play --data` (default `$TMPDIR/h2boot-data`): saved game variants and
//! map packages go there. The disc defaults to `H2_DISC`, else
//! `disc/halo2.iso` in the repository (docs/editor.md). Paths are chosen in
//! the system's file dialog; `H2_EDITOR_TYPED_PATHS=1` types them instead.

use std::path::PathBuf;

use anyhow::{Result, bail};
use editor::dialog::NativeDialog;
use editor::hopper::HopperTool;
use editor::scenario::ScenarioTool;
use editor::shell::Shell;
use editor::source::default_disc;
use editor::variants::VariantTool;

const USAGE: &str = "usage: h2-editor [--disc <iso> | --map <file.map>] [--hopper <multiplayer_hopper.dat>] [--data <dir>]";

struct App {
    shell: Shell,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.shell.ui(ui);
    }
}

fn main() -> Result<()> {
    let mut source = None;
    let mut hopper = None;
    let mut data = content::store::default_data_root();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().map(PathBuf::from);
        match a.as_str() {
            "--disc" | "--map" => source = value(),
            "--hopper" => hopper = value(),
            "--data" => {
                data =
                    value().ok_or_else(|| anyhow::anyhow!("--data needs a directory\n{USAGE}"))?
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => bail!("unknown argument {a:?}\n{USAGE}"),
        }
    }

    let native = std::env::var_os("H2_EDITOR_TYPED_PATHS").is_none_or(|v| v.is_empty() || v == "0");
    let mut shell = Shell::new();
    let mut hoppers = HopperTool::new();
    hoppers.set_data_root(data.clone());
    if native {
        shell.set_file_dialog(Box::new(NativeDialog));
        hoppers.set_file_dialog(Box::new(NativeDialog));
    }
    if let Some(p) = &hopper {
        let bytes = std::fs::read(p)?;
        hoppers
            .open_bytes(&bytes, Some(p.clone()))
            .map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
    }
    shell.add_tool(Box::new(hoppers));
    let mut scenario = ScenarioTool::new();
    scenario.set_data_root(data.clone());
    shell.add_tool(Box::new(scenario));
    let mut variants = VariantTool::new();
    variants.set_data_root(data);
    shell.add_tool(Box::new(variants));
    if let Some(p) = default_disc(source) {
        shell.open_source(p);
    }

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title("h2-editor")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "h2-editor",
        options,
        Box::new(move |cc| {
            shell.set_render_state(cc.wgpu_render_state.clone());
            Ok(Box::new(App { shell }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
