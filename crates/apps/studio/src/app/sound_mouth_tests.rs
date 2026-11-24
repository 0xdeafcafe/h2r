//! Mouth curve writes on invented data: the refusals, the generator.
use std::sync::Arc;

use map::{RawPointer, ResourceFile};
use tags::sound::MouthSpan;

use super::audition::Clip;
use super::sound_mouth::{check_writable, envelope};

fn span(file: ResourceFile, len: usize) -> MouthSpan {
    MouthSpan {
        resource: RawPointer {
            file,
            offset: 0x4000,
        },
        resource_size: 0x200,
        offset: 0x40,
        curve: vec![1; len],
    }
}

#[test]
fn a_same_length_curve_in_the_local_file_may_be_written() {
    assert_eq!(
        check_writable(&span(ResourceFile::Local, 4), 1, &[9, 9, 9, 9]),
        Ok(())
    );
    // No chunks (a sound with no audio) is not a chain.
    assert_eq!(
        check_writable(&span(ResourceFile::Local, 2), 0, &[0, 0]),
        Ok(())
    );
}

#[test]
fn a_shared_cache_a_split_chain_and_another_length_are_refused_by_name() {
    let why = check_writable(&span(ResourceFile::Shared, 4), 1, &[0; 4]).unwrap_err();
    assert!(
        why.contains("shared cache") && why.contains("never writes"),
        "{why}"
    );
    let why = check_writable(&span(ResourceFile::MainMenu, 4), 1, &[0; 4]).unwrap_err();
    assert!(why.contains("MainMenu"), "{why}");
    let why = check_writable(&span(ResourceFile::Local, 4), 3, &[0; 4]).unwrap_err();
    assert!(why.contains("split chain of 3 chunks"), "{why}");
    let why = check_writable(&span(ResourceFile::Local, 4), 1, &[0; 5]).unwrap_err();
    assert!(why.contains("4 ticks and 5 were given"), "{why}");
}

fn clip(rate: u32, pcm: &[i16]) -> Clip {
    Clip {
        rate,
        channels: 1,
        pcm: Arc::from(pcm),
    }
}

#[test]
fn the_envelope_scales_the_loudest_tick_to_the_gain() {
    // 30 Hz: one frame per tick.
    let c = clip(30, &[0, 16384, 32767, 0]);
    assert_eq!(envelope(&c, 4, 1.0, 0.0), vec![0, 128, 255, 0]);
    // Half the gain: the loudest tick is half open.
    assert_eq!(envelope(&c, 4, 0.5, 0.0), vec![0, 64, 128, 0]);
    // More than one clamps.
    assert_eq!(envelope(&c, 4, 4.0, 0.0), vec![0, 255, 255, 0]);
    // Smoothing carries a tick into the next.
    assert_eq!(envelope(&c, 4, 1.0, 0.5), vec![0, 64, 159, 80]);
}

#[test]
fn the_envelope_keeps_the_asked_length_and_a_silent_clip_is_closed() {
    let c = clip(30, &[0, 0, 0]);
    assert_eq!(envelope(&c, 3, 1.0, 0.3), vec![0, 0, 0]);
    // Ticks past the audio are silent; the length is the curve's.
    let c = clip(30, &[1000, 1000]);
    assert_eq!(envelope(&c, 5, 1.0, 0.0), vec![255, 255, 0, 0, 0]);
    // A stereo frame is the average of its channels.
    let c = Clip {
        rate: 30,
        channels: 2,
        pcm: Arc::from(&[1000i16, -1000, 2000, 2000][..]),
    };
    assert_eq!(envelope(&c, 2, 1.0, 0.0), vec![0, 255]);
}
