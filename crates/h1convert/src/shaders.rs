//! Which of the base map's structure shaders each Halo 1 shader draws
//! with, until Halo 1's own shaders and bitmaps convert.
//!
//! A shader is chosen by name: the words of the Halo 1 shader's name
//! (`grass`, `cliff`, `metal`, ...) against the words of each base shader's.
//! Shaders that can't stand in for a tiled surface are never chosen for an
//! ordinary one: water, glass, sky, decals and transparent templates, and
//! `unwrapped` ones, whose textures are laid out for one mesh's own UVs
//! (on other geometry they look like scattered blocks, which is what every
//! surface got when all used the base's first shader). Halo 1 water
//! (`swat`) takes a base water shader if there is one.

/// One of the base structure's shaders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseShader {
    /// Tag name.
    pub name: String,
    /// Its template's tag name (`shad +0`).
    pub template: String,
}

/// Template or name words of shaders that only suit their own kind of
/// surface.
const SPECIAL: [&str; 9] = [
    "water",
    "glass",
    "sky",
    "decal",
    "transparent",
    "illum",
    "plasma",
    "unwrapped",
    "lightmap",
];

/// Words that say nothing about a surface.
const NOISE: [&str; 12] = [
    "levels",
    "scenarios",
    "shaders",
    "shader",
    "multi",
    "halo",
    "test",
    "map",
    "bsp",
    "the",
    "and",
    "base",
];

/// The lowercase words (3+ letters, a plural `s` dropped) of a tag name's
/// last component.
fn words(name: &str) -> Vec<String> {
    let last = name.rsplit(['\\', '/']).next().unwrap_or(name);
    last.split(|c: char| !c.is_ascii_alphabetic())
        .map(str::to_ascii_lowercase)
        .filter(|w| w.len() >= 3 && !NOISE.contains(&w.as_str()))
        .map(|w| match w.strip_suffix('s') {
            Some(stem) if stem.len() >= 3 && !w.ends_with("ss") => stem.to_owned(),
            _ => w,
        })
        .collect()
}

fn special(s: &BaseShader) -> bool {
    let n = s.name.to_ascii_lowercase();
    let t = s.template.to_ascii_lowercase();
    SPECIAL.iter().any(|w| n.contains(w) || t.contains(w))
}

fn water(s: &BaseShader) -> bool {
    s.template.to_ascii_lowercase().contains("water")
}

/// For each Halo 1 shader (tag name, group FourCC such as `senv` or
/// `swat`), the index of the base shader to use. `base` must not be empty.
pub fn pick(h1: &[(String, String)], base: &[BaseShader]) -> Vec<usize> {
    let plain: Vec<usize> = (0..base.len()).filter(|&i| !special(&base[i])).collect();
    let pool = if plain.is_empty() {
        (0..base.len()).collect()
    } else {
        plain
    };
    // Default: the first plain shader with an opaque template, else the
    // first plain one.
    let fallback = pool
        .iter()
        .copied()
        .find(|&i| base[i].template.to_ascii_lowercase().contains("opaque"))
        .unwrap_or(pool[0]);
    h1.iter()
        .map(|(name, group)| {
            if group == "swat"
                && let Some(i) = (0..base.len()).find(|&i| water(&base[i]))
            {
                return i;
            }
            let want = words(name);
            pool.iter()
                .copied()
                .map(|i| {
                    let have = words(&base[i].name);
                    (want.iter().filter(|w| have.contains(w)).count(), i)
                })
                .filter(|&(score, _)| score > 0)
                // Most shared words; the earlier shader on a tie.
                .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
                .map_or(fallback, |(_, i)| i)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(name: &str, template: &str) -> BaseShader {
        BaseShader {
            name: name.into(),
            template: template.into(),
        }
    }

    #[test]
    fn names_choose_and_special_shaders_stand_aside() {
        let base = [
            b(
                "scenarios\\shaders\\multi\\x\\x_water_pool",
                "templates\\water\\water_pool",
            ),
            b(
                "scenarios\\shaders\\multi\\x\\x_cliff_unwrapped",
                "templates\\opaque\\tex_bump",
            ),
            b(
                "scenarios\\shaders\\multi\\x\\x_glass",
                "templates\\opaque\\tex_bump",
            ),
            b(
                "scenarios\\shaders\\multi\\x\\x_metal_panel",
                "templates\\opaque\\tex_bump",
            ),
            b(
                "scenarios\\shaders\\multi\\x\\x_grass",
                "templates\\opaque\\tex_bump",
            ),
            b(
                "scenarios\\shaders\\multi\\x\\x_cliffs_rock",
                "templates\\opaque\\tex_bump",
            ),
        ];
        let h1 = [
            ("levels\\test\\gulch\\shaders\\grass".into(), "senv".into()),
            ("levels\\test\\gulch\\shaders\\cliff".into(), "senv".into()),
            (
                "levels\\test\\gulch\\shaders\\metal panels".into(),
                "senv".into(),
            ),
            ("levels\\test\\gulch\\shaders\\stream".into(), "swat".into()),
            (
                "levels\\test\\gulch\\shaders\\something".into(),
                "senv".into(),
            ),
        ];
        // grass; the cliff that isn't unwrapped; metal; base water for
        // Halo 1 water; else the first plain opaque one.
        assert_eq!(pick(&h1, &base), [4, 5, 3, 0, 3]);
    }

    #[test]
    fn with_only_special_shaders_any_will_do() {
        let base = [b("x_water", "water\\pool"), b("x_sky", "sky\\sky")];
        let h1 = [("levels\\a\\grass".into(), "senv".into())];
        assert_eq!(pick(&h1, &base), [0]);
    }

    #[test]
    fn words_drop_noise_short_words_and_plurals() {
        assert_eq!(
            words("levels\\test\\x\\shaders\\Cliffs_2 rock-ss"),
            ["cliff", "rock"]
        );
        assert_eq!(words("a\\grass"), ["grass"]);
    }
}
