//! Zed extension for h2recomp's scripts: starts `h2 rules lsp` for the
//! rules language and `h2 hs lsp` for HaloScript.
//!
//! The binary is, in order: the `lsp.<server>.binary.path` setting
//! (`h2-rules-lsp` or `h2-hs-lsp`),
//! `h2` on the worktree's `PATH`, then `$HOME/.cargo/bin/h2` (where
//! `cargo install --locked --path crates/apps/cli` puts it; `h2` runs the
//! server from the `h2-tools` installed beside it). The extension
//! runs in a WebAssembly sandbox that cannot see the host's files, so the
//! last path is returned without a check; if it is missing, Zed's log
//! ("zed: open log") says the server failed to start.

use zed_extension_api::{self as zed, settings::LspSettings, LanguageServerId, Result};

/// The rules server's id in extension.toml and in the `lsp` settings.
const RULES_SERVER: &str = "h2-rules-lsp";
/// The HaloScript server's id.
const HS_SERVER: &str = "h2-hs-lsp";

struct H2Rules;

impl zed::Extension for H2Rules {
    fn new() -> Self {
        H2Rules
    }

    fn language_server_command(
        &mut self,
        id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let name: &str = id.as_ref();
        let (server, default_args) = match name {
            HS_SERVER => (HS_SERVER, ["hs", "lsp"]),
            _ => (RULES_SERVER, ["rules", "lsp"]),
        };
        let binary = LspSettings::for_worktree(server, worktree)
            .ok()
            .and_then(|s| s.binary);
        let args = binary
            .as_ref()
            .and_then(|b| b.arguments.clone())
            .unwrap_or_else(|| default_args.iter().map(|a| a.to_string()).collect());
        let env: Vec<(String, String)> = binary
            .as_ref()
            .and_then(|b| b.env.clone())
            .map(|m| m.into_iter().collect())
            .unwrap_or_default();
        let command = match binary.and_then(|b| b.path) {
            Some(path) => path,
            None => match worktree.which("h2") {
                Some(path) => path,
                None => {
                    let home = worktree
                        .shell_env()
                        .into_iter()
                        .find(|(k, _)| k == "HOME")
                        .map(|(_, v)| v);
                    match home {
                        Some(home) => format!("{home}/.cargo/bin/h2"),
                        None => {
                            return Err(format!(
                                "h2 is not on PATH. Install it and the \
                                h2-tools it runs with `cargo install --locked --path \
                                crates/apps/cli` and `cargo install --locked --path \
                                crates/apps/h2-tools` from an h2recomp checkout, or set \
                                lsp.{server}.binary.path in Zed's settings."
                            ))
                        }
                    }
                }
            },
        };
        Ok(zed::Command { command, args, env })
    }
}

zed::register_extension!(H2Rules);
