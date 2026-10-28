//! Motion conformance: the torogao cast repertoire exports
//! (Prima, Yui, Alma × Greeting, Daydream) load against their
//! models through the real bridge path — `parse_model` then
//! `parse_motion` — and evaluate deterministically. Fixture
//! provenance: `fixtures/conformance/README.md`.

use std::fs;
use std::path::Path;

use lumen_rig_core::{Model, Motion, MotionPlayer, parse_model, parse_motion};

fn fixture(relative: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join(relative);
    fs::read(&path).unwrap_or_else(|error| panic!("fixture {relative}: {error}"))
}

fn load(member: &str, performance: &str) -> (Model, Motion) {
    let model = parse_model(&fixture(&format!(
        "conformance/{member}-repertoire.model.json"
    )))
    .unwrap_or_else(|error| panic!("{member} model should parse: {error}"));
    let motion = parse_motion(
        &fixture(&format!(
            "conformance/{member}-repertoire.motion-{performance}.json"
        )),
        &model,
    )
    .unwrap_or_else(|error| panic!("{member} {performance} should parse: {error}"));
    (model, motion)
}

/// FNV-1a over a full fixed-step performance: start, play at
/// 1/120 s steps past the end (the non-looping player clamps
/// and holds), stop, fade out. Same law as the utilization
/// example; the gate report records the values per machine.
fn performance_hash(motion: &Motion) -> u64 {
    let mut player = MotionPlayer::new(motion);
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mix = |hash: &mut u64, bytes: &[u8]| {
        for byte in bytes {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    player.start();
    for _ in 0..480 {
        player.advance(1.0 / 120.0);
        mix(&mut hash, &player.time_seconds().to_bits().to_le_bytes());
        for entry in player.evaluate() {
            mix(&mut hash, entry.parameter.as_bytes());
            mix(&mut hash, &entry.weight.to_bits().to_le_bytes());
            mix(&mut hash, &entry.value.to_bits().to_le_bytes());
        }
    }
    player.stop();
    for _ in 0..60 {
        player.advance(1.0 / 120.0);
        for entry in player.evaluate() {
            mix(&mut hash, &entry.value.to_bits().to_le_bytes());
        }
    }
    hash
}

#[test]
fn all_six_cast_motions_load_bind_and_evaluate() {
    for member in ["prima", "yui", "alma"] {
        for performance in ["Greeting", "Daydream"] {
            let (model, motion) = load(member, performance);
            assert_eq!(motion.model, model.id);
            assert_eq!(motion.id, performance);
            assert_eq!(motion.frames_per_second, 60);
            assert!(!motion.looping);
            assert_eq!(motion.curves.len(), 10, "{member} {performance}");
            // Every evaluated value lands inside its
            // parameter's declared range (the loader checked
            // the keys; evaluation must not escape them).
            let mut player = MotionPlayer::new(&motion);
            player.start();
            for _ in 0..240 {
                player.advance(1.0 / 120.0);
                for entry in player.evaluate() {
                    let param = model
                        .parameters
                        .iter()
                        .find(|param| param.id() == entry.parameter)
                        .expect("contribution targets a model parameter");
                    let (min, max) = match param {
                        lumen_rig_core::Parameter::Scalar(scalar) => (scalar.min, scalar.max),
                        lumen_rig_core::Parameter::Combined(combined) => {
                            (combined.min, combined.max)
                        }
                    };
                    assert!(
                        entry.value >= f64::from(min) && entry.value <= f64::from(max),
                        "{member} {performance}: {} value {} outside [{min}, {max}]",
                        entry.parameter,
                        entry.value
                    );
                }
            }
        }
    }
}

#[test]
fn cast_performances_replay_identically() {
    for member in ["prima", "yui", "alma"] {
        for performance in ["Greeting", "Daydream"] {
            let (_model, motion) = load(member, performance);
            let first = performance_hash(&motion);
            // A fresh parse of the same bytes evaluates to the
            // same trace: determinism is a property of the
            // format, not of one process's state.
            let (_model, motion) = load(member, performance);
            let second = performance_hash(&motion);
            assert_eq!(first, second, "{member} {performance}");
        }
    }
}

#[test]
fn cast_motion_for_the_wrong_cast_model_is_refused() {
    let (prima_model, _motion) = load("prima", "Greeting");
    let bytes = fixture("conformance/yui-repertoire.motion-Greeting.json");
    match parse_motion(&bytes, &prima_model) {
        Err(error) => assert_eq!(error.code(), "LRF-203"),
        Ok(_) => panic!("Yui's motion must not bind to Prima's model"),
    }
}
