use super::*;

/// A sealed film file of `game` with `ticks` ticks, in the layout
/// `film::file` writes. Its tick chunks, tail checkpoints and game start
/// are not zstd frames: an ingest that inflated one would fail.
pub(crate) fn build(game: &str, ticks: u32, game_start: bool) -> Vec<u8> {
    let seed = serde_json::to_vec(&json!({
        "header": {"meta": {"id": format!("film-of-{game}")}, "game_id": game},
        "descriptor": {},
    }))
    .unwrap();
    let seed = ruzstd::encoding::compress_to_vec(
        &seed[..],
        ruzstd::encoding::CompressionLevel::Uncompressed,
    );
    let mut out = vec![0; HEADER];
    out.extend_from_slice(&seed);
    let block = |out: &mut Vec<u8>, kind: u8, payload: &[u8]| {
        let at = out.len() as u64;
        out.push(kind);
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        at
    };
    if game_start {
        block(&mut out, KIND_GAME, b"a game start, never inflated");
    }
    let mut chunks = Vec::new();
    for i in 0..ticks.div_ceil(256) {
        block(
            &mut out,
            KIND_TAIL,
            format!("checkpoint before chunk {i}").as_bytes(),
        );
        chunks.push(block(
            &mut out,
            KIND_TICKS,
            format!("tick chunk {i}, never inflated").as_bytes(),
        ));
    }
    let trailer = out.len() as u64;
    out.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
    for at in chunks {
        out.extend_from_slice(&at.to_le_bytes());
    }
    let flags = FLAG_SEALED | if game_start { FLAG_GAME_START } else { 0 };
    out[..8].copy_from_slice(MAGIC);
    out[8..10].copy_from_slice(&VERSION.to_le_bytes());
    out[10..12].copy_from_slice(&flags.to_le_bytes());
    out[12] = 30;
    out[13] = 16;
    out[14..16].copy_from_slice(&256u16.to_le_bytes());
    out[16..32].copy_from_slice(&header_id(&format!("film-of-{game}")));
    out[32..36].copy_from_slice(&ticks.to_le_bytes());
    out[36..40].copy_from_slice(&(seed.len() as u32).to_le_bytes());
    out[40..48].copy_from_slice(&trailer.to_le_bytes());
    out
}

/// The counting double: how many times, and on what, the ingest inflates.
#[derive(Default)]
struct Counting {
    inflated: Vec<usize>,
}
impl Inflate for Counting {
    fn inflate(&mut self, packed: &[u8], limit: usize) -> ApiResult<Vec<u8>> {
        self.inflated.push(packed.len());
        Zstd.inflate(packed, limit)
    }
}

fn seed_len(film: &[u8]) -> usize {
    u32::from_le_bytes(film[36..40].try_into().unwrap()) as usize
}

#[tokio::test]
async fn a_film_file_is_stored_byte_for_byte_and_only_its_seed_is_inflated() {
    let film = build("game-a", 3 * 256 - 10, true);
    let store = BlobStore::memory();
    let mut inflate = Counting::default();
    let checked = accept(
        &store,
        "input-films/game-a/x.h2f",
        "game-a",
        film.clone().into(),
        &mut inflate,
    )
    .await
    .unwrap();
    // Three tick chunks, three tail checkpoints and a game start; one
    // inflation, of the seed.
    assert_eq!(inflate.inflated, [seed_len(&film)]);
    assert_eq!(
        checked,
        Checked {
            size: film.len() as u64,
            sha256: format!("{:x}", sha2::Sha256::digest(&film)),
            ticks: 3 * 256 - 10,
        }
    );
    let stored = store
        .get("input-films/game-a/x.h2f")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.as_ref(), film.as_slice());
}

#[test]
fn pieces_of_any_size_walk_to_the_same_result() {
    let film = build("game-b", 5 * 256, false);
    let whole = {
        let mut scan = Scan::default();
        scan.feed("game-b", &film, &mut Zstd).unwrap();
        scan.finish(film.len() as u64).unwrap()
    };
    let first = HEADER + seed_len(&film);
    for step in [1, 2, 3, 4, 5, 7, 9, 13, 64] {
        let mut inflate = Counting::default();
        let mut scan = Scan::default();
        scan.feed("game-b", &film[..first], &mut inflate).unwrap();
        for piece in film[first..].chunks(step) {
            // A part's cursor goes through JSON between requests.
            scan = serde_json::from_slice(&serde_json::to_vec(&scan).unwrap()).unwrap();
            scan.feed("game-b", piece, &mut inflate).unwrap();
        }
        assert_eq!(
            scan.finish(film.len() as u64).unwrap(),
            whole,
            "pieces of {step}"
        );
        assert_eq!(inflate.inflated.len(), 1);
    }
}

#[test]
fn a_damaged_or_foreign_film_file_is_refused() {
    let good = build("game-c", 600, true);
    let check = |film: &[u8], game: &str| {
        let mut scan = Scan::default();
        scan.feed(game, film, &mut Zstd)
            .and_then(|()| scan.finish(film.len() as u64))
    };
    assert!(check(&good, "game-c").is_ok());
    assert!(check(&good, "game-d").is_err(), "another game's film");
    let trailer = u64::from_le_bytes(good[40..48].try_into().unwrap()) as usize;
    let damage: [(&str, Box<dyn Fn(&mut Vec<u8>)>); 8] = [
        (
            "unsealed",
            Box::new(|f: &mut Vec<u8>| f[10] &= !(FLAG_SEALED as u8)),
        ),
        ("an unknown flag", Box::new(|f: &mut Vec<u8>| f[10] |= 1)),
        ("a dictionary", Box::new(|f: &mut Vec<u8>| f[60] = 1)),
        (
            "another film's seed",
            Box::new(|f: &mut Vec<u8>| f[20] ^= 1),
        ),
        (
            "a miscounted trailer",
            Box::new(move |f: &mut Vec<u8>| f[trailer] += 1),
        ),
        (
            "a listed chunk moved",
            Box::new(move |f: &mut Vec<u8>| f[trailer + 4] ^= 1),
        ),
        (
            "cut short",
            Box::new(|f: &mut Vec<u8>| {
                f.pop();
            }),
        ),
        (
            "a block of unknown kind",
            Box::new(|f: &mut Vec<u8>| {
                let at = HEADER + seed_len(f);
                f[at] = 9;
            }),
        ),
    ];
    for (what, damage) in damage {
        let mut film = good.clone();
        damage(&mut film);
        assert!(check(&film, "game-c").is_err(), "{what}");
    }
    // A game start in a film whose header says it has none.
    let mut film = good.clone();
    film[10] &= !(FLAG_GAME_START as u8);
    assert!(check(&film, "game-c").is_err());
    // A block of kind 1, the call tape's old envelope, is of no known kind.
    let mut film = good.clone();
    let at = HEADER + seed_len(&film);
    assert_eq!(film[at], KIND_GAME);
    film[at] = 1;
    assert!(check(&film, "game-c").is_err());
}
