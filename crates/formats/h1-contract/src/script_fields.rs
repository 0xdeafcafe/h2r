//! The Halo 2 scenario fields a converted level's script tables go in
//! (docs/formats/haloscript.md). `h1campaign::scripts::compile` fills them;
//! `boot::h1_script_builtins` reads the string data back.

/// Script string data (byte count, address).
pub const STRINGS: u32 = 0x1b0;
/// Scripts block.
pub const SCRIPTS: u32 = 0x1b8;
/// Globals block.
pub const GLOBALS: u32 = 0x1c0;
/// Script references block.
pub const REFERENCES: u32 = 0x1c8;
/// Syntax nodes block.
pub const NODES: u32 = 0x238;
/// Bytes of scenario these fields need.
pub const MIN_LEN: usize = 0x240;
