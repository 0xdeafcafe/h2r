//! Named guest locations: the one table of hook points and globals.
//!
//! Every guest function a feature hooks and every global more than one
//! module reads is named here once, keyed by its address in the base (1.0)
//! image, with the page that records the evidence. Code elsewhere refers to
//! the name (`points::GAME_TICK.base`), never to a second copy of the
//! number. This is the anchor list docs/runtime/title-updates.md section 3.3 asks
//! for; a title update maps these through `ImageAddrs` (docs/engine/feature-engine.md
//! "Version key").
//!
//! No game data here: addresses and argument locations are research notes,
//! and each carries a doc reference.
//!
//! ```text
//! points! {
//!     /// The game tick.
//!     GAME_TICK @ 0x0013_7fe0: [fn() ret 0], hot; "docs/engine/engine-map.md";
//!     /// Not read yet.
//!     SOMETHING @ 0x0012_3450: [opaque]; "docs/x.md";
//! }
//! ```

/// Where one argument or the result lives at the function's entry or exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loc {
    Eax,
    Al,
    Ecx,
    Edx,
    Ebx,
    Esi,
    Edi,
    Xmm0,
    /// The n-th dword argument on the stack, 0-based: `[esp + 4 + 4n]` at
    /// entry (the return address is at `[esp]`).
    Stack(u8),
}

/// A point's calling convention, as far as it has been read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sig {
    /// Not read: observers and after-hooks only, never typed arguments.
    Opaque,
    Fn {
        args: &'static [(&'static str, Loc)],
        ret: Option<Loc>,
        /// Bytes the callee pops (`ret n`).
        pop: u16,
    },
}

impl Sig {
    pub fn is_opaque(&self) -> bool {
        matches!(self, Sig::Opaque)
    }
    /// The location of the named argument.
    pub fn arg(&self, name: &str) -> Option<Loc> {
        match self {
            Sig::Opaque => None,
            Sig::Fn { args, .. } => args.iter().find(|(n, _)| *n == name).map(|(_, l)| *l),
        }
    }
}

/// Whether an image without this point can still run the features on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// A hook on it is required: missing, its feature is turned off.
    Required,
    /// A hook on it is dropped with a note when missing.
    Optional,
}

/// Flags the `points!` macro takes after the signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flag {
    /// Called thousands of times a frame: a hook leaves the direct-call fast
    /// path, and the plan warns.
    Hot,
    /// [`Class::Optional`].
    Optional,
}

/// A named guest function.
#[derive(Debug, PartialEq, Eq)]
pub struct Point {
    pub name: &'static str,
    /// Address in the base (1.0) image.
    pub base: u32,
    pub sig: Sig,
    pub class: Class,
    pub hot: bool,
    /// The page that records the evidence.
    pub doc: &'static str,
}

impl Point {
    pub const fn new(name: &'static str, base: u32, sig: Sig, doc: &'static str) -> Self {
        Point {
            name,
            base,
            sig,
            class: Class::Required,
            hot: false,
            doc,
        }
    }
    pub const fn with_flags(mut self, flags: &[Flag]) -> Self {
        let mut i = 0;
        while i < flags.len() {
            match flags[i] {
                Flag::Hot => self.hot = true,
                Flag::Optional => self.class = Class::Optional,
            }
            i += 1;
        }
        self
    }
}

/// A named guest global (a word or a structure at a fixed address).
#[derive(Debug, PartialEq, Eq)]
pub struct Global {
    pub name: &'static str,
    pub base: u32,
    pub doc: &'static str,
}

#[doc(hidden)]
#[macro_export]
macro_rules! point_loc {
    (eax) => {
        $crate::points::Loc::Eax
    };
    (al) => {
        $crate::points::Loc::Al
    };
    (ecx) => {
        $crate::points::Loc::Ecx
    };
    (edx) => {
        $crate::points::Loc::Edx
    };
    (ebx) => {
        $crate::points::Loc::Ebx
    };
    (esi) => {
        $crate::points::Loc::Esi
    };
    (edi) => {
        $crate::points::Loc::Edi
    };
    (xmm0) => {
        $crate::points::Loc::Xmm0
    };
    (stack $n:literal) => {
        $crate::points::Loc::Stack($n)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! point_sig {
    (opaque) => {
        $crate::points::Sig::Opaque
    };
    (fn ( $($arg:ident : $loc:ident $($n:literal)?),* ) $(-> $ret:ident)? ret $pop:literal) => {
        $crate::points::Sig::Fn {
            args: &[$((stringify!($arg), $crate::point_loc!($loc $($n)?))),*],
            ret: $crate::point_ret!($($ret)?),
            pop: $pop,
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! point_ret {
    () => {
        None
    };
    ($r:ident) => {
        Some($crate::point_loc!($r))
    };
}

/// Declares named [`Point`]s (see the module docs for the grammar) and adds
/// them to a slice named by the first token (`points! { ALL; ... }`).
#[macro_export]
macro_rules! points {
    ($all:ident; $( $(#[doc = $d:literal])* $name:ident @ $addr:literal : [$($sig:tt)*] $(, $flag:ident)* ; $doc:literal ; )*) => {
        $(
            $(#[doc = $d])*
            pub static $name: $crate::points::Point = $crate::points::Point::new(
                stringify!($name),
                $addr,
                $crate::point_sig!($($sig)*),
                $doc,
            )
            .with_flags(&[$($crate::points::Flag::$flag),*]);
        )*
        /// Every point declared in this table.
        pub static $all: &[&$crate::points::Point] = &[$(&$name),*];
    };
}

/// Declares named [`Global`]s: `NAME @ 0x...; "doc";`.
#[macro_export]
macro_rules! globals {
    ($all:ident; $( $(#[doc = $d:literal])* $name:ident @ $addr:literal ; $doc:literal ; )*) => {
        $(
            $(#[doc = $d])*
            pub static $name: $crate::points::Global = $crate::points::Global {
                name: stringify!($name),
                base: $addr,
                doc: $doc,
            };
        )*
        /// Every global declared in this table.
        pub static $all: &[&$crate::points::Global] = &[$(&$name),*];
    };
}

points! { ALL;
    /// `game_tick`: the per-tick phases, then the tick count. Called once per
    /// simulated tick by `game_update_ticks`.
    GAME_TICK @ 0x0013_7fe0: [fn() ret 0]; "docs/engine/engine-map.md";
    /// Game activation after the scenario's cache is loaded: options in EAX,
    /// AL 1 once the map and its first BSP are in.
    GAME_ACTIVATE @ 0x0018_f000: [fn(options: eax) -> al ret 0]; "docs/runtime/level-boot.md";
    /// The game-end path; the end latch is game globals `+0x1128`.
    GAME_END @ 0x0013_89c0: [opaque]; "docs/live/live-gamestate.md";
    /// A player was killed: `(attacker player, victim player, betrayal,
    /// packed damage)` on the stack. Who pops them is not read.
    PLAYER_KILLED @ 0x0015_ce70: [opaque]; "docs/live/live-gamestate.md";
    /// One weapon barrel fires: stdcall `(weapon object datum, barrel
    /// index)`. One call is one shot (likely).
    WEAPON_FIRE @ 0x0010_4150: [fn(weapon: stack 0, barrel: stack 1) ret 8], Optional; "docs/live/live-gamestate.md";
    /// A damage report is applied to its victim: the report in EDI, the
    /// victim object datum on the stack. Called once per damage instance.
    DAMAGE_REPORT @ 0x000d_9640: [fn(report: edi, victim: stack 0) ret 4], Optional; "docs/live/live-gamestate.md";
    /// An object's damage update: stdcall `(object datum)`, AL whether
    /// anything changed. Called once per object per tick by
    /// `object_update` (`0xbc470`, its only caller): stun timers, shield
    /// recharge, and the body's regeneration (`0xd63de`..`0xd64b5`).
    OBJECT_DAMAGE_UPDATE @ 0x000d_5de0: [fn(object: stack 0) -> al ret 4]; "docs/game/health.md";
    /// Restore an object's body to full (EAX the object datum), AL 1 when
    /// it did (refused when full or depleted). Its only caller is the
    /// powerup pickup's health case (`0x151f73`).
    BODY_RESTORE @ 0x000d_6790: [fn(object: eax) -> al ret 0]; "docs/game/health.md";
    /// `locale_string_id_find`: the string for the packed id in EDI, its
    /// address in EAX.
    LOCALE_FIND @ 0x001a_02d0: [fn(id: edi) -> eax ret 0], Hot; "docs/engine/locale_string_id_find.md";
    /// `locale_string_id_exists`: whether the packed id has a string (AL).
    LOCALE_EXISTS @ 0x001a_0310: [opaque]; "docs/engine/locale_string_id_find.md";
    /// `ice_cream_flavor_stock`'s worker: turns skull ESI on (byte ESI of
    /// the 15-byte array at `0x4f55dc`, only when it was 0), then the HUD
    /// message and sound.
    SKULL_STOCK @ 0x001d_f3e0: [fn(index: esi) ret 0]; "docs/game/skulls.md";
    /// Queue new game options (EAX = the options; `+0` the mode, 1 a
    /// campaign). The menu's campaign start and level boot both end here.
    OPTIONS_QUEUE @ 0x0018_e790: [fn(options: eax) ret 0]; "docs/game/skulls.md";
    /// `render_frame_and_present`: renders the game views (or the fallback
    /// screen) and presents. Once a frame, from the main loop.
    RENDER_FRAME @ 0x0022_3240: [opaque]; "docs/engine/engine-map.md";
    /// `render_game_views`: builds the per-player view records and cameras,
    /// then calls `render_views`.
    RENDER_GAME_VIEWS @ 0x0022_34e0: [opaque]; "docs/engine/engine-map.md";
    /// `render_views`: begin-frame setup, the loop over view records, end of
    /// drawing.
    RENDER_VIEWS @ 0x0002_b5d0: [opaque]; "docs/engine/engine-map.md";
    /// `render_game_view`: one game view (camera, render state, scene
    /// passes, UI composition).
    RENDER_GAME_VIEW @ 0x0002_b790: [opaque]; "docs/engine/engine-map.md";
    /// `render_scene_passes`: the scene pass sequence of one view.
    RENDER_SCENE_PASSES @ 0x0002_bcd0: [opaque]; "docs/graphics/e3-lighting.md";
    /// `render_light_passes`: draws one dynamic light's passes.
    RENDER_LIGHT_PASSES @ 0x0003_1170: [opaque]; "docs/graphics/e3-lighting.md";
    /// Draws the current layer (`[0x00467130]`) of the current list: layer
    /// setup, then `LAYER_ENTRIES` for the layers in its mask.
    LAYER_DRAW @ 0x0004_4550: [opaque]; "docs/graphics/layer-draw.md";
    /// The layer draw's setup call (ESI = `0x00485b48`).
    LAYER_SETUP @ 0x0001_6b10: [opaque]; "docs/graphics/layer-draw.md";
    /// Draws a list's entries for a layer: builds them, then calls each
    /// entry type's handler. EDI the list, EBX the layer, stack 0 an
    /// argument (`[0x00467134]`), `ret 4`.
    LAYER_ENTRIES @ 0x0003_bc30: [fn(list: edi, layer: ebx, arg: stack 0) ret 4]; "docs/graphics/layer-draw.md";
    /// Builds a layer's entries (0x18 bytes each) into the caller's buffer;
    /// the count in EAX. Stdcall `(list, arg, layer, buffer)`.
    LAYER_BUILD @ 0x0003_bab0: [fn(list: stack 0, arg: stack 1, layer: stack 2, buffer: stack 3) -> eax ret 16]; "docs/graphics/layer-draw.md";
    /// Shader draw begin: per draw.
    SHADER_DRAW_BEGIN @ 0x0001_bbf0: [opaque], Hot; "docs/formats/shader-instances.md";
    /// `submit_structure_or_model_geometry`: per draw.
    GEOMETRY_SUBMIT @ 0x0004_1040: [opaque], Hot; "docs/formats/structure-bsp.md";
    /// A shader parameter's value for this draw: its sources through
    /// `[0x00485a58]`, the CRT `fmod`, then `FUNCTION_EVALUATE`.
    SHADER_PARAMETER @ 0x0001_b230: [opaque], Hot; "docs/graphics/layer-draw.md";
    /// A texture binding's source by type (a jump table).
    TEXTURE_SOURCE @ 0x0001_9530: [opaque], Hot; "docs/graphics/layer-draw.md";
    /// An animated bitmap's frame for a texture binding.
    BITMAP_FRAME @ 0x0001_43c0: [opaque], Hot; "docs/graphics/layer-draw.md";
    /// A tag function's value (two inputs on the stack).
    FUNCTION_EVALUATE @ 0x0013_b390: [opaque], Hot; "docs/graphics/layer-draw.md";
    /// The CRT's x87 `fmod` (`_CIfmod`: ST1 mod ST0), through the CRT's
    /// transcendental dispatcher.
    CRT_FMOD @ 0x0037_3b6a: [opaque], Hot; "docs/graphics/layer-draw.md";
    /// The top-level Settings list's row text (thiscall on the list, the
    /// item and a word).
    SETTINGS_ROW_TEXT @ 0x002b_1d6e: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The top-level Settings list's row chosen (thiscall on the list, the
    /// player chain and a pointer to the row).
    SETTINGS_ROW_CHOSEN @ 0x002b_1da9: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The Controls list's row text: the label and value of rows 0 to 5.
    CONTROLS_ROW_TEXT @ 0x002c_8548: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The Controls list's row chosen (rows past 4 open the vibration
    /// dialog).
    CONTROLS_ROW_CHOSEN @ 0x002c_8719: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/settings-menu.md";
    /// The campaign difficulty list's row text.
    DIFFICULTY_ROW_TEXT @ 0x002c_9ce7: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/skulls.md";
    /// The campaign difficulty list's row chosen: clamps the row to 0..3,
    /// stores the difficulty and starts the campaign.
    DIFFICULTY_ROW_CHOSEN @ 0x002c_9d38: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/skulls.md";
    /// The variant editor's category list (screen 178,
    /// `variant_editing_options`) row text: looks the row's word up in a
    /// table of six; an unknown word leaves the text as it was.
    VARIANT_OPTIONS_ROW_TEXT @ 0x002b_7581: [fn(list: ecx, item: stack 0, word: stack 1) ret 8]; "docs/game/skulls.md";
    /// The variant editor's category list row chosen: words 0 to 5 open a
    /// category's screen; any other word returns having done nothing.
    VARIANT_OPTIONS_ROW_CHOSEN @ 0x002b_75e8: [fn(list: ecx, chain: stack 0, row: stack 1) ret 8]; "docs/game/skulls.md";
    /// Whether the HUD draws (AL); 0 in a campaign with Blind on.
    HUD_SHOWN @ 0x0013_939b: [fn() -> al ret 0]; "docs/game/skulls.md";
    /// Draws the first-person weapons of one local user (stdcall, four
    /// arguments, EAX the result); skipped whole in a campaign with Blind on.
    FIRST_PERSON_RENDER @ 0x0016_5f16: [fn(user: stack 0, a: stack 1, b: stack 2, c: stack 3) -> eax ret 16]; "docs/game/skulls.md";
    /// A weapon object made from its scenario placement (stdcall: the
    /// object datum, the placement); its rounds are the placement's
    /// `+0x4c`, halved in a campaign with Famine on.
    WEAPON_FROM_PLACEMENT @ 0x000f_da00: [fn(object: stack 0, placement: stack 1) ret 8]; "docs/game/skulls.md";
    /// Open a screen: build a 0x20-byte request for the player mask and
    /// call `loader` with it (stdcall).
    OPEN_SCREEN @ 0x0014_9f1e: [fn(mask: stack 0, loader: stack 1) ret 8]; "docs/formats/ui.md";
    /// Allocate a UI object of `size` bytes (stdcall); 0 when out of memory.
    UI_ALLOC @ 0x001a_47fd: [fn(size: stack 0) -> eax ret 4]; "docs/formats/ui.md";
    /// The Controls screen loader (stdcall, the request).
    CONTROLS_LOADER @ 0x002b_7714: [fn(request: stack 0) -> eax ret 4]; "docs/formats/ui.md";
    /// The Controls screen constructor: the screen in EDI, the request's
    /// `+4`, `+8` and `+2` (word) on the stack; the screen in EAX.
    CONTROLS_SCREEN_CTOR @ 0x002b_7752: [fn(screen: edi, a: stack 0, b: stack 1, player: stack 2) -> eax ret 12]; "docs/formats/ui.md";
    /// Push a constructed screen (EDI) opened by the request in EBX; it
    /// calls screen slot 21. Who pops what is not read: entries restore
    /// the stack themselves.
    SCREEN_PUSH @ 0x0014_7f6d: [opaque]; "docs/formats/ui.md";
    /// Screen vtable slot 18 of the Controls class: set up the current pane
    /// (header, then the pane's widgets).
    SCREEN_SETUP_PANE @ 0x0023_047e: [fn(screen: ecx, arg: stack 0) ret 4]; "docs/formats/ui.md";
    /// Set a text widget's string id (the widget in EDI).
    TEXT_SET @ 0x0025_3b1a: [fn(text: edi, id: stack 0) ret 4]; "docs/formats/ui.md";
    /// The pane builder's text step: make a text widget from a text block
    /// definition (`0x2c` bytes), a child of the screen in EBX, and give it
    /// the block's string; the widget in EAX, or 0.
    PANE_TEXT_ADD @ 0x0022_f9bf: [fn(screen: ebx, index: stack 0, block: stack 1) -> eax ret 8]; "docs/formats/ui.md";
    /// The pane builder's bitmap step: make a bitmap widget from a bitmap
    /// block definition (`0x38` bytes), a child of the screen in ESI; the
    /// widget in EAX, or 0.
    PANE_BITMAP_ADD @ 0x0022_fa30: [fn(screen: esi, block: stack 0) -> eax ret 4]; "docs/formats/ui.md";
    /// The current interface's screen tag for a screen id (stdcall): the
    /// `wgit` tag's datum handle, or -1.
    SCREEN_TAG_BY_ID @ 0x0014_8098: [fn(id: stack 0) -> eax ret 4]; "docs/formats/ui.md";
    /// The focused item's row handle of the list in EAX, or -1.
    LIST_FOCUSED_ROW @ 0x0024_c447: [fn(list: eax) -> eax ret 0]; "docs/formats/ui.md";
    /// An item widget's `index`-th text widget of kind 6 (thiscall on the
    /// item; the game pushes 0, the index, 6), or 0.
    ITEM_TEXT @ 0x0022_edb8: [fn(item: ecx, kind: stack 0, index: stack 1, zero: stack 2) -> eax ret 12]; "docs/audio/playstation-audio.md";
    /// The network session's game start: builds game options from the
    /// session in EDX and starts the game (`0x18e8b0`); AL 1 when it
    /// started. Refuses a campaign session with more than one peer
    /// (`0x6eea5`).
    SESSION_GAME_START @ 0x0006_ec80: [fn(session: edx, flag: stack 0) -> al ret 4]; "docs/live/live-coop.md";
    /// Clear and default a game-options block (ESI). The session start
    /// calls it right after its peer and player checks.
    OPTIONS_NEW @ 0x0013_8110: [fn(options: esi) ret 0]; "docs/runtime/level-boot.md";
    /// Whether the squad session is joinable (Live presence, AL); false
    /// for a campaign session (`0x63e7a`).
    SESSION_JOINABLE @ 0x0006_3e00: [fn() -> al ret 0]; "docs/live/live-coop.md";
    /// The host's game-start session parameters (session in EBX, state 5
    /// only); for a campaign session it sets the peer limit to 1.
    SESSION_GAME_LIMITS @ 0x0006_e720: [fn(session: ebx) -> al ret 0]; "docs/live/live-coop.md";
    /// Lobby: set the squad session's map (host only): stdcall `(campaign
    /// id)`, the map id in ESI, AL 1 when set.
    LOBBY_SET_MAP @ 0x0019_a6f2: [fn(map: esi, campaign: stack 0) -> al ret 4]; "docs/live/live-coop.md";
    /// Lobby: set the squad session's campaign difficulty (AX; `0xffff`
    /// none) and, for a difficulty, clear the variant (synchronous
    /// protocol). AL 1 when set.
    LOBBY_SET_DIFFICULTY @ 0x0019_a76d: [fn(difficulty: eax) -> al ret 0]; "docs/live/live-coop.md";
    /// Lobby: put the squad lobby in a mode (stdcall; 0 co-op campaign,
    /// 2 custom game): the Xbox Live menu's Custom Game row calls it with 2
    /// once its session is made (`0x23101b`), the lobby's "Switch To: Co-Op
    /// Game" with 0 (`0x2bba30`).
    LOBBY_MODE_SET @ 0x0019_9a03: [fn(mode: stack 0) ret 4]; "docs/live/live-coop.md";
    /// Lobby: why the game cannot start yet (stdcall, an optional float
    /// out); 9 ready, 7 the teams or co-op check (`0x19a4be`: co-op needs
    /// exactly two players, both on this console), 8 and 10 the load and
    /// countdown states. Its status line picks the text (`0x250008`).
    LOBBY_START_STATE @ 0x0019_a2ce: [fn(progress: stack 0) -> eax ret 4]; "docs/live/live-coop.md";
    /// The start state's tail after a co-op check that passed: a countdown
    /// running (`>= 0`, state 10), else everyone ready (`0x592f0`, AL: 9,
    /// else 8).
    LOBBY_COUNTDOWN @ 0x0006_4a60: [fn() -> eax ret 0]; "docs/live/live-coop.md";
    LOBBY_ALL_READY @ 0x0005_92f0: [fn() -> al ret 0]; "docs/live/live-coop.md";
    /// The per-tick player update (from `game_tick`, `0x13801f`): every
    /// player; a first spawn pending (flags byte `+2` bit 3) goes to
    /// `PLAYER_SPAWN`; the co-op respawn (`0x14cdd0`) runs inside it.
    PLAYER_UPDATE @ 0x0014_d3a0: [fn(update: stack 0) ret 4]; "docs/live/live-coop.md";
    /// Spawn one player (handle): unit type, a starting location from
    /// `PLAYER_SPAWN_LOCATION`, the unit; AL 0 when no location was free.
    /// Clears the first-spawn bit (`0x14f6c1`) when it spawned.
    PLAYER_SPAWN @ 0x0014_f2e0: [fn(player: stack 0) -> al ret 4]; "docs/live/live-coop.md";
    /// The best free starting location for a player (handle in ECX): AX the
    /// index into `scnr+0x100`, `0xffff` when every spot is blocked.
    PLAYER_SPAWN_LOCATION @ 0x0014_ef00: [fn(player: ecx, unit: stack 0, a: stack 1, b: stack 2) -> eax ret 12]; "docs/live/live-coop.md";
    /// Retail co-op placement: move a player (EBX) next to a position (ESI,
    /// three floats) beside an anchor unit (stack). The co-op respawn's
    /// own call is at `0x14cffd`. AL 1 when placed.
    PLAYER_PLACE_NEAR @ 0x0014_e970: [fn(player: ebx, position: esi, anchor: stack 0) -> al ret 4]; "docs/live/live-coop.md";
    /// Teleport an object to a cutscene flag: the body of HaloScript
    /// `object_teleport` (`0x2a2517`) and of
    /// `volume_teleport_players_not_inside` (`0x29f641`).
    SCRIPT_TELEPORT @ 0x0029_ffb0: [fn(object: stack 0, flag: stack 1, a: stack 2, b: stack 3) ret 16], Optional; "docs/live/live-coop.md";
    /// Save the game state (checkpoint): the save callbacks, then the
    /// writer `0x2151f0`.
    GAME_STATE_SAVE @ 0x0012_3e20: [opaque], Optional; "docs/runtime/checkpoints.md";
    /// Revert the game state to a saved slot.
    GAME_STATE_REVERT @ 0x0012_3f60: [opaque], Optional; "docs/runtime/checkpoints.md";
    /// Switch the structure BSP (prepare, dispose, load, publish).
    BSP_SWITCH @ 0x0011_c1b0: [opaque], Optional; "docs/runtime/level-preflight.md";
    /// HaloScript `cinematic_start`.
    CINEMATIC_START @ 0x002a_9740: [opaque], Optional; "docs/runtime/level-preflight.md";
    /// HaloScript `cinematic_stop`.
    CINEMATIC_STOP @ 0x002a_9760: [opaque], Optional; "docs/runtime/level-preflight.md";
    /// The level select's list: one campaign's map ids from the start-level
    /// list, sorted, into the data array (stdcall `(array, campaign id)`).
    /// 1.0 asks for campaign 1 only (`0x19c190`).
    CAMPAIGN_LEVEL_LIST @ 0x0019_c6d0: [fn(array: stack 0, campaign: stack 1) ret 8]; "docs/game/multiple-campaigns.md";
    /// The menu's campaign start: options from the selection globals
    /// `0x54e7c0`-`0x54e7c8`, the level looked up by `(campaign, map)`.
    CAMPAIGN_START @ 0x0023_643f: [fn() ret 0]; "docs/game/multiple-campaigns.md";
    /// The next level of a campaign: the smallest map id above `map` with
    /// that campaign id in the start-level list (stdcall, EAX, -1 none).
    /// The game-won path (`0x18e960`) and `0x163610` ask it.
    NEXT_LEVEL @ 0x0019_c440: [fn(campaign: stack 0, map: stack 1) -> eax ret 8]; "docs/game/multiple-campaigns.md";
    /// The start-level record of `(campaign, map)`: the first with both ids
    /// in the loaded globals' start-level list (stdcall, EAX, 0 none). Some
    /// callers pass campaign 1 whatever the map: the level select's press
    /// (`0x2c5118`), which faults on 0, and Continue (`0x199ab7`).
    CAMPAIGN_LEVEL_LOOKUP @ 0x0019_c270: [fn(campaign: stack 0, map: stack 1) -> eax ret 8]; "docs/game/multiple-campaigns.md";
    /// A finished level raises one slot's profile progress for a difficulty
    /// to map id ESI (stdcall `(slot, difficulty)`).
    PROGRESS_RAISE @ 0x0019_043f: [fn(map: esi, slot: stack 0, difficulty: stack 1) ret 8]; "docs/game/multiple-campaigns.md";
    /// The last level started (map id EAX) into every signed-in profile and
    /// `0x510840`.
    LAST_LEVEL_SET @ 0x0019_040d: [fn(map: eax) ret 0]; "docs/game/multiple-campaigns.md";
    /// The level select screen's constructor (thiscall; the lobby's passes
    /// 1 in its second argument). Sets `0x54e7c0` to 1.
    LEVEL_SELECT_NEW @ 0x002c_4ea0: [fn(screen: ecx, a: stack 0, lobby: stack 1) -> eax ret 8]; "docs/game/multiple-campaigns.md";
    /// The highest map id the signed-in profiles finished, any difficulty
    /// (EAX, -1 for none).
    PROGRESS_MAX @ 0x0019_0565: [fn() -> eax ret 0]; "docs/game/multiple-campaigns.md";
    /// Fill a screen request (ECX): the player mask, two words and the
    /// loader; `OPEN_SCREEN` and the menus' own opens call it.
    SCREEN_REQUEST @ 0x0014_9f49: [fn(request: ecx, mask: stack 0, a: stack 1, b: stack 2, loader: stack 3) ret 16]; "docs/game/multiple-campaigns.md";
    /// One local player's control update (look input to the player's
    /// action): stdcall, the local user index first of five stack words,
    /// the player's local controller index (player `+0x24`) in EDX. Its
    /// only caller is `0x186ab0` (`0x186b16`), which `0x1870a0` runs for
    /// each bound local user. Reads magnetism friction and adhesion at
    /// `0x18622b` and calls the aim-assist search `0x1a4900` at `0x1861a1`.
    PLAYER_CONTROL_UPDATE @ 0x0018_5be0: [fn(user: stack 0, controller: edx) ret 0x14]; "docs/game/aim-assist.md";
    /// The aim-assist parameters of a unit's held weapons, from the weapon
    /// tags (autoaim and magnetism angles and ranges, zoom applied):
    /// unit in ECX, the 0x24-byte result at ESI (kept to the return), zoom
    /// level on the stack, AL whether there is one. Callers: the local
    /// aim-assist search `0x1a4900` (`0x1a49a1`) and the shot aim
    /// `0x1a4c00` (`0x1a4cbf`, from the barrel fire `0x104150`).
    AIM_ASSIST_PARAMS @ 0x001a_50a0: [fn(unit: ecx, out: esi, zoom: stack 0) -> al ret 4]; "docs/game/aim-assist.md";
}

globals! { GLOBALS;
    /// Pointer to the game globals: active options at `+8`, "map active" at
    /// `+0x1120`, the end latch at `+0x1128`.
    GAME_GLOBALS @ 0x004e_6948; "docs/runtime/level-boot.md";
    /// Pointer to the game-time record; `+8` is the tick count.
    GAME_TIME @ 0x0051_0c54; "docs/game/high-framerate.md";
    /// Pointer to the players datum array (`0x21c`-byte players).
    PLAYERS @ 0x004e_8c24; "docs/live/live-gamestate.md";
    /// Pointer to the objects datum array (`0xc`-byte headers).
    OBJECTS @ 0x004e_0300; "docs/formats/objects.md";
    /// Pointer to the loaded scenario tag (`scnr`): player starting
    /// locations at `+0x100` (count) and `+0x104`, `0x34` each.
    SCENARIO @ 0x004e_0350; "docs/editor/scenario-editor.md";
    /// Holds the guest address of the tag-index header.
    TAG_INDEX @ 0x0054_7f00; "docs/engine/tag_get_checked.md";
    /// Holds the guest address of the 16-byte tag-entry table.
    TAG_ENTRIES @ 0x004e_3b44; "docs/engine/tag_get_checked.md";
    /// The Controls screen's vtable (28 slots).
    CONTROLS_SCREEN_VTABLE @ 0x0045_be30; "docs/formats/ui.md";
    /// The Controls list handler's vtable (22 slots).
    CONTROLS_LIST_VTABLE @ 0x0045_d240; "docs/formats/ui.md";
    /// Pointer to the simulation globals: the mode at `+8` (0 none, 1
    /// local, 2 sync-client, 3 sync-server, 4 dist-client, 5
    /// dist-server), the update number at `+0x28`, the out-of-sync latch
    /// byte at `+0x2c`.
    SIM_GLOBALS @ 0x004c_f77c; "docs/live/live-coop.md";
    /// The simulation's "ended" latch byte.
    SIM_ENDED @ 0x004c_f772; "docs/live/live-coop.md";
    /// The network life cycle's ready byte.
    NETWORK_READY @ 0x0052_7330; "docs/live/live-custom-games.md";
    /// Pointer to the squad (lobby) network session.
    SQUAD_SESSION @ 0x0052_7364; "docs/live/live-custom-games.md";
    /// The base address of the game-state block (`0x3fe000` bytes).
    GAME_STATE_BASE @ 0x004e_6080; "docs/runtime/checkpoints.md";
}

/// The point at `base`, if the table names one.
pub fn by_base(base: u32) -> Option<&'static Point> {
    ALL.iter().copied().find(|p| p.base == base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_address_is_named_once() {
        let mut seen = std::collections::BTreeMap::new();
        for p in ALL {
            assert!(seen.insert(p.base, p.name).is_none(), "{} twice", p.name);
            assert!(p.doc.starts_with("docs/"), "{}: doc {}", p.name, p.doc);
        }
        for g in GLOBALS {
            assert!(seen.insert(g.base, g.name).is_none(), "{} twice", g.name);
        }
    }

    #[test]
    fn the_macro_reads_signatures_and_flags() {
        assert_eq!(
            GAME_TICK.sig,
            Sig::Fn {
                args: &[],
                ret: None,
                pop: 0
            }
        );
        assert_eq!(GAME_ACTIVATE.sig.arg("options"), Some(Loc::Eax));
        assert_eq!(WEAPON_FIRE.sig.arg("barrel"), Some(Loc::Stack(1)));
        assert_eq!(WEAPON_FIRE.class, Class::Optional);
        assert!(LOCALE_FIND.hot && !GAME_TICK.hot);
        assert!(PLAYER_KILLED.sig.is_opaque());
        assert_eq!(by_base(0x0013_7fe0).map(|p| p.name), Some("GAME_TICK"));
    }
}
