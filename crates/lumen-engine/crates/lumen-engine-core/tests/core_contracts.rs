//! Core contract tests: clock arithmetic, value forms,
//! track sampling against the seam's conventions, the
//! dialogue / effect / audio sub-runtimes, the flash law,
//! state snapshot/restore event-stream equality, and
//! strict record parsing. Half validation, half
//! adversarial (the estate's test split).

use std::collections::BTreeMap;

use lumen_engine_core::{
    AudioBindings, DialogueRuntime, EffectPlayer, EngineError, FixedClock, Runtime, SceneRecord,
    StateRecord, ValueExpr, FLASH_MAX_PER_SECOND,
};

fn names(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

/* ---------------- clock ---------------- */

#[test]
fn clock_time_is_step_over_rate() {
    let mut clock = FixedClock::new(60).expect("clock");
    assert_eq!(clock.time_seconds(), 0.0);
    for _ in 0..90 {
        clock.advance();
    }
    assert_eq!(clock.step(), 90);
    assert_eq!(clock.time_seconds(), 1.5);
    assert_eq!(clock.dt_seconds(), 1.0 / 60.0);
}

#[test]
fn clock_refuses_zero_rate() {
    let err = FixedClock::new(0).expect_err("zero rate refused");
    assert_eq!(err.code(), "LE-109");
}

/* ---------------- value forms ---------------- */

#[test]
fn value_forms_evaluate() {
    let ns = names(&[("g", 0.7), ("cd", 0.8)]);
    assert_eq!(ValueExpr::Const(1.5).eval(&ns).expect("const"), 1.5);
    assert_eq!(ValueExpr::Ref("g".into()).eval(&ns).expect("ref"), 0.7);
    assert_eq!(
        ValueExpr::Add("g".into(), 0.05).eval(&ns).expect("add"),
        0.75
    );
    assert_eq!(
        ValueExpr::Mul("cd".into(), 0.3125).eval(&ns).expect("mul"),
        0.25
    );
}

#[test]
fn value_form_unknown_name_is_refused() {
    let ns = names(&[]);
    let err = ValueExpr::Ref("nope".into())
        .eval(&ns)
        .expect_err("unknown name refused");
    assert!(matches!(err, EngineError::UnknownName(_)));
    assert_eq!(err.code(), "LE-103");
}

/* ---------------- tracks ---------------- */

#[test]
fn pose_track_interpolates_per_parameter_with_zero_default() {
    let record = SceneRecord::load_ron(
        r#"(version: 1, id: "t", steps: 10, rate: 60, settle_steps: 0,
            actors: [(id: "a", display: "A", rig: "r",
                      pose_track: [(t: 0.0, params: {}),
                                   (t: 2.0, params: {"P": Const(1.0)})],
                      placement: Fixed(x: 0.0, y: 0.0))],
            contacts: [], roots: ["Root"], canvas: (width: 4, height: 4, scale: 1.0),
            key_frames: [], tune: None, dialogue: None, effects: [], audio: [])"#,
    )
    .expect("record parses");
    let track = lumen_engine_core::resolve_pose_track(&record.actors[0].pose_track, &names(&[]))
        .expect("track resolves");
    // Halfway between the keys, smoothstep(0.5) = 0.5.
    let mid = lumen_engine_core::sample_pose_track(&track, 1.0);
    assert_eq!(mid, vec![("P".to_string(), [0.5, 0.0])]);
    // Before the first key and after the last: the key itself.
    assert!(lumen_engine_core::sample_pose_track(&track, 0.0).is_empty());
    let end = lumen_engine_core::sample_pose_track(&track, 3.0);
    assert_eq!(end, vec![("P".to_string(), [1.0, 0.0])]);
}

#[test]
fn pivot_turn_folds_about_the_pivot() {
    // At the peak key the pivot point maps to itself plus
    // ground lift plus full compensation.
    let keys = [(0.0, 0.0), (1.0, 30.0), (2.0, 0.0)];
    let pivot = [10.0, -100.0];
    let placement = lumen_engine_core::sample_pivot_turn(&keys, 12.0, 271.0, pivot, 1.0);
    assert_eq!(placement.rotation_degrees, 30.0);
    let at_pivot = placement.apply(pivot);
    assert!((at_pivot[0] - pivot[0]).abs() < 1e-9);
    assert!((at_pivot[1] - (pivot[1] + 271.0 + 12.0)).abs() < 1e-9);
    // At rest: pure ground lift.
    let rest = lumen_engine_core::sample_pivot_turn(&keys, 12.0, 271.0, pivot, 0.0);
    assert_eq!(rest.translation, [0.0, 271.0]);
    assert_eq!(rest.rotation_degrees, 0.0);
}

#[test]
fn step_in_out_eases_match_the_seam_values() {
    // Step-in 90 over [0, 2]: at t=0 the full offset, at
    // t=2 none; step-back 70 over [10, 11].
    let at0 = lumen_engine_core::sample_step_in_out(
        5.0,
        90.0,
        (0.0, 2.0),
        70.0,
        (10.0, 11.0),
        271.0,
        0.0,
    );
    assert_eq!(at0.translation, [95.0, 271.0]);
    let at2 = lumen_engine_core::sample_step_in_out(
        5.0,
        90.0,
        (0.0, 2.0),
        70.0,
        (10.0, 11.0),
        271.0,
        2.0,
    );
    assert_eq!(at2.translation, [5.0, 271.0]);
    let at11 = lumen_engine_core::sample_step_in_out(
        5.0,
        90.0,
        (0.0, 2.0),
        70.0,
        (10.0, 11.0),
        271.0,
        11.0,
    );
    assert_eq!(at11.translation, [75.0, 271.0]);
}

/* ---------------- sub-runtimes ---------------- */

const PROBE_SCENE: &str = r#"
(version: 1, id: "probe", steps: 720, rate: 60, settle_steps: 0,
 actors: [(id: "a", display: "A", rig: "r",
           pose_track: [(t: 0.0, params: {})],
           placement: Fixed(x: 0.0, y: 0.0))],
 contacts: [], roots: ["Root"], canvas: (width: 4, height: 4, scale: 1.0),
 key_frames: [], tune: None,
 dialogue: Some((start_step: 10, chars_per_step: 2, hold_steps: 3,
                 lines: [(speaker: "A", text: "hello"),
                         (speaker: "B", text: "hi")])),
 effects: [(id: "snap", start_step: 20, life_steps: 12,
            keys: [(at_step: 0, kind: Burst, x: 1.0, y: 2.0, count: 8, seed: 7, strobe: false),
                   (at_step: 5, kind: Ring, x: 0.0, y: 0.0, count: 4, seed: 9, strobe: false)])],
 audio: [(id: "cue1", cue: "lift.strain", bus: "sfx", gain_db: -3.0, at_step: Some(30), window: None),
         (id: "bed", cue: "room.tone", bus: "music", gain_db: -20.0, at_step: None, window: Some((0, 100, 50)))])
"#;

#[test]
fn probe_scene_parses_and_steps() {
    let scene = SceneRecord::load_ron(PROBE_SCENE).expect("probe scene parses");
    let mut runtime = Runtime::load(&scene).expect("runtime loads");
    let mut log = Vec::new();
    for _ in 0..200 {
        let events = runtime.step();
        if !events.is_empty() {
            log.push(events);
        }
    }
    // Dialogue: starts at 10, line 0 completes at 12
    // (5 chars at 2/step: steps 10, 11, 12), holds 3
    // (steps 13-15, advancing on the third), line 1
    // completes at 16, holds, done at 19.
    let flat: Vec<String> = log
        .iter()
        .flat_map(|e| {
            e.dialogue
                .iter()
                .map(|d| format!("{}:{d:?}", e.step))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        flat.iter().any(|s| s.starts_with("10:LineStarted")),
        "{flat:?}"
    );
    assert!(
        flat.iter().any(|s| s.starts_with("12:LineCompleted")),
        "{flat:?}"
    );
    assert!(
        flat.iter().any(|s| s.starts_with("19:DialogueCompleted")),
        "{flat:?}"
    );
    // Effects fire at 20 and 25.
    let fx: Vec<u64> = log
        .iter()
        .filter(|e| !e.effects.is_empty())
        .map(|e| e.step)
        .collect();
    assert_eq!(fx, vec![20, 25]);
    // Audio: bed at 0, 50, 100; one-shot at 30.
    let au: Vec<u64> = log
        .iter()
        .filter(|e| !e.audio.is_empty())
        .map(|e| e.step)
        .collect();
    assert_eq!(au, vec![0, 30, 50, 100]);
}

#[test]
fn snapshot_restore_reproduces_the_event_stream() {
    let scene = SceneRecord::load_ron(PROBE_SCENE).expect("probe scene parses");
    let mut whole = Runtime::load(&scene).expect("runtime loads");
    let mut whole_log = Vec::new();
    for _ in 0..200 {
        whole_log.push(whole.step());
    }
    let mut first = Runtime::load(&scene).expect("runtime loads");
    for _ in 0..14 {
        first.step();
    }
    let state = first.snapshot();
    let ron = state.to_ron();
    let parsed = StateRecord::load_ron(&ron).expect("state parses");
    assert_eq!(parsed, state);
    assert_eq!(parsed.hash(), state.hash());
    let mut restored = Runtime::restore(&scene, &parsed).expect("restore");
    let mut tail = Vec::new();
    for _ in 14..200 {
        tail.push(restored.step());
    }
    assert_eq!(tail, whole_log[14..]);
}

#[test]
fn restore_into_the_wrong_scene_is_refused() {
    let scene = SceneRecord::load_ron(PROBE_SCENE).expect("probe scene parses");
    let runtime = Runtime::load(&scene).expect("runtime loads");
    let mut state = runtime.snapshot();
    state.scene_id = "other".into();
    let err = Runtime::restore(&scene, &state).expect_err("mismatch refused");
    assert_eq!(err.code(), "LE-108");
}

/* ---------------- flash law + refusals (adversarial) ---------------- */

#[test]
fn flash_law_refuses_a_four_strobe_second() {
    let keys: Vec<String> = (0..=FLASH_MAX_PER_SECOND)
        .map(|i| {
            format!(
                "(at_step: {i}, kind: Burst, x: 0.0, y: 0.0, count: 1, seed: {i}, strobe: true)"
            )
        })
        .collect();
    let text = format!(
        "(version: 1, id: \"f\", steps: 120, rate: 60, settle_steps: 0,
          actors: [(id: \"a\", display: \"A\", rig: \"r\",
                    pose_track: [(t: 0.0, params: {{}})],
                    placement: Fixed(x: 0.0, y: 0.0))],
          contacts: [], roots: [\"Root\"], canvas: (width: 4, height: 4, scale: 1.0),
          key_frames: [], tune: None, dialogue: None,
          effects: [(id: \"s\", start_step: 0, life_steps: 4, keys: [{}])], audio: [])",
        keys.join(", ")
    );
    let scene = SceneRecord::load_ron(&text).expect("scene parses");
    let err = Runtime::load(&scene).expect_err("flash law refuses");
    assert_eq!(err.code(), "LE-106");
}

#[test]
fn three_strobes_in_a_second_are_legal() {
    let keys: Vec<String> = (0..FLASH_MAX_PER_SECOND)
        .map(|i| {
            format!(
                "(at_step: {i}, kind: Burst, x: 0.0, y: 0.0, count: 1, seed: {i}, strobe: true)"
            )
        })
        .collect();
    let text = format!(
        "(version: 1, id: \"f\", steps: 120, rate: 60, settle_steps: 0,
          actors: [(id: \"a\", display: \"A\", rig: \"r\",
                    pose_track: [(t: 0.0, params: {{}})],
                    placement: Fixed(x: 0.0, y: 0.0))],
          contacts: [], roots: [\"Root\"], canvas: (width: 4, height: 4, scale: 1.0),
          key_frames: [], tune: None, dialogue: None,
          effects: [(id: \"s\", start_step: 0, life_steps: 4, keys: [{}])], audio: [])",
        keys.join(", ")
    );
    let scene = SceneRecord::load_ron(&text).expect("scene parses");
    Runtime::load(&scene).expect("three strobes load");
}

#[test]
fn unknown_fields_and_bad_versions_are_refused() {
    let unknown = PROBE_SCENE.replace("id: \"probe\"", "id: \"probe\", bogus: 3");
    let err = SceneRecord::load_ron(&unknown).expect_err("unknown field refused");
    assert_eq!(err.code(), "LE-101");
    let bad_version = PROBE_SCENE.replace("version: 1", "version: 99");
    let err = SceneRecord::load_ron(&bad_version).expect_err("bad version refused");
    assert_eq!(err.code(), "LE-102");
}

#[test]
fn dialogue_and_audio_refusals() {
    let mut runtime_scene = SceneRecord::load_ron(PROBE_SCENE).expect("scene parses");
    runtime_scene.audio[0].bus = "loud".into();
    let err = AudioBindings::new(&runtime_scene.audio).expect_err("bad bus refused");
    assert_eq!(err.code(), "LE-107");

    let mut dialogue = runtime_scene.dialogue.clone().expect("dialogue present");
    dialogue.chars_per_step = 0;
    let err = DialogueRuntime::new(&dialogue).expect_err("zero pace refused");
    assert_eq!(err.code(), "LE-105");
}

#[test]
fn particle_positions_are_pure() {
    use lumen_engine_core::{particle_position, EffectKind};
    let a = particle_position(EffectKind::Burst, 42, 3, 10);
    let b = particle_position(EffectKind::Burst, 42, 3, 10);
    assert_eq!(a, b);
    let c = particle_position(EffectKind::Burst, 43, 3, 10);
    assert_ne!(a, c);
    // Ring radius grows at 2 units per step regardless of seed.
    let ring = particle_position(EffectKind::Ring, 1, 0, 5);
    assert!((ring[0].hypot(ring[1]) - 10.0).abs() < 1e-9);
}

#[test]
fn effect_player_counts_live_particles() {
    let scene = SceneRecord::load_ron(PROBE_SCENE).expect("scene parses");
    let player = EffectPlayer::new(&scene.effects, 60).expect("player builds");
    assert_eq!(player.live_particles(19), 0);
    assert_eq!(player.live_particles(20), 8);
    assert_eq!(player.live_particles(25), 12);
    assert_eq!(player.live_particles(32), 4);
    assert_eq!(player.live_particles(37), 0);
}
