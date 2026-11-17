//! Exploration helper, not a test: print a map's `ustr` and `hmt ` tags
//! through the library readers, and the scenario's text references.
//!
//! ```sh
//! H1_ISO=disc/halo1.iso cargo run -p h1text --example dump -- a30 [group]
//! ```

use h1map::{Group, H1Map, disc};
use h1text::h1::{Hmt, MessagePart, Ustr, scenario_text};
use std::io::Cursor;

fn main() {
    let map_name = std::env::args().nth(1).unwrap_or("a30".into());
    let want = std::env::args().nth(2).and_then(|g| Group::parse(&g));
    let path = std::env::var("H1_ISO").unwrap_or_else(|_| "../../disc/halo1.iso".into());
    let mut iso = xiso::Xiso::open(std::io::BufReader::new(
        std::fs::File::open(&path).expect("open the Halo 1 disc (H1_ISO)"),
    ))
    .unwrap();
    let d = disc::Disc::open(&mut iso).unwrap();
    let entry = d
        .map(&map_name)
        .unwrap_or_else(|| panic!("{map_name} not on the disc"));
    let bytes = iso.read(entry).unwrap();
    let m = H1Map::from_reader(Cursor::new(&bytes)).unwrap();

    match scenario_text(&m) {
        Ok(text) => eprintln!(
            "scenario: names {:?}, help {:?}, hud {:?}",
            text.object_names.map(|r| m.tag(r.id).unwrap().name.clone()),
            text.help_text.map(|r| m.tag(r.id).unwrap().name.clone()),
            text.hud_messages.map(|r| m.tag(r.id).unwrap().name.clone()),
        ),
        Err(e) => eprintln!("scenario text fields: {e}"),
    }

    for t in m.tags() {
        let texty = t.group == Group::new(b"ustr") || t.group == Group::new(b"hmt ");
        if let Some(w) = want {
            if t.group != w {
                continue;
            }
        } else if !texty {
            continue;
        }
        if t.group == Group::new(b"ustr") {
            let ustr = Ustr::read(&m, t).unwrap();
            eprintln!("== ustr {} ({} strings)", t.name, ustr.strings.len());
            for (i, s) in ustr.strings.iter().enumerate() {
                eprintln!("  [{i}] {}", s.chars().take(60).collect::<String>());
            }
        } else {
            let hmt = Hmt::read(&m, t).unwrap();
            eprintln!("== hmt  {} ({} messages)", t.name, hmt.messages.len());
            for message in &hmt.messages {
                let parts: String = message
                    .parts
                    .iter()
                    .map(|p| match p {
                        MessagePart::Text(t) => t.clone(),
                        MessagePart::Glyph(g) => format!("<{g}>"),
                    })
                    .collect();
                eprintln!(
                    "  {} = {}",
                    message.name,
                    parts.chars().take(70).collect::<String>()
                );
            }
        }
    }
}
