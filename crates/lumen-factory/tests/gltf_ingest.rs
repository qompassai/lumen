//! Round-trip + accounting tests for gltf-import/1 (Stage 2 gate).
//!
//! Validation: the synthetic fixture GLB ingests to a record whose
//! zero-silent-drop identities hold per feature class; the ingest
//! JSON is byte-deterministic and pinned as a golden; the profile
//! RON emission is byte-deterministic and carries every table row.
//! Adversarial: a non-PMX GLB and a truncated GLB fail typed; a
//! physics-bearing synthetic model exercises strands, anchors,
//! and the dropped ledger (joint limits/springs, collision,
//! impulse morphs) with the same identities.

use lumen_factory::convert::gltf::{emit_profile_ron, ingest_glb};
use lumen_factory::convert::pmx::census::census;
use lumen_factory::convert::pmx::glb::{ConvertOptions, convert};
use lumen_factory::convert::pmx::parse::{
    Bone, BoneTail, Deform, Globals, Joint, Material, Morph, MorphOffset, PmxModel, RigidBody,
    ToonRef, Vertex, parse_model,
};
use lumen_factory::sha256_hex;

fn fixture_bytes(name: &str) -> Vec<u8> {
    let path = format!("{}/fixtures/pmx/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|err| panic!("fixture {path} must exist: {err}"))
}

fn fixture_glb() -> Vec<u8> {
    let bytes = fixture_bytes("minimal-utf8.pmx");
    let record = census(&bytes).expect("fixture censuses");
    let model = parse_model(&bytes).expect("fixture parses");
    let options = ConvertOptions {
        source_sha256: Some(record.file_sha256),
        ..ConvertOptions::default()
    };
    convert(&model, &options).expect("fixture converts").0
}

fn assert_identities(record: &lumen_factory::convert::gltf::IngestRecord) {
    for row in &record.accounting {
        assert_eq!(
            row.mapped_count + row.ledgered_count,
            row.census_count,
            "zero-silent-drop identity failed for {}",
            row.feature
        );
    }
}

// ------------------------------------------------------------------
// Validation
// ------------------------------------------------------------------

#[test]
fn fixture_round_trip_accounting_identities_hold() {
    let glb = fixture_glb();
    let record = ingest_glb(&glb).expect("fixture GLB ingests");
    assert_eq!(record.schema, "torogao-mmd-ingest/1");
    assert_eq!(record.provenance.profile_id, "mmd-profile/1");
    assert_eq!(record.provenance.glb_sha256, sha256_hex(&glb));
    let census = census(&fixture_bytes("minimal-utf8.pmx")).expect("census");
    assert_eq!(
        record.provenance.source_pmx_sha256.as_deref(),
        Some(census.file_sha256.as_str()),
        "provenance anchors to the censused source"
    );
    assert_identities(&record);
    // The fixture's standard bones map; its blink/a morph targets
    // carry canonical Fac_ channels.
    let hips = record
        .skeleton
        .iter()
        .filter(|entry| entry.torogao.as_deref() == Some("Hips"))
        .count();
    assert!(hips >= 1, "fixture maps a Hips joint");
    let blink = record
        .face_parameters
        .iter()
        .find(|entry| entry.name_universal == "blink")
        .expect("blink morph row");
    assert_eq!(blink.fac_channel.as_deref(), Some("Fac_Eye_Close"));
}

#[test]
fn fixture_ingest_json_is_deterministic_and_pinned() {
    let glb = fixture_glb();
    let first = ingest_glb(&glb).expect("ingests").to_json();
    let second = ingest_glb(&glb).expect("ingests").to_json();
    assert_eq!(first, second, "ingest record must be byte-deterministic");
    assert_eq!(
        sha256_hex(first.as_bytes()),
        "329d0472edf2b848e07667afdbffd6ecb74316ba08d49f59229d83400adceb30",
        "ingest golden moved: {}",
        sha256_hex(first.as_bytes())
    );
}

#[test]
fn profile_ron_emission_is_deterministic_and_complete() {
    let first = emit_profile_ron();
    let second = emit_profile_ron();
    assert_eq!(first, second, "RON emission must be byte-deterministic");
    assert!(first.contains("id: \"mmd-profile/1\""));
    // Every profile bone row and morph row appears exactly once.
    for name in ["センター", "頭", "左手首", "右つま先"] {
        assert!(first.contains(name), "bone row for {name} in RON");
    }
    for channel in ["Fac_Eye_Close", "Fac_Mth_Aa", "Fac_Ebr_Relax"] {
        assert!(first.contains(channel), "Fac_ channel {channel} in RON");
    }
    assert!(first.contains("unmapped_ledgered_verbatim"));
    assert!(first.contains("machine-derived"));
}

// ------------------------------------------------------------------
// Physics-bearing synthetic model
// ------------------------------------------------------------------

fn physics_model() -> PmxModel {
    let globals = Globals {
        encoding_utf8: true,
        additional_vec4_count: 0,
        vertex_index_size: 2,
        texture_index_size: 2,
        material_index_size: 2,
        bone_index_size: 2,
        morph_index_size: 2,
        rigid_index_size: 2,
    };
    let bone = |name: &str, parent: i64, y: f32| Bone {
        name_local: name.to_string(),
        name_universal: String::new(),
        position: [0.0, y, 0.0],
        parent,
        layer: 0,
        flags: 0x0001,
        tail: BoneTail::Offset([0.0, -1.0, 0.0]),
        inherit: None,
        fixed_axis: None,
        local_axes: None,
        external_parent: None,
        ik: None,
    };
    let rigid = |name: &str, bone: i64, mode: u8, group: u8| RigidBody {
        name_local: name.to_string(),
        name_universal: String::new(),
        bone,
        group_id: group,
        non_collision_mask: 0,
        shape: 2,
        size: [0.5, 1.0, 0.0],
        position: [0.0, 1.0, 0.0],
        rotation: [0.0, 0.0, 0.0],
        mass: 1.0,
        move_attenuation: 0.25,
        rotation_damping: 0.5,
        repulsion: 0.0,
        friction: 0.5,
        physics_mode: mode,
    };
    PmxModel {
        pmx_version: 2.0,
        globals,
        model_name_local: String::new(),
        model_name_universal: "synthetic physics".to_string(),
        model_comment_local: String::new(),
        model_comment_universal: String::new(),
        vertices: vec![Vertex {
            position: [0.0, 0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            additional: Vec::new(),
            deform: Deform::Bdef1 { bone: 0 },
            edge_scale: 1.0,
        }],
        surface_indices: Vec::new(),
        textures: Vec::new(),
        materials: vec![Material {
            name_local: String::new(),
            name_universal: "mat".to_string(),
            diffuse: [1.0, 1.0, 1.0, 1.0],
            specular: [0.0, 0.0, 0.0],
            specular_strength: 0.0,
            ambient: [0.0, 0.0, 0.0],
            flags: 0,
            edge_color: [0.0, 0.0, 0.0, 1.0],
            edge_scale: 1.0,
            texture_index: -1,
            environment_index: -1,
            environment_blend: 0,
            toon: ToonRef::Internal(0),
            meta: String::new(),
            surface_count: 0,
        }],
        bones: vec![
            bone("センター", -1, 0.0),
            bone("髪1", 0, 1.0),
            bone("髪2", 1, 2.0),
        ],
        morphs: vec![
            Morph {
                name_local: "まばたき".to_string(),
                name_universal: "blink".to_string(),
                panel: 2,
                morph_type: 1,
                offsets: vec![MorphOffset::Vertex {
                    vertex: 0,
                    translation: [0.0, -0.1, 0.0],
                }],
            },
            Morph {
                name_local: "まとめ".to_string(),
                name_universal: "combo".to_string(),
                panel: 4,
                morph_type: 0,
                offsets: vec![MorphOffset::Group {
                    morph: 0,
                    influence: 1.0,
                }],
            },
            Morph {
                name_local: "衝撃".to_string(),
                name_universal: "kick".to_string(),
                panel: 4,
                morph_type: 10,
                offsets: Vec::new(),
            },
        ],
        display_frames: Vec::new(),
        rigid_bodies: vec![
            rigid("anchor", 0, 0, 1),
            rigid("hair_a", 1, 1, 2),
            rigid("hair_b", 2, 1, 2),
        ],
        joints: vec![Joint {
            name_local: "joint1".to_string(),
            name_universal: String::new(),
            joint_type: 0,
            rigid_a: 1,
            rigid_b: 2,
            position: [0.0, 1.5, 0.0],
            rotation: [0.0, 0.0, 0.0],
            position_min: [0.0, 0.0, 0.0],
            position_max: [0.0, 0.0, 0.0],
            rotation_min: [-0.5, 0.0, 0.0],
            rotation_max: [0.5, 0.0, 0.0],
            position_spring: [0.0, 0.0, 0.0],
            rotation_spring: [10.0, 0.0, 0.0],
        }],
        soft_bodies: Vec::new(),
    }
}

#[test]
fn physics_model_strands_anchors_and_ledger() {
    let model = physics_model();
    let (glb, _) = convert(&model, &ConvertOptions::default()).expect("converts");
    let record = ingest_glb(&glb).expect("ingests");
    assert_identities(&record);
    // follow-bone rigid -> one static anchor; the hair chain -> one
    // strand of two particles in chain order.
    assert_eq!(record.physics.static_anchors.len(), 1);
    assert_eq!(record.physics.strands.len(), 1);
    let strand = &record.physics.strands[0];
    assert_eq!(strand.particles.len(), 2);
    assert_eq!(strand.particles[0].bone_name, "髪1");
    assert_eq!(strand.particles[1].bone_name, "髪2");
    // Capsule radius = size[0] * scale; mobility = 1 - attenuation.
    assert!((strand.particles[0].radius - 0.5 * 0.08).abs() < 1e-6);
    assert!((strand.particles[0].mobility - 0.75).abs() < 1e-6);
    assert_eq!(record.physics.groups.len(), 1);
    // The joint is fully ledgered: limits + springs + collision.
    assert_eq!(record.dropped_ledger.joint_axis_limits.len(), 1);
    assert_eq!(
        record.dropped_ledger.joint_axis_limits[0].rotation_max[0],
        0.5
    );
    assert_eq!(record.dropped_ledger.joint_springs.len(), 1);
    assert_eq!(
        record.dropped_ledger.joint_springs[0].rotation_spring[0],
        10.0
    );
    assert_eq!(record.dropped_ledger.collision.len(), 3);
    assert_eq!(
        record.dropped_ledger.impulse_morphs,
        vec!["kick".to_string()]
    );
    // The group morph routes to its recipe; the blink morph maps.
    let combo = record
        .face_parameters
        .iter()
        .find(|entry| entry.name_universal == "combo")
        .expect("combo row");
    assert_eq!(combo.route, "recipe");
}

// ------------------------------------------------------------------
// Adversarial
// ------------------------------------------------------------------

#[test]
fn non_pmx_glb_is_refused_typed() {
    let glb = lumen_factory::gltf::build_glb(
        &serde_json::json!({"asset": {"version": "2.0", "generator": "test"}}),
        &[],
    )
    .expect("plain glb");
    let err = ingest_glb(&glb).expect_err("non-PMX GLB must be refused");
    assert!(
        err.to_string().contains("pmx_provenance"),
        "typed reason: {err}"
    );
}

#[test]
fn truncated_glb_is_refused_typed() {
    let glb = fixture_glb();
    let err = ingest_glb(&glb[..glb.len() / 2]).expect_err("truncated GLB must be refused");
    assert!(!err.to_string().is_empty());
}

#[test]
fn accounting_tamper_breaks_identity() {
    // The identity is arithmetic on the record's own rows: a
    // tampered row (as a downstream consumer would detect) fails.
    let glb = fixture_glb();
    let mut record = ingest_glb(&glb).expect("ingests");
    record.accounting[0].ledgered_count += 1;
    let row = &record.accounting[0];
    assert_ne!(row.mapped_count + row.ledgered_count, row.census_count);
}
