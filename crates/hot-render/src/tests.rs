use super::*;
use std::path::PathBuf;

/// A fresh scratch directory under $TMPDIR.
fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "hot-render-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const ID: u64 = 0x0123_4567_89ab_cdef;

/// A whole (synthetic) program, with `body` as the fragment's colour.
fn program(body: &str) -> String {
    format!(
        "struct V {{ @builtin(position) p: vec4f, @location(0) d0: vec4f }}
@vertex fn vs_main(@location(0) pos: vec4f) -> V {{ var o: V; o.p = pos; o.d0 = vec4f(1.0); return o; }}
@fragment fn fs_main(v: V) -> @location(0) vec4f {{ return {body}; }}
"
    )
}

#[test]
fn file_names_name_programs() {
    assert_eq!(program_of_file("0123456789abcdef.wgsl"), Some(ID));
    assert_eq!(program_of_file("0123456789abcdef_water.wgsl"), Some(ID));
    assert_eq!(program_of_file("0123456789abcdefg.wgsl"), None);
    assert_eq!(program_of_file("0123456789abcde.wgsl"), None);
    assert_eq!(program_of_file("0123456789abcdef.txt"), None);
    assert_eq!(parse_program("0x0123456789abcdef"), Some(ID));
    assert_eq!(program_text(ID), "0123456789abcdef");
}

#[test]
fn reload_loads_reports_and_bumps() {
    let dir = scratch("reload");
    let hub = Hub::new();
    assert!(!hub.active());
    assert_eq!(hub.source(ID), None);
    hub.set_dir(Some(dir.clone()));
    let g0 = hub.generation();
    std::fs::write(
        dir.join("0123456789abcdef_red.wgsl"),
        program("vec4f(1.0, 0.0, 0.0, 1.0)"),
    )
    .unwrap();
    std::fs::write(dir.join("notes.wgsl"), "x").unwrap();
    std::fs::write(dir.join("00000000000000aa.wgsl"), "fn broken(").unwrap();
    let r = hub.reload();
    assert_eq!(r.loaded, vec![program_text(ID)]);
    assert_eq!(r.changed, vec![program_text(ID)]);
    assert_eq!(r.problems.len(), 2, "{:?}", r.problems);
    assert!(r.generation > g0);
    assert!(hub.active());
    let (src, hash) = hub.source(ID).unwrap();
    assert!(src.contains("vec4f(1.0, 0.0, 0.0, 1.0)"));
    assert_ne!(hash, 0);

    // The same files again: nothing changed.
    let r = hub.reload();
    assert!(r.changed.is_empty());
    assert_eq!(hub.source(ID).unwrap().1, hash);

    // Status shows the provider's verdict until the file changes.
    hub.report(ID, Err("no".into()));
    let s = hub.status();
    assert_eq!(
        s.overrides[0].applied,
        Applied::Rejected { error: "no".into() }
    );
    hub.reload();
    assert_eq!(
        hub.status().overrides[0].applied,
        Applied::Rejected { error: "no".into() }
    );
    std::fs::write(
        dir.join("0123456789abcdef_red.wgsl"),
        program("vec4f(0.0, 1.0, 0.0, 1.0)"),
    )
    .unwrap();
    let r = hub.reload();
    assert_eq!(r.changed, vec![program_text(ID)]);
    assert_eq!(hub.status().overrides[0].applied, Applied::Pending);
    assert_ne!(hub.source(ID).unwrap().1, hash);

    // Removing it changes it back.
    std::fs::remove_file(dir.join("0123456789abcdef_red.wgsl")).unwrap();
    let r = hub.reload();
    assert_eq!(r.changed, vec![program_text(ID)]);
    assert!(!hub.active());
    assert_eq!(hub.source(ID), None);
}

#[test]
fn two_files_for_one_program_keep_the_first() {
    let dir = scratch("twice");
    let hub = Hub::new();
    hub.set_dir(Some(dir.clone()));
    std::fs::write(dir.join("0123456789abcdef_a.wgsl"), program("vec4f(1.0)")).unwrap();
    std::fs::write(dir.join("0123456789abcdef_b.wgsl"), program("vec4f(0.0)")).unwrap();
    let r = hub.reload();
    assert_eq!(r.problems.len(), 1);
    assert!(r.problems[0].error.contains("_a.wgsl"), "{:?}", r.problems);
    assert!(hub.source(ID).unwrap().0.contains("vec4f(1.0)"));
}

#[test]
fn the_overrides_toggle_switches_them_all_off() {
    let dir = scratch("toggle");
    let hub = Hub::new();
    hub.set_dir(Some(dir.clone()));
    std::fs::write(dir.join("0123456789abcdef.wgsl"), program("vec4f(1.0)")).unwrap();
    hub.reload();
    let g = hub.generation();
    hub.set_toggle(OVERRIDES, Some(false)).unwrap();
    assert!(hub.generation() > g);
    assert_eq!(hub.source(ID), None);
    hub.set_toggle(OVERRIDES, None).unwrap();
    assert!(hub.source(ID).is_some());
    assert!(hub.set_toggle("nope", Some(true)).is_err());
}

#[test]
fn parameters_come_from_files_and_survive_reloads() {
    let dir = scratch("params");
    let hub = Hub::new();
    hub.set_dir(Some(dir.clone()));
    std::fs::write(
        dir.join("0123456789abcdef.wgsl"),
        program("vec4f({{tint.r=0.5}}, {{tint.g}}, 0.0, 1.0)"),
    )
    .unwrap();
    hub.reload();
    assert_eq!(hub.param("tint.r"), Some(0.5));
    assert_eq!(hub.param("tint.g"), Some(0.0));
    let (src, h1) = hub.source(ID).unwrap();
    assert!(src.contains("vec4f(0.5, 0.0, 0.0, 1.0)"), "{src}");

    let g = hub.generation();
    let p = hub.set_param("tint.r", Some(0.25)).unwrap();
    assert!(p.set);
    assert!(hub.generation() > g, "an override uses it: recompile");
    let (src, h2) = hub.source(ID).unwrap();
    assert!(src.contains("vec4f(0.25, 0.0"), "{src}");
    assert_ne!(h1, h2);
    hub.reload();
    assert_eq!(
        hub.param("tint.r"),
        Some(0.25),
        "set values survive a reload"
    );
    hub.set_param("tint.r", None).unwrap();
    assert_eq!(hub.param("tint.r"), Some(0.5));
    assert!(hub.set_param("tint.r", Some(f32::NAN)).is_err());
    assert!(hub.set_param("nope", Some(1.0)).is_err());
}

#[test]
fn host_parameters_do_not_recompile() {
    let hub = Hub::new();
    hub.declare_param("bloom.gain", 1.0, "bloom pass gain");
    hub.declare_param("bloom.gain", 9.0, "again");
    assert_eq!(hub.param("bloom.gain"), Some(1.0));
    let g = hub.generation();
    hub.set_param("bloom.gain", Some(2.0)).unwrap();
    assert_eq!(hub.generation(), g);
    assert_eq!(hub.param("bloom.gain"), Some(2.0));
    hub.declare_toggle("bloom", true, "bloom pass");
    assert_eq!(hub.toggle("bloom"), Some(true));
    hub.set_toggle("bloom", Some(false)).unwrap();
    assert_eq!(hub.toggle("bloom"), Some(false));
}

#[test]
fn dumps_queue_for_the_provider() {
    let hub = Hub::new();
    assert!(!hub.dumps_pending());
    hub.request_dump(DumpRequest {
        program: Some(ID),
        to: "/x".into(),
        tries: 0,
    });
    assert!(hub.dumps_pending());
    let d = hub.take_dumps();
    assert_eq!(d.len(), 1);
    assert!(!hub.dumps_pending());
    hub.dump_done(DumpReport {
        dir: "/x".into(),
        written: vec!["a".into()],
        errors: vec![],
    });
    assert_eq!(
        hub.status().last_dump.unwrap().written,
        vec!["a".to_string()]
    );
}

#[test]
fn the_watch_debounces_a_change_until_it_holds_still() {
    let a: watch::Signature = vec![("a.wgsl".into(), 1, None)];
    let b: watch::Signature = vec![("a.wgsl".into(), 2, None)];
    let mut seen = a.clone();
    let mut cand = None;
    assert!(!watch::step(a.clone(), &mut seen, &mut cand));
    assert!(
        !watch::step(b.clone(), &mut seen, &mut cand),
        "first sight waits"
    );
    assert!(
        watch::step(b.clone(), &mut seen, &mut cand),
        "held still: reload"
    );
    assert!(!watch::step(b.clone(), &mut seen, &mut cand));
    // A change that keeps changing waits.
    assert!(!watch::step(a.clone(), &mut seen, &mut cand));
    assert!(!watch::step(vec![], &mut seen, &mut cand));
    assert!(watch::step(vec![], &mut seen, &mut cand));
}

#[test]
fn the_watch_thread_reloads_an_edited_file() {
    let dir = scratch("thread");
    let hub: &'static Hub = Box::leak(Box::new(Hub::new()));
    hub.set_dir(Some(dir.clone()));
    hub.reload();
    watch(hub, std::time::Duration::from_millis(20)).unwrap();
    std::fs::write(dir.join("0123456789abcdef.wgsl"), program("vec4f(1.0)")).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while hub.source(ID).is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "the watch never reloaded"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(hub.status().watching);
}
