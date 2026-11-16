//! Checks against the user's own disc and XBE (`$H2_XBE`, the ISO beside
//! it or `$H2_ISO`). Ignored: nothing of the disc is in the repository.

use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

fn iso() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("H2_ISO") {
        return Some(p.into());
    }
    let xbe = PathBuf::from(std::env::var("H2_XBE").ok()?);
    Some(xbe.parent()?.join("halo2.iso"))
}

pub fn mainmenu() -> map::Map<BufReader<File>> {
    let path = iso().expect("set H2_XBE or H2_ISO");
    let mut x = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let e = x.find("/maps/mainmenu.map").unwrap();
    let base = x.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
    map::Map::from_region(BufReader::new(File::open(&path).unwrap()), base, e.size.into()).unwrap()
}

#[test]
#[ignore = "needs the user's disc"]
fn dump_goof() {
    let mut m = mainmenu();
    let tag = m.tags().iter().find(|t| t.group == tags::ui::VARIANT_SETTINGS).unwrap().clone();
    let loc = tag.location.unwrap();
    let goof = m.read_file_range(loc.file_offset().unwrap(), loc.size.into()).unwrap();
    println!("goof {} bytes", goof.len());
    for c in goof.chunks(16) { println!("{:02x?}", c); }
    let rd = |m: &mut map::Map<BufReader<File>>, a: u32, s: u32| { let o = m.file_offset(a, s).unwrap(); m.read_file_range(o, s.into()).unwrap() };
    let le = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at+4].try_into().unwrap());
    let pages = rd(&mut m, le(&goof, 0x1c), le(&goof, 0x18) * 0x18);
    for p in pages.chunks(0x18) {
        print!("page {:02x?} :", p);
        let refs = rd(&mut m, le(p, 0xc), le(p, 8) * 8);
        for r in refs.chunks(8) {
            let t = m.tag(map::TagId(le(r, 4))).unwrap().clone();
            let l = t.location.unwrap();
            let s = m.read_file_range(l.file_offset().unwrap(), l.size.into()).unwrap();
            print!(" {}", le(&s, 0));
        }
        println!();
    }
    let first = m.tags().iter().find(|t| t.group == variants::schema::SETTING_DEFINITION).unwrap().clone();
    let l = first.location.unwrap();
    let s = m.read_file_range(l.file_offset().unwrap(), l.size.into()).unwrap();
    println!("sily {:?} {:02x?}", first.name, s);
    let d = rd(&mut m, le(&goof, 0x3c), le(&goof, 0x38) * 0x14);
    for c in d.chunks(0x14) { println!("defaults entry {:02x?}", c); }
}
