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
fn header(id: u64) -> Header {
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
        epoch_100ns: 0,
        lease_id: Some("lease".into()),
        xnet_addr: "127.0.0.1:3074".into(),
        transfers_per_slice: 10,
        lobby: false,
        lobby_machine_start: true,
        host_bots: false,
        telemetry: false,
        live_service: false,
        features: String::new(),
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
            .contains("64 MiB")
    );
}

#[test]
fn upload_posts_the_exact_bytes_with_the_host_bearer() {
    use std::{
        io::{BufRead, BufReader},
        net::TcpListener,
    };
    let dir = Temp::new();
    let path = dir.0.join("host.h2film");
    let bytes = encoded();
    fs::write(&path, &bytes).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(&mut stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "POST /v1/games/a%2Fb/film HTTP/1.1\r\n");
        let mut headers = String::new();
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            headers.push_str(&line.to_ascii_lowercase());
        }
        assert!(headers.contains("authorization: bearer sample\r\n"));
        let mut body = vec![0; bytes.len()];
        reader.read_exact(&mut body).unwrap();
        assert_eq!(body, bytes);
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
            .unwrap();
    });
    upload(&base, "sample", "a/b", &path).unwrap();
    server.join().unwrap();
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
