use super::*;
use crate::tape::{Call, Check, Mark, Playback};
use std::{fs, io::Cursor, path::PathBuf};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("film-unit-{}", live_proto::sign::random_id()));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
pub(super) fn header(id: u64) -> Header {
    Header {
        version: 1,
        meta: FilmMeta {
            id: id.to_string(),
            map: "synthetic".into(),
            variant: "test".into(),
            started_ms: id,
            length_ticks: 30,
            players: vec![Player {
                name: "one".into(),
                team: 0,
                score: 4,
            }],
            source: Source::Host,
            image_stamp: "image A".into(),
        },
        title_id: 1,
        xbe_sha256: "test".into(),
        runtime_stamp: String::new(),
        simulation_hooks: None,
        guest_image: None,
        epoch_100ns: 0,
        lease_id: Some("lease".into()),
        game_id: None,
        xnet_addr: "127.0.0.1:3074".into(),
        transfers_per_slice: 10,
        lobby: false,
        lobby_machine_start: true,
        host_bots: false,
        telemetry: false,
        telemetry_hooks: None,
        render_hooks: None,
        live_service: false,
        features: String::new(),
    }
}
#[test]
fn older_headers_reconstruct_their_historical_observer_plan() {
    use live_proto::film::TelemetryHooks as Hooks;
    let mut h = header(3);
    assert!(
        serde_json::to_value(&h)
            .unwrap()
            .get("telemetry_hooks")
            .is_none()
    );
    for (version, present, expected) in [
        (1, false, Hooks::OFF),
        (1, true, Hooks::EVENTS),
        (2, true, Hooks::EVENTS),
        (3, true, Hooks::UNIFIED),
        (3, false, Hooks::OFF),
    ] {
        h.version = version;
        h.telemetry = present;
        assert_eq!(h.telemetry_plan(), expected);
    }
    // An explicit plan takes precedence over the legacy boolean.
    h.telemetry_hooks = Some(Hooks::EVENTS);
    assert_eq!(h.telemetry_plan(), Hooks::EVENTS);
}

#[test]
fn render_hook_profiles_round_trip_and_require_the_recorders_groups() {
    use RenderHooks::{Desktop, Gpu, Slim};
    let mut h = header(1);
    let old = serde_json::to_value(&h).unwrap();
    assert!(old.get("render_hooks").is_none());
    assert_eq!(serde_json::from_value::<Header>(old).unwrap().render_hooks, None);
    for (index, recording) in [Slim, Desktop, Gpu].into_iter().enumerate() {
        h.render_hooks = Some(recording);
        let bytes = serde_json::to_vec(&h).unwrap();
        assert_eq!(serde_json::from_slice::<Header>(&bytes).unwrap().render_hooks, Some(recording));
        for (player_index, player) in [Slim, Desktop, Gpu].into_iter().enumerate() {
            assert_eq!(player.supports(recording), player_index >= index);
        }
    }
}

fn encoded() -> Vec<u8> {
    let mut w = Writer::new(Cursor::new(Vec::new()), &header(1)).unwrap();
    w.chunk(DATA, &manifest::Manifest::default()).unwrap();
    for (i, (tag, op)) in [(NETR, "Recv"), (RAND, "Random")].into_iter().enumerate() {
        w.chunk(
            tag,
            &vec![Call {
                index: i as u64,
                op: op.into(),
                thread: 7,
                args: vec![2],
                result: Ok(1),
                outputs: vec![vec![42]],
            }],
        )
        .unwrap();
    }
    w.chunk(KEYF, &vec![1, 2, 3]).unwrap();
    w.chunk(
        HASH,
        &Check {
            tick: 30,
            sim: Some(99),
            calls: 2,
        },
    )
    .unwrap();
    w.chunk(
        MARK,
        &Mark::End {
            slices: 100,
            tick: 30,
            calls: 2,
        },
    )
    .unwrap();
    w.finish().unwrap().into_inner()
}
#[test]
fn round_trip_all_chunks_index_and_wrong_stamp() {
    let b = encoded();
    let mut r = Reader::new(Cursor::new(b.clone())).unwrap();
    assert_eq!(r.header, header(1));
    assert!(r.header.meta.playable_here("image A"));
    assert!(!r.header.meta.playable_here("image B"));
    assert!(r.finalized);
    assert_eq!(
        r.entries.iter().map(|e| e.tag).collect::<Vec<_>>(),
        vec![HEAD, DATA, NETR, RAND, KEYF, HASH, MARK]
    );
    for e in &r.entries {
        assert_eq!(b[e.offset as usize..][..4], e.tag);
    }
    let entry = r.entries.iter().find(|e| e.tag == HASH).unwrap().clone();
    assert_eq!(r.read::<Check>(&entry).unwrap().sim, Some(99));
}
#[test]
fn truncated_last_chunk_and_missing_index_rebuild() {
    let b = encoded();
    let r = Reader::new(Cursor::new(&b)).unwrap();
    let last = r.entries.last().unwrap();
    let index = (last.offset + 8 + u64::from(last.size)) as usize;
    for end in index..b.len() {
        let scan = Reader::new(Cursor::new(&b[..end])).unwrap();
        assert!(!scan.finalized);
        assert_eq!(scan.entries, r.entries);
    }
    let mark = last.offset as usize;
    for end in [mark, mark + 3, mark + 8, index - 1] {
        let scan = Reader::new(Cursor::new(&b[..end])).unwrap();
        assert_eq!(scan.entries, r.entries[..r.entries.len() - 1]);
    }
    assert!(Reader::new(Cursor::new(&b[..8])).is_err());
}
#[test]
fn complete_but_damaged_index_is_an_error() {
    let mut b = encoded();
    let n = b.len();
    b[n - 2] ^= 0xff;
    assert!(Reader::new(Cursor::new(b)).is_err());
}
#[test]
fn calls_require_the_same_order_arguments_and_thread() {
    let dir = Temp::new();
    let path = dir.0.join("x");
    fs::write(&path, encoded()).unwrap();
    let tape = || Playback::load(Reader::open(&path).unwrap()).unwrap();
    let mut t = tape();
    assert_eq!(t.call(0, "Recv", &[2], 7).unwrap().outputs, vec![vec![42]]);
    assert_eq!(t.call(1, "Random", &[2], 7).unwrap().result, Ok(1));
    assert!(t.call(2, "Random", &[2], 7).is_err());
    assert!(tape().call(0, "Random", &[2], 7).is_err());
    assert!(tape().call(1, "Recv", &[2], 7).is_err());
    assert!(tape().call(0, "Recv", &[1], 7).is_err());
    assert!(tape().call(0, "Recv", &[2], 8).is_err());
}
#[test]
fn library_keeps_25_recent_plus_pins_by_header_id() {
    let dir = Temp::new();
    let lib = Library::open(&dir.0).unwrap();
    File::create(dir.0.join("unfinished.h2film")).unwrap();
    for i in 0..27 {
        Writer::new(
            File::create(dir.0.join(format!("other-{i}.h2film"))).unwrap(),
            &header(i),
        )
        .unwrap()
        .finish()
        .unwrap();
    }
    assert_eq!(lib.list().unwrap().len(), 25);
    assert_eq!(lib.list().unwrap()[0].meta.id, "26");
    lib.save("0").unwrap();
    lib.save("0").unwrap();
    let reopened = Library::open(&dir.0).unwrap();
    let rows = reopened.list().unwrap();
    assert_eq!(rows.len(), 26);
    assert!(rows.last().unwrap().saved);
    reopened.unsave("0").unwrap();
    reopened.unsave("0").unwrap();
    assert_eq!(reopened.list().unwrap().len(), 25);
    reopened.save("26").unwrap();
    reopened.delete("26").unwrap();
    assert_eq!(reopened.list().unwrap()[0].meta.id, "25");
    assert!(!dir.0.join("other-26.h2film.saved").exists());
    assert!(reopened.delete("../../escape").is_err());
}
#[test]
fn manifest_restores_guest_files_into_a_new_disk() {
    let dir = Temp::new();
    fs::create_dir_all(dir.0.join("disk/U")).unwrap();
    fs::write(dir.0.join("disk/U/profile"), [4, 3, 2, 1]).unwrap();
    fs::write(dir.0.join("accounts.json"), "outside the guest disk").unwrap();
    let snap = manifest::Manifest::capture(&dir.0).unwrap();
    let restored = dir.0.join("restored");
    snap.restore(&restored).unwrap();
    assert_eq!(
        fs::read(restored.join("disk/U/profile")).unwrap(),
        [4, 3, 2, 1]
    );
    assert!(!restored.join("accounts.json").exists());
    assert!(snap.restore(&restored).is_err());
    let bad: manifest::Manifest = serde_json::from_value(serde_json::json!({"entries":[{
        "path":"disk/../../escape","sha256":"","data":null}]}))
    .unwrap();
    assert!(bad.restore(&dir.0.join("bad")).is_err());
}
#[test]
fn upload_rejects_large_files_before_making_a_request() {
    let dir = Temp::new();
    let p = dir.0.join("oversize");
    File::create(&p).unwrap().set_len(MAX_UPLOAD + 1).unwrap();
    assert!(
        upload("http://127.0.0.1", "test", "game", &p)
            .unwrap_err()
            .to_string()
            .contains("2 GiB")
    );
}

// Reply to each observed request independently so a lost connection or a
// continuation is visible on the wire. Dropping the wake-up connection after
// upload returns also fails a script that was still expecting more requests.
fn run_upload(path: &std::path::Path, serve: impl FnOnce(std::net::TcpListener) + Send + 'static) {
    use std::net::{TcpListener, TcpStream};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || serve(listener));
    let result = upload(&format!("http://{addr}"), "sample", "a/b", path);
    drop(TcpStream::connect(addr));
    server.join().unwrap();
    result.unwrap();
}

fn film_request(
    listener: &std::net::TcpListener,
    expected: &str,
) -> (std::net::TcpStream, Vec<u8>) {
    use std::io::{BufRead, BufReader};
    let (stream, _) = listener.accept().unwrap();
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert_eq!(line.trim_end(), format!("{expected} HTTP/1.1"));
    let mut authorised = false;
    let mut continues = false;
    let mut len = None;
    loop {
        line.clear();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        if line == "\r\n" {
            break;
        }
        let (name, value) = line.trim_end().split_once(':').unwrap();
        if name.eq_ignore_ascii_case("authorization") {
            authorised = value.trim() == "Bearer sample";
        }
        if name.eq_ignore_ascii_case("x-h2-film-continue") {
            continues = value.trim() == "1";
        }
        if name.eq_ignore_ascii_case("content-length") {
            len = Some(value.trim().parse::<usize>().unwrap());
        }
    }
    assert!(authorised);
    assert!(continues, "upload must declare continuation support");
    let mut body = vec![0; len.expect("upload must declare its length")];
    reader.read_exact(&mut body).unwrap();
    drop(reader);
    (stream, body)
}

fn film_reply(mut stream: std::net::TcpStream, status: &str) {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
}

#[test]
fn upload_posts_the_exact_bytes_then_advances_without_resending_them() {
    let dir = Temp::new();
    let path = dir.0.join("host.h2film");
    let bytes = encoded();
    fs::write(&path, &bytes).unwrap();
    run_upload(&path, move |listener| {
        let (stream, body) = film_request(&listener, "POST /v1/games/a%2Fb/film");
        assert_eq!(body, bytes);
        film_reply(stream, "202 Accepted");
        for status in [Some("202 Accepted"), None, Some("204 No Content")] {
            let (stream, body) = film_request(&listener, "POST /v1/games/a%2Fb/film?continue=1");
            assert!(body.is_empty());
            // One continuation advances the cursor but loses its response.
            if let Some(status) = status {
                film_reply(stream, status);
            }
        }
    });
}

#[test]
fn multipart_upload_advances_each_part_before_uploading_the_next_and_waits_for_publication() {
    use live_proto::film::{PART_BYTES, SINGLE_UPLOAD_BYTES};
    use sha2::{Digest, Sha256};
    let dir = Temp::new();
    let path = dir.0.join("split.h2film");
    let size = SINGLE_UPLOAD_BYTES + 1;
    File::create(&path).unwrap().set_len(size).unwrap();

    run_upload(&path, move |listener| {
        let mut hash = Sha256::new();
        for part in 0..size.div_ceil(PART_BYTES) {
            let target = format!("PUT /v1/games/a%2Fb/film?part={part}");
            let (stream, body) = film_request(&listener, &target);
            assert_eq!(body.len() as u64, (size - part * PART_BYTES).min(PART_BYTES));
            hash.update(&body);
            film_reply(stream, "202 Accepted");
            let (stream, body) = film_request(&listener, "POST /v1/games/a%2Fb/film?continue=1");
            assert!(body.is_empty());
            film_reply(stream, "204 No Content");
        }
        let complete = format!(
            "POST /v1/games/a%2Fb/film?complete={size}&sha256={:x}",
            hash.finalize()
        );
        let (stream, body) = film_request(&listener, &complete);
        assert!(body.is_empty());
        film_reply(stream, "202 Accepted");
        let (stream, body) = film_request(&listener, "POST /v1/games/a%2Fb/film?continue=1");
        assert!(body.is_empty());
        film_reply(stream, "204 No Content");
    });
}

#[test]
fn referenced_start_files_require_matching_content() {
    let dir = Temp::new();
    let file = dir.0.join("disk/large");
    fs::create_dir(dir.0.join("disk")).unwrap();
    fs::write(&file, vec![42u8; 2 << 20]).unwrap();
    let manifest = manifest::Manifest::capture(&dir.0).unwrap();
    assert!(serde_json::to_vec(&manifest).unwrap().len() < 1024);
    let dest = dir.0.join("play");
    manifest.restore_with(&dest, Some(&dir.0)).unwrap();
    assert_eq!(fs::read(dest.join("disk/large")).unwrap().len(), 2 << 20);
    fs::write(file, [1]).unwrap();
    assert!(
        manifest
            .restore_with(&dir.0.join("again"), Some(&dir.0))
            .is_err()
    );
}

#[test]
fn v2_menu_reads_the_summary_without_decoding_gameplay() {
    let dir = Temp::new();
    let path = dir.0.join("local.h2film");
    let mut head = header(7);
    head.version = 2;
    head.meta.source = Source::Local;
    let summary = blam::Summary {
        level: "ascension".into(),
        mode: blam::Mode::SystemLink,
        game_type: "Slayer".into(),
        variant_name: "Team Slayer".into(),
        duration_ticks: 42,
        ..Default::default()
    };
    let mut w = Writer::new(File::create(&path).unwrap(), &head).unwrap();
    w.chunk_bytes(blam::INPT, b"broken gameplay").unwrap();
    w.chunk(blam::SUMM, &summary).unwrap();
    w.finish().unwrap();
    let entry = Library::open(&dir.0).unwrap().get("7").unwrap();
    assert_eq!(entry.version, 2);
    assert_eq!(entry.summary.as_ref(), Some(&summary));
    assert!(entry.meta.playable_here("image A"));
}

#[test]
fn v2_requires_a_complete_index_and_footer() {
    let mut h = header(0);
    h.version = 2;
    let mut w = Writer::new(Cursor::new(Vec::new()), &h).unwrap();
    w.chunk_bytes(blam::INPT, &[1, 2, 3]).unwrap();
    let bytes = w.finish().unwrap().into_inner();
    for n in [1, 8, 16, 17] {
        assert!(Reader::new(Cursor::new(&bytes[..bytes.len() - n])).is_err());
    }
    let mut corrupt = bytes;
    let pointer = corrupt.len() - 8;
    corrupt[pointer..].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(Reader::new(Cursor::new(corrupt)).is_err());
}

#[test]
fn machine_seek_resumes_at_next_hle_call_and_lobby_boundary() {
    let dir = Temp::new();
    let path = dir.0.join("resume.h2film");
    let mut h = header(0);
    h.version = 2;
    h.meta.length_ticks = 31;
    let mut w = Writer::new(File::create(&path).unwrap(), &h).unwrap();
    let call = |index| Call {
        index,
        op: "Random".into(),
        thread: 1,
        args: vec![4],
        result: Ok(0),
        outputs: vec![vec![index as u8; 4]],
    };
    let lobby = |boundary| Mark::Lobby {
        boundary,
        method: "host.lobby.state".into(),
        params: serde_json::Value::Null,
    };
    w.chunk(DATA, &manifest::Manifest::default()).unwrap();
    w.chunk(RAND, &vec![call(0)]).unwrap();
    w.chunk(MARK, &lobby(9)).unwrap();
    w.chunk(
        HASH,
        &Check {
            tick: 30,
            calls: 1,
            sim: Some(6),
        },
    )
    .unwrap();
    // A failed capture can still have drained some guest push-buffer
    // work. Its barrier is replayed even though it produced no snapshot.
    w.chunk(MARK, &Mark::D3dBarrier { slices: 50, tick: 25, saved: false }).unwrap();
    w.chunk(MARK, &Mark::D3dBarrier { slices: 60, tick: 30, saved: true }).unwrap();
    w.chunk_bytes(KEYF, b"snapshot restored by the machine")
        .unwrap();
    w.chunk(MARK, &lobby(10)).unwrap();
    w.chunk(MARK, &Mark::D3dBarrier { slices: 64, tick: 31, saved: true }).unwrap();
    w.chunk(RAND, &vec![call(1)]).unwrap();
    let size = Call {
        index: 2, op: tape::VIDEO_MENU_SIZE.into(), thread: 0, args: vec![],
        result: Ok((3440 << 16) | 1440), outputs: vec![],
    };
    w.chunk(tape::VSIZ, &[&size]).unwrap();
    w.chunk(
        MARK,
        &Mark::End {
            slices: 70,
            tick: 31,
            calls: 3,
        },
    )
    .unwrap();
    w.finish().unwrap();
    let mut linear = Playback::load(Reader::open(&path).unwrap()).unwrap();
    assert_eq!(linear.d3d_barrier(48, 24).unwrap(), None);
    assert!(linear.d3d_barrier(50, 24).is_err());
    assert_eq!(linear.d3d_barrier(50, 25).unwrap(), Some(false));
    assert_eq!(linear.d3d_barrier(60, 30).unwrap(), Some(true));
    let reader = Reader::open(path).unwrap();
    let offset = reader
        .entries
        .iter()
        .find(|entry| entry.tag == KEYF)
        .unwrap()
        .offset;
    let mut tape = Playback::load(reader).unwrap();
    assert!(tape.resume_keyframe(offset + 1, 30, 1, 9).is_err());
    tape.resume_keyframe(offset, 30, 1, 9).unwrap();
    assert_eq!(tape.d3d_barrier(60, 30).unwrap(), None);
    assert_eq!(tape.d3d_barrier(64, 31).unwrap(), Some(true));
    assert!(tape.checks.is_empty());
    assert_eq!(tape.lobby.pop_front().unwrap().0, 10);
    assert_eq!(tape.call(1, "Random", &[4], 1).unwrap(), call(1));
    assert_eq!(tape.call(2, tape::VIDEO_MENU_SIZE, &[], 0).unwrap(), size);
    assert!(tape.exhausted());
}

#[test]
fn ring_cursor_replays_partial_call_batch_hash_lobby_and_barrier_twice() {
    let dir = Temp::new();
    let path = dir.0.join("ring-cursor.h2film");
    let mut header = header(0);
    header.version = 2;
    let mut writer = Writer::new(File::create(&path).unwrap(), &header).unwrap();
    let call = |index| Call {
        index,
        op: "Random".into(),
        thread: 1,
        args: vec![1],
        result: Ok(0),
        outputs: vec![vec![index as u8]],
    };
    let check = Check { tick: 30, calls: 2, sim: Some(7) };
    writer.chunk(DATA, &manifest::Manifest::default()).unwrap();
    writer.chunk(RAND, &[call(0), call(1)]).unwrap();
    writer.chunk(MARK, &Mark::D3dBarrier { slices: 40, tick: 22, saved: true }).unwrap();
    writer.chunk(MARK, &Mark::Lobby {
        boundary: 15, method: "host.lobby.state".into(), params: serde_json::Value::Null,
    }).unwrap();
    writer.chunk(HASH, &check).unwrap();
    writer.chunk(MARK, &Mark::End { slices: 50, tick: 30, calls: 2 }).unwrap();
    writer.finish().unwrap();

    let mut tape = Playback::load(Reader::open(&path).unwrap()).unwrap();
    assert_eq!(tape.call(0, "Random", &[1], 1).unwrap(), call(0));
    let cursor = tape.cursor();
    for _ in 0..3 {
        assert_eq!(tape.call(1, "Random", &[1], 1).unwrap(), call(1));
        assert_eq!(tape.d3d_barrier(40, 22).unwrap(), Some(true));
        assert_eq!(tape.lobby.pop_front().unwrap().0, 15);
        assert_eq!(tape.checks.pop_front(), Some(check));
        assert!(tape.exhausted());
        tape.resume_cursor(cursor).unwrap();
    }
}

#[test]
fn keyframe_restores_guest_disk_changes_into_a_private_directory() {
    let dir = Temp::new();
    let disk = dir.0.join("disk");
    fs::create_dir(&disk).unwrap();
    fs::write(disk.join("profile"), b"start").unwrap();
    fs::write(disk.join("retired"), b"old").unwrap();
    fs::write(disk.join("unchanged"), b"keep").unwrap();
    let start = manifest::Manifest::capture(&dir.0).unwrap();
    assert!(start.keyframe_delta(&dir.0).unwrap().is_empty());

    fs::write(disk.join("profile"), b"mid-match").unwrap();
    fs::rename(disk.join("retired"), disk.join("renamed")).unwrap();
    let delta = start.keyframe_delta(&dir.0).unwrap();
    let private = dir.0.join("replay");
    start.restore_keyframe(&private, None, &delta).unwrap();
    assert_eq!(
        fs::read(private.join("disk/profile")).unwrap(),
        b"mid-match"
    );
    assert_eq!(fs::read(private.join("disk/unchanged")).unwrap(), b"keep");
    assert_eq!(fs::read(private.join("disk/renamed")).unwrap(), b"old");
    assert!(!private.join("disk/retired").exists());
}


#[test]
fn a_single_chunk_cannot_hide_a_second_zstd_frame() {
    let mut packed = zstd::stream::encode_all(&b"first"[..], 1).unwrap();
    packed.extend(zstd::stream::encode_all(&b""[..], 1).unwrap());
    let size = u32::try_from(packed.len()).unwrap();
    let mut data = b"TICK".to_vec();
    data.extend_from_slice(&size.to_le_bytes());
    data.extend_from_slice(&packed);
    let e = Entry { tag: *b"TICK", offset: 0, size };
    assert!(read_bytes(&mut Cursor::new(data), &e, 256).is_err());
}
