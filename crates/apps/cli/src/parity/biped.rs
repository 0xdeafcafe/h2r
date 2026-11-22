use super::*;
use h1objects::parity::{Pose, RegionRay, Target};

impl Case {
    fn unit(&mut self, tag: &Tag) -> Result<String> {
        let player = self.player()?;
        if player["tag_name"] == tag.name
            || (self.h2_sandbox && tag.name == r"characters\cyborg_mp\cyborg_mp")
        {
            return Ok(player["unit"].as_str().context("player unit")?.into());
        }
        let world = self.call(
            agent_proto::method::WORLD_QUERY,
            json!({"kind":["actor"],"tag":tag.name,"fields":["id","tag_name"]}),
        )?;
        if let Some(unit) = world["objects"]
            .as_array()
            .context("actor list")?
            .iter()
            .filter(|o| o["tag_name"].as_str() == Some(tag.name.as_str()))
            .min_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()))
        {
            return Ok(unit["id"]
                .as_str()
                .and_then(|s| s.strip_prefix("actor."))
                .context("actor datum")?
                .into());
        }
        let mut pos: [f32; 3] = serde_json::from_value(player["pos"].clone())?;
        pos[0] += 3.;
        let spawned = self.call(
            agent_proto::sim::method::SPAWN,
            json!({"tag":format!("bipd:{}",tag.name),"pos":pos}),
        )?;
        Ok(spawned["object"]
            .as_str()
            .context("spawned biped datum")?
            .into())
    }

    fn pose(&mut self, id: &str, unit: &str, graph: &str, p: &Pose) -> Result<()> {
        let v = self.call(
            agent_proto::probe::method::ANIMATION,
            json!({"unit":unit,"path":[p.mode,p.class,p.kind,p.action],"overlay":p.overlay}),
        )?;
        let clip = h1_assets::models::names::map_name(&p.clip).full();
        let expected = json!([graph, clip, p.index, p.frames]);
        let observed = json!([v["graph_name"], v["clip"], v["index"], v["frames"]]);
        self.table.row(
            id,
            &expected,
            format!("{observed} via {}", v["selected_names"]),
            observed == expected,
        );
        Ok(())
    }

    pub(super) fn biped(&mut self, tag: &Tag) -> Result<()> {
        let target = h1objects::parity::target(&self.h1, tag, &self.rules)?;
        let unit = self.unit(tag)?;
        if target.poses.is_empty() {
            self.table.row(
                &format!("{} animation coverage", tag.name),
                "source unit slots",
                &target.graph,
                false,
            );
        }
        for p in &target.poses {
            let id = format!(
                "{} {} {}:{}:{}:{}",
                tag.name, p.state, p.mode, p.class, p.kind, p.action
            );
            if let Err(e) = self.pose(&id, &unit, &target.graph, p) {
                self.table.error(&id, e);
            }
        }
        ensure!(!target.rays.is_empty(), "no default collision surfaces");
        let rifle = self
            .h1
            .find(h1map::Group::new(b"weap"), RIFLE)
            .context("missing reference assault rifle")?;
        let gun = h1objects::parity::gun(&self.h1, rifle)?;
        let bullet = self.h1.tag(
            gun.weapon
                .triggers
                .first()
                .and_then(|t| t.projectile)
                .context("rifle has no primary projectile")?,
        )?;
        let effect = self
            .h1
            .tag(
                h1objects::h1::Projectile::read(&self.h1, bullet)?
                    .impact_damage
                    .context("rifle projectile has no impact damage")?,
            )?
            .clone();
        let projectile = if self.h2_sandbox {
            format!(
                "0x{:08x}",
                self.rifle_projectile
                    .context("no live projectile observed from the sandbox rifle")?
            )
        } else {
            format!("proj:{}", bullet.name)
        };
        let meta = tags::object::Metadata::read(&mut self.h2)?;
        let materials = h1objects::base::global_materials(&mut self.h2, &meta).map_err(|e| anyhow::anyhow!(e))?;
        let snapshot = format!("parity-target-{}", std::process::id());
        self.call(agent_proto::state::method::SAVE, json!({"name":snapshot}))?;
        let trials = (|| -> Result<()> {
            for ray in &target.rays {
                for shield in [false, true] {
                    if shield && target.shield <= 0. {
                        continue;
                    }
                    self.call(agent_proto::state::method::LOAD, json!({"name":snapshot}))?;
                    let id = format!(
                        "{} {} node {} {} {}",
                        tag.name,
                        ray.name,
                        ray.node,
                        ray.material,
                        if shield { "shield" } else { "body" }
                    );
                    if let Err(e) = self.ray(
                        &id,
                        &unit,
                        &projectile,
                        &effect,
                        &target,
                        ray,
                        shield,
                        &materials,
                    ) {
                        self.table.error(&id, e);
                    }
                }
            }
            Ok(())
        })();
        self.call(agent_proto::state::method::LOAD, json!({"name":snapshot}))?;
        self.call(agent_proto::state::method::DROP, json!({"name":snapshot}))?;
        trials
    }

    fn ray(
        &mut self,
        id: &str,
        unit: &str,
        projectile: &str,
        effect: &Tag,
        target: &Target,
        ray: &RegionRay,
        shield: bool,
        materials: &[String],
    ) -> Result<()> {
        // Start just outside the measured face, in its posed node's coordinates.
        let point = |dist: f32| -> [f32; 3] {
            std::array::from_fn(|i| ray.center[i] + dist * ray.normal[i])
        };
        let v = self.call(
            agent_proto::probe::method::HIT,
            json!({"target":unit,"projectile":projectile,"node":ray.node,
            "shield":if shield {1.} else {0.},"from":point(0.01),"to":point(-0.01),"ticks":12}),
        )?;
        let hit = &v["hit"];
        let m = hit["material"]
            .as_u64()
            .and_then(|i| materials.get(i as usize))
            .context("unknown global material")?;
        let expected_material = h1objects::arsenal::MATERIALS
            .get(usize::from(ray.material_type))
            .context("unknown CE material")?
            .1;
        let contact = json!([hit["object"], hit["region"], hit["node"], m]);
        let want = json!([
            u32::from_str_radix(unit, 16)?,
            ray.region,
            ray.node,
            expected_material
        ]);
        self.table
            .row(&format!("{id} contact"), &want, &contact, contact == want);
        ensure!(hit["object"] == want[0], "probe hit another object");
        let (channel, material, scale, vitality) = if shield {
            (1, target.shield_type, ray.shield_scale, target.shield)
        } else {
            (0, ray.material_type, ray.body_scale, target.body)
        };
        ensure!(vitality > 0., "invalid source vitality {vitality}");
        let bounds = h1objects::parity::damage(&self.h1, effect, material, &self.rules)?;
        let starting_shield = hit["before"][1]
            .as_f64()
            .context("missing starting shield")?;
        ensure!(
            starting_shield == if shield { 1. } else { 0. },
            "unexpected starting shield {starting_shield}"
        );
        let before = hit["before"][channel]
            .as_f64()
            .context("missing pre-impact vitality")? as f32;
        let after = hit["after"][channel]
            .as_f64()
            .context("missing post-impact vitality")? as f32;
        ensure!(before > 0., "target already depleted before impact");
        // Compare remaining vitality in f32: subtracting the snapshots again
        // would lose significant bits on small hits.
        let [low, high] = bounds.map(|damage| (before - damage * scale / vitality).max(0.));
        self.table.row(
            &format!("{id} damage"),
            format!("{before} -> [{high},{low}]"),
            format!("{before} -> {after}"),
            (high..=low).contains(&after),
        );
        if !shield || high > 0. {
            let other = 1 - channel;
            self.table.row(
                &format!("{id} other vitality"),
                &hit["before"][other],
                &hit["after"][other],
                hit["before"][other].is_number() && hit["before"][other] == hit["after"][other],
            );
        }
        Ok(())
    }
}
