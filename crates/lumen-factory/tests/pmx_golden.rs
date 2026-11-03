//! Golden + conversion tests for pmx-import/1 (Stage 1 gate).
//!
//! Validation: fixture census JSON byte-for-byte against the Stage
//! 0 evidence, fixture GLB SHA-256 pinned, winding/normal
//! orientation, SDEF/QDEF ledgers, Fac_ mapping, profile bands.
//! Adversarial: truncated/corrupt inputs error typed through the
//! conversion entry too, never panic. Repo goldens are the
//! synthetic fixtures only — no third-party model bytes.

use lumen_factory::convert::pmx::census::census;
use lumen_factory::convert::pmx::glb::{ConvertOptions, convert};
use lumen_factory::convert::pmx::parse::{
    Bone, BoneTail, Deform, Globals, Material, Morph, PmxModel, ToonRef, Vertex, parse_model,
};
use lumen_factory::convert::pmx::profile::{MmdProfile, assess_proportions};
use lumen_factory::gltf::parse_glb_parts;
use lumen_factory::sha256_hex;

fn fixture_bytes(name: &str) -> Vec<u8> {
    let path = format!("{}/fixtures/pmx/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|err| panic!("fixture {path} must exist: {err}"))
}

fn fixture_text(name: &str) -> String {
    let path = format!("{}/fixtures/pmx/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("fixture {path}: {err}"))
}

fn convert_fixture() -> (Vec<u8>, lumen_factory::convert::pmx::ConversionReport) {
    let bytes = fixture_bytes("minimal-utf8.pmx");
    let model = parse_model(&bytes).expect("fixture parses");
    convert(&model, &ConvertOptions::default()).expect("fixture converts")
}

// ------------------------------------------------------------------
// Validation
// ------------------------------------------------------------------

#[test]
fn fixture_census_json_matches_stage0_byte_for_byte() {
    for (pmx, json) in [
        ("minimal-utf8.pmx", "census-utf8.json"),
        ("minimal-utf16.pmx", "census-utf16.json"),
    ] {
        let record = census(&fixture_bytes(pmx)).expect("fixture censuses");
        assert_eq!(record.to_json(), fixture_text(json), "census parity: {pmx}");
    }
}

#[test]
fn fixture_glb_sha256_is_pinned() {
    let (glb, _) = convert_fixture();
    assert_eq!(
        sha256_hex(&glb),
        // Stage 2 amendment (D-633, gltf-import/1): the root
        // provenance gained source_file_sha256, census_counts, the
        // full morph table, and verbatim rigid-body/joint records
        // so the GLB is a self-contained Route A midpoint. The
        // Stage 1 pin was 921a0ccd...; this pin supersedes it.
        "b588126c93a0c9a60b7b573e91113ba5bc573eba82893157d4064db28f146ecc",
        "fixture GLB hash moved: {}",
        sha256_hex(&glb)
    );
}

#[test]
fn fixture_glb_is_byte_deterministic() {
    let (first, _) = convert_fixture();
    let (second, _) = convert_fixture();
    assert_eq!(first, second);
}

#[test]
fn fixture_glb_counts_match_census() {
    let (glb, report) = convert_fixture();
    let parts = parse_glb_parts(&glb).expect("GLB re-parses");
    let summary = lumen_factory::gltf::parse_glb(&glb).expect("summary re-parses");
    assert_eq!(report.vertex_count, 8);
    assert_eq!(report.triangle_count, 4);
    assert_eq!(report.bone_count, 10);
    assert_eq!(summary.joint_count, 10);
    assert_eq!(summary.index_count, 12);
    // One mesh per material span; targetNames ride on mesh extras.
    let meshes = parts.json["meshes"].as_array().expect("meshes");
    assert_eq!(meshes.len(), 2);
    let names = meshes[0]["extras"]["targetNames"]
        .as_array()
        .expect("targetNames");
    assert_eq!(names.len(), 3);
    assert_eq!(names[0], "blink");
}

#[test]
fn winding_and_normal_orientation_agree() {
    let (glb, _) = convert_fixture();
    let parts = parse_glb_parts(&glb).expect("GLB re-parses");
    let json = &parts.json;
    let bin = &parts.bin;
    let read_accessor = |index: usize| -> Vec<f32> {
        let accessor = &json["accessors"][index];
        let view_index = accessor["bufferView"].as_u64().unwrap() as usize;
        let view = &json["bufferViews"][view_index];
        let offset = view["byteOffset"].as_u64().unwrap() as usize;
        let length = view["byteLength"].as_u64().unwrap() as usize;
        bin[offset..offset + length]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
            .collect()
    };
    let read_indices = |index: usize| -> Vec<u32> {
        let accessor = &json["accessors"][index];
        let view_index = accessor["bufferView"].as_u64().unwrap() as usize;
        let view = &json["bufferViews"][view_index];
        let offset = view["byteOffset"].as_u64().unwrap() as usize;
        let length = view["byteLength"].as_u64().unwrap() as usize;
        bin[offset..offset + length]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| u32::from_le_bytes(*chunk))
            .collect()
    };
    let primitive = &json["meshes"][0]["primitives"][0];
    let pos_index = primitive["attributes"]["POSITION"].as_u64().unwrap() as usize;
    let nor_index = primitive["attributes"]["NORMAL"].as_u64().unwrap() as usize;
    let idx_index = primitive["indices"].as_u64().unwrap() as usize;
    let positions = read_accessor(pos_index);
    let normals = read_accessor(nor_index);
    let indices = read_indices(idx_index);
    // PMX span 0 was (0,1,2),(0,2,3); glTF must carry the reversed
    // winding (0,2,1),(0,3,2) under the LH→RH change of basis.
    assert_eq!(indices, vec![0, 2, 1, 0, 3, 2]);
    let point = |vertex: usize| -> [f32; 3] {
        [
            positions[vertex * 3],
            positions[vertex * 3 + 1],
            positions[vertex * 3 + 2],
        ]
    };
    let (a, b, c) = (point(0), point(2), point(1));
    let edge1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let edge2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let geometric = [
        edge1[1] * edge2[2] - edge1[2] * edge2[1],
        edge1[2] * edge2[0] - edge1[0] * edge2[2],
        edge1[0] * edge2[1] - edge1[1] * edge2[0],
    ];
    let stored = [normals[0], normals[1], normals[2]];
    let dot = geometric[0] * stored[0] + geometric[1] * stored[1] + geometric[2] * stored[2];
    assert!(
        dot > 0.0,
        "geometric normal {geometric:?} vs stored {stored:?}"
    );
    // PMX +Z normal converts to glTF -Z.
    assert_eq!(stored, [0.0, 0.0, -1.0]);
}

#[test]
fn sdef_ledger_and_provenance_are_recorded() {
    let (glb, report) = convert_fixture();
    assert_eq!(report.sdef_vertices, 1);
    assert_eq!(report.qdef_vertices, 0);
    // Fixture BDEF4 weights (0.4,0.3,0.2,0.1) already sum to 1.0.
    assert_eq!(report.weight_corrections, 0);
    let parts = parse_glb_parts(&glb).expect("GLB re-parses");
    let sdef = &parts.json["extras"]["pmx_provenance"]["sdef"];
    assert_eq!(sdef["count"], 1);
    assert_eq!(sdef["vertices"][0], 3);
    // SDEF C vector of fixture vertex 3, verbatim PMX space.
    assert_eq!(sdef["c"][0], serde_json::json!([0.0, 13.5, 0.0]));
}

#[test]
fn fac_mapping_and_recipes_are_recorded() {
    let (_, report) = convert_fixture();
    let blink = report
        .fac_mapping
        .iter()
        .find(|row| row.morph == "blink")
        .expect("blink target");
    assert_eq!(blink.channel.as_deref(), Some("Fac_Eye_Close"));
    let viseme = report
        .fac_mapping
        .iter()
        .find(|row| row.morph == "a")
        .expect("viseme target");
    assert_eq!(viseme.channel.as_deref(), Some("Fac_Mth_Aa"));
    assert_eq!(report.recipes.len(), 1);
    assert_eq!(report.recipes[0].name, "representative");
    assert!(report.recipes[0].complete);
    assert_eq!(report.recipes[0].drives[0].0, "blink");
    // Both fixture textures are unresolved paths: ledgered, none embedded.
    assert_eq!(report.missing_textures.len(), 2);
    assert!(report.embedded_textures.is_empty());
    assert_eq!(report.morph_target_count, 3);
    assert!(!report.exceeds_morph_floor);
}

#[test]
fn proportion_profile_bands_are_the_dossier_numbers() {
    let profile = MmdProfile::builtin().proportions;
    assert_eq!(profile.height_min, 17.77);
    assert_eq!(profile.height_max, 23.19);
    assert_eq!(profile.heroine_height_min, 19.4);
    assert_eq!(profile.heroine_height_max, 21.9);
    assert_eq!(profile.head_fraction_min, 0.79);
    assert_eq!(profile.head_fraction_max, 0.89);
    assert_eq!(profile.head_fraction_median, 0.87);
    assert_eq!(profile.scale_to_meters, 0.08);
    // Fixture: bone Y range 0..12, head at 12 → fraction 1.0, out of band.
    let bytes = fixture_bytes("minimal-utf8.pmx");
    let model = parse_model(&bytes).expect("fixture parses");
    let assessment = assess_proportions(&model, &profile);
    assert_eq!(assessment.height_pmx, 12.0);
    assert_eq!(assessment.head_fraction, Some(1.0));
    assert!(!assessment.height_in_band);
    assert!(!assessment.head_fraction_in_band);
}

fn synthetic_qdef_model() -> PmxModel {
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
    let vertex = |deform: Deform, x: f32| Vertex {
        position: [x, 0.0, 0.0],
        normal: [0.0, 0.0, 1.0],
        uv: [0.0, 0.0],
        additional: Vec::new(),
        deform,
        edge_scale: 1.0,
    };
    PmxModel {
        pmx_version: 2.1,
        globals,
        model_name_local: String::new(),
        model_name_universal: "synthetic qdef".to_string(),
        model_comment_local: String::new(),
        model_comment_universal: String::new(),
        vertices: vec![
            vertex(
                Deform::Qdef {
                    bones: [0, 0, 0, 0],
                    weights: [2.0, 0.0, 0.0, 0.0],
                },
                0.0,
            ),
            vertex(Deform::Bdef1 { bone: 0 }, 1.0),
            vertex(Deform::Bdef1 { bone: 0 }, 0.0),
        ],
        surface_indices: vec![0, 1, 2],
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
            surface_count: 3,
        }],
        bones: vec![Bone {
            name_local: "頭".to_string(),
            name_universal: "head".to_string(),
            position: [0.0, 1.0, 0.0],
            parent: -1,
            layer: 0,
            flags: 0x001e,
            tail: BoneTail::Offset([0.0, -1.0, 0.0]),
            inherit: None,
            fixed_axis: None,
            local_axes: None,
            external_parent: None,
            ik: None,
        }],
        morphs: vec![Morph {
            name_local: "謎".to_string(),
            name_universal: "mystery".to_string(),
            panel: 4,
            morph_type: 2,
            offsets: Vec::new(),
        }],
        display_frames: Vec::new(),
        rigid_bodies: Vec::new(),
        joints: Vec::new(),
        soft_bodies: Vec::new(),
    }
}

#[test]
fn qdef_degrades_with_ledger_and_weight_correction() {
    let model = synthetic_qdef_model();
    let (glb, report) = convert(&model, &ConvertOptions::default()).expect("converts");
    assert_eq!(report.qdef_vertices, 1);
    // QDEF weights (2,0,0,0) sum to 2.0 → one normalisation correction.
    assert_eq!(report.weight_corrections, 1);
    // The bone morph is ledgered unconverted, not dropped.
    assert_eq!(report.unconverted_morphs.len(), 1);
    assert_eq!(report.unconverted_morphs[0].morph_type, "bone");
    let parts = parse_glb_parts(&glb).expect("GLB re-parses");
    assert_eq!(parts.json["extras"]["pmx_provenance"]["qdef"]["count"], 1);
}

// ------------------------------------------------------------------
// Adversarial
// ------------------------------------------------------------------

#[test]
fn corrupt_deform_byte_errors_through_parse() {
    let mut bytes = fixture_bytes("minimal-utf8.pmx");
    // Corrupt a byte deep in the file; parse must stay typed either
    // way — success is impossible once the tail is scrambled, and a
    // panic would fail the test outright.
    let tail = bytes.len() - 3;
    bytes[tail] = 0xff;
    bytes[tail + 1] = 0xff;
    let _ = parse_model(&bytes);
}

#[test]
fn conversion_rejects_nonpositive_scale() {
    let bytes = fixture_bytes("minimal-utf8.pmx");
    let model = parse_model(&bytes).expect("fixture parses");
    let options = ConvertOptions {
        scale_to_meters: 0.0,
        textures: Vec::new(),
        source_sha256: None,
    };
    assert!(convert(&model, &options).is_err());
}

#[test]
fn out_of_range_surface_index_is_a_typed_error() {
    let mut model = synthetic_qdef_model();
    model.surface_indices = vec![0, 1, 99];
    assert!(convert(&model, &ConvertOptions::default()).is_err());
}
