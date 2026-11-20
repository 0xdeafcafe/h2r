use super::*;
use crate::scripts::cinematics::{Field, MARKER};

#[test]
fn convolution_shares_initializer_nodes_and_writes_distinct_cells() {
    let name = "cinematic_screen_effect_set_convolution";
    let catalog = Catalog::new(
        vec![
            f("begin", PASS, 0x2000, &[]),
            f(name, VOID, ORDINARY, &[7, 7, 6, 6, 6]),
        ],
        h1_types(),
        ["startup", "dormant", "continuous", "static", "stub"]
            .map(String::from)
            .to_vec(),
    )
    .unwrap();
    let mut target = h2_target();
    for (name, index, ret, special) in [("set", 4, PASS, true), ("game_time", 888, 8, false)] {
        target.builtins.insert(
            name.into(),
            H2Builtin {
                index,
                return_type: ret,
                parameters: vec![],
                special,
            },
        );
    }
    let mut b = Builder::default();
    let args = [
        b.atom(7, 7, "1", 1),
        b.atom(7, 7, "1", 1),
        b.atom(6, 6, "3.0", 3f32.to_bits()),
        b.atom(6, 6, "0.0", 0),
        b.atom(6, 6, "7.0", 7f32.to_bits()),
    ];
    let blur = b.call(1, VOID, name, &args);
    let input = read(&scenario(&b, &[("intro", 0, VOID, blur)], &[])).unwrap();
    let compiled = compile(
        &input,
        &catalog,
        &Table::build(&catalog, &target),
        &target,
        Missing::Stub,
        &[],
    )
    .unwrap();
    assert!(compiled.missing.is_empty());
    let bytes = compiled.scenario_bytes(BASE);
    let output = hs::Scenario::decode(&bytes, BASE).unwrap();
    output.validate_graph().unwrap();
    let globals = output.globals();
    assert_eq!(globals.last().unwrap().name().unwrap(), MARKER);
    for field in Field::ALL {
        let global = &globals[field as usize];
        assert_eq!(global.name().unwrap(), field.name());
        assert_eq!(global.value_type(), field.ty());
        assert_eq!(output.node(global.root()).unwrap().data(), 0);
    }
    assert_eq!(
        globals[Field::Passes as usize].root(),
        globals[Field::Kernel as usize].root()
    );
    assert_ne!(
        globals[Field::Passes as usize].root(),
        globals[Field::RadiusFrom as usize].root()
    );
    let hs::Expression::Call {
        name: "begin",
        children,
        ..
    } = output
        .expression(output.script("intro").unwrap().root())
        .unwrap()
    else {
        panic!("blur body")
    };
    let fields = [
        Field::Passes,
        Field::Kernel,
        Field::RadiusFrom,
        Field::RadiusTo,
        Field::BlurSeconds,
        Field::Video,
        Field::BlurTick,
    ];
    let children = children.collect::<hs::Result<Vec<_>>>().unwrap();
    assert_eq!(children.len(), fields.len());
    for (call, field) in children.into_iter().zip(fields) {
        let hs::Expression::Call {
            name: "set",
            children,
            ..
        } = output.expression(call).unwrap()
        else {
            panic!("expected a global write")
        };
        let args = children.collect::<hs::Result<Vec<_>>>().unwrap();
        let variable = output.node(args[0]).unwrap();
        assert!(variable.global());
        assert_eq!(variable.data(), field as u32);
        if field == Field::BlurTick {
            assert!(matches!(
                output.expression(args[1]).unwrap(),
                hs::Expression::Call {
                    function: 888,
                    name: "game_time",
                    ..
                }
            ));
        }
    }
}
