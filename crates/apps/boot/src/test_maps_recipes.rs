//! The recipes behind `maps.test.*`: `game::test_maps::Recipes` over the
//! scenario editor and the Firefight kit (docs/game/test-maps.md). `game` may
//! not use `tools`, so the composition root supplies this.

use std::path::Path;

use agent_proto::test_maps::RecipeInfo;
use game::test_maps::{BaseMap, Built, Recipes, Resolved};
use scenario_edit::MapSource;
use scenario_edit::recipe::{self, Builder, Fidelity, Recipe};
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
                let saved = firefight_kit::build_package(
                    &bytes,
                    &r.base,
                    firefight_kit::Mode::Probe,
                    &content::store::maps_dir(data_root),
                    &r.package_dir(),
                    replace,
                )
                .map_err(|e| format!("{e:#}"))?;
                Ok(Built {
                    map_id: saved.id.to_string(),
                    report: serde_json::json!({
                        "recipe": r.name, "base": r.base, "fidelity": r.fidelity,
                        "counts": {"squads": saved.squads, "zones": saved.zones},
                        "costs": saved.costs, "expect": r.expect,
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
    fn every_built_in_recipe_is_listed() {
        assert_eq!(
            ScenarioRecipes.built_in().unwrap().len(),
            recipe::BUILT_IN.len()
        );
    }
}
