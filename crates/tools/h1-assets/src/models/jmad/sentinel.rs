//! c40's Sentinel uses a fixed weapon class with idle in slot 0 and its
//! nine-frame aim in slot 25. 04b's aggressor looks both up under any/any.
use super::*;

pub(super) fn bindings(g: &Graph, plan: &mut VehiclePlan, actions: &mut Actions) -> Overlays {
    let mut overlays = Overlays::new();
    let class = |u: &crate::models::h1::UnitSlots| u.classes.iter().position(|c| c.label == "fixed");
    let slot = |slots: &[i16], k: usize| {
        slots
            .get(k)
            .and_then(|&i| usize::try_from(i).ok())
            .filter(|&i| i < g.animations.len())
    };
    let stand = g
        .units
        .iter()
        .find(|u| u.label == "stand")
        .and_then(|u| class(u).map(|i| &u.classes[i]));
    let aim = stand.and_then(|c| slot(&c.slots, 25).map(|i| (i, c.screen)));
    let look = g
        .animations
        .iter()
        .position(|a| a.name == "stand fixed look");
    for u in &g.units {
        let mode = match u.label.as_str() {
            "stand" => "combat",
            "alert" => "patrol",
            "crouch" => "crouch",
            _ => continue,
        };
        let Some(c) = class(u).map(|i| &u.classes[i]) else {
            continue;
        };
        let any = || "any".to_owned();
        let list = actions
            .entry(mode.to_owned())
            .or_default()
            .entry(any())
            .or_default()
            .entry(any())
            .or_default();
        if let Some(i) = slot(&c.slots, 0) {
            list.push(("idle".into(), i));
        }
        let over = overlays.entry((mode.to_owned(), any(), any())).or_default();
        if let Some(i) = look {
            over.push(("look".into(), i));
        }
        if mode != "crouch"
            && let Some((i, screen)) = aim
            && screen.is_set()
        {
            let s = plan.screen("aiming", screen.bytes());
            plan.screen_of.insert(i, s);
            if let Some(l) = look
                && g.animations[l].frame_count == g.animations[i].frame_count
            {
                plan.screen_of.insert(l, s);
            }
            for action in [
                "aim_still_up",
                "aim_still_down",
                "aim_move_up",
                "aim_move_down",
            ] {
                over.push((action.into(), i));
            }
        }
    }
    overlays
}
