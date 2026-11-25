//! `h2-tools`: the offline map, tag and script tools. They read and write
//! map, tag, script and disc files and never boot the game, so this binary
//! links none of the runtime, renderer, audio or agent server and relinks in
//! seconds. `h2 <tool> ...` runs it with the same words
//! (crates/apps/cli/src/tools.rs, docs/runtime/cli-tools.md).

use anyhow::{Result, bail};
use cli_common::{default_disc, load_xbe, open_iso, safe_out};

mod collision;
mod effects;
mod extract_shaders;
mod firefight_cmd;
mod forge_cmd;
mod hs_lsp;
mod hud_cmd;
mod lights;
mod map_testbuild;
mod maps;
mod object;
mod package_cmd;
mod rules_cmd;
mod rules_editors;
mod rules_lsp;
mod scripts;
mod shader;
mod strings;
mod studio_link;
mod superbounce_cmd;
mod tag_repo_cmd;
mod testmap_cmd;
mod variants_cmd;

const USAGE: &str = "usage: h2 <tool> ... (or h2-tools <tool> ...)

The offline map, tag and script tools. `h2 <tool>` runs this binary, which
sits beside h2 (docs/runtime/cli-tools.md).

  rules check|build|run|sim|caps|builtin|settings|lsp|editors
                          the scripted game types' compiler, verifier,
                          scenario runner, the record a script makes,
                          language server and editor extensions
                          (docs/game/rules-engine.md)
  variants port <input>... [--script builtin|FILE] [-o DIR]
                          self-contained .h2variant files from records, saves,
                          hopper slots or old files; --all-builtins from the
                          disc (docs/editor/variants.md)
  variants from-script <file.rules> [--option K=V]... [-o DIR]
                          a script's own .h2variant: the stock record with
                          its settings (docs/editor/variants.md)
  maps [disc.iso]         map headers and tag counts on the disc
  tags <map> [group]      tags in a disc map or local .map; optional --disc <iso>
  tags extract|build ...  the tag repository (h2 tags extract --help)
  tag <map> <tag name>    decode one tag; optional group and --disc <iso>
  firefight kit MAP [--home red|blue] [--probe]  build a Firefight package
                          from a disc map (docs/game/firefight.md)
  map testbuild --out DIR builder load-test maps for H2_MAP_DIR (docs/formats/map-builder.md)
  testmap <map> <script> --out <new.map>  a test map: a JSON edit script over the
                          placements (docs/runtime/agent-api.md, phase 5)
  forge build|check <doc.h2forge> [--disc <iso>]  materialise or verify a local Forge map
  package check <dir>...  what is wrong with package folders, before any map
  package build <dir>... --base <map> -o <out.map>  apply packages to a map
                          (docs/editor/studio-redesign.md, Packages)
  collision <map> <tag>   summarise a coll/phmo tag; --obj FILE exports geometry
  collision --all <map|all>  validate every coll/phmo tag (all = every disc map)
  superbounce scan <map>  bounce strips and forced-crouch spots in a map's
                          collision; --bsp N, --json (docs/game/superbounce.md)
  hud preview <hud> --out DIR  a HUD kit HUD drawn over the retail player HUD,
                          PNG and JSON per state, and its checks; hud sheet
                          for every HUD (docs/game/hud-kit.md)
  object <map> <tag>      object-family tag, its hlmt and default variant;
                          --all <map|all> validates every object-family tag
  strings <map>           localized text; --lang en, --tag name, --all, --disc iso
  effect <map> <tag>      print an effects-family tag (effe tree); --all <map|all> validates the family
                          (option --disc <iso>)
  lights <map>            the ligh tags a map defines (shape, colour, shadow flag)
                          and where each comes from; --all <map|all> checks the layout
                          (option --disc <iso>)
  shader <map> <tag>      print the material a shad resolves to (passes, textures,
                          constants, render state); --all <map|all> validates
                          every shad/stem/spas (options --disc <iso>)
  hs <map> [--script name] [--builtins] [--disc iso]
  hs --all [--disc iso]   verify all disc maps (codec symmetry and node counts)
  hs lsp                  the language server for Studio's HaloScript working
                          copies (.hsc; docs/editor/rules-editors.md)
  extract-shaders --out DIR [--disc iso] [--force]
                          write every distinct vertex program (*.xvu) and pixel shader
                          definition (*.psdef) from the disc's maps to DIR, the corpus
                          for the shaders and d3d ignored tests (keep it out of the repo)
  (Output is never written over an existing file without --force, and never
  over the disc being read, a .iso/.xiso or anything holding a disc image.)

<map> is a disc map name (ascension) or a .map file. Without --disc, the
tools use $H2_DISC, then the Halo 2 .iso in the repo's disc/ folder.";

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(cmd) = args.next() else {
        bail!("{USAGE}")
    };
    let rest: Vec<String> = args.collect();
    match cmd.as_str() {
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        "rules" => rules_cmd::run(rest),
        "variants" => variants_cmd::run(rest),
        "firefight" => firefight_cmd::run(rest),
        "forge" => forge_cmd::run(rest),
        "package" => package_cmd::run(rest),
        "map" => map_testbuild::run(rest),
        "testmap" => testmap_cmd::run(rest),
        "tags" if tag_repo_cmd::handles(&rest) => tag_repo_cmd::run(rest),
        "maps" | "tags" | "tag" => maps::run(&cmd, rest),
        "collision" => collision::run(rest),
        "superbounce" => superbounce_cmd::run(rest),
        "hud" => hud_cmd::run(rest),
        "object" => object::run(rest),
        "strings" => strings::run(rest),
        "effect" => effects::run(rest),
        "lights" => lights::run(rest),
        "shader" => shader::run(rest),
        "hs" => scripts::run(rest),
        "extract-shaders" => extract_shaders::run(rest),
        _ => bail!("h2-tools has no command {cmd:?}\n\n{USAGE}"),
    }
}
