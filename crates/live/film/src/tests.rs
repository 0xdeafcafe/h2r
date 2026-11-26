use super::*;
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
pub(crate) fn header(id: u64) -> Header {
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
        inputs_in_film: false,
        live_service: false,
        features: String::new(),
        player_features: false,
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

/// A private chunk of the container's own: any JSON, under any tag.
const NOTE: Tag = *b"NOTE";
const DONE: Tag = *b"DONE";

fn encoded() -> Vec<u8> {
    let mut w = Writer::new(Cursor::new(Vec::new()), &header(1)).unwrap();
    w.chunk(NOTE, &vec![42, 1]).unwrap();
    w.chunk(KEYF, &vec![1, 2, 3]).unwrap();
    w.chunk(NOTE, &99).unwrap();
    w.chunk(DONE, &30).unwrap();
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
        vec![HEAD, NOTE, KEYF, NOTE, DONE]
    );
    for e in &r.entries {
        assert_eq!(b[e.offset as usize..][..4], e.tag);
    }
    let entry = r.entries[3].clone();
    assert_eq!(r.read::<u64>(&entry).unwrap(), 99);
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
    let mut menus = header(100);
    menus.meta.source = Source::Local;
    menus.meta.map.clear();
    menus.meta.players.clear();
    Writer::new(File::create(dir.0.join("menus.h2film")).unwrap(), &menus)
        .unwrap().finish().unwrap();
    let rows = reopened.list().unwrap();
    assert_eq!(rows.len(), 26, "25 recent games plus the pin");
    assert!(rows.iter().all(LibraryEntry::is_game));
    assert_eq!(rows.last().unwrap().meta.id, "1");
    reopened.delete("26").unwrap();
    assert_eq!(reopened.list().unwrap()[0].meta.id, "25");
    assert!(!dir.0.join("other-26.h2film.saved").exists());
    assert!(reopened.delete("../../escape").is_err());
}
#[test]
fn upload_rejects_large_files_before_making_a_request() {
    let dir = Temp::new();
    let p = dir.0.join("oversize.h2film");
    File::create(crate::file::stats_of(&p)).unwrap().set_len(MAX_UPLOAD + 1).unwrap();
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
    // A stats container with no film file beside it.
    fs::write(crate::file::stats_of(&path), &bytes).unwrap();
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
fn a_film_file_follows_its_stats_container_as_its_own_object() {
    let dir = Temp::new();
    let path = dir.0.join("host.h2film");
    let stats = encoded();
    fs::write(crate::file::stats_of(&path), &stats).unwrap();
    let film = b"H2FILM4! stands in for the film file".to_vec();
    fs::write(&path, &film).unwrap();
    run_upload(&path, move |listener| {
        let (stream, body) = film_request(&listener, "POST /v1/games/a%2Fb/film");
        assert_eq!(body, stats);
        film_reply(stream, "204 No Content");
        let (stream, body) = film_request(&listener, "POST /v1/games/a%2Fb/film?object=film");
        assert_eq!(body, film);
        film_reply(stream, "204 No Content");
    });
}

#[test]
fn multipart_upload_advances_each_part_before_uploading_the_next_and_waits_for_publication() {
    use live_proto::film::{PART_BYTES, SINGLE_UPLOAD_BYTES};
    use sha2::{Digest, Sha256};
    let dir = Temp::new();
    let path = dir.0.join("split.h2film");
    let size = SINGLE_UPLOAD_BYTES + 1;
    File::create(crate::file::stats_of(&path)).unwrap().set_len(size).unwrap();

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

/// A v3 film whose boot `HEAD` has another ID than its final one is
/// refused.
#[test]
fn a_film_whose_boot_header_has_another_id_is_refused() {
    use blam::{MatchHeader, Summary, Tick, TrackWriter};
    let mut boot = header(7);
    boot.version = 3;
    let mut w = Writer::new(Cursor::new(Vec::new()), &boot).unwrap();
    let mut track = TrackWriter::default();
    track
        .push(
            &mut w,
            Tick {
                tick: 1,
                inputs: vec![],
                events: vec![],
                closing_events: vec![],
                post_hash: None,
                forge: vec![],
                firefight: vec![],
            },
        )
        .unwrap();
    // Forge a final header with another ID.
    let mut other = boot.clone();
    other.meta.id = "8".into();
    w.ranges.as_mut().unwrap().front.id = "8".into();
    w.chunk(HEAD, &other).unwrap();
    track.finish(&mut w, &MatchHeader::default(), &Summary::default()).unwrap();
    let bytes = w.finish().unwrap().into_inner();
    let error = Reader::new(Cursor::new(bytes)).err().unwrap().to_string();
    assert!(error.contains("headers differ"), "{error}");
}
