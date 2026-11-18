//! The communicator's device types (docs/live/live-voice.md, 2.1): the
//! microphone and headphone types answer with the voice port mask, each
//! with its own change state, and nothing changes while the mask is 0.
mod common;

use common::*;
use input::InputManager;
use input::host::ConvertConfig;
use xapi::{Config, VoiceTypes, Xapi};

const MIC: u32 = 0x200;
const PHONE: u32 = 0x20c;

fn with_voice() -> (Xapi<Utility>, Memory, Pads) {
    let pads = Pads::new();
    let mut input = InputManager::new(ConvertConfig::default()).unwrap();
    input.add_backend(Box::new(pads.clone()));
    let mut config = Config::for_gamepad(0x100);
    config.cache_drive.globals = test_cache_globals();
    config.devices.voice = Some(VoiceTypes {
        microphone: MIC,
        headphone: PHONE,
    });
    let mut xapi = Xapi::new(config, input, Utility::default()).unwrap();
    let mut memory = Memory::new();
    xapi.init_devices(&mut memory, 0, 0).unwrap();
    (xapi, memory, pads)
}

fn changes(xapi: &mut Xapi<Utility>, memory: &mut Memory, ty: u32) -> (bool, u32, u32) {
    let any = xapi.get_device_changes(memory, ty, 0x400, 0x404).unwrap();
    (any, memory.word(0x400), memory.word(0x404))
}

#[test]
fn retail_types_are_the_traced_objects() {
    assert_eq!(VoiceTypes::RETAIL.microphone, 0x0040_88b0);
    assert_eq!(VoiceTypes::RETAIL.headphone, 0x0040_88bc);
    assert_eq!(Config::for_gamepad(0x100).devices.voice, None);
}

#[test]
fn mask_zero_reports_no_communicator() {
    let (mut xapi, mut memory, _) = with_voice();
    assert_eq!(xapi.get_devices(MIC), 0);
    assert_eq!(xapi.get_devices(PHONE), 0);
    assert_eq!(xapi.peek_devices(&mut memory, MIC, 0, 0).unwrap(), 0);
    assert_eq!(changes(&mut xapi, &mut memory, MIC), (false, 0, 0));
    assert_eq!(changes(&mut xapi, &mut memory, PHONE), (false, 0, 0));
    // The type objects hold Cur/Chg/Prev, all zero.
    assert_eq!(
        (memory.word(MIC), memory.word(MIC + 4), memory.word(MIC + 8)),
        (0, 0, 0)
    );
}

#[test]
fn insertion_and_removal_reach_both_types_once() {
    let (mut xapi, mut memory, _) = with_voice();
    xapi.set_voice_ports(0b0101);
    assert_eq!(xapi.get_devices(MIC), 0b0101);
    assert_eq!(
        xapi.peek_devices(&mut memory, PHONE, 0x408, 0).unwrap(),
        0b0101
    );
    assert_eq!(memory.word(0x408), 0, "previous before consumption");

    assert_eq!(changes(&mut xapi, &mut memory, MIC), (true, 0b0101, 0));
    assert_eq!(
        (memory.word(MIC), memory.word(MIC + 4), memory.word(MIC + 8)),
        (0b0101, 0, 0b0101)
    );
    assert_eq!(changes(&mut xapi, &mut memory, MIC), (false, 0, 0));
    // The headphone type has its own state: it still sees the insertion.
    assert_eq!(changes(&mut xapi, &mut memory, PHONE), (true, 0b0101, 0));

    xapi.set_voice_ports(0b0001);
    assert_eq!(changes(&mut xapi, &mut memory, MIC), (true, 0, 0b0100));
    assert_eq!(changes(&mut xapi, &mut memory, PHONE), (true, 0, 0b0100));
}

#[test]
fn gamepad_changes_are_untouched_by_voice() {
    let (mut xapi, mut memory, pads) = with_voice();
    pads.present(true);
    xapi.set_voice_ports(0b0010);
    assert_eq!(changes(&mut xapi, &mut memory, 0x100), (true, 1, 0));
    assert_eq!(changes(&mut xapi, &mut memory, MIC), (true, 0b0010, 0));
    assert_eq!(xapi.get_devices(0x100), 1);
}

#[test]
fn without_voice_types_the_mask_is_ignored() {
    let (mut xapi, mut memory, _) = initialized();
    xapi.set_voice_ports(0xf);
    assert_eq!(xapi.voice_ports(), 0);
    assert_eq!(xapi.get_devices(0x0040_88b0), 0);
    assert_eq!(xapi.get_devices(MIC), 0);
    assert_eq!(changes(&mut xapi, &mut memory, MIC), (false, 0, 0));
    assert_eq!(
        memory.word(MIC),
        0xcccc_cccc,
        "unknown type objects untouched"
    );
}

#[test]
fn ports_above_three_are_dropped() {
    let (mut xapi, _, _) = with_voice();
    xapi.set_voice_ports(0xff);
    assert_eq!(xapi.voice_ports(), 0xf);
}
