//! What a player looks like while they fly as a Forge monitor: the model,
//! its colours, its tint and its light (docs/game/forge-monitor-appearance.md).
//!
//! A look is presentation. Nothing here reads guest memory, a GPU or the
//! disc, and nothing here is read by the tick. [`MonitorLook`] owns the
//! value, its bounds and its canonical bytes; [`Refusal`] names every way a
//! look is turned down; [`census`] lists the models a machine can draw;
//! [`saved`] keeps a look in the profile's settings; [`body`] and
//! [`appearance`] place the drawn body in the world and on the game's
//! Appearance page.
pub mod appearance;
pub mod body;
pub mod census;
pub mod monitors;
mod refusal;
pub mod saved;
pub mod session;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use census::Entry;

pub use refusal::{CODES, Refusal};

#[cfg(test)]
mod tests;

/// The look's version. A joiner whose version differs from the host's is
/// refused before the map loads.
pub const VERSION: u32 = 1;

/// The model key of the marker: a ring and the player's name. No disc tag
/// is needed to draw it, so it can never be refused for a missing source.
pub const MARKER: &str = "marker";

/// Palette indices run `0..PALETTE`: the retail player palette's 18
/// colours (`live_telemetry::layout::PALETTE_COLOURS`). Their RGB is read
/// from the loaded map at run time; none is in this repository.
pub const PALETTE: u8 = 18;
/// Tint steps: none, light, medium, strong.
pub const TINT_MAX: u8 = 3;
/// Light steps: off, dim, bright.
pub const LIGHT_MAX: u8 = 2;
/// The longest model key (the art extractor's species keys, `is_id`).
pub const KEY_MAX: usize = 32;
/// The longest variant name.
pub const VARIANT_MAX: usize = 64;

/// The fields a look has, in the order a page lists them. A refusal and a
/// lock name a field by one of these.
pub const FIELDS: [&str; 8] = [
    "version",
    "model",
    "variant",
    "primary",
    "secondary",
    "tint",
    "light",
    "light_colour",
];

/// Whether the build draws a body for a disc model. The marker is always
/// drawn; until the native renderer draws presentation bodies, a look that
/// names a disc model is held and shown as the marker with the reason.
pub const BODIES_DRAWN: bool = false;

/// One value per player, the same shape for every use: the picker, the
/// profile, the network and the film.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorLook {
    pub version: u32,
    /// [`MARKER`], or a census key.
    pub model: String,
    /// One of the model's named variants; `None` is its default.
    pub variant: Option<String>,
    pub primary: u8,
    pub secondary: u8,
    /// 0 none, 1 light, 2 medium, 3 strong: how far the lit colour is mixed
    /// towards primary.
    pub tint: u8,
    /// 0 off, 1 dim, 2 bright.
    pub light: u8,
    pub light_colour: u8,
}

/// A look with some fields given: what `forge.look.set` and a picker send.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Patch {
    pub model: Option<String>,
    /// `Some(None)` clears the variant; `None` leaves it.
    pub variant: Option<Option<String>>,
    pub primary: Option<u8>,
    pub secondary: Option<u8>,
    pub tint: Option<u8>,
    pub light: Option<u8>,
    pub light_colour: Option<u8>,
}

impl Patch {
    pub fn is_empty(&self) -> bool {
        *self == Patch::default()
    }

    /// The fields this patch gives, by name.
    pub fn fields(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        for (name, given) in [
            ("model", self.model.is_some()),
            ("variant", self.variant.is_some()),
            ("primary", self.primary.is_some()),
            ("secondary", self.secondary.is_some()),
            ("tint", self.tint.is_some()),
            ("light", self.light.is_some()),
            ("light_colour", self.light_colour.is_some()),
        ] {
            if given {
                out.push(name);
            }
        }
        out
    }

    /// Reads a patch from a JSON object: any of the look's fields but
    /// `version`. An unknown key, a value of the wrong type or out of its
    /// bounds is refused naming the field. `"variant": null` clears it.
    pub fn from_value(v: &Value) -> Result<Patch, Refusal> {
        let object = v
            .as_object()
            .ok_or_else(|| Refusal::invalid("look", "must be an object"))?;
        let mut patch = Patch::default();
        for (key, value) in object {
            match key.as_str() {
                "model" => patch.model = Some(text(key, value)?),
                "variant" => {
                    patch.variant = Some(if value.is_null() {
                        None
                    } else {
                        Some(text(key, value)?)
                    })
                }
                "primary" => patch.primary = Some(small(key, value, PALETTE - 1)?),
                "secondary" => patch.secondary = Some(small(key, value, PALETTE - 1)?),
                "tint" => patch.tint = Some(small(key, value, TINT_MAX)?),
                "light" => patch.light = Some(small(key, value, LIGHT_MAX)?),
                "light_colour" => patch.light_colour = Some(small(key, value, PALETTE - 1)?),
                other => return Err(Refusal::invalid(other, "not a field of a monitor look")),
            }
        }
        Ok(patch)
    }
}

fn text(field: &str, v: &Value) -> Result<String, Refusal> {
    v.as_str()
        .map(str::to_owned)
        .ok_or_else(|| Refusal::invalid(field, "must be text"))
}

fn small(field: &str, v: &Value, max: u8) -> Result<u8, Refusal> {
    v.as_u64()
        .and_then(|n| u8::try_from(n).ok())
        .filter(|n| *n <= max)
        .ok_or_else(|| Refusal::invalid(field, format!("must be a whole number from 0 to {max}")))
}

/// A model key or variant name is lower-case ASCII letters, digits and
/// underscores, at most `max` bytes.
pub fn name_ok(name: &str, max: usize) -> bool {
    (1..=max).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

impl MonitorLook {
    /// The default look: the marker in the seat's colour, no tint, no light.
    pub fn default_for(seat_colour: u8) -> Self {
        MonitorLook {
            version: VERSION,
            model: MARKER.into(),
            variant: None,
            primary: seat_colour,
            secondary: seat_colour,
            tint: 0,
            light: 0,
            light_colour: seat_colour,
        }
    }

    pub fn is_marker(&self) -> bool {
        self.model == MARKER
    }

    /// Every bound, with no census: the version is this build's, the model
    /// and variant are well-formed names, each index is in range, and the
    /// marker has no variant, tint or light (it draws none).
    pub fn validate(&self) -> Result<(), Refusal> {
        if self.version != VERSION {
            return Err(Refusal::Version {
                theirs: self.version,
                ours: VERSION,
            });
        }
        if !name_ok(&self.model, KEY_MAX) {
            return Err(Refusal::invalid(
                "model",
                format!(
                    "must be {MARKER:?} or a model key of 1 to {KEY_MAX} characters from a-z, 0-9 and _"
                ),
            ));
        }
        if let Some(v) = &self.variant
            && !name_ok(v, VARIANT_MAX)
        {
            return Err(Refusal::invalid(
                "variant",
                format!("must be 1 to {VARIANT_MAX} characters from a-z, 0-9 and _"),
            ));
        }
        for (field, value, max) in [
            ("primary", self.primary, PALETTE - 1),
            ("secondary", self.secondary, PALETTE - 1),
            ("tint", self.tint, TINT_MAX),
            ("light", self.light, LIGHT_MAX),
            ("light_colour", self.light_colour, PALETTE - 1),
        ] {
            if value > max {
                return Err(Refusal::invalid(
                    field,
                    format!("must be a whole number from 0 to {max}"),
                ));
            }
        }
        if self.is_marker() {
            for (field, set) in [
                ("variant", self.variant.is_some()),
                ("tint", self.tint != 0),
                ("light", self.light != 0),
            ] {
                if set {
                    return Err(Refusal::invalid(field, "the marker has none"));
                }
            }
        }
        Ok(())
    }

    /// This look with `patch` laid over it, checked by [`MonitorLook::validate`].
    /// Changing the model drops the variant unless the patch names one, and a
    /// change to the marker clears the tint and light it cannot draw.
    pub fn with(&self, patch: &Patch) -> Result<MonitorLook, Refusal> {
        let mut out = self.clone();
        if let Some(model) = &patch.model
            && *model != out.model
        {
            out.model = model.clone();
            out.variant = None;
            if out.is_marker() {
                out.tint = 0;
                out.light = 0;
            }
        }
        if let Some(variant) = &patch.variant {
            out.variant = variant.clone();
        }
        for (slot, value) in [
            (&mut out.primary, patch.primary),
            (&mut out.secondary, patch.secondary),
            (&mut out.tint, patch.tint),
            (&mut out.light, patch.light),
            (&mut out.light_colour, patch.light_colour),
        ] {
            if let Some(v) = value {
                *slot = v;
            }
        }
        out.validate()?;
        Ok(out)
    }

    /// The look as a JSON object. A variant of `None` is left out.
    pub fn to_value(&self) -> Value {
        let mut o = Map::new();
        o.insert("version".into(), json!(self.version));
        o.insert("model".into(), json!(self.model));
        if let Some(v) = &self.variant {
            o.insert("variant".into(), json!(v));
        }
        o.insert("primary".into(), json!(self.primary));
        o.insert("secondary".into(), json!(self.secondary));
        o.insert("tint".into(), json!(self.tint));
        o.insert("light".into(), json!(self.light));
        o.insert("light_colour".into(), json!(self.light_colour));
        Value::Object(o)
    }

    /// A look from a JSON object with every field but `variant` present.
    /// An unknown key, a missing field or a value out of bounds is refused,
    /// naming the field; the result is [`validate`](MonitorLook::validate)d.
    pub fn from_value(v: &Value) -> Result<MonitorLook, Refusal> {
        let object = v
            .as_object()
            .ok_or_else(|| Refusal::invalid("look", "must be an object"))?;
        if let Some(extra) = object.keys().find(|k| !FIELDS.contains(&k.as_str())) {
            return Err(Refusal::invalid(extra, "not a field of a monitor look"));
        }
        let need = |field: &str| {
            object
                .get(field)
                .ok_or_else(|| Refusal::invalid(field, "is missing"))
        };
        let version = need("version")?
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| Refusal::invalid("version", "must be a whole number"))?;
        if version != VERSION {
            return Err(Refusal::Version {
                theirs: version,
                ours: VERSION,
            });
        }
        let look = MonitorLook {
            version,
            model: text("model", need("model")?)?,
            variant: match object.get("variant") {
                None | Some(Value::Null) => None,
                Some(v) => Some(text("variant", v)?),
            },
            primary: small("primary", need("primary")?, PALETTE - 1)?,
            secondary: small("secondary", need("secondary")?, PALETTE - 1)?,
            tint: small("tint", need("tint")?, TINT_MAX)?,
            light: small("light", need("light")?, LIGHT_MAX)?,
            light_colour: small("light_colour", need("light_colour")?, PALETTE - 1)?,
        };
        look.validate()?;
        Ok(look)
    }

    /// The canonical form: RFC 8785 JSON through `content::canonicalize`,
    /// the form the document uses, after [`validate`](MonitorLook::validate).
    /// Equal looks have equal bytes on every machine.
    pub fn canonical(&self) -> Result<String, Refusal> {
        self.validate()?;
        content::canonicalize(&self.to_value()).map_err(|e| Refusal::invalid("look", e.to_string()))
    }

    /// A look from [`canonical`](MonitorLook::canonical) bytes.
    pub fn from_canonical(bytes: &[u8]) -> Result<MonitorLook, Refusal> {
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|e| Refusal::invalid("look", format!("not JSON: {e}")))?;
        let look = MonitorLook::from_value(&value)?;
        if look.canonical()?.as_bytes() != bytes {
            return Err(Refusal::invalid("look", "not in canonical form"));
        }
        Ok(look)
    }
}

/// What cannot be changed on `model`, by field, with the reason a row
/// shows. A model this machine's catalogue lacks has no locks: it is
/// refused outright by [`MonitorLook::check_model`].
pub fn locks(model: &str, catalogue: &[Entry]) -> BTreeMap<&'static str, String> {
    if model == MARKER {
        return [
            ("variant", "The marker has no variants"),
            ("tint", "The marker has no tint"),
            ("light", "The marker has no light"),
        ]
        .into_iter()
        .map(|(k, v)| (k, v.to_owned()))
        .collect();
    }
    catalogue
        .iter()
        .find(|e| e.key == model)
        .map(Entry::locks)
        .unwrap_or_default()
}

impl MonitorLook {
    /// The look's model and variant against this machine's catalogue: the
    /// marker always stands; a disc model must be in the catalogue and a
    /// variant must be one of its own.
    pub fn check_model(&self, catalogue: &[Entry]) -> Result<(), Refusal> {
        if self.is_marker() {
            return Ok(());
        }
        let entry = catalogue
            .iter()
            .find(|e| e.key == self.model)
            .ok_or_else(|| {
                Refusal::invalid("model", "is not in this machine's catalogue of models")
            })?;
        match &self.variant {
            Some(v) if !entry.variants.contains(v) => Err(Refusal::invalid(
                "variant",
                "is not a variant of this model",
            )),
            _ => Ok(()),
        }
    }
}

/// `patch` laid over `current`: bounds ([`MonitorLook::with`]), then the
/// catalogue ([`MonitorLook::check_model`]), then the locks. A field the
/// patch names that is locked on the resulting model and would change is
/// refused as [`Refusal::Locked`]; naming a locked field at its present
/// value is not a change.
pub fn apply(
    current: &MonitorLook,
    patch: &Patch,
    catalogue: &[Entry],
) -> Result<MonitorLook, Refusal> {
    let next = current.with(patch)?;
    next.check_model(catalogue)?;
    let locks = locks(&next.model, catalogue);
    for field in patch.fields() {
        let Some(reason) = locks.get(field) else {
            continue;
        };
        let changed = match field {
            "variant" => next.variant != current.variant,
            "primary" => next.primary != current.primary,
            "secondary" => next.secondary != current.secondary,
            "tint" => next.tint != current.tint,
            "light" => next.light != current.light,
            _ => false,
        };
        if changed {
            return Err(Refusal::locked(field, reason.clone()));
        }
    }
    Ok(next)
}
