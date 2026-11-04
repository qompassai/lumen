//! Host-level record tests: the frozen two-body scene
//! record parses, validates, and its tune family key
//! times exist on the hand actor's track. (Rig staging
//! itself is exercised by the host run, not unit tests —
//! the imports live outside the repository.)

use lumen_engine_core::SceneRecord;

#[test]
fn twobody_scene_record_is_well_formed() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scenes/twobody-lift.scene.ron"
    );
    let text = std::fs::read_to_string(path).expect("scene record reads");
    let scene = SceneRecord::load_ron(&text).expect("scene record parses and validates");
    assert_eq!(scene.id, "twobody-lift");
    assert_eq!(scene.steps, 720);
    assert_eq!(scene.rate, 60);
    assert_eq!(scene.actors.len(), 2);
    assert_eq!(scene.contacts.len(), 2);
    let tune = scene.tune.as_ref().expect("tune block present");
    assert_eq!(tune.expected.predicted_slip, 0.33800482554031674);
    assert_eq!(tune.expected.turn_lift, 24.548784220188338);
    // Family key times exist on the hand actor's track.
    let track = &scene.actors[tune.hand_actor].pose_track;
    for at in [
        tune.family.grasp_at,
        tune.family.crouch_at,
        tune.family.lift_at,
        tune.family.setdown_at,
    ] {
        assert!(
            track.iter().any(|k| k.t == at),
            "family key time {at} missing from the hand actor's track"
        );
    }
    // The probe material is present and inside the scene.
    assert!(scene.dialogue.is_some());
    assert_eq!(scene.effects.len(), 1);
    assert_eq!(scene.audio.len(), 3);
}
