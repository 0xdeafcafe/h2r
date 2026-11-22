//! Zed extension for h2recomp's rules language: starts `h2 rules lsp`.
//!
//! The binary is, in order: the `lsp.h2-rules-lsp.binary.path` setting,
//! `h2` on the worktree's `PATH`, then `$HOME/.cargo/bin/h2` (where
//! `cargo install --locked --path crates/apps/cli` puts it). The extension
//! runs in a WebAssembly sandbox that cannot see the host's files, so the
//! last path is returned without a check; if it is missing, Zed's log
//! ("zed: open log") says the server failed to start.

use zed_extension_api::{self as zed, settings::LspSettings, LanguageServerId, Result};

/// The server's id in extension.toml and in the `lsp` settings.
const SERVER: &str = "h2-rules-lsp";

struct H2Rules;

impl zed::Extension for H2Rules {
    fn new() -> Self {
        H2Rules
    }

    fn language_server_command(
        &mut self,
        _id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let binary = LspSettings::for_worktree(SERVER, worktree)
            .ok()
            .and_then(|s| s.binary);
        let args = binary
            .as_ref()
            .and_then(|b| b.arguments.clone())
            .unwrap_or_else(|| vec!["rules".to_string(), "lsp".to_string()]);
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
                            return Err("h2 is not on PATH. Install it with \
                                `cargo install --locked --path crates/apps/cli` \
                                from an h2recomp checkout, or set \
                                lsp.h2-rules-lsp.binary.path in Zed's settings."
                                .to_string())
                        }
                    }
                }
            },
        };
        Ok(zed::Command { command, args, env })
    }
}

zed::register_extension!(H2Rules);
