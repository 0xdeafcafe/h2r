//! Game variants as self-contained bundles, in the game
//! (docs/game/rules-engine.md, "Live game variants in the game";
//! docs/game/community-gametypes.md, "The merged block"):
//!
//! - [`store`]: the local store of bundles (imported `.h2variant` files, the
//!   signed-in player's Live variants, ported saves) and the scripts they
//!   name, by full hash with a prefix index;
//! - [`inject`]: the store's variants in the game's variant list, beside the
//!   retail and community built-ins, and their block put on the game at its
//!   start;
//! - [`port`]: the player's saved variants ported into the store at boot;
//! - [`suggest`]: the map variants on this machine a bundle may suggest,
//!   written by [`store::Store::set_maps`];
//! - [`GameScripts`]: what the rules runtime asks for a blob the binary does
//!   not have: the store first, then Live by full hash.
//!
//! [`install`] opens the store at boot, ports the saves off the game thread
//! and adds the start hook ([`FEATURE`]). [`refresh_live`] (from the
//! variant list's load) asks Live for the player's variants and their
//! scripts on a thread of its own; the next load lists them.

pub mod bundle;
pub mod inject;
pub mod port;
pub mod store;
pub mod suggest;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use content::Sha256;

pub use store::Store;

/// Our guest page for the store variants' `(setting, value)` pairs (the
/// community variants' page is `community_variants::DATA_BASE`, the variant
/// options' `variant_options::BASE`).
pub const PAIRS_BASE: u32 = 0xa0fd_8000;
pub const PAIRS_SIZE: u32 = 0x4000;
/// The feature id of the start hook.
pub const FEATURE: &str = "game-variants";
/// How long the list of the player's Live variants may take.
pub const LIST_WAIT: Duration = Duration::from_secs(5);
/// How long a script fetch at menu load may take.
pub const PREFETCH_WAIT: Duration = Duration::from_secs(5);
/// How long a host's game start waits for a script from Live (as a guest's
/// start waits for its host's options, `variant_ext::HOST_WAIT`).
pub const START_WAIT: Duration = Duration::from_millis(1500);

static STORE: OnceLock<Arc<Mutex<Store>>> = OnceLock::new();
static LIVE: OnceLock<Arc<live_hle::Hle>> = OnceLock::new();
static REFRESHING: AtomicBool = AtomicBool::new(false);

/// The run's store: the one [`install`] opened, else an empty one in
/// memory.
pub fn store() -> Arc<Mutex<Store>> {
    Arc::clone(STORE.get_or_init(|| Arc::new(Mutex::new(Store::in_memory()))))
}

fn hex(bytes: &[u8]) -> String {
    bundle::hex(bytes)
}

/// What [`install`] needs.
pub struct Boot {
    /// The data root (`<data>`): the store is `<data>/game-variants`, the
    /// saves `<data>/disk/Partition1/UDATA/4D530064`.
    pub data_root: PathBuf,
    pub live: Option<Arc<live_hle::Hle>>,
}

/// Opens the store, ports the saved variants on a thread of its own, and
/// adds the start hook. Says why when the hook is left out.
pub fn install(registry: &mut hooks::Registry, boot: Boot) -> anyhow::Result<bool> {
    let root = boot.data_root.join(store::DIR);
    let (opened, lines) = Store::open(&root);
    for line in lines {
        eprintln!("{line}");
    }
    eprintln!(
        "game variants: {} in the store at {}",
        opened.entries().len(),
        root.display()
    );
    let shared = Arc::new(Mutex::new(opened));
    if STORE.set(Arc::clone(&shared)).is_err() {
        crate::say_once("game variants: a store was already open; this one is not used".into());
    }
    if let Some(h) = &boot.live {
        let _ = LIVE.set(Arc::clone(h));
    }
    let saves = variants::save::SaveStore::new(
        variants::save::SaveStore::root_under(&boot.data_root),
        None,
    );
    // Preserve a save's named script, or attach the retail type's built-in.
    let script = port::PortScript::Builtin;
    let to = store();
    let _ = std::thread::Builder::new()
        .name("h2-game-variants-port".into())
        .spawn(move || {
            // Port into a store of our own, then add what it made: the
            // shared store is never held over disk work.
            let (mut scratch, _) = Store::open(&root);
            let report = port::port_saves(&mut scratch, &saves, &script);
            eprintln!("{}", report.line());
            let mut s = to.lock().unwrap_or_else(|e| e.into_inner());
            for e in scratch.entries() {
                if e.origin == store::Origin::Ported {
                    let _ = s.add(e.origin, e.bundle.clone());
                }
            }
        });
    registry
        .add(&StartHook)
        .map_err(|e| anyhow::anyhow!("{FEATURE}: {e}"))
}

/// Asks Live for the signed-in player's variants and the scripts they name
/// that the store lacks, on a thread of its own; one at a time. Offline or
/// signed out it logs why and the store keeps what it had.
pub fn refresh_live() {
    let Some(hle) = LIVE.get().cloned() else {
        return;
    };
    if REFRESHING.swap(true, Ordering::SeqCst) {
        return;
    }
    let to = store();
    let spawned = std::thread::Builder::new()
        .name("h2-game-variants-live".into())
        .spawn(move || {
            refresh_from(
                &to,
                |w| hle.my_game_variants(w),
                |h, w| hle.fetch_rules_ir(h, w),
            );
            REFRESHING.store(false, Ordering::SeqCst);
        });
    if spawned.is_err() {
        REFRESHING.store(false, Ordering::SeqCst);
    }
}

/// [`refresh_live`]'s body over any list and fetch (tests pass fakes).
/// Never holds the store over a network wait.
pub fn refresh_from(
    to: &Mutex<Store>,
    list: impl Fn(
        Duration,
    ) -> Result<
        Vec<live_hle::game_variants::LiveVariant>,
        live_hle::game_variants::FetchError,
    >,
    fetch: impl Fn(&str, Duration) -> Result<Vec<u8>, live_hle::game_variants::FetchError>,
) {
    let variants = match list(LIST_WAIT) {
        Ok(v) => v,
        Err(why) => {
            eprintln!("game variants: no Live variants this time: {why}");
            return;
        }
    };
    let lock = || to.lock().unwrap_or_else(|e| e.into_inner());
    let mut added = 0;
    for v in &variants {
        // The script first, so the bundle is saved with its IR.
        if let Some(h) = v.script_hash.as_deref()
            && let Ok(hash) = h.parse::<Sha256>()
        {
            let mut p = [0; 8];
            p.copy_from_slice(&hash.0[..8]);
            if lock().script(p).is_none() {
                match fetch(h, PREFETCH_WAIT) {
                    Ok(blob) if Sha256::of(&blob) == hash => {
                        if let Err(why) = lock().save_script(blob) {
                            eprintln!("game variants: script {h} not kept: {why}");
                        }
                    }
                    Ok(blob) => eprintln!(
                        "game variants: Live sent script {h} hashing to {}; not kept",
                        Sha256::of(&blob)
                    ),
                    Err(why) => eprintln!("game variants: script {h} of {}: {why}", v.name),
                }
            }
        }
        match lock().put_live(v) {
            Ok(true) => added += 1,
            Ok(false) => {}
            Err(why) => eprintln!("game variants: Live variant {} left out: {why}", v.name),
        }
    }
    eprintln!(
        "game variants: {} Live variants of this player, {added} new in the store",
        variants.len()
    );
}

/// The blobs the rules runtime can have that are not built in: the
/// store's, then Live's by the full hash the store names for the record's
/// prefix (a Live variant's bundle names it). The Live fetch is made with
/// no guest memory held ([`GameScripts::prefetch`], from the start hook
/// before the runtime starts); the runtime only reads its outcome
/// ([`GameScripts::fetched`]), so its start never waits on the network.
pub struct GameScripts {
    pub store: Arc<Mutex<Store>>,
    pub hle: Option<Arc<live_hle::Hle>>,
    /// The last prefetch: its prefix and why it has no blob.
    last: Mutex<Option<([u8; 8], String)>>,
    /// The fetch (tests pass a fake).
    fetch: Box<dyn Fn(&str, Duration) -> Result<Vec<u8>, String> + Send + Sync>,
}

impl GameScripts {
    /// Over the run's store and Live client.
    pub fn in_game(hle: Option<Arc<live_hle::Hle>>) -> Self {
        let h = hle.clone();
        Self::new(
            store(),
            hle,
            Box::new(move |hash: &str, wait: Duration| match &h {
                Some(h) => h.fetch_rules_ir(hash, wait).map_err(|e| e.to_string()),
                None => Err("this machine has no Live client".into()),
            }),
        )
    }

    pub fn new(
        store: Arc<Mutex<Store>>,
        hle: Option<Arc<live_hle::Hle>>,
        fetch: Box<dyn Fn(&str, Duration) -> Result<Vec<u8>, String> + Send + Sync>,
    ) -> Self {
        Self {
            store,
            hle,
            last: Mutex::new(None),
            fetch,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The store's blob whose SHA-256 starts with `prefix`.
    pub fn local(&self, prefix: [u8; 8]) -> Option<Vec<u8>> {
        self.lock().script(prefix).map(<[u8]>::to_vec)
    }

    /// Fetches the blob for `prefix` from Live into the store, waiting at
    /// most `wait`, unless the store has it. Call with no guest memory
    /// held. A failure is kept, by name, for [`GameScripts::fetched`].
    pub fn prefetch(&self, prefix: [u8; 8], wait: Duration) {
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if self.local(prefix).is_some() {
            return;
        }
        let why = match self.fetch_into_store(prefix, wait) {
            Ok(()) => return,
            Err(why) => why,
        };
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some((prefix, why));
    }

    fn fetch_into_store(&self, prefix: [u8; 8], wait: Duration) -> Result<(), String> {
        let hash = self.lock().full_hash(prefix).ok_or_else(|| {
            format!(
                "no built-in script and no game variant on this machine has hash prefix {}, so its full hash is not known",
                hex(&prefix)
            )
        })?;
        let h = hash.to_string();
        let blob =
            (self.fetch)(&h, wait).map_err(|e| format!("fetching script {h} from Live: {e}"))?;
        let got = Sha256::of(&blob);
        if got != hash {
            return Err(format!("the blob Live sent for script {h} hashes to {got}"));
        }
        self.lock()
            .save_script(blob)
            .map(|_| ())
            .map_err(|e| format!("keeping script {h}: {e}"))
    }

    /// The runtime's fetch: the store's blob after a prefetch, else the
    /// prefetch's refusal. Never waits.
    pub fn fetched(&self, prefix: [u8; 8]) -> Result<Vec<u8>, String> {
        if let Some(blob) = self.local(prefix) {
            return Ok(blob);
        }
        match &*self.last.lock().unwrap_or_else(|e| e.into_inner()) {
            Some((p, why)) if *p == prefix => Err(why.clone()),
            _ => Err(format!(
                "no built-in script and no game variant on this machine has hash prefix {}, and this machine did not fetch it (a guest does not)",
                hex(&prefix)
            )),
        }
    }
}

/// The start hook: a variant built from one of the store's list entries
/// gets its bundle's block before the game copies the options
/// ([`inject::apply_block`]), ahead of the options' transport to guests.
pub struct StartHook;

impl hooks::Feature for StartHook {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(
            FEATURE,
            "Game variants from the store: their options and script on the game",
        )
        .doc("docs/game/rules-engine.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use hooks::Flow;
        use hooks::points::GAME_ACTIVATE;
        b.raw()
            .before(&GAME_ACTIVATE, "the store variant's block", move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let options = c.cpu.eax;
                let line = {
                    let mut mem = rt.memory();
                    if crate::skulls::options_mode(&*mem, options)
                        != Some(crate::skulls_mp::MODE_MULTIPLAYER)
                    {
                        return Ok(Flow::Continue);
                    }
                    inject::apply_block(&mut *mem, options)
                };
                if let Some(line) = line {
                    eprintln!("{line}");
                }
                Ok(Flow::Continue)
            })
            .first()
            // Halo 1's scope reset only clears its own vision states, so
            // either order is right; this one names it.
            .after_feature("h1-scope-zoom");
        // The listing the guest's list entries name goes with the guest
        // memory. The store is files on disk and the Live refresh adds to
        // it; neither is the machine's, and a load does not rewind them.
        b.snapshot(hooks::SnapshotPolicy::section(inject::save_listed, |bytes, _| {
            inject::load_listed(bytes)
        }));
        Ok(())
    }
}

#[cfg(all(test, feature = "rules"))]
mod tests;
