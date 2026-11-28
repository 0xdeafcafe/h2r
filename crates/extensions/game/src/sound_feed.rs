//! The sound table's mixer feed at boot (docs/engine/sound-instances.md,
//! "Mixer feed", stage D2): the table's sounds play through DirectSound's mixer, decoded from the running map.
//!
//! [`MapClips`] is the feed's clip source: a `ClipSource` over the running
//! map's gestalt and raw files (`native_map::open`), opened again when the
//! map changes. A map that does not open is named once and refused until
//! the next map; its sounds count as `feed.clip_failed`.

use std::sync::{Arc, Mutex};

use sim_bridge::sim_sound::mixer_feed::{Chunks, Clips, MixerHost};
use sound_clips::{ClipError, ClipKey, ClipSource, GestaltLibrary, Got, OwnedRaw};

/// The running map's clips.
pub struct MapClips {
    tables: Option<Arc<wma::Tables>>,
    /// The map's name and its source.
    open: Option<(String, ClipSource)>,
    /// The map that would not open, and why.
    failed: Option<(String, String)>,
}

impl MapClips {
    pub fn new(tables: Option<Arc<wma::Tables>>) -> Self {
        Self { tables, open: None, failed: None }
    }

    fn source(&mut self) -> Result<&ClipSource, ClipError> {
        let name = crate::native_map::running_name().ok_or_else(|| ClipError::Failed("no map is running".into()))?;
        if let Some((failed, why)) = &self.failed
            && failed.eq_ignore_ascii_case(&name)
        {
            return Err(ClipError::Failed(why.clone()));
        }
        let current = self.open.as_ref().is_some_and(|(n, _)| n.eq_ignore_ascii_case(&name));
        if !current {
            self.open = None;
            match open(&name, self.tables.clone()) {
                Ok(source) => self.open = Some((name, source)),
                Err(why) => {
                    eprintln!("sound feed: {name}: {why}; its sounds are not heard");
                    self.failed = Some((name, why.clone()));
                    return Err(ClipError::Failed(why));
                }
            }
        }
        Ok(&self.open.as_ref().expect("opened above").1)
    }
}

fn open(name: &str, tables: Option<Arc<wma::Tables>>) -> Result<ClipSource, String> {
    let (mut map, files) = crate::native_map::open(name).map_err(|e| e.to_string())?;
    let library = GestaltLibrary::from_map(&mut map, Box::new(OwnedRaw(files))).map_err(|e| e.to_string())?;
    Ok(ClipSource::new(library, tables, ClipSource::DEFAULT_BUDGET))
}

impl Clips for MapClips {
    fn get(&mut self, key: ClipKey) -> Result<Got, ClipError> {
        self.source()?.get(key)
    }
    fn stream(&mut self, key: ClipKey) -> Result<Box<dyn Chunks>, ClipError> {
        let reader = self.source()?.stream(key)?;
        Ok(Box::new(reader))
    }
}

/// Give the sound table the mixer to play into, with the executable's
/// codec tables when they loaded (boot, once the audio is up).
pub fn connect(mixer: Arc<Mutex<audio::Mixer>>, tables: Option<Arc<wma::Tables>>) {
    let host = MixerHost::new(mixer, MapClips::new(tables));
    sim_bridge::sim_sound::entries::set_host(Box::new(host));
}
