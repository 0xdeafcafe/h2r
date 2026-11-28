use guest_layout::testing::FakeGuest;
use guest_layout::view::fake;

use super::*;

const WORLD: u32 = 0x8000_1000;
const ISLAND: u32 = 0x8000_2000;
const FIXED: u32 = 0x8000_3000;
const COMPONENT: u32 = 0x8000_4000;
const ENTITY: u32 = 0x8000_5000;
const LEVEL: u32 = 0x8000_6000;
const PAIRS: u32 = 0x8000_7000;
const AGENT: u32 = 0x8000_8000;
const CHILDREN: u32 = 0x8000_9000;
const LEAF: u32 = 0x8000_a000;
const SHAPE: u32 = 0x8000_b000;
const EMPTY: u32 = 0x0048_05a4;

fn list(m: &mut FakeGuest, at: u32, data: u32, count: u32) {
    m.put_u32(at, data);
    m.put_u32(at + 4, count);
    m.put_u32(at + 8, count | 0x8000_0000);
}

fn scene() -> (FakeGuest, u32) {
    let mut m = FakeGuest::new();
    let header = 0x8001_0000;
    m.put_u32(layout::OBJECTS_PTR, header);
    let table = fake::datums::<objects::Header>(&mut m, header, 4);
    let (handle, object) = fake::push(&mut m, &table);
    let body = 0x8001_1000;
    object.set_body(&mut m, body).unwrap();
    m.put_u32(body + layout::BODY_PHYSICS, 0x4321_0000);
    let table = 0x8001_2000;
    m.put_u32(layout::PHYSICS_COMPONENTS_PTR, table);
    m.put_u32(table + layout::DATUM_DATA, COMPONENT);
    m.put_u32(COMPONENT + layout::COMPONENT_OBJECT, handle.raw());
    let rigids = COMPONENT + 0x100;
    list(&mut m, COMPONENT + layout::COMPONENT_BODIES, rigids, 2);
    // Only the second body has a pair.
    m.put_u32(rigids + layout::COMPONENT_BODY_RIGID, ENTITY + 0x800);
    m.put_u32(
        rigids + layout::COMPONENT_BODY_SIZE + layout::COMPONENT_BODY_RIGID,
        ENTITY,
    );

    m.put_u32(layout::HAVOK_WORLD_PTR, WORLD);
    list(
        &mut m,
        WORLD + layout::WORLD_INACTIVE_ISLANDS,
        WORLD + 0x200,
        1,
    );
    m.put_u32(WORLD + 0x200, ISLAND);
    m.put_u32(WORLD + layout::WORLD_FIXED_ISLAND, FIXED);
    m.map(FIXED, 0xb0);
    list(&mut m, ISLAND + ISLAND_PAIRS, PAIRS, 3);
    // First is an unrelated pair with an unreadable cache. Second has no agent.
    m.put_u32(PAIRS + 8, 0xcccc_cccc);
    m.put_u32(PAIRS + 0x10, ENTITY + HANDLE_IN_ENTITY);
    let p = PAIRS + 0x20;
    m.put_u32(p, LEVEL + HANDLE_IN_ENTITY);
    m.put_u32(p + 4, ENTITY + HANDLE_IN_ENTITY);
    m.put_u32(p + 8, AGENT);
    m.put_u32(p + 12, u32::MAX);
    m.put_u32(LEVEL + 0x24, 1);
    m.put_u32(LEVEL + 0x2c, LEVEL);
    m.put_u32(LEVEL + 0x44, FIXED);
    m.put_u32(LEVEL + 0x0c, SHAPE);
    m.put_u32(SHAPE, 0x0045_6160);

    m.put_u32(AGENT, cache::TREE_A);
    m.put_u16(AGENT + 4, 0x40);
    list(&mut m, AGENT + 0x0c, CHILDREN, 2);
    m.put_f32(AGENT + 0x20, -1.5);
    m.put_f32(AGENT + 0x30, 1.5);
    m.put_u32(CHILDREN, 0x2000_4321);
    m.put_u32(CHILDREN + 8, LEAF);
    m.put_u32(CHILDREN + 12, 0x4000_4321);
    m.put_u32(CHILDREN + 20, EMPTY);
    m.put_u32(EMPTY, 0x0041_357c);

    m.put_u32(LEAF, 0x0041_2290);
    m.put_u16(LEAF + 4, 0x100);
    m.put_u16(LEAF + 0x0c, 0x10);
    m.put_u16(LEAF + 0x0e, 3);
    m.put_u16(LEAF + 0x10, 7);
    m.put_u8(LEAF + 0x14, 1);
    m.put_u8(LEAF + 0x15, 2);
    m.put_u32(LEAF + 0x18, 1);
    m.put_f32(LEAF + 0x24, 1.0);
    m.put_f32(LEAF + 0x2c, 0.25);
    m.put_u16(LEAF + 0x4c, 9);
    m.put_u32(LEAF + 0xf0, 1);
    (m, handle.raw())
}

#[test]
fn sleeping_body_against_structure_has_surface_keys_and_child_state() {
    let (m, datum) = scene();
    let pairs = agents(&m, datum).unwrap();
    assert_eq!(pairs.len(), 1);
    let p = &pairs[0];
    assert_eq!(p.island, ISLAND);
    assert_eq!(p.entry, PAIRS + 0x20);
    assert_eq!(p.object_side, 1);
    assert_eq!(p.entity, ENTITY);
    assert_eq!(p.collidables, [LEVEL + 0x0c, ENTITY + 0x0c]);
    assert_eq!(p.other_entity, Some(LEVEL));
    assert!(p.other_fixed && p.other_datum.is_none());
    assert_eq!(p.cache.bytes.len(), 0x40);
    let t = p.tree.as_ref().unwrap();
    assert_eq!(t.min, Some([-1.5, 0.0, 0.0]));
    assert_eq!(t.max, Some([1.5, 0.0, 0.0]));
    assert_eq!(t.children.len(), 2);
    let c = &t.children[0];
    assert_eq!((c.key, c.surface), (0x2000_4321, Some(0x4321)));
    assert_eq!(c.cache.base, LEAF);
    assert_eq!(c.cache.bytes.len(), 0x100);
    assert_eq!(&c.cache.bytes[0x2c..0x30], &0.25f32.to_le_bytes());
    let state = c.cache.convex.as_ref().unwrap();
    assert_eq!(state.vertices_a, [0x10]);
    assert_eq!(state.vertices_b, [3, 7]);
    assert_eq!(state.contact_ids, [9]);
    assert_eq!(state.swept_sides, Some(1));
    assert_eq!(state.separating_axis, Some([0.0, 1.0, 0.0]));
    assert_eq!(state.separating_distance, Some(0.25));
    assert_eq!(t.children[1].surface, None);
    assert_eq!(t.children[1].cache.kind, "empty");
    assert_eq!(t.children[1].cache.bytes.len(), 12);
}

#[test]
fn reversed_tree_dispatch_and_non_structure_keys() {
    let (mut m, datum) = scene();
    m.put_u32(AGENT, cache::TREE_B);
    m.put_u32(PAIRS + 0x20, ENTITY + HANDLE_IN_ENTITY);
    m.put_u32(PAIRS + 0x24, LEVEL + HANDLE_IN_ENTITY);
    let p = agents(&m, datum).unwrap().pop().unwrap();
    assert_eq!(p.object_side, 0);
    assert_eq!(p.tree.unwrap().children[0].surface, Some(0x4321));
    m.put_u32(SHAPE, 0x1234);
    let p = agents(&m, datum).unwrap().pop().unwrap();
    assert_eq!(p.tree.unwrap().children[0].surface, None);
}

#[test]
fn rejects_bad_counts_sizes_and_wrapping_ranges() {
    let (mut m, datum) = scene();
    m.put_u32(AGENT + 0x10, 3);
    assert!(agents(&m, datum).is_err());
    list(&mut m, AGENT + 0x0c, CHILDREN, 513);
    assert!(agents(&m, datum).is_err());
    list(&mut m, AGENT + 0x0c, u32::MAX - 7, 1);
    assert!(agents(&m, datum).is_err());
    list(&mut m, AGENT + 0x0c, CHILDREN, 1);
    m.put_u16(LEAF + 4, 12);
    assert!(agents(&m, datum).is_err());
    m.put_u16(LEAF + 4, 0x100);
    m.put_u8(LEAF + 0x14, 4);
    assert!(agents(&m, datum).is_err());
    m.put_u8(LEAF + 0x14, 1);
    m.put_u32(LEAF + 0x18, 5);
    assert!(agents(&m, datum).is_err());
    list(&mut m, ISLAND + ISLAND_PAIRS, PAIRS, MAX_PAIRS + 1);
    assert_eq!(
        agents(&m, datum).unwrap_err().addr,
        ISLAND + ISLAND_PAIRS
    );
}

#[test]
fn unknown_agent_uses_its_size_and_ignores_pointer_like_cache_words() {
    let (mut m, datum) = scene();
    m.put_u32(AGENT, 0xaabb_ccdd);
    m.put_u16(AGENT + 4, 24);
    m.put_u32(AGENT + 12, u32::MAX);
    let p = agents(&m, datum).unwrap().pop().unwrap();
    assert_eq!(p.cache.bytes.len(), 24);
    assert_eq!(p.cache.kind, "other");
    assert!(p.tree.is_none());
    assert!(cache::cache(&m, AGENT, &mut 23).is_err());
}

#[test]
fn nonfixed_peer_resolves_through_its_own_component() {
    let (mut m, datum) = scene();
    let props = LEVEL + 0x100;
    list(&mut m, LEVEL + layout::ENTITY_PROPERTIES, props, 1);
    m.put_u32(props, layout::PROPERTY_PHYSICS_COMPONENT);
    m.put_u32(props + 4, 0xcafe_0001);
    let peer = 0xfeed_0042;
    m.put_u32(
        COMPONENT + layout::PHYSICS_COMPONENT_SIZE + layout::COMPONENT_OBJECT,
        peer,
    );
    m.put_u32(LEVEL + 0x44, ISLAND);
    let p = agents(&m, datum).unwrap().pop().unwrap();
    assert_eq!(p.other_datum, Some(peer));
    assert!(!p.other_fixed);
}

#[test]
fn retail_welded_tree_uses_its_wrapped_structure_in_both_orders() {
    let (mut m, datum) = scene();
    let wrapper = SHAPE + 0x100;
    m.put_u32(LEVEL + 0x0c, wrapper);
    m.put_u32(wrapper, 0x0045_5540);
    m.put_u32(wrapper + 0x0c, SHAPE);
    m.put_u32(CHILDREN + 4, 0x1122_3344);
    for (vtable, tree_first) in [(0x0041_2f7c, true), (0x0041_2f5c, false)] {
        let sides = if tree_first {
            [LEVEL, ENTITY]
        } else {
            [ENTITY, LEVEL]
        };
        m.put_u32(AGENT, vtable);
        m.put_u32(PAIRS + 0x20, sides[0] + HANDLE_IN_ENTITY);
        m.put_u32(PAIRS + 0x24, sides[1] + HANDLE_IN_ENTITY);
        let p = agents(&m, datum).unwrap().pop().unwrap();
        assert_eq!(p.cache.kind, "welded_bv_tree");
        let t = p.tree.unwrap();
        assert_eq!(t.children[0].surface, Some(0x4321));
        assert_eq!(t.children[0].word_04, 0x1122_3344);
        assert_eq!(t.children[0].cache.kind, "swept_convex");
    }
}

#[test]
fn pair_word_writes_only_the_objects_own_pairs() {
    let (mut m, datum) = scene();
    let entry = PAIRS + 0x20;
    assert_eq!(set_pair_word(&mut m, datum, entry, 0xffff_fffd).unwrap(), Some(u32::MAX));
    assert_eq!(agents(&m, datum).unwrap()[0].pair_word, 0xffff_fffd);
    assert_eq!(fixed_pair(&m, datum).unwrap(), Some(entry));
    // The unrelated pair and the one without an agent are refused, untouched.
    for other in [PAIRS, PAIRS + 0x10] {
        let before = m.u32(other + PAIR_WORD).unwrap();
        assert_eq!(set_pair_word(&mut m, datum, other, 7).unwrap(), None);
        assert_eq!(m.u32(other + PAIR_WORD).unwrap(), before);
    }
}

#[test]
fn the_light_pair_word_reads_the_levels_pair_without_decoding_caches() {
    let (mut m, datum) = scene();
    assert_eq!(fixed_pair_word(&m, datum).unwrap(), Some(u32::MAX));
    // The same pair `fixed_pair` finds.
    let entry = fixed_pair(&m, datum).unwrap().unwrap();
    m.put_u32(entry + PAIR_WORD, 0xffff_fffd);
    assert_eq!(fixed_pair_word(&m, datum).unwrap(), Some(0xffff_fffd));
    // An agent cache that cannot decode does not matter to it.
    m.put_u32(AGENT, 0xdead_beef);
    assert_eq!(fixed_pair_word(&m, datum).unwrap(), Some(0xffff_fffd));
    // No Havok world: none.
    m.put_u32(layout::HAVOK_WORLD_PTR, 0);
    assert_eq!(fixed_pair_word(&m, datum).unwrap(), None);
}

#[test]
fn level_pairs_reads_every_objects_level_pairs_in_one_walk() {
    let (mut m, datum) = scene();
    let entry = PAIRS + 0x20;
    // The scene's island is inactive: the body sleeps.
    let got = level_pairs(&m, &[datum, 0x1234_5678]).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(
        got[0].pairs,
        vec![LevelPair {
            entry,
            agent: AGENT,
            word: u32::MAX
        }]
    );
    assert!(got[0].asleep);
    // An object with no physics: nothing.
    assert_eq!(got[1], LevelPairs::default());
    // The same word the single-pair reader sees.
    assert_eq!(fixed_pair_word(&m, datum).unwrap(), Some(u32::MAX));
    // Written over the pairs that differ, and read back.
    assert_eq!(set_level_words(&mut m, &got[0].pairs, 0xffff_fffd).unwrap(), 1);
    let again = level_pairs(&m, &[datum]).unwrap();
    assert_eq!(again[0].pairs[0].word, 0xffff_fffd);
    assert_eq!(set_level_words(&mut m, &again[0].pairs, 0xffff_fffd).unwrap(), 0);
    // An agent cache that cannot decode does not matter to it.
    m.put_u32(AGENT, 0xdead_beef);
    assert_eq!(level_pairs(&m, &[datum]).unwrap()[0].pairs.len(), 1);
    // No Havok world: none.
    m.put_u32(layout::HAVOK_WORLD_PTR, 0);
    assert_eq!(level_pairs(&m, &[datum]).unwrap()[0], LevelPairs::default());
}
