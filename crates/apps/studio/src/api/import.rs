//! `import.sound` ([`super::Provider`], registered in [`super::seams`]): a
//! WAV file added to a local package as a `sound` item, over
//! `app::sound_import`. docs/editor/agent-api.md, "Sounds".

use serde_json::Value;
use studio_proto::{code, method as m};

use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::app::assets::CopyRefusal;

/// The provider.
#[derive(Default)]
pub struct Import;

const SOUND: MethodDef = def(
    m::IMPORT_SOUND,
    "Import a WAV into a package as a `sound` item that writes the `snd!` `tag`: the file is copied into the package under sounds/ and the build resamples it (to 22,050, 32,000 or 44,100 Hz, the nearest not below), keeps it mono or stereo, encodes it as Xbox ADPCM, cuts it into chunks and merges its rows into the map's sound gestalt, keeping the rows already there. A 16-bit PCM WAV of one or two channels at any rate. `class` is a sound class name or number (default ui; any other is written to a `<name>.sound.yaml` data item beside the WAV). Answers what the build will write: the rate, the frames, the length and the encoded bytes. Refused for a package that is built in, a tag the package adds already, or a file that is not a 16-bit PCM WAV",
    r#"{"file":"string (a WAV path)","package":"string (a local package's id)","tag":"string (the snd! path, e.g. sounds\\pogo\\boing)","class?":"string|integer (a sound class: weapon_fire, ui, 51, ...)"}"#,
    r#"{"package":"string","tag":"string (tag:snd!:<path>)","source":"string","sidecar":"string|null","item":"integer","dir":"string","class":"integer","from":"{rate,channels,frames}","rate":"integer","frames":"integer","duration_ms":"integer","bytes":"integer"}"#,
    r#"{"file":"$OUT/clip.wav","package":"mine","tag":"sounds\\clip"}"#,
);

impl Provider for Import {
    fn methods(&self) -> Vec<MethodDef> {
        vec![SOUND]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        (method == m::IMPORT_SOUND).then(|| sound(app, &P(params)))
    }
}

fn sound(app: &mut App, p: &P) -> Reply {
    // A class is a name or a number.
    let class = match p.get("class") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(_) => return Err(err(code::INVALID_PARAMS, "class: a name or a number")),
    };
    crate::app::sound_import::import_sound(
        &app.data_root(),
        std::path::Path::new(p.str("file")?),
        p.str("package")?,
        p.str("tag")?,
        class.as_deref(),
    )
    .map_err(|e| match e {
        CopyRefusal::Params(m) => err(code::INVALID_PARAMS, m),
        CopyRefusal::Refused(m) => err(code::REFUSED, m),
        CopyRefusal::Failed(m) => err(code::FAILED, m),
    })
}
