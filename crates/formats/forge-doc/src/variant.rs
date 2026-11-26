//! What a document may be saved as: a map variant or a Forge map, its play
//! and session forms, and a variant's name (docs/game/forge-map-variants.md,
//! "The map variant file").
//!
//! A map variant holds scenario edits only: placement adds, entry ops, links,
//! labels and groups, and may ask for AI ground ([`crate::GroundRecipe`]). Its build is a scenario patch, so it fits every map the kit
//! fits. A document with anything more (a recipe, an import slot, a source
//! map, a tag-field write, a placement set or removal by running index, a
//! block-out piece) is a
//! Forge map. The tier is computed from the ops, never declared, and judged
//! on the play form: the session's kit recipe does not count.
use crate::{Edit, Forge, ForgeDoc, Placement, Recipe, Refusal, Result};

/// The generate recipe that makes the session kit part of a document
/// (docs/game/forge-design.md, "The kit"). A session's document has it and
/// a saved variant does not.
pub const KIT_RECIPE: &str = "forge_kit";

/// The most characters a map variant's name has: the length of the game's
/// own variant names, so it fits every retail text that shows one.
pub const MAX_VARIANT_NAME: usize = 16;

/// What a document may be offered as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// Scenario edits only: placement adds, entry ops, links and labels.
    /// Saved as `.h2mapvariant` and listed per base map.
    Variant,
    /// Anything more. Saved as `.h2forge` and picked as a map.
    Map,
}

impl Tier {
    /// The tier as `h2 forge check` and the stores name it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Variant => "variant",
            Self::Map => "map",
        }
    }
}

impl ForgeDoc {
    /// The document's tier, judged on its play form.
    pub fn tier(&self) -> Tier {
        let recipes = self
            .recipes
            .iter()
            .all(|r| is_kit(r) || crate::ground::is_ground(r));
        let ops = self.ops.iter().all(|op| match &op.edit {
            Edit::Placement(Placement::Add { .. }) | Edit::Entry(_) => true,
            // A piece's definition installs tags, which a scenario patch has none of.
            Edit::Forge(Forge::Piece { .. }) => false,
            Edit::Forge(_) => true,
            Edit::Placement(_) | Edit::TagField(_) | Edit::Undo | Edit::Mesh | Edit::Bitmap => {
                false
            }
        });
        if recipes && ops && self.sources.is_empty() && self.imports.is_empty() {
            Tier::Variant
        } else {
            Tier::Map
        }
    }

    /// The checks a document passes to be saved or offered as a map
    /// variant: the variant tier and a variant's name as its title.
    pub fn check_variant(&self) -> Result<()> {
        if self.tier() != Tier::Variant {
            return Err(Refusal::Tier);
        }
        variant_name(&self.title)
    }
}

/// A map variant's name: 1 to [`MAX_VARIANT_NAME`] characters, not blank,
/// with no control character. Two variants of one base map may not share
/// a name compared without case; that is the store's check.
pub fn variant_name(name: &str) -> Result<()> {
    let count = name.chars().count();
    if count == 0 || name.trim().is_empty() {
        return Err(Refusal::invalid("variant name", "must have a name"));
    }
    if count > MAX_VARIANT_NAME {
        return Err(Refusal::invalid(
            "variant name",
            format!("a map variant's name is at most {MAX_VARIANT_NAME} characters"),
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(Refusal::invalid(
            "variant name",
            "contains a control character",
        ));
    }
    Ok(())
}

/// The form a game is played from and a variant is saved in: the document
/// without the session's kit recipe. A document without one is returned
/// as it is. Dropping the recipe changes the document's identity, so its
/// `expect` is dropped with it.
pub fn play_form(doc: &ForgeDoc) -> ForgeDoc {
    let mut play = doc.clone();
    play.recipes.retain(|recipe| !is_kit(recipe));
    if play.recipes.len() != doc.recipes.len() {
        play.expect = None;
    }
    play
}

/// The form a session opens: the document with the kit recipe, appended
/// after its other recipes. A document that has it is returned as it is;
/// otherwise its `expect` is dropped, as the identity changes.
pub fn session_form(doc: &ForgeDoc) -> ForgeDoc {
    let mut session = doc.clone();
    if !session.recipes.iter().any(is_kit) {
        session.recipes.push(kit());
        session.expect = None;
    }
    session
}

/// The kit recipe as a document holds it.
pub fn kit() -> Recipe {
    Recipe::Generate {
        generator: KIT_RECIPE.into(),
        parameters: vec![],
    }
}

fn is_kit(recipe: &Recipe) -> bool {
    matches!(
        recipe,
        Recipe::Generate { generator, parameters } if generator == KIT_RECIPE && parameters.is_empty()
    )
}
