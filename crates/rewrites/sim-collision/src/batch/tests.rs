use super::*;
use crate::mopp::Source;
use crate::objects::List;
use crate::synth::{Image, Reference, References};

#[test]
fn packets_preserve_query_and_key_order_across_packet_boundaries() {
    let tree = Tree::decode(
        0x1000,
        Source {
            origin: [0; 3],
            scale: 1.0f32.to_bits(),
            bytecode: vec![0x10, 3, 2, 1, 0x32, 0x31],
        },
    );
    let boxes: Vec<_> = (0..19)
        .map(|i| Corners {
            lo: [(i % 5) << 16, 0, 0],
            hi: [(i % 5) << 16, 0, 0],
        })
        .collect();
    let serial: Vec<_> = boxes
        .iter()
        .map(|b| tree.query_box(b, usize::MAX).unwrap().keys)
        .collect();
    assert_ne!(serial[0], serial[4]);
    assert_eq!(box_batch(&tree, &boxes).unwrap(), serial);
    assert!(box_batch(&tree, &[]).unwrap().is_empty());
}

#[test]
fn visits_forward_marks_and_generations_through_wrap_and_failed_batches() {
    let mut ram = Image::new(0x1000, 0x2000);
    let list = List {
        heads: 0x1000,
        array: 0x1004,
    };
    ram.word(list.heads, 0x1100);
    ram.word(list.array, 0x1200);
    let object = Reference {
        handle: 0x1234_0001,
        kind: 2,
        bounds: [0.0, 0.0, 0.0, 1.0],
    };
    References {
        clusters: vec![vec![object, object]],
    }
    .write(0x1100, 0x1200, 0x1400, |at, bytes| ram.put(at, bytes));
    let q = VisitQuery {
        lists: 1,
        types: 0,
        cluster: 0,
        centre: [0; 3],
        radius: 0,
        capacity: 8,
        zero: 0,
        control: 0x027f,
        x87: true,
        cluster_marks: 0x1800,
        flood_epoch: 7,
        object_marks: 0x2000,
        generation: u32::MAX - 1,
        slots: 8,
        lists_at: [list; 2],
    };
    // The second query sees generation 0 already in RAM unless the first
    // query's write of MAX to the same slot is forwarded.
    let qs = [
        q,
        VisitQuery { capacity: 0, ..q },
        VisitQuery {
            cluster: u16::MAX,
            ..q
        },
        q,
    ];
    let original = ram.bytes.clone();
    let answers = visit_batch(&ram, &qs, || panic!("radius zero")).unwrap();
    assert_eq!(ram.bytes, original);
    let mut generation = q.generation;
    for (query, answer) in qs.iter().zip(&answers) {
        let visit = object_visit(
            &ram,
            &VisitQuery {
                generation,
                ..*query
            },
            || panic!("no flood"),
        )
        .unwrap();
        generation = generation.wrapping_add(1);
        assert_eq!(answer.generation, generation);
        assert_eq!(answer.visit, visit);
        for mark in visit.marks {
            ram.word(q.object_marks + 4 * u32::from(mark), generation);
        }
    }
    assert_eq!(answers[1].visit.full, Some(object.handle));
    assert_eq!(answers[3].visit.handles, [object.handle]);
    let before = ram.bytes.clone();
    assert_eq!(
        visit_batch(&ram, &[q, VisitQuery { slots: 1, ..q }], || panic!(
            "no flood"
        )),
        Err(Bail::Data)
    );
    assert_eq!(ram.bytes, before);
}
