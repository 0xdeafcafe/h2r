//! The save and load orders through the agent, over fake parts: what the
//! boot loop does between slices when a `state.*` job is due.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, sync_channel};
use std::time::Duration;

use agent_proto::state::method as state;
use agent_proto::{Request, code};
use serde_json::{Value, json};

use super::*;

type Log = Rc<RefCell<Vec<String>>>;

/// A part whose whole state is one number, saved under its own name.
struct Fake {
    name: &'static str,
    value: Rc<Cell<u64>>,
    log: Log,
    busy: Rc<RefCell<Vec<String>>>,
    /// Added to the value on load (a load that writes guest RAM).
    skew: u64,
}

impl Part for Fake {
    fn name(&self) -> &'static str {
        self.name
    }
    fn blockers(&self) -> Vec<String> {
        self.busy.borrow().clone()
    }
    fn save(&mut self, out: &mut SnapshotWriter) -> Result<(), String> {
        self.log.borrow_mut().push(format!("save {}", self.name));
        let mut w = Writer::new();
        w.u64(self.value.get());
        out.section(Tag::new(self.name), 1, Codec::Stored, w.into_bytes())
            .map(drop)
            .map_err(|e| e.to_string())
    }
    fn load(&mut self, snapshot: &Snapshot<'_>) -> Result<(), String> {
        self.log.borrow_mut().push(format!("load {}", self.name));
        let bytes = snapshot
            .require(Tag::new(self.name), 1..=1)
            .map_err(|e| e.to_string())?;
        let mut r = Reader::new(&bytes, "fake");
        self.value
            .set(r.u64().map_err(|e| e.to_string())? + self.skew);
        Ok(())
    }
}

struct World {
    log: Log,
    /// Every part's value, by name; `guest` is what the RAM hash reads.
    values: Vec<(&'static str, Rc<Cell<u64>>)>,
    busy: Rc<RefCell<Vec<String>>>,
    shelf: Shelf,
    root: PathBuf,
}

impl World {
    fn new(test: &str) -> World {
        let root =
            std::env::temp_dir().join(format!("h2-snapshot-glue-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let build = BuildKey {
            stamp: "xbe 0 generator 0".into(),
            switches: "switches ; features agent; host guest".into(),
        };
        World {
            log: Rc::default(),
            values: SAVE_ORDER
                .iter()
                .map(|n| (*n, Rc::new(Cell::new(0))))
                .collect(),
            busy: Rc::default(),
            shelf: Shelf::new(&root, build),
            root,
        }
    }

    fn value(&self, name: &str) -> &Cell<u64> {
        &self.values.iter().find(|(n, _)| *n == name).unwrap().1
    }

    /// The parts, listed in neither order, and a RAM hash over `guest`.
    fn machine(&self, guest_skew: u64) -> BootMachine<'_> {
        let mut parts: Vec<Box<dyn Part + '_>> = Vec::new();
        for (name, value) in self.values.iter().rev() {
            parts.push(Box::new(Fake {
                name,
                value: Rc::clone(value),
                log: Rc::clone(&self.log),
                busy: if *name == "dsound" {
                    Rc::clone(&self.busy)
                } else {
                    Rc::default()
                },
                skew: if *name == "guest" { guest_skew } else { 0 },
            }));
        }
        let guest = Rc::clone(&self.values.iter().find(|(n, _)| *n == "guest").unwrap().1);
        BootMachine {
            parts,
            checks: Box::new(Vec::new),
            shelf: self.shelf.clone(),
            hash: Box::new(move || {
                let mut chunks = vec![0; lockstep::CHUNKS];
                chunks[3] = guest.get();
                lockstep::RamHash {
                    total: guest.get(),
                    chunks,
                }
            }),
            presents: 77,
            rt: None,
        }
    }

    fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.borrow_mut())
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct Menu;

impl agent::Reader for Menu {
    fn status(&mut self) -> agent::GameStatus {
        agent::GameStatus {
            tick: Some(812),
            map: Some("03a_oldmombasa".into()),
            mode: Some("campaign".into()),
            bsp: Some(1),
        }
    }
    fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
        Ok(Vec::new())
    }
}

fn agent_on(w: &World) -> (agent::Agent, agent::ConnId, Receiver<String>) {
    let a = agent::Agent::new();
    a.set_snapshots(Box::new(w.shelf.clone()));
    let (tx, rx) = sync_channel(64);
    let c = a.connect(tx);
    (a, c, rx)
}

/// One request, answered by the agent between slices and then by the
/// boot's snapshot pass (the game isn't ticking: every pass is a try).
fn call(
    a: &agent::Agent,
    c: agent::ConnId,
    rx: &Receiver<String>,
    m: &mut BootMachine<'_>,
    method: &str,
    params: Value,
) -> Value {
    let r = Request::new(1, method, params);
    a.submit(c, &serde_json::to_string(&r).unwrap());
    for _ in 0..8 {
        a.between_slices(&mut Menu, 77);
        a.run_snapshots(m);
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(50)) {
            return serde_json::from_str(&line).unwrap();
        }
    }
    panic!("no answer to {method}");
}

#[test]
fn a_save_writes_the_parts_in_order_and_a_load_puts_them_back_in_order() {
    let w = World::new("order");
    for (i, (_, v)) in w.values.iter().enumerate() {
        v.set(100 + i as u64);
    }
    let (a, c, rx) = agent_on(&w);
    let mut m = w.machine(0);
    let v = call(&a, c, &rx, &mut m, state::SAVE, json!({"name": "fight"}));
    assert_eq!(v["result"]["name"], "fight", "{v}");
    let want: Vec<String> = SAVE_ORDER.iter().map(|n| format!("save {n}")).collect();
    assert_eq!(w.take_log(), want);
    let mut tags: Vec<&str> = SAVE_ORDER.to_vec();
    tags.push("ramhash");
    assert_eq!(v["result"]["sections"], json!(tags));
    let path = PathBuf::from(v["result"]["path"].as_str().unwrap());
    assert!(path.starts_with(&w.root) && path.is_file(), "{path:?}");

    // The header says when it is from.
    let v = call(&a, c, &rx, &mut m, state::LIST, Value::Null);
    let listed = &v["result"]["snapshots"][0];
    assert_eq!(listed["name"], "fight");
    assert_eq!(listed["tick"], 812);
    assert_eq!(listed["presents"], 77);
    assert_eq!(listed["notes"]["map"], "03a_oldmombasa");
    assert_eq!(listed["notes"]["bsp"], "1");

    // Play on, then load.
    for (_, v) in &w.values {
        v.set(v.get() + 1000);
    }
    let v = call(&a, c, &rx, &mut m, state::LOAD, json!({"name": "fight"}));
    let want: Vec<String> = LOAD_ORDER.iter().map(|n| format!("load {n}")).collect();
    assert_eq!(w.take_log(), want);
    assert_eq!(v["result"]["applied"], json!(LOAD_ORDER));
    assert_eq!(v["result"]["tick"], 812);
    assert_eq!(
        v["result"]["hash"],
        json!({"checked": true, "equal": true}),
        "{v}"
    );
    for (i, (name, v)) in w.values.iter().enumerate() {
        assert_eq!(v.get(), 100 + i as u64, "{name}");
    }
    // A second load of the same file is the same moment.
    let v = call(&a, c, &rx, &mut m, state::LOAD, json!({"name": "fight"}));
    assert_eq!(v["result"]["hash"]["equal"], true);

    // Saving over it needs `replace`; dropping removes the file.
    let v = call(&a, c, &rx, &mut m, state::SAVE, json!({"name": "fight"}));
    assert_eq!(v["error"]["code"], code::INVALID_PARAMS);
    let v = call(&a, c, &rx, &mut m, state::DROP, json!({"name": "fight"}));
    assert_eq!(v["result"]["name"], "fight");
    assert!(!path.exists());
}

/// A feature whose after hooks give its chain a return shadow.
struct Ticker;

impl hooks::Feature for Ticker {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new("ticker", "test")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        b.frame().on_tick("count", |_| {});
        Ok(())
    }
}

#[test]
fn a_deferred_after_hooks_continuation_names_its_point() {
    let mut registry = hooks::Registry::new(hooks::Switches::defaults());
    registry.add(&Ticker).unwrap();
    let plan = registry.build(&hooks::Base).unwrap();
    let chain = plan.chains.values().next().unwrap();
    let shadow = chain.shadow.expect("an after hook needs a shadow");
    let blocker = kernel::quiesce::Blocker::Continuation {
        thread: 2,
        origin: hooks::install::AFTER_HOOKS,
        target: shadow,
    };
    let text = kernel_blocker(&blocker, &plan);
    assert!(
        text.starts_with(&format!(
            "thread 2: continuation feature: after hooks -> {shadow:#010x} (after hooks on "
        )),
        "{text}"
    );
    assert!(text.contains("ticker:count"), "{text}");
    // Any other origin, and any other blocker, stays as the kernel wrote it.
    assert_eq!(
        kernel_blocker(
            &kernel::quiesce::Blocker::Continuation {
                thread: 2,
                origin: "kernel: DPC return",
                target: shadow,
            },
            &plan
        ),
        format!("thread 2: continuation kernel: DPC return -> {shadow:#010x}")
    );
    assert_eq!(
        kernel_blocker(&kernel::quiesce::Blocker::Stopped, &plan),
        "machine stopped"
    );
}

#[test]
fn a_v4_kernel_keyframe_is_refused_before_the_live_and_guest_parts_run() {
    const STACK: u32 = 0x10000;
    const SIZE: u32 = 0x10000;
    let mut guest = runtime::Guest::new();
    guest
        .map_zeroed(STACK, SIZE, runtime::guest::Permissions::RW)
        .unwrap();
    let rt = runtime::Runtime::new(guest, runtime::Dispatch::default());
    let kernel = kernel::Kernel::install(&rt, kernel::Config::default()).unwrap();
    let cpu = runtime::CpuAbi {
        esp: STACK + SIZE - 4,
        ..runtime::CpuAbi::xbox_thread()
    };
    let mut machine = kernel::Machine::new(rt, kernel, cpu, STACK, STACK + SIZE).unwrap();
    let rebuilders = kernel::snapshot::Rebuilders::new();
    let no_hooks = hooks::Registry::new(hooks::Switches::defaults())
        .build(&hooks::Base)
        .unwrap();
    let w = World::new("kcore-version");
    let mut parts = w.machine(0).parts;
    parts.retain(|part| part.name() == "guest");
    parts.push(Box::new(KernelPart {
        m: &mut machine,
        rebuilders: &rebuilders,
        plan: &no_hooks,
        prepared: None,
    }));
    let mut out = SnapshotWriter::new(Header {
        build: w.shelf.build().clone(),
        name: "keyframe".into(),
        tick: 1,
        presents: 1,
        notes: Vec::new(),
    });
    out.section(Tag::new("guest"), 1, Codec::Stored, 99u64.to_le_bytes().to_vec())
        .unwrap();
    out.section(kernel::snapshot::KCORE, 5, Codec::Zstd, vec![0; 32])
        .unwrap();
    let bytes = out.finish().unwrap();
    let snap = Snapshot::open(&bytes, w.shelf.build()).unwrap();
    let error = load_parts_with_base(&mut parts, &snap, None, None).unwrap_err();
    assert!(error.contains("kcore has version 5"), "{error}");
    assert!(w.take_log().is_empty());
    assert_eq!(w.value("guest").get(), 0);
}

#[test]
fn a_load_that_changes_ram_says_which_mib() {
    let w = World::new("hash");
    w.value("guest").set(5);
    let (a, c, rx) = agent_on(&w);
    let mut m = w.machine(1);
    call(&a, c, &rx, &mut m, state::SAVE, json!({"name": "a"}));
    let v = call(&a, c, &rx, &mut m, state::LOAD, json!({"name": "a"}));
    assert_eq!(
        v["result"]["hash"],
        json!({"checked": true, "equal": false, "differing_mib": [3]}),
        "{v}"
    );
}

#[test]
fn blockers_are_named_by_owner_and_a_save_waits_them_out() {
    let w = World::new("busy");
    w.busy.borrow_mut().push("a buffer is locked".into());
    let (a, c, rx) = agent_on(&w);
    let mut m = w.machine(0);
    m.checks = Box::new(|| vec![("bink", "1 movie streams open".to_owned())]);
    let v = call(
        &a,
        c,
        &rx,
        &mut m,
        state::SAVE,
        json!({"name": "a", "max_ticks": 2}),
    );
    assert_eq!(v["error"]["code"], code::BUSY, "{v}");
    assert_eq!(
        v["error"]["data"]["blockers"],
        json!(["dsound: a buffer is locked", "bink: 1 movie streams open"])
    );
    assert!(w.take_log().is_empty(), "nothing saved");
    assert!(!w.shelf.store().dir().join("a.h2snap").exists());

    // Clear by the next try: saved, with the retries counted.
    m.checks = Box::new(Vec::new);
    let r = Request::new(2, state::SAVE, json!({"name": "a"}));
    a.submit(c, &serde_json::to_string(&r).unwrap());
    a.between_slices(&mut Menu, 0);
    a.run_snapshots(&mut m);
    w.busy.borrow_mut().clear();
    a.run_snapshots(&mut m);
    let line = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["result"]["retries"], 1, "{v}");
}

#[test]
fn a_blocker_that_appears_during_the_save_throws_the_file_away() {
    let w = World::new("late");
    let (a, c, rx) = agent_on(&w);
    let mut m = w.machine(0);
    let asked = Rc::new(Cell::new(0));
    let n = Rc::clone(&asked);
    m.checks = Box::new(move || {
        n.set(n.get() + 1);
        if n.get() > 1 {
            vec![("coop", "hosting or joined to a co-op session".to_owned())]
        } else {
            Vec::new()
        }
    });
    let v = call(&a, c, &rx, &mut m, state::SAVE, json!({"name": "a"}));
    assert_eq!(v["error"]["code"], code::BUSY, "{v}");
    assert!(v["error"]["message"].as_str().unwrap().contains("co-op"));
    assert!(!w.shelf.store().dir().join("a.h2snap").exists());
}

#[test]
fn parts_must_have_a_place_in_the_order() {
    let w = World::new("names");
    let mut parts: Vec<Box<dyn Part + '_>> = Vec::new();
    parts.push(Box::new(Fake {
        name: "movies",
        value: Rc::default(),
        log: Rc::clone(&w.log),
        busy: Rc::default(),
        skew: 0,
    }));
    let header = Header {
        build: w.shelf.build().clone(),
        name: "x".into(),
        tick: 0,
        presents: 0,
        notes: Vec::new(),
    };
    let hash = || lockstep::RamHash {
        total: 0,
        chunks: Vec::new(),
    };
    let e = save_parts(&mut parts, header, &hash).unwrap_err();
    assert!(e.contains("movies"), "{e}");
    assert!(w.take_log().is_empty());
}

#[test]
fn the_build_key_follows_switches_and_host_parts() {
    let plan = hooks::Registry::new(hooks::Switches::default())
        .build(&hooks::Base)
        .unwrap();
    let key = |switches: &str, host: &[&str]| build_key("s".into(), switches, &plan, host);
    assert_eq!(key("", &["guest"]), key("", &["guest"]));
    assert_ne!(key("", &["guest"]).id(), key("", &["guest", "dsound"]).id());
    assert_ne!(key("a=on", &["guest"]).id(), key("", &["guest"]).id());
}
