//! Ordered GPU work of one invocation of the light passes. Construction checks
//! every reached layer, including layers without draws. Stage preparation can
//! refuse the last group while the caller still owns the entire guest pass.
use frame_scene::{
    GroupSite, LightConstants, LightGroup, LightStep, LightTarget, PROJECTION_ROWS, ShadowFilter,
};
use scene_extract::layout::light_passes;

pub type Projection = [[u32; 4]; PROJECTION_ROWS];
/// A half-open rectangle in guest target pixels: x, y, width and height.
pub type Scissor = [u32; 4];

/// One eligible additional caster in list order. Each builds its own suffix
/// of the lists. Its clamped screen bounds enter `0x000494b0`'s projection.
#[derive(Clone, Copy, Debug)]
pub struct Additional<'a> {
    pub steps: [LightStep; 2],
    pub projection: &'a Projection,
}

#[derive(Clone, Copy, Debug)]
pub enum Pass<'a> {
    Light {
        site: GroupSite,
        steps: &'a [LightStep],
        additional: Option<&'a [Additional<'a>]>,
        constants: &'a LightConstants,
        /// Effective device scissor, inherited at entry. The sphere bounds are light globals.
        scissor: Option<Scissor>,
        projection: Option<&'a Projection>,
    },
    /// The mask branch once both list bits admit the filter. Layer 10 can
    /// still decline its begin, and the filter precedes the next mask check.
    ObjectShadow {
        steps: &'a [LightStep],
        filter: &'a ShadowFilter,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Node<'a> {
    Constants(&'a LightConstants),
    /// Render the current light's shader gel into target 13.
    Gel,
    Scissor(Option<Scissor>),
    Projection(&'a Projection),
    Begin {
        site: GroupSite,
        step: LightStep,
    },
    /// Clear the group's entire shadow attachment as selected by its ShadowPass.
    ClearShadow {
        group: usize,
    },
    /// The index in the invocation's gathered groups.
    Draw {
        group: usize,
    },
    End {
        site: GroupSite,
        layer: u8,
    },
    Filter(&'a ShadowFilter),
    RestoreView,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Refusal<E> {
    Steps,
    Group,
    Projection,
    Filter,
    Scissor,
    Stage(E),
}

fn valid_rect(rect: Option<Scissor>) -> bool {
    rect.is_none_or(|[x, y, w, h]| {
        w > 0 && h > 0 && x.checked_add(w).is_some() && y.checked_add(h).is_some()
    })
}

/// The two mask layers may decline their draws independently.
fn pair(steps: &[LightStep]) -> bool {
    matches!(
        steps,
        [
            LightStep {
                layer: 10,
                stencil: None,
                target: LightTarget::ShadowMap,
                ..
            },
            LightStep {
                layer: 11,
                stencil: Some(0),
                target: LightTarget::View,
                ..
            }
        ]
    )
}

fn valid_filter(filter: &ShadowFilter) -> bool {
    let mut source = 10;
    for draw in &filter.draws {
        let dest = if source == 10 { 11 } else { 10 };
        if draw.source != source || draw.dest != dest {
            return false;
        }
        source = dest;
    }
    !filter.size.contains(&0) && filter.copy == (source != 10).then_some((source, 10))
}

struct Builder<'a, 'g> {
    nodes: Vec<Node<'a>>,
    groups: &'g [&'a LightGroup],
    next: usize,
}

impl<'a> Builder<'a, '_> {
    fn layer<E>(
        &mut self,
        site: GroupSite,
        step: LightStep,
        projection: Option<&Projection>,
    ) -> Result<(), Refusal<E>> {
        if !step.draws {
            return Ok(());
        }
        let i = self.next;
        let g = self.groups.get(i).ok_or(Refusal::Group)?;
        if !g.whole()
            || g.site != site
            || g.layer != step.layer
            || g.entries.start > g.entries.end
            || g.light != self.groups[0].light
        {
            return Err(Refusal::Group);
        }
        if matches!(g.layer, 10 | 11 | 14) {
            crate::shadow_scene::validate(g).map_err(|_| Refusal::Group)?;
        }
        if matches!(g.layer, 10 | 11)
            && projection.is_some_and(|p| g.projection.as_ref() != Some(p))
        {
            return Err(Refusal::Projection);
        }
        self.nodes.push(Node::Begin { site, step });
        if step.layer == 10 {
            self.nodes.push(Node::ClearShadow { group: i });
        }
        self.nodes.push(Node::Draw { group: i });
        self.nodes.push(Node::End {
            site,
            layer: step.layer,
        });
        self.next += 1;
        Ok(())
    }
}

/// Plan the complete invocation before asking the stage owner to prepare any
/// command. The callback may reserve resources, but must leave issuing or
/// committing work until this function returns all commands successfully.
pub fn prepare<'a, T, E>(
    pass: Pass<'a>,
    groups: &[&'a LightGroup],
    mut stage: impl FnMut(Node<'a>) -> Result<T, E>,
) -> Result<Vec<T>, Refusal<E>> {
    let mut b = Builder {
        nodes: Vec::new(),
        groups,
        next: 0,
    };
    match pass {
        Pass::Light {
            site,
            steps,
            additional,
            constants,
            scissor,
            projection,
        } => {
            let mut masks = light_passes::Inputs {
                flags: constants.flags,
                definition_flags: constants.definition_flags,
                ..Default::default()
            };
            for s in steps.iter().filter(|s| s.draws) {
                let bit = 1u32.checked_shl(s.layer.into()).ok_or(Refusal::Steps)?;
                if s.layer == 10 {
                    masks.casters |= bit
                } else {
                    masks.receivers |= bit
                }
            }
            let expected = match site {
                GroupSite::Light => light_passes::steps(&masks),
                GroupSite::FirstPerson if additional.is_none() && !constants.shadowed() => {
                    light_passes::first_person_steps(&masks)
                }
                _ => return Err(Refusal::Steps),
            };
            if steps != expected.as_slice() || additional.is_some() && !constants.shadowed() {
                return Err(Refusal::Steps);
            }
            if !valid_rect(scissor) {
                return Err(Refusal::Scissor);
            }
            b.nodes.push(Node::Constants(constants));
            if constants.animated_gel {
                b.nodes.push(Node::Gel);
            }
            b.nodes.push(Node::Scissor(scissor));
            if constants.shadowed() {
                b.nodes
                    .push(Node::Projection(projection.ok_or(Refusal::Projection)?));
            }
            for &step in steps {
                if step.layer == 14
                    && let Some(casters) = additional
                {
                    for caster in casters {
                        if !pair(&caster.steps) {
                            return Err(Refusal::Steps);
                        }
                        b.nodes.push(Node::Projection(caster.projection));
                        for step in caster.steps {
                            b.layer(GroupSite::AdditionalCasters, step, Some(caster.projection))?;
                        }
                    }
                }
                b.layer(site, step, projection)?;
            }
            b.nodes.push(Node::Scissor(None));
            b.nodes.push(Node::RestoreView);
        }
        Pass::ObjectShadow { steps, filter } => {
            if !pair(steps) {
                return Err(Refusal::Steps);
            }
            if !valid_filter(filter) {
                return Err(Refusal::Filter);
            }
            if let Some(g) = groups.first() {
                let p = g.projection.as_ref().ok_or(Refusal::Projection)?;
                b.nodes.push(Node::Projection(p));
            }
            let projection = groups.first().and_then(|g| g.projection.as_ref());
            if steps[0].draws
                && !groups
                    .first()
                    .and_then(|g| g.pass)
                    .is_some_and(|p| p.colour_mask() && p.size == filter.size)
            {
                return Err(Refusal::Group);
            }
            b.layer(GroupSite::ObjectShadow, steps[0], projection)?;
            b.nodes.push(Node::Filter(filter));
            b.layer(GroupSite::ObjectShadow, steps[1], projection)?;
        }
    }
    if b.next != groups.len() {
        return Err(Refusal::Group);
    }
    b.nodes
        .into_iter()
        .map(|node| stage(node).map_err(Refusal::Stage))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::{ShadowFilterDraw, ShadowPass};

    fn constants(flags: u32) -> LightConstants {
        LightConstants {
            frame: [0; 36],
            flags,
            colour_a: [0; 3],
            colour_b: [0; 3],
            mode: 0,
            definition_flags: 0,
            definition_8a: 0,
            definition_80: 0,
            falloff_0: 0,
            gel: None,
            animated_gel: false,
        }
    }

    fn step(layer: u8) -> LightStep {
        LightStep {
            layer,
            stencil: (layer != 10).then_some(0),
            target: match layer {
                10 => LightTarget::ShadowMap,
                14 => LightTarget::Target(1),
                _ => LightTarget::View,
            },
            draws: true,
        }
    }

    const P: Projection = [[7; 4]; PROJECTION_ROWS];

    fn group(site: GroupSite, layer: u8) -> LightGroup {
        let mask = site == GroupSite::ObjectShadow;
        LightGroup {
            light: 2,
            site,
            layer,
            entries: 0..1,
            remainder: 0,
            projection: matches!(layer, 10 | 11).then_some(P),
            pass: matches!(layer, 10 | 11 | 14).then_some(ShadowPass {
                target: match layer {
                    10 if mask => 10,
                    10 => 9,
                    14 => 1,
                    _ => 0,
                },
                size: [16, 16],
                flags: if mask { 1 << 4 } else { 0 },
                clip_control: 0,
                raster: 0,
                offset_enable: 0,
                offset_constant: 0,
                offset_slope: 0,
                stencil_mode: 0,
                shadow_function: 0,
            }),
        }
    }

    fn filter() -> ShadowFilter {
        ShadowFilter {
            draws: vec![ShadowFilterDraw {
                source: 10,
                dest: 11,
                spread: 1,
            }],
            copy: Some((11, 10)),
            combiner: [0; frame_scene::SHADOW_COMBINER_WORDS],
            signs: [0; 2],
            size: [16, 16],
        }
    }

    fn nodes<'a>(pass: Pass<'a>, groups: &'a [LightGroup]) -> Vec<Node<'a>> {
        prepare(pass, &groups.iter().collect::<Vec<_>>(), Ok::<_, ()>).unwrap()
    }

    fn draws<'a>(nodes: &[Node<'a>], groups: &[LightGroup]) -> Vec<(GroupSite, u8)> {
        nodes
            .iter()
            .filter_map(|node| match *node {
                Node::Draw { group } => Some((groups[group].site, groups[group].layer)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn dynamic_light_sets_up_gel_projection_and_map_before_receivers() {
        let site = GroupSite::Light;
        let c = LightConstants {
            animated_gel: true,
            ..constants(7)
        };
        let steps = [10, 11, 14, 12, 13].map(step);
        let groups = steps.map(|s| group(site, s.layer));
        let rect = Some([4, 5, 6, 7]);
        let out = nodes(
            Pass::Light {
                site,
                steps: &steps,
                constants: &c,
                additional: None,
                scissor: rect,
                projection: Some(&P),
            },
            &groups,
        );
        assert_eq!(
            &out[..8],
            &[
                Node::Constants(&c),
                Node::Gel,
                Node::Scissor(rect),
                Node::Projection(&P),
                Node::Begin {
                    site,
                    step: steps[0]
                },
                Node::ClearShadow { group: 0 },
                Node::Draw { group: 0 },
                Node::End { site, layer: 10 }
            ]
        );
        assert_eq!(
            draws(&out, &groups),
            [10, 11, 14, 12, 13].map(|l| (site, l))
        );
        assert_eq!(
            &out[out.len() - 2..],
            &[Node::Scissor(None), Node::RestoreView]
        );
    }

    #[test]
    fn each_additional_caster_has_its_own_pair_and_projection_before_layer_14() {
        use GroupSite::{AdditionalCasters as A, Light as L};
        let c = constants(7);
        let steps = [10, 11, 14, 12, 13].map(step);
        let pair = [step(10), step(11)];
        let projections = [[[1; 4]; PROJECTION_ROWS], [[2; 4]; PROJECTION_ROWS]];
        let extra = projections.each_ref().map(|projection| Additional {
            steps: pair,
            projection,
        });
        let expected = [
            (L, 10),
            (L, 11),
            (A, 10),
            (A, 11),
            (A, 10),
            (A, 11),
            (L, 14),
            (L, 12),
            (L, 13),
        ];
        let mut groups = expected.map(|(s, l)| group(s, l));
        for (pair, projection) in groups[2..6].chunks_exact_mut(2).zip(projections) {
            for group in pair {
                group.projection = Some(projection);
            }
        }
        let out = nodes(
            Pass::Light {
                site: L,
                steps: &steps,
                constants: &c,
                additional: Some(&extra),
                scissor: None,
                projection: Some(&P),
            },
            &groups,
        );
        assert_eq!(draws(&out, &groups), expected);
        let rows: Vec<_> = out
            .iter()
            .filter_map(|n| match n {
                Node::Projection(p) => Some(*p),
                _ => None,
            })
            .collect();
        assert_eq!(rows, [&P, &projections[0], &projections[1]]);
        let clears: Vec<_> = out
            .iter()
            .filter_map(|n| {
                if let Node::ClearShadow { group } = n {
                    Some(*group)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(clears, [0, 2, 4]);
    }

    #[test]
    fn first_person_uses_its_own_draw_masks_without_generating_a_shadow() {
        let site = GroupSite::FirstPerson;
        let c = constants(3);
        let mut steps = [11, 14, 12, 13].map(step);
        steps[1].draws = false;
        let groups = [11, 12, 13].map(|l| group(site, l));
        let out = nodes(
            Pass::Light {
                site,
                steps: &steps,
                constants: &c,
                additional: None,
                scissor: None,
                projection: None,
            },
            &groups,
        );
        assert_eq!(draws(&out, &groups), [11, 12, 13].map(|l| (site, l)));
        assert!(
            !out.iter()
                .any(|n| matches!(n, Node::ClearShadow { .. } | Node::Projection(_)))
        );
    }

    #[test]
    fn object_shadow_filters_between_layers_even_when_the_caster_begin_declines() {
        let site = GroupSite::ObjectShadow;
        let filter = filter();
        for caster in [true, false] {
            let steps = [
                LightStep {
                    draws: caster,
                    ..step(10)
                },
                step(11),
            ];
            let all = [group(site, 10), group(site, 11)];
            let groups = &all[usize::from(!caster)..];
            let out = nodes(
                Pass::ObjectShadow {
                    steps: &steps,
                    filter: &filter,
                },
                groups,
            );
            let filter_pos = out
                .iter()
                .position(|n| matches!(n, Node::Filter(_)))
                .unwrap();
            assert_eq!(out.first(), Some(&Node::Projection(&P)));
            assert_eq!(draws(&out[..filter_pos], groups).len(), usize::from(caster));
            assert_eq!(draws(&out[filter_pos + 1..], groups), [(site, 11)]);
        }
    }

    #[test]
    fn malformed_late_input_keeps_the_entire_invocation_and_prepares_nothing() {
        let site = GroupSite::Light;
        let c = constants(3);
        let steps = [11, 14, 12, 13].map(step);
        let mut groups = steps.map(|s| group(site, s.layer));
        let pass = Pass::Light {
            site,
            steps: &steps,
            constants: &c,
            additional: None,
            scissor: None,
            projection: None,
        };
        let mut foreign = groups.clone();
        groups[3].remainder = 1;
        foreign[3].site = GroupSite::FirstPerson;
        for bad in [&groups[..], &foreign[..], &groups[..3]] {
            let result = prepare(
                pass,
                &bad.iter().collect::<Vec<_>>(),
                |_| -> Result<(), ()> { panic!("malformed pass reached stage preparation") },
            );
            assert_eq!(result, Err(Refusal::Group));
        }
    }

    #[test]
    fn a_depth_map_cannot_supply_the_object_masks_filter() {
        let site = GroupSite::ObjectShadow;
        let filter = filter();
        let steps = [step(10), step(11)];
        let mut groups = [group(site, 10), group(site, 11)];
        let mut pass = groups[0].pass.unwrap();
        pass.flags = 0;
        pass.target = 9;
        groups[0].pass = Some(pass);
        assert_eq!(
            prepare(
                Pass::ObjectShadow {
                    steps: &steps,
                    filter: &filter
                },
                &groups.each_ref(),
                Ok::<_, ()>
            ),
            Err(Refusal::Group)
        );
        let groups = [group(site, 10), group(site, 11)];
        let mut invalid = filter;
        invalid.copy = None;
        assert_eq!(
            prepare(
                Pass::ObjectShadow {
                    steps: &steps,
                    filter: &invalid
                },
                &groups.each_ref(),
                Ok::<_, ()>
            ),
            Err(Refusal::Filter)
        );
    }

    #[test]
    fn last_stage_refusing_releases_all_earlier_prepared_work() {
        struct Reservation<'c>(&'c std::cell::Cell<usize>);
        impl Drop for Reservation<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() - 1);
            }
        }
        let site = GroupSite::FirstPerson;
        let c = constants(3);
        let steps = [11, 14, 12, 13].map(step);
        let groups = steps.map(|s| group(site, s.layer));
        let live = std::cell::Cell::new(0);
        let pass = Pass::Light {
            site,
            steps: &steps,
            constants: &c,
            additional: None,
            scissor: None,
            projection: None,
        };
        let result = prepare(pass, &groups.iter().collect::<Vec<_>>(), |node| {
            if node == (Node::Draw { group: 3 }) {
                return Err("missing specular material");
            }
            live.set(live.get() + 1);
            Ok(Reservation(&live))
        });
        assert!(matches!(
            result,
            Err(Refusal::Stage("missing specular material"))
        ));
        assert_eq!(live.get(), 0);
    }
}
