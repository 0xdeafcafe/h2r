use super::{donors, h1_iso, h2_map, scripting};
use h1campaign::{Options, h2assets::Assets};
use h1convert::convert::{Sandbox, halo1, known, structure_mopp};
use h1map::{CacheType, H1Map, disc::Disc};
use h1objects::{base::Base, sandbox::Objects};

#[test]
#[ignore = "reads H1_ISO and H2_DISC and converts both asset sets for every retail level"]
fn both_sandboxes_rebuild_every_retail_map() {
    let mut iso = h1_iso();
    let disc = Disc::open(&mut iso).unwrap();
    let donors = donors();
    let scripts = scripting();
    let mut failed = Vec::new();
    let (mut campaign, mut multiplayer) = (0, 0);
    for entry in &disc.maps {
        let file = iso.read(entry).unwrap();
        let h1 = H1Map::from_bytes(&file).unwrap();
        let name = &h1.header().name;
        match h1.header().cache_type {
            CacheType::Campaign => {
                campaign += 1;
                let shared = h1campaign::Shared::default();
                for assets in [Assets::Halo1, Assets::Halo2] {
                    let mut base = h2_map(h1campaign::base_for(name));
                    let options = Options {
                        assets,
                        scripts: Some(&scripts),
                        donors: Some(&donors),
                        sounds: usize::MAX,
                        shared: Some(&shared),
                        ..Default::default()
                    };
                    if let Err(e) = h1campaign::convert(&h1, &mut base, &options) {
                        failed.push(format!("{name} {assets:?}: {e}"));
                    }
                }
            }
            CacheType::Multiplayer => {
                multiplayer += 1;
                let (_, native) =
                    known(name).unwrap_or_else(|| panic!("no retail base for {name}"));
                let hash = content::Sha256::of(&file);
                for sandbox in [Sandbox::Halo1, Sandbox::Halo2] {
                    let mut base = h2_map(native);
                    let programs = Base::read(&mut base).unwrap();
                    let mut objects = Objects::new(&h1, &programs);
                    let prepared = match sandbox {
                        Sandbox::Halo1 => halo1::prepare_halo1(&h1, hash, &mut base, &mut objects),
                        Sandbox::Halo2 => halo1::prepare_halo2(&h1, hash, &mut base, &mut objects),
                    };
                    let result = prepared.and_then(|p| {
                        let mopp = structure_mopp(&p.collision)?;
                        p.build_fresh(&mut base, &mopp)
                    });
                    if let Err(e) = result {
                        failed.push(format!("{name} {sandbox:?}: {e}"));
                    }
                }
            }
            _ => {}
        }
    }
    assert_eq!((campaign, multiplayer), (10, 13), "retail source maps");
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
