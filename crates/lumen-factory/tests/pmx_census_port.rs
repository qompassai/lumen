//! Ported Stage 0 prototype integration tests (16), adapted to the
//! lumen-factory module path and fixture location. Validation tests
//! prove the synthetic fixtures census exactly as authored;
//! adversarial tests prove malformed input returns typed errors and
//! never panics. Fixtures are estate-authored bytes; regenerate
//! them from `~/workspace/mmd-port-20261102/fixtures/`, never by
//! editing the .pmx files.

use lumen_factory::convert::pmx::CensusError;
use lumen_factory::convert::pmx::census::census;

fn fixture_bytes(name: &str) -> Vec<u8> {
    let path = format!("{}/fixtures/pmx/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|err| panic!("fixture {path} must exist: {err}"))
}

/// A minimal header + empty model-info prefix, for crafting
/// adversarial inputs whose failure must occur after the header.
fn header_prefix(encoding: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"PMX ");
    bytes.extend_from_slice(&2.0f32.to_le_bytes());
    bytes.push(8); // globals count
    bytes.extend_from_slice(&[encoding, 0, 2, 2, 2, 2, 2, 2]); // globals
    for _ in 0..4 {
        bytes.extend_from_slice(&0i32.to_le_bytes()); // four empty texts
    }
    bytes
}

// ------------------------------------------------------------------
// Validation
// ------------------------------------------------------------------

#[test]
fn utf8_fixture_counts_are_exact() {
    let record = census(&fixture_bytes("minimal-utf8.pmx")).expect("fixture must census");
    assert_eq!(record.pmx_version, 2.0);
    assert_eq!(record.encoding, "utf-8");
    assert_eq!(record.vertex_count, 8);
    assert_eq!(record.surface_count, 12);
    assert_eq!(record.face_count, 4);
    assert_eq!(record.textures.len(), 2);
    assert_eq!(record.materials.len(), 2);
    assert_eq!(record.bones.len(), 10);
    assert_eq!(record.morphs.len(), 4);
    assert_eq!(record.display_frame_count, 2);
    assert_eq!(record.rigid_bodies.len(), 2);
    assert_eq!(record.joint_count, 1);
    assert_eq!(record.soft_body_count, 0);
    assert!(record.material_surface_sum_matches);
}

#[test]
fn utf8_fixture_names_are_verbatim() {
    let record = census(&fixture_bytes("minimal-utf8.pmx")).expect("fixture must census");
    assert_eq!(record.model_name_universal, "Stage0 Fixture Humanoid");
    assert_eq!(record.bones[0].name_local, "全ての親");
    assert_eq!(record.bones[0].parent, -1);
    assert_eq!(record.bones[5].name_universal, "head");
    assert_eq!(record.morphs[0].name_local, "まばたき");
    assert_eq!(record.morphs[0].name_universal, "blink");
    assert_eq!(record.textures[0].path, "fixture_body.png");
    assert_eq!(record.rigid_bodies[0].shape, "sphere");
    assert_eq!(record.rigid_bodies[1].shape, "box");
}

#[test]
fn utf8_fixture_morph_and_deform_tallies() {
    let record = census(&fixture_bytes("minimal-utf8.pmx")).expect("fixture must census");
    assert_eq!(record.morph_type_counts, [1, 3, 0, 0, 0, 0, 0]);
    assert_eq!(record.deform_counts, [5, 1, 1, 1, 0]);
    assert_eq!(record.morphs[0].morph_type, "vertex");
    assert_eq!(record.morphs[0].offset_count, 2);
    assert_eq!(record.morphs[3].morph_type, "group");
}

#[test]
fn utf16_fixture_decodes_to_the_same_model() {
    let utf8 = census(&fixture_bytes("minimal-utf8.pmx")).expect("utf8 fixture");
    let utf16 = census(&fixture_bytes("minimal-utf16.pmx")).expect("utf16 fixture");
    assert_eq!(utf16.encoding, "utf-16le");
    assert_eq!(utf16.vertex_count, utf8.vertex_count);
    assert_eq!(utf16.bones.len(), utf8.bones.len());
    assert_eq!(utf16.model_name_local, utf8.model_name_local);
    assert_eq!(utf16.model_name_universal, utf8.model_name_universal);
    assert_eq!(utf16.bones[1].name_local, "センター");
    assert_eq!(utf16.morphs[0].name_local, utf8.morphs[0].name_local);
}

#[test]
fn census_json_is_byte_stable_across_runs() {
    let bytes = fixture_bytes("minimal-utf8.pmx");
    let first = census(&bytes).expect("first run").to_json();
    let second = census(&bytes).expect("second run").to_json();
    assert_eq!(first, second);
    assert!(first.contains("\"format\": \"pmx-census/1\""));
    assert!(first.contains("\"vertices\": 8"));
}

#[test]
fn census_records_the_file_hash() {
    let bytes = fixture_bytes("minimal-utf8.pmx");
    let record = census(&bytes).expect("fixture must census");
    assert_eq!(record.input_bytes, bytes.len());
    assert_eq!(record.file_sha256.len(), 64);
    assert_eq!(
        record.file_sha256,
        "af496825f1d7d82224e764e754580e7f0d2adec830b7709df69431c9de35f0b0"
    );
}

// ------------------------------------------------------------------
// Adversarial
// ------------------------------------------------------------------

#[test]
fn empty_input_is_a_typed_error() {
    assert!(census(&[]).is_err());
}

#[test]
fn bad_magic_is_refused() {
    let mut bytes = fixture_bytes("minimal-utf8.pmx");
    bytes[0] = b'X';
    assert_eq!(census(&bytes), Err(CensusError::BadMagic));
}

#[test]
fn unsupported_version_is_refused() {
    let mut bytes = fixture_bytes("minimal-utf8.pmx");
    bytes[4..8].copy_from_slice(&3.0f32.to_le_bytes());
    assert!(matches!(
        census(&bytes),
        Err(CensusError::UnsupportedVersion(_))
    ));
}

#[test]
fn wrong_encoding_flag_is_refused() {
    let mut bytes = fixture_bytes("minimal-utf8.pmx");
    bytes[9] = 7;
    assert_eq!(
        census(&bytes),
        Err(CensusError::BadGlobals("text encoding"))
    );
}

#[test]
fn invalid_index_size_is_refused() {
    let mut bytes = fixture_bytes("minimal-utf8.pmx");
    bytes[11] = 3;
    assert_eq!(census(&bytes), Err(CensusError::BadGlobals("index size")));
}

#[test]
fn truncations_never_panic_and_always_error() {
    let bytes = fixture_bytes("minimal-utf8.pmx");
    let cuts = [
        1usize,
        4,
        8,
        9,
        20,
        bytes.len() / 3,
        bytes.len() / 2,
        bytes.len() - 1,
    ];
    for cut in cuts {
        assert!(
            census(&bytes[..cut]).is_err(),
            "truncation at {cut} bytes must be a typed error"
        );
    }
}

#[test]
fn absurd_vertex_count_is_refused_before_allocation() {
    let mut bytes = header_prefix(1);
    bytes.extend_from_slice(&i32::MAX.to_le_bytes());
    assert!(matches!(
        census(&bytes),
        Err(CensusError::CountOutOfRange("vertex count", _))
    ));
}

#[test]
fn negative_texture_count_is_refused() {
    let mut bytes = header_prefix(1);
    bytes.extend_from_slice(&0i32.to_le_bytes());
    bytes.extend_from_slice(&0i32.to_le_bytes());
    bytes.extend_from_slice(&(-1i32).to_le_bytes());
    assert!(matches!(
        census(&bytes),
        Err(CensusError::CountOutOfRange("texture count", _))
    ));
}

#[test]
fn trailing_bytes_are_refused() {
    let mut bytes = fixture_bytes("minimal-utf8.pmx");
    bytes.push(0);
    assert_eq!(census(&bytes), Err(CensusError::TrailingBytes(1)));
}

#[test]
fn surface_count_not_multiple_of_three_is_refused() {
    let mut bytes = header_prefix(1);
    bytes.extend_from_slice(&0i32.to_le_bytes());
    bytes.extend_from_slice(&4i32.to_le_bytes());
    assert!(matches!(
        census(&bytes),
        Err(CensusError::InvalidValue("surface count", 4))
    ));
}
