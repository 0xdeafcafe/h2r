//! The recipes behind `maps.test.*`: `game::test_maps::Recipes` over the
//! scenario editor and the Firefight kit (docs/game/test-maps.md). `game` may
//! not use `tools`, so the composition root supplies this.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_proto::test_maps::RecipeInfo;
use game::test_maps::{BaseMap, Built, Recipes, Resolved};
use scenario_edit::recipe::{self, Builder, Fidelity, Recipe};
use scenario_edit::{MapSource, Session};
use serde_json::Value;

/// Placement and Firefight test maps built from local disc data.
pub struct ScenarioRecipes;

/// The recipe a `maps.test.create` names: a built-in by name or a whole
/// recipe, on its own base or `base`.
fn recipe_of(recipe: &Value, base: Option<&str>) -> Result<Recipe, String> {
    let r = match recipe {
        Value::String(name) => recipe::built_in_named(name),
        v @ Value::Object(_) => Recipe::from_json(v.clone()),
        _ => return Err("recipe: a built-in recipe's name, or a recipe object".into()),
    }
    .map_err(|e| e.to_string())?;
    match base {
        Some(b) => r.on_base(b.trim()).map_err(|e| e.to_string()),
        None => Ok(r),
    }
}

/// A placement recipe as a package (docs/editor/studio-redesign.md, "Each
/// content path as a package"): the recipe runs on `base`, the base map's
/// bytes, and the package is an `onto` map with the `edits` that make the
/// same scenario, so the build that makes the map is the one the game runs
/// ([`workspace::prepare`]). The Firefight probe is built by its kit, not
/// by edits, and is refused.
pub fn recipe_package(recipe: &Recipe, base: &[u8]) -> Result<workspace::Loaded, String> {
    if recipe.builder != Builder::Placements {
        return Err(format!(
            "{}: only a placement recipe is a package; the Firefight probe is its kit's",
            recipe.name
        ));
    }
    let source = MapSource {
        path: PathBuf::from(format!("{}.map", recipe.base)),
        region: None,
        bytes: Some(Arc::from(base)),
        shared: None,
    };
    let mut s = Session::new();
    s.build_scenes = false;
    s.open_named(source.clone(), &recipe.base)
        .map_err(|e| e.to_string())?;
    recipe::build(&mut s, recipe).map_err(|e| e.to_string())?;
    let scenario = s.scenario().ok_or("no map is open")?;
    let after =
        scenario_edit::package::edited_bytes(&source, scenario).map_err(|e| e.to_string())?;
    let edits = workspace::scenario_edits(base, &after)
        .map_err(|e| format!("{}: {e}", recipe.name))?
        .ok_or_else(|| format!("{}: the recipe changes nothing", recipe.name))?;
    let dir = recipe.package_dir();
    workspace::test_map_package(
        &workspace::TestMap {
            id: &dir,
            name: &dir,
            base: &recipe.base,
            title: &recipe.title,
            purpose: &recipe.purpose,
        },
        &edits,
    )
}

fn info(r: &Recipe) -> RecipeInfo {
    RecipeInfo {
        name: r.name.clone(),
        title: r.title.clone(),
        base: r.base.clone(),
        fidelity: match r.fidelity {
            Fidelity::Full => "full".into(),
            Fidelity::Approximate => "approximate".into(),
        },
        purpose: r.purpose.clone(),
        note: r.note.clone(),
    }
}

impl Recipes for ScenarioRecipes {
    fn built_in(&self) -> Result<Vec<RecipeInfo>, String> {
        let all = recipe::built_in().map_err(|e| e.to_string())?;
        Ok(all.iter().map(info).collect())
    }

    fn resolve(&self, recipe: &Value, base: Option<&str>) -> Result<Resolved, String> {
        let r = recipe_of(recipe, base)?;
        Ok(Resolved {
            package_dir: r.package_dir(),
            name: r.name,
            base: r.base,
        })
    }

    fn build(
        &self,
        recipe: &Value,
        base: Option<&str>,
        source: BaseMap,
        data_root: &Path,
        replace: bool,
    ) -> Result<Built, String> {
        let r = recipe_of(recipe, base)?;
        let source = MapSource::region(source.path, source.region);
        match r.builder {
            Builder::Placements => {
                let (built, saved) = recipe::build_package_with_replace(source, &r, data_root, replace)
                    .map_err(|e| e.to_string())?;
                Ok(Built {
                    map_id: saved.id.to_string(),
                    report: serde_json::to_value(&built).map_err(|e| e.to_string())?,
                })
            }
            Builder::FirefightProbe => {
                let bytes = source.read_base().map_err(|e| e.to_string())?;
                // The ground is a build step (docs/game/game-types-without-carriers.md,
                // "Maps for scripted games"): the kit's own, no labels placed.
                let recipe = firefight_kit::ground::Recipe::new(firefight_kit::Mode::Probe);
                let saved = firefight_kit::ground::build(
                    &bytes,
                    &r.base,
                    &firefight_kit::ground::Labels::default(),
                    &recipe,
                )
                .and_then(|ground| {
                    ground.install(
                        &bytes,
                        &r.base,
                        &content::store::maps_dir(data_root),
                        &r.package_dir(),
                        replace,
                    )
                })
                .map_err(|e| format!("{e:#}"))?;
                Ok(Built {
                    map_id: saved.id.to_string(),
                    report: serde_json::json!({
                        "recipe": r.name, "base": r.base, "fidelity": r.fidelity,
                        "counts": {"squads": saved.squads, "zones": saved.zones},
                        "costs": saved.costs, "bases": saved.bases, "expect": r.expect,
                    }),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_recipe_is_named_or_given_and_may_move_to_another_base() {
        let r = ScenarioRecipes;
        let named = r.resolve(&json!("spawn-grid"), None).unwrap();
        assert_eq!(named.name, "spawn-grid");
        assert_eq!(named.package_dir, "test-spawn-grid");
        let moved = r.resolve(&json!("spawn-grid"), Some("lockout")).unwrap();
        assert_eq!((moved.name.as_str(), moved.base.as_str()), ("spawn-grid-lockout", "lockout"));
        let inline = json!({
            "name": "mine", "title": "Mine", "purpose": "p", "base": "lockout",
            "fidelity": "full", "place": [{"block": "starting_locations"}]
        });
        assert_eq!(r.resolve(&inline, None).unwrap().name, "mine");
        for (bad, base) in [
            (json!("nope"), None),
            (json!(3), None),
            (json!({"name": "x"}), None),
            (json!("superbounce"), Some("lockout")),
        ] {
            assert!(r.resolve(&bad, base).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_placement_recipe_is_a_package_and_the_probe_is_not() {
        let base = scenario_edit::fixture::fixture();
        let r = Recipe::from_toml(
            "name = \"two\"\ntitle = \"Two\"\npurpose = \"p\"\nbase = \"fixture\"\n\
             fidelity = \"full\"\nanchor = { at = [0.0, 0.0, 0.0] }\n\
             [[place]]\nblock = \"starting_locations\"\nat = [4.0, 0.0, 0.0]\nfloor = false\n\
             [[place]]\nblock = \"starting_locations\"\nat = [8.0, 0.0, 0.0]\nfloor = false\n",
        )
        .unwrap();
        let p = recipe_package(&r, &base).unwrap();
        assert_eq!(p.package.manifest.package.id, "test-two");
        let kinds: Vec<_> = p.package.manifest.items.iter().map(|i| i.kind).collect();
        assert_eq!(
            kinds,
            [content::package::Kind::Map, content::package::Kind::Edits]
        );
        let probe = Recipe::from_toml(
            "name = \"p\"\ntitle = \"P\"\npurpose = \"p\"\nbase = \"fixture\"\n\
             fidelity = \"full\"\nbuilder = \"firefight-probe\"\n",
        )
        .unwrap();
        assert!(recipe_package(&probe, &base).is_err());
    }

    #[test]
    fn every_built_in_recipe_is_listed() {
        assert_eq!(
            ScenarioRecipes.built_in().unwrap().len(),
            recipe::BUILT_IN.len()
        );
    }
}
