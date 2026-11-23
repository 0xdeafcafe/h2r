//! Image generation without a generator: the fake source, the fit to a
//! slot's format, the job lifecycle, and the Codex CLI driven through
//! stand-in scripts (missing, signed out, out of allowance, a picture
//! made). No test runs the real Codex.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tags::PixelFormat as F;

use super::codex::{self, CodexCli, Events};
use super::fake::{FakeSource, SIDE};
use super::fit::{Alpha, fit};
use super::*;

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-gen-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An executable shell script in `dir` named `codex` running `body`.
#[cfg(unix)]
pub(crate) fn script(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("codex");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn solid(w: u32, h: u32, p: [u8; 4]) -> Vec<u8> {
    p.repeat((w * h) as usize)
}

// ---- fit ------------------------------------------------------------------

#[test]
fn a_32_bit_slot_of_the_same_size_keeps_every_pixel() {
    let src: Vec<u8> = (0..4 * 4 * 4).map(|i| (i * 7) as u8).collect();
    let f = fit(&src, 4, 4, 4, 4, Some(F::Argb8888), Alpha::Auto).unwrap();
    assert_eq!(f.rgba, src);
    assert_eq!((f.width, f.height), (4, 4));
    assert_eq!(f.alpha, Alpha::Image);
}

#[test]
fn a_picture_is_resampled_to_the_slot() {
    let f = fit(&solid(64, 64, [255, 0, 255, 255]), 64, 64, 8, 8, None, Alpha::Auto).unwrap();
    assert_eq!((f.width, f.height), (8, 8));
    assert_eq!(f.rgba, solid(8, 8, [255, 0, 255, 255]));
}

#[test]
fn an_opaque_picture_in_an_alpha_only_slot_takes_its_brightness() {
    // White on black: the white becomes the cut-out.
    let mut src = solid(2, 1, [0, 0, 0, 255]);
    src[4..8].copy_from_slice(&[255, 255, 255, 255]);
    let f = fit(&src, 2, 1, 2, 1, Some(F::A8), Alpha::Auto).unwrap();
    assert_eq!(f.alpha, Alpha::Luma);
    assert_eq!(f.rgba, [255, 255, 255, 0, 255, 255, 255, 255]);
    // A picture with its own alpha keeps it.
    let src = [9, 9, 9, 100, 9, 9, 9, 200];
    let f = fit(&src, 2, 1, 2, 1, Some(F::A8), Alpha::Auto).unwrap();
    assert_eq!(f.alpha, Alpha::Image);
    assert_eq!(f.rgba, [255, 255, 255, 100, 255, 255, 255, 200]);
    // Asked opaque.
    let f = fit(&src, 2, 1, 2, 1, Some(F::Argb8888), Alpha::Opaque).unwrap();
    assert_eq!(f.rgba, [9, 9, 9, 255, 9, 9, 9, 255]);
}

#[test]
fn sixteen_bit_formats_round_as_the_encoder_and_decoder_do() {
    let src = [100, 100, 100, 127, 100, 100, 100, 128];
    // 1555: one bit of alpha at half, 5-bit colour (100 -> 12 -> 99).
    let f = fit(&src, 2, 1, 2, 1, Some(F::Argb1555), Alpha::Image).unwrap();
    assert_eq!(f.rgba, [99, 99, 99, 0, 99, 99, 99, 255]);
    // 565: no alpha; green has 6 bits (100 -> 25 -> 101).
    let f = fit(&src, 2, 1, 2, 1, Some(F::Rgb565), Alpha::Image).unwrap();
    assert_eq!(&f.rgba[..4], &[99, 101, 99, 255]);
    // 4444: 4 bits each (100 -> 6 -> 102, 127 -> 7 -> 119).
    let f = fit(&src, 2, 1, 2, 1, Some(F::Argb4444), Alpha::Image).unwrap();
    assert_eq!(&f.rgba[..4], &[102, 102, 102, 119]);
    // Grey formats take the luma.
    let f = fit(&[255, 0, 0, 255], 1, 1, 1, 1, Some(F::Y8), Alpha::Image).unwrap();
    assert_eq!(f.rgba, [77, 77, 77, 255]);
    let f = fit(&[255, 0, 0, 90], 1, 1, 1, 1, Some(F::A8Y8), Alpha::Image).unwrap();
    assert_eq!(f.rgba, [77, 77, 77, 90]);
}

#[test]
fn dxt_slots_hold_what_the_blocks_decode_to() {
    // A gradient with cut-out texels in a 6 by 5 slot: edge blocks too.
    let (w, h) = (6u32, 5u32);
    let mut src = Vec::new();
    for y in 0..h {
        for x in 0..w {
            src.extend_from_slice(&[(x * 40) as u8, (y * 50) as u8, 128, if x == 0 { 0 } else { 255 }]);
        }
    }
    for (format, kind) in [
        (F::Dxt1, gfx::dxt::BcKind::Bc1),
        (F::Dxt3, gfx::dxt::BcKind::Bc2),
        (F::Dxt5, gfx::dxt::BcKind::Bc3),
    ] {
        let f = fit(&src, w, h, w, h, Some(format), Alpha::Image).unwrap();
        // The first block, encoded and decoded by hand.
        let mut block = [[0u8; 4]; 16];
        for (i, t) in block.iter_mut().enumerate() {
            let (x, y) = ((i % 4) as u32, (i / 4) as u32);
            let o = ((y * w + x) * 4) as usize;
            t.copy_from_slice(&src[o..o + 4]);
        }
        let back = gfx::dxt::decode_block(kind, &assets::encode::encode_block(kind, &block));
        for (i, t) in back.iter().enumerate() {
            let (x, y) = ((i % 4) as u32, (i / 4) as u32);
            let o = ((y * w + x) * 4) as usize;
            assert_eq!(&f.rgba[o..o + 4], t, "{format:?} texel {x},{y}");
        }
        // The cut-out column stays cut out.
        assert_eq!(f.rgba[3], 0, "{format:?}");
    }
}

#[test]
fn a_picture_of_another_aspect_is_centred_on_transparent_pixels() {
    let f = fit(&solid(8, 4, [255, 0, 0, 255]), 8, 4, 4, 4, None, Alpha::Image).unwrap();
    // 4 by 2 of picture in rows 1 and 2; rows 0 and 3 clear.
    for x in 0..4 {
        assert_eq!(f.rgba[x * 4 + 3], 0);
        assert_eq!(f.rgba[(4 + x) * 4..(4 + x) * 4 + 4], [255, 0, 0, 255]);
        assert_eq!(f.rgba[(12 + x) * 4 + 3], 0);
    }
    // Within a tenth it stretches.
    let f = fit(&solid(41, 40, [0, 255, 0, 255]), 41, 40, 40, 40, None, Alpha::Image).unwrap();
    assert!(f.rgba.chunks_exact(4).all(|p| p == [0, 255, 0, 255]));
}

#[test]
fn a_format_that_is_not_a_picture_is_refused() {
    let e = fit(&solid(1, 1, [0; 4]), 1, 1, 1, 1, Some(F::P8), Alpha::Auto).unwrap_err();
    assert!(e.contains("P8"), "{e}");
    assert!(fit(&[0; 3], 1, 1, 1, 1, None, Alpha::Auto).is_err());
}

// ---- the prompt -----------------------------------------------------------

fn slot(format: tags::PixelFormat) -> Slot {
    Slot {
        tag: map::TagId(0xe100_0001),
        bitmap: "ui\\global_bitmaps\\icons".into(),
        image: 3,
        width: 40,
        height: 40,
        format,
    }
}

fn spec(prompt: &str, refs: usize, slot: Option<Slot>) -> Spec {
    Spec {
        prompt: prompt.into(),
        references: (0..refs)
            .map(|i| Reference {
                path: PathBuf::from(format!("/r{i}.png")),
                origin: RefOrigin::File,
            })
            .collect(),
        slot,
        size: None,
        alpha: Alpha::Auto,
        out: None,
        from: None,
    }
}

#[test]
fn the_prompt_carries_the_slot_and_the_references() {
    let p = compose_prompt(&spec("a radar dish", 3, Some(slot(F::Dxt5))));
    assert!(p.starts_with("a radar dish"));
    assert!(p.contains("The 3 attached images are style references"));
    assert!(p.contains("40 by 40 pixels"));
    assert!(p.contains("transparent background"));
    // An opaque format asks for no transparency; no references, no line.
    let p = compose_prompt(&spec("a radar dish", 0, Some(slot(F::Rgb565))));
    assert!(!p.contains("transparent"));
    assert!(!p.contains("reference"));
    let p = compose_prompt(&spec("a radar dish", 1, None));
    assert!(p.contains("The attached image is a style reference"));
}

// ---- the fake source and the jobs -----------------------------------------

#[test]
fn the_fake_draws_the_same_picture_for_the_same_prompt() {
    let t = Temp::new("fake");
    let f = FakeSource::new();
    let req = GenRequest {
        prompt: "a star".into(),
        references: Vec::new(),
        size: Some((4, 4)),
        work_dir: t.0.clone(),
    };
    let made = f.generate(&req).unwrap();
    assert_eq!(made.generator, "fake");
    let (w, h, rgba) = bitmap_edit::decode_png(&std::fs::read(&made.png).unwrap()).unwrap();
    assert_eq!((w, h), (SIDE, SIDE));
    assert_eq!(rgba, FakeSource::picture("a star"));
    assert_ne!(FakeSource::picture("a star"), FakeSource::picture("a moon"));
    // Corners are clear, the middle is the disc.
    assert_eq!(rgba[3], 0);
    assert_eq!(rgba[((32 * SIDE + 32) * 4 + 3) as usize], 255);
    assert_eq!(f.requests.lock().unwrap().len(), 1);
    // A reference that is not there is refused.
    let mut bad = req.clone();
    bad.references.push(t.path("nope.png"));
    assert!(matches!(f.generate(&bad), Err(GenError::Failed(_))));
}

fn jobs(t: &Temp, source: Arc<dyn ImageSource>) -> ImageGen {
    ImageGen::new(source, t.path("jobs"))
}

#[test]
fn a_job_runs_then_is_done_with_its_pictures_and_provenance() {
    let t = Temp::new("lifecycle");
    let (source, go) = FakeSource::gated();
    let mut g = jobs(&t, Arc::new(source));
    let id = g.start(spec("a star", 0, Some(slot(F::Argb8888)))).unwrap();
    assert_eq!(id, 1);
    assert!(g.busy());
    assert_eq!(g.job(id).unwrap().state_name(), "running");
    assert_eq!(g.job(id).unwrap().report()["state"], "running");
    // One at a time.
    let e = g.start(spec("another", 0, None)).unwrap_err();
    assert!(e.contains("still being generated"), "{e}");
    go.send(()).unwrap();
    assert!(g.wait(id, Duration::from_secs(20)));
    let job = g.job(id).unwrap();
    assert_eq!(job.state_name(), "done");
    let o = job.outcome().unwrap();
    assert_eq!(o.raw_size, (SIDE, SIDE));
    assert_eq!(o.fitted_size, (40, 40));
    assert!(o.raw.is_file() && o.fitted.is_file());
    assert!(o.raw.starts_with(&job.dir));
    let prov: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(job.dir.join("provenance.json")).unwrap())
            .unwrap();
    assert_eq!(prov["prompt"], "a star");
    assert_eq!(prov["generator"], "fake");
    assert_eq!(prov["slot"]["image"], 3);
    assert_eq!(prov["accepted"], false);
    // The next one runs now; an empty prompt never starts.
    assert!(g.start(spec("  ", 0, None)).is_err());
    let next = g.start(spec("a moon", 0, None)).unwrap();
    assert_eq!(next, 2);
    assert!(g.wait(next, Duration::from_secs(20)));
}

#[test]
fn a_failed_job_keeps_the_sources_reason() {
    let t = Temp::new("failed");
    let source = FakeSource::failing(GenError::Unauthenticated("sign in".into()));
    let mut g = jobs(&t, Arc::new(source));
    let id = g.start(spec("a star", 0, None)).unwrap();
    assert!(g.wait(id, Duration::from_secs(20)));
    let r = g.job(id).unwrap().report();
    assert_eq!(r["state"], "failed");
    assert_eq!(r["error"], "unauthenticated");
    assert_eq!(r["why"], "sign in");
}

#[test]
fn only_the_newest_jobs_are_kept() {
    let t = Temp::new("keep");
    let mut g = jobs(&t, Arc::new(FakeSource::new()));
    for i in 0..KEEP + 2 {
        let id = g.start(spec(&format!("p{i}"), 0, None)).unwrap();
        assert!(g.wait(id, Duration::from_secs(20)));
    }
    assert_eq!(g.jobs().len(), KEEP);
    assert!(g.job(1).is_none());
    assert!(g.job((KEEP + 2) as u64).is_some());
}

// ---- the Codex CLI ----------------------------------------------------------

/// The events of a real `codex exec --json` run that hit the account's
/// limit (2026-11-23, codex-cli 0.170.0).
const QUOTA_RUN: &str = r#"{"type":"thread.started","thread_id":"01a208eb-5838-7771-896a-29760aba20b7"}
{"type":"turn.started"}
{"type":"error","message":"You’ve hit your usage limit. Visit https://chatgpt.com/settings/usage to purchase more credits or try again at Nov 27th, 2026 1:44 AM."}
{"type":"turn.failed","error":{"message":"You’ve hit your usage limit. Visit https://chatgpt.com/settings/usage to purchase more credits or try again at Nov 27th, 2026 1:44 AM."}}
"#;

#[test]
fn a_runs_events_name_its_thread_and_its_failure() {
    let ev = codex::parse_events(QUOTA_RUN);
    assert_eq!(ev.thread_id.as_deref(), Some("01a208eb-5838-7771-896a-29760aba20b7"));
    assert_eq!(ev.errors.len(), 1);
    assert!(matches!(codex::classify(&ev.errors[0]), GenError::Quota(_)));
    assert!(ev.saved.is_empty());
    // A picture's path and the last message, wherever an event has them.
    let ok = r#"not json
{"type":"item.completed","item":{"type":"image_generation","saved_path":"/h/.codex/generated_images/t/exec-1.png","revised_prompt":"a star, flat"}}
{"type":"item.completed","item":{"type":"agent_message","text":"/w/out.png"}}"#;
    let ev = codex::parse_events(ok);
    assert_eq!(ev.saved, [PathBuf::from("/h/.codex/generated_images/t/exec-1.png")]);
    assert_eq!(ev.last_message.as_deref(), Some("/w/out.png"));
    assert_eq!(ev.revised_prompt.as_deref(), Some("a star, flat"));
}

#[test]
fn failures_are_sorted_by_what_the_user_does_next() {
    assert!(matches!(codex::classify("401 Unauthorized"), GenError::Unauthenticated(_)));
    assert!(matches!(
        codex::classify("Your refresh token has expired; please log in again"),
        GenError::Unauthenticated(_)
    ));
    assert!(matches!(codex::classify("rate limit reached"), GenError::Quota(_)));
    assert!(matches!(codex::classify("stream disconnected"), GenError::Failed(_)));
    let e = codex::classify("401 Unauthorized");
    assert!(e.message().contains("codex login"), "{e}");
}

#[test]
fn the_exec_call_puts_the_images_first_and_the_prompt_on_stdin() {
    let req = GenRequest {
        prompt: "a star".into(),
        references: vec![PathBuf::from("/a.png"), PathBuf::from("/b.png")],
        size: Some((40, 40)),
        work_dir: PathBuf::from("/job"),
    };
    let a = codex::exec_args(&req, None);
    assert_eq!(&a[..5], ["exec", "--image", "/a.png", "--image", "/b.png"]);
    // A flag follows the last image, so nothing more is read as one.
    assert!(a[5].starts_with("--"));
    assert!(a.windows(2).any(|w| w == ["--cd", "/job"]));
    assert!(a.windows(2).any(|w| w == ["--sandbox", "workspace-write"]));
    assert!(a.contains(&"--json".to_owned()));
    assert!(!a.contains(&"a star".to_owned()));
    let i = codex::instructions(&req);
    assert!(i.contains("a star"));
    assert!(i.contains(codex::OUT_FILE));
    assert!(i.contains("square"));
}

#[test]
fn the_picture_is_found_in_the_job_then_in_codexs_folder() {
    let t = Temp::new("collect");
    let png = bitmap_edit::encode_png(1, 1, &[1, 2, 3, 4]).unwrap();
    let home = t.path("home");
    let thread = home.join("generated_images").join("t1");
    std::fs::create_dir_all(&thread).unwrap();
    std::fs::write(thread.join("exec-1.png"), &png).unwrap();
    let cli = CodexCli {
        home: Some(home.clone()),
        ..CodexCli::default()
    };
    let work = t.path("work");
    std::fs::create_dir_all(&work).unwrap();
    let ev = Events {
        thread_id: Some("t1".into()),
        ..Events::default()
    };
    assert_eq!(cli.collect(&work, &ev), Some(thread.join("exec-1.png")));
    std::fs::write(work.join(codex::OUT_FILE), &png).unwrap();
    assert_eq!(cli.collect(&work, &ev), Some(work.join(codex::OUT_FILE)));
    // Nothing anywhere: none.
    let empty = t.path("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert_eq!(cli.collect(&empty, &Events::default()), None);
}

#[test]
fn a_missing_codex_is_named_with_how_to_get_it() {
    let t = Temp::new("missing");
    let cli = CodexCli {
        program: t.path("nowhere/codex"),
        ..CodexCli::default()
    };
    let req = GenRequest {
        prompt: "x".into(),
        references: Vec::new(),
        size: None,
        work_dir: t.0.clone(),
    };
    let e = cli.generate(&req).unwrap_err();
    assert_eq!(e.kind(), "missing");
    assert!(e.message().contains("codex login"), "{e}");
}

#[cfg(unix)]
#[test]
fn a_signed_out_codex_is_named_before_any_run() {
    let t = Temp::new("signed-out");
    let program = script(
        &t.path("bin"),
        "case \"$1\" in --version) echo 'codex-cli 0.170.0';; login) echo 'Not logged in'; exit 1;; exec) touch ran;; esac",
    );
    let cli = CodexCli {
        program,
        ..CodexCli::default()
    };
    let req = GenRequest {
        prompt: "x".into(),
        references: Vec::new(),
        size: None,
        work_dir: t.0.clone(),
    };
    let e = cli.generate(&req).unwrap_err();
    assert_eq!(e.kind(), "unauthenticated");
    assert!(e.message().contains("Not logged in"), "{e}");
    assert!(!t.path("ran").exists());
}

#[cfg(unix)]
#[test]
fn a_codex_run_is_driven_and_its_picture_taken() {
    let t = Temp::new("run");
    let made = t.path("made.png");
    std::fs::write(&made, bitmap_edit::encode_png(2, 2, &[9; 16]).unwrap()).unwrap();
    // The stand-in keeps its args and stdin, copies a picture to
    // ./out.png and says which thread it was.
    let program = script(
        &t.path("bin"),
        &format!(
            "case \"$1\" in --version) echo 'codex-cli 0.170.0';; login) echo 'Logged in using ChatGPT';; exec) echo \"$@\" > args.txt; cat > prompt.txt; cp '{}' ./out.png; echo '{{\"type\":\"thread.started\",\"thread_id\":\"t9\"}}';; esac",
            made.display()
        ),
    );
    let work = t.path("job");
    std::fs::create_dir_all(&work).unwrap();
    let reference = t.path("ref.png");
    std::fs::write(&reference, bitmap_edit::encode_png(1, 1, &[0; 4]).unwrap()).unwrap();
    let cli = CodexCli {
        program,
        home: Some(t.path("home")),
        ..CodexCli::default()
    };
    let req = GenRequest {
        prompt: "a star".into(),
        references: vec![reference.clone()],
        size: Some((40, 40)),
        work_dir: work.clone(),
    };
    let g = cli.generate(&req).unwrap();
    assert_eq!(g.png, work.join(codex::OUT_FILE));
    assert_eq!(g.generator, "codex-cli 0.170.0 (ChatGPT image generation)");
    let args = std::fs::read_to_string(work.join("args.txt")).unwrap();
    assert!(args.starts_with(&format!("exec --image {}", reference.display())), "{args}");
    let prompt = std::fs::read_to_string(work.join("prompt.txt")).unwrap();
    assert!(prompt.contains("a star"));
    assert!(prompt.contains("image generation tool"));
    // The events are kept beside the job.
    assert!(work.join("codex-events.jsonl").is_file());
}

#[cfg(unix)]
#[test]
fn a_codex_run_out_of_allowance_says_so() {
    let t = Temp::new("quota");
    std::fs::write(t.path("events.jsonl"), QUOTA_RUN).unwrap();
    let program = script(
        &t.path("bin"),
        &format!(
            "case \"$1\" in --version) echo 'codex-cli 0.170.0';; login) echo 'Logged in using ChatGPT';; exec) cat > /dev/null; cat '{}'; exit 1;; esac",
            t.path("events.jsonl").display()
        ),
    );
    let cli = CodexCli {
        program,
        home: Some(t.path("home")),
        ..CodexCli::default()
    };
    let work = t.path("job");
    std::fs::create_dir_all(&work).unwrap();
    let req = GenRequest {
        prompt: "a star".into(),
        references: Vec::new(),
        size: None,
        work_dir: work,
    };
    let e = cli.generate(&req).unwrap_err();
    assert_eq!(e.kind(), "quota");
    assert!(e.message().contains("usage limit"), "{e}");
}
