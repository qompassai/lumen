//! Exporter (R5c, phase E1) tests: the synthetic native
//! fixture exports to moc3 V3_0, the R5a importer parser reads
//! the export back with every mapped construct compared
//! (MAPPING.md gate (a)), and the export is byte-for-byte
//! deterministic (gate (d)). MAPPING.md gate (b) — the
//! independent Ayagami parser accepting the same bytes — runs
//! only in the converter's original workspace: ayagami lives
//! in the torogao tree and is not a dependency of this crate
//! as landed in lumen (2026-10-29); the fixture test below
//! keeps the same structural assertions on the crate's own
//! parser instead. Adversarial cases land after the
//! validation cases: every MAPPING.md typed-error row must
//! refuse, never degrade silently.

mod common;

use common::TestDir;
use lumen_rig_core::{
    Anchor, AnchorKind, BlendMode, CombinedParameter, CombinedTerm, Deformer, Lattice, Mesh,
    MeshForm, Model, Parameter, Part, Particle, PhysicsGroup, PhysicsInput, PhysicsOutput,
    RotationDeformer, RotationForm, ScalarParameter, Strand, Texture, WarpDeformer, WarpForm,
};
use lumen_rig_import::moc3::Moc3File;
use lumen_rig_import::{CanvasSpec, ConvertError, Moc3Version, export_model};

const ANGLE_KEYS: [f32; 3] = [-30.0, 0.0, 30.0];

fn scalar(id: &str, min: f32, max: f32, default: f32) -> Parameter {
    Parameter::Scalar(ScalarParameter {
        id: id.to_string(),
        min,
        max,
        default,
    })
}

fn angle_parameters() -> Vec<Parameter> {
    vec![
        scalar("ParamAngleX", -30.0, 30.0, 0.0),
        scalar("ParamAngleY", -30.0, 30.0, 0.0),
        scalar("ParamBlink", 0.0, 1.0, 1.0),
    ]
}

fn keys2() -> [Vec<f32>; 2] {
    [ANGLE_KEYS.to_vec(), ANGLE_KEYS.to_vec()]
}

/// Warp forms in canonical native order (ascending `at`,
/// first axis = ParamAngleY). Each form offsets the 3x3
/// grid by (7*iy + 3*ix, iy*ix) so every one of the 9 forms
/// is distinguishable and a moc3-order permutation error
/// changes the bytes (MAPPING.md section 3.3).
fn warp_forms() -> Vec<WarpForm> {
    let mut forms = Vec::new();
    for iy in 0..3u32 {
        for ix in 0..3u32 {
            let mut positions = Vec::new();
            for gy in 0..3 {
                for gx in 0..3 {
                    positions.push([
                        f32::from(u16::try_from(gx).unwrap()) - 1.0 + (iy * 7 + ix * 3) as f32,
                        f32::from(u16::try_from(gy).unwrap()) - 1.0 + (iy * ix) as f32,
                    ]);
                }
            }
            forms.push(WarpForm {
                at: vec![iy, ix],
                positions,
            });
        }
    }
    forms
}

fn warp_rest() -> Vec<[f32; 2]> {
    let mut positions = Vec::new();
    for gy in 0..3 {
        for gx in 0..3 {
            positions.push([
                f32::from(u16::try_from(gx).unwrap()) - 1.0,
                f32::from(u16::try_from(gy).unwrap()) - 1.0,
            ]);
        }
    }
    positions
}

/// Mesh-quad forms, 2-axis (ParamAngleY fastest in native
/// storage order), x shifted by 100*iy + ix per form.
fn quad_forms() -> Vec<MeshForm> {
    let mut forms = Vec::new();
    for iy in 0..3u32 {
        for ix in 0..3u32 {
            let shift = (iy * 100 + ix) as f32;
            forms.push(MeshForm {
                at: vec![iy, ix],
                vertices: vec![
                    [-0.5 + shift, -0.5],
                    [0.5 + shift, -0.5],
                    [0.5 + shift, 0.5],
                    [-0.5 + shift, 0.5],
                ],
            });
        }
    }
    forms
}

fn quad_vertices() -> Vec<[f32; 2]> {
    vec![[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]]
}

fn fixture_model() -> Model {
    Model {
        id: "fixture-export".to_string(),
        generator: None,
        parameters: angle_parameters(),
        parts: vec![
            Part {
                id: "PartRoot".to_string(),
                parent: None,
                opacity: 1.0,
            },
            Part {
                id: "PartChild".to_string(),
                parent: Some("PartRoot".to_string()),
                opacity: 0.9,
            },
        ],
        textures: vec![Texture {
            id: "texture_00".to_string(),
            source: "fixture/texture_00.png".to_string(),
        }],
        deformers: vec![
            Deformer::Rotation(RotationDeformer {
                id: "DeformerRot".to_string(),
                parent: None,
                origin: [10.0, 20.0],
                base_angle_degrees: 5.0,
                keyforms: Some(Lattice {
                    axes: vec!["ParamAngleX".to_string()],
                    keys: vec![ANGLE_KEYS.to_vec()],
                    forms: vec![
                        RotationForm {
                            at: vec![0],
                            angle_degrees: -10.0,
                            scale: [1.0, 1.0],
                        },
                        RotationForm {
                            at: vec![1],
                            angle_degrees: 0.0,
                            scale: [1.0, 1.0],
                        },
                        RotationForm {
                            at: vec![2],
                            angle_degrees: 10.0,
                            scale: [2.0, 2.0],
                        },
                    ],
                }),
            }),
            Deformer::Warp(WarpDeformer {
                id: "DeformerWarp".to_string(),
                parent: Some("DeformerRot".to_string()),
                columns: 2,
                rows: 2,
                rest_positions: warp_rest(),
                keyforms: Some(Lattice {
                    axes: vec!["ParamAngleY".to_string(), "ParamAngleX".to_string()],
                    keys: keys2().into(),
                    forms: warp_forms(),
                }),
            }),
        ],
        meshes: vec![
            Mesh {
                id: "MeshMask".to_string(),
                part: "PartChild".to_string(),
                texture: "texture_00".to_string(),
                deformer: Some("DeformerWarp".to_string()),
                vertices: vec![[0.6, 0.6], [1.0, 0.6], [1.0, 1.0], [0.6, 1.0]],
                uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                indices: vec![0, 1, 2, 0, 2, 3],
                opacity: 1.0,
                blend_mode: BlendMode::Multiply,
                draw_order: -5000,
                rendered: true,
                clipping_mask_ids: Vec::new(),
                keyforms: Some(Lattice {
                    axes: vec!["ParamAngleX".to_string()],
                    keys: vec![ANGLE_KEYS.to_vec()],
                    forms: (0..3u32)
                        .map(|i| MeshForm {
                            at: vec![i],
                            vertices: vec![
                                [0.6 + 0.25 * i as f32, 0.6],
                                [1.0 + 0.25 * i as f32, 0.6],
                                [1.0 + 0.25 * i as f32, 1.0],
                                [0.6 + 0.25 * i as f32, 1.0],
                            ],
                        })
                        .collect(),
                }),
            },
            Mesh {
                id: "MeshQuad".to_string(),
                part: "PartChild".to_string(),
                texture: "texture_00".to_string(),
                deformer: Some("DeformerWarp".to_string()),
                vertices: quad_vertices(),
                uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                indices: vec![0, 1, 2, 0, 2, 3],
                opacity: 0.8,
                blend_mode: BlendMode::Normal,
                draw_order: 0,
                rendered: true,
                clipping_mask_ids: vec!["MeshMask".to_string()],
                keyforms: Some(Lattice {
                    axes: vec!["ParamAngleY".to_string(), "ParamAngleX".to_string()],
                    keys: keys2().into(),
                    forms: quad_forms(),
                }),
            },
        ],
        physics: vec![PhysicsGroup {
            id: "PhysicsHair".to_string(),
            gravity: [0.0, -9.8],
            wind: [0.0, 0.0],
            turbulence_amplitude: 0.3,
            inputs: vec![PhysicsInput {
                parameter: "ParamAngleX".to_string(),
                scale_degrees: 10.0,
            }],
            strands: vec![Strand {
                id: "PhysicsHair_out_0".to_string(),
                anchor: Anchor {
                    kind: AnchorKind::Deformer,
                    id: "DeformerWarp".to_string(),
                    offset: [0.0, 0.0],
                },
                particles: vec![Particle {
                    length: 10.0,
                    radius: 5.0,
                    mobility: 0.9,
                    delay: 0.8,
                    damping: 0.5,
                }],
                output: Some(PhysicsOutput {
                    parameter: "ParamAngleY".to_string(),
                    scale: 0.5,
                    offset: 0.0,
                }),
            }],
        }],
    }
}

fn fixture_canvas() -> CanvasSpec {
    CanvasSpec {
        pixels_per_unit: 100.0,
        origin_x: 50.0,
        origin_y: 60.0,
        width: 200.0,
        height: 300.0,
    }
}

fn export_fixture() -> lumen_rig_import::ExportOutcome {
    export_model(&fixture_model(), &fixture_canvas(), Moc3Version::V3_0).expect("fixture exports")
}

/// SHA-256 (FIPS 180-4), implemented here so the
/// determinism proof needs no external tool.
fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_length = (data.len() as u64) * 8;
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_be_bytes());
    for chunk in message.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    h.iter().map(|value| format!("{value:08x}")).collect()
}

#[test]
fn sha256_matches_the_published_vectors() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn export_reads_back_through_the_r5a_parser() {
    let outcome = export_fixture();
    let parsed = Moc3File::parse(&outcome.bytes).expect("R5a parser reads the export");

    let counts = &parsed.counts;
    assert_eq!(counts.parts, 2);
    assert_eq!(counts.deformers, 2);
    assert_eq!(counts.warp_deformers, 1);
    assert_eq!(counts.rotation_deformers, 1);
    assert_eq!(counts.art_meshes, 2);
    assert_eq!(counts.parameters, 3);
    assert_eq!(counts.part_keyforms, 2);
    assert_eq!(counts.warp_keyforms, 9);
    assert_eq!(counts.rotation_keyforms, 3);
    assert_eq!(counts.art_mesh_keyforms, 12);
    assert_eq!(counts.keyform_positions, 258);
    assert_eq!(counts.parameter_bindings, 2);
    assert_eq!(counts.keys, 6);
    assert_eq!(counts.keyform_bindings, 3);
    assert_eq!(counts.parameter_binding_index, 3);
    assert_eq!(counts.uvs, 16);
    assert_eq!(counts.position_indices, 12);
    assert_eq!(counts.drawable_masks, 1);
    assert_eq!(counts.draw_order_groups, 1);
    assert_eq!(counts.draw_order_group_objects, 2);

    let canvas = &parsed.canvas;
    assert_eq!(canvas.pixels_per_unit, 100.0);
    assert_eq!(canvas.origin_x, 50.0);
    assert_eq!(canvas.origin_y, 60.0);
    assert_eq!(canvas.width, 200.0);
    assert_eq!(canvas.height, 300.0);

    assert_eq!(parsed.parts[0].id, "PartRoot");
    assert_eq!(parsed.parts[0].parent, -1);
    assert_eq!(parsed.parts[1].id, "PartChild");
    assert_eq!(parsed.parts[1].parent, 0);

    assert_eq!(parsed.parameters[0].id, "ParamAngleX");
    assert_eq!(parsed.parameters[0].min, -30.0);
    assert_eq!(parsed.parameters[0].max, 30.0);
    assert_eq!(parsed.parameters[0].default, 0.0);
    assert!(!parsed.parameters[0].is_repeat);
    assert_eq!(parsed.parameters[2].id, "ParamBlink");
    assert_eq!(parsed.parameters[2].default, 1.0);

    // Binding sets: 0 = empty (parts), 1 = [AngleX],
    // 2 = [AngleY, AngleX].
    assert_eq!(parsed.bindings.len(), 3);
    assert!(parsed.bindings[0].is_empty());
    assert_eq!(parsed.bindings[1].len(), 1);
    assert_eq!(parsed.bindings[1][0].parameter, 0);
    assert_eq!(parsed.bindings[1][0].keys, ANGLE_KEYS.to_vec());
    assert_eq!(parsed.bindings[2].len(), 2);
    assert_eq!(parsed.bindings[2][0].parameter, 1);
    assert_eq!(parsed.bindings[2][1].parameter, 0);

    assert_eq!(parsed.deformers[0].id, "DeformerRot");
    assert_eq!(parsed.deformers[0].binding, 1);
    assert_eq!(parsed.deformers[0].parent_deformer, -1);
    assert_eq!(parsed.deformers[1].id, "DeformerWarp");
    assert_eq!(parsed.deformers[1].binding, 2);
    assert_eq!(parsed.deformers[1].parent_deformer, 0);

    let rotation = &parsed.rotations[0];
    assert_eq!(rotation.base_angle, 5.0);
    let angles: Vec<f32> = rotation.keyforms.iter().map(|form| form.angle).collect();
    assert_eq!(angles, vec![-10.0, 0.0, 10.0]);
    let scales: Vec<f32> = rotation.keyforms.iter().map(|form| form.scale).collect();
    assert_eq!(scales, vec![1.0, 1.0, 2.0]);
    assert!(rotation.keyforms.iter().all(|form| !form.reflect_x));

    // Warp: the section 3.3 permutation — moc3 ordinal o
    // is native form at [iy = o % 3, ix = o / 3].
    let warp = &parsed.warps[0];
    assert_eq!(warp.rows, 2);
    assert_eq!(warp.columns, 2);
    assert_eq!(warp.binding, 2);
    assert_eq!(warp.count, 9);
    let native_forms = warp_forms();
    for (ordinal, &begin_raw) in warp.keyform_position_begins.iter().enumerate().take(9) {
        let iy = ordinal % 3;
        let ix = ordinal / 3;
        let begin = begin_raw as usize;
        assert_eq!(begin, ordinal * 18);
        let expected: Vec<f32> = native_forms[iy * 3 + ix]
            .positions
            .iter()
            .flat_map(|point| [point[0], point[1]])
            .collect();
        let actual: Vec<f32> = parsed.keyform_positions[begin..begin + 18].to_vec();
        assert_eq!(actual, expected, "warp form ordinal {ordinal}");
    }
    // Negative control: ordinal 1 must NOT equal the
    // unpermuted native form at [0, 1].
    let unpermuted: Vec<f32> = native_forms[1]
        .positions
        .iter()
        .flat_map(|point| [point[0], point[1]])
        .collect();
    let begin = warp.keyform_position_begins[1] as usize;
    assert_ne!(parsed.keyform_positions[begin..begin + 18], unpermuted[..]);

    // Meshes: mask first (native order), then the quad.
    let mask = &parsed.meshes[0];
    assert_eq!(mask.id, "MeshMask");
    assert_eq!(mask.binding, 1);
    assert_eq!(mask.parent_part, 1);
    assert_eq!(mask.parent_deformer, 1);
    assert_eq!(mask.texture_number, 0);
    assert_eq!(mask.drawable_flag, 0x2 | 0x4);
    assert_eq!(mask.count, 3);
    for form in 0..3usize {
        assert_eq!(mask.keyform_draw_orders[form], 250.0);
        let begin = mask.keyform_position_begins[form] as usize;
        assert_eq!(begin, 162 + form * 8);
    }
    assert_eq!(mask.mask_count, 0);

    let quad = &parsed.meshes[1];
    assert_eq!(quad.id, "MeshQuad");
    assert_eq!(quad.binding, 2);
    assert_eq!(quad.drawable_flag, 0x4);
    assert_eq!(quad.count, 9);
    assert_eq!((quad.mask_begin, quad.mask_count), (0, 1));
    assert_eq!(parsed.drawable_masks, vec![0]);
    for ordinal in 0..9usize {
        let iy = ordinal % 3;
        let ix = ordinal / 3;
        assert_eq!(quad.keyform_draw_orders[ordinal], 500.0);
        assert_eq!(quad.keyform_opacities[ordinal], 0.8);
        let begin = quad.keyform_position_begins[ordinal] as usize;
        assert_eq!(begin, 186 + ordinal * 8);
        let shift = (iy * 100 + ix) as f32;
        let expected: Vec<f32> = quad_vertices()
            .iter()
            .flat_map(|point| [point[0] + shift, point[1]])
            .collect();
        let actual: Vec<f32> = parsed.keyform_positions[begin..begin + 8].to_vec();
        assert_eq!(actual, expected, "quad form ordinal {ordinal}");
    }

    assert_eq!(
        parsed.position_indices,
        vec![0, 1, 2, 0, 2, 3, 0, 1, 2, 0, 2, 3]
    );

    // Draw order: one flat group spanning [0, 1000] as
    // f32 bit patterns (MAPPING.md D-2 lossy row).
    let group = &parsed.draw_order_groups[0];
    assert_eq!(group.objects, vec![(0, 0), (0, 1)]);
    assert_eq!(group.minimum, 0.0f32.to_bits() as i32);
    assert_eq!(group.maximum, 1000.0f32.to_bits() as i32);

    // The report names every lossy / no-map occurrence.
    let report = &outcome.report;
    assert_eq!(report.model_id, "fixture-export");
    assert_eq!(report.moc3_version, "V3_0");
    assert_eq!(report.counts.art_meshes, 2);
    assert_eq!(report.lossy.len(), 3);
    assert_eq!(
        report
            .lossy
            .iter()
            .filter(|entry| entry.item == "draw order rescale")
            .count(),
        2
    );
    assert!(
        report
            .lossy
            .iter()
            .any(|entry| entry.item == "warp evaluation semantics")
    );
    assert!(
        report
            .no_map
            .iter()
            .any(|entry| entry.item == "part opacity" && entry.detail.contains("PartChild"))
    );
    assert!(
        report
            .no_map
            .iter()
            .any(|entry| entry.item == "physics group" && entry.detail.contains("PhysicsHair"))
    );
}

#[test]
fn export_is_byte_for_byte_deterministic() {
    let first = export_fixture();
    let second = export_fixture();
    assert_eq!(first.bytes, second.bytes);
    let first_hash = sha256_hex(&first.bytes);
    let second_hash = sha256_hex(&second.bytes);
    assert_eq!(first_hash, second_hash);
    println!("fixture .moc3 sha256 (run 1): {first_hash}");
    println!("fixture .moc3 sha256 (run 2): {second_hash}");
    println!("fixture .moc3 length: {} bytes", first.bytes.len());
}

#[test]
fn own_parser_reads_the_exported_fixture() {
    // Gate (b) substitute (see the module header): the same
    // structural assertions the Ayagami cross-check made —
    // version plus every fixture id present — run against the
    // crate's own parser, since ayagami is not a dependency
    // of this crate as landed in lumen.
    let outcome = export_fixture();
    let parsed = Moc3File::parse(&outcome.bytes).expect("own parser reads the exported moc3");
    assert_eq!(parsed.version, 1, "export parses back as V3 (byte 1)");
    let mut ids: Vec<&str> = Vec::new();
    ids.extend(parsed.parameters.iter().map(|p| p.id.as_str()));
    ids.extend(parsed.deformers.iter().map(|d| d.id.as_str()));
    ids.extend(parsed.parts.iter().map(|p| p.id.as_str()));
    ids.extend(parsed.meshes.iter().map(|m| m.id.as_str()));
    for needle in [
        "MeshQuad",
        "MeshMask",
        "ParamAngleX",
        "ParamBlink",
        "DeformerWarp",
        "DeformerRot",
        "PartChild",
    ] {
        assert!(
            ids.contains(&needle),
            "parsed export should contain id {needle}"
        );
    }
}

#[test]
fn cli_export_writes_a_parseable_moc3_and_a_report() {
    let dir = TestDir::new("export-cli");
    let model_path = dir.root.join("model.json");
    std::fs::write(&model_path, fixture_model().to_canonical_json()).expect("write native model");
    let out_dir = dir.root.join("out");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_rig-import"))
        .args([
            "export",
            "--model",
            model_path.to_str().unwrap(),
            "--out",
            out_dir.to_str().unwrap(),
            "--canvas-pixels-per-unit",
            "100",
            "--canvas-origin-x",
            "50",
            "--canvas-origin-y",
            "60",
            "--canvas-width",
            "200",
            "--canvas-height",
            "300",
        ])
        .status()
        .expect("run rig-import export");
    assert!(status.success());
    let bytes = std::fs::read(out_dir.join("fixture-export.moc3")).expect("moc3 written");
    let parsed = Moc3File::parse(&bytes).expect("CLI export parses back");
    assert_eq!(parsed.counts.art_meshes, 2);
    assert_eq!(parsed.canvas.width, 200.0);
    let report_text = std::fs::read_to_string(out_dir.join("fixture-export.export-report.json"))
        .expect("export report written");
    assert!(report_text.contains("\"model_id\": \"fixture-export\""));
    assert!(report_text.contains("\"moc3_version\": \"V3_0\""));
}

// ----------------------------------------------------------------
// Adversarial cases: every MAPPING.md typed-error row must
// refuse loudly (ConvertError::Export), never degrade.
// ----------------------------------------------------------------

fn expect_export_error(model: &Model, canvas: &CanvasSpec, needle: &str) {
    match export_model(model, canvas, Moc3Version::V3_0) {
        Err(ConvertError::Export { message }) => {
            assert!(
                message.contains(needle),
                "error {message:?} should mention {needle:?}"
            );
        }
        Err(other) => panic!("expected ConvertError::Export, got {other:?}"),
        Ok(_) => panic!("expected an export error mentioning {needle:?}"),
    }
}

#[test]
fn combined_parameter_is_a_typed_error_d1() {
    let mut model = fixture_model();
    model
        .parameters
        .push(Parameter::Combined(CombinedParameter {
            id: "ParamCombo".to_string(),
            min: 0.0,
            max: 1.0,
            terms: vec![CombinedTerm {
                parameter: "ParamAngleX".to_string(),
                weight: 1.0,
            }],
            offset: 0.0,
        }));
    expect_export_error(&model, &fixture_canvas(), "Combined");
}

#[test]
fn non_uniform_rotation_scale_is_a_typed_error_d3() {
    let mut model = fixture_model();
    if let Deformer::Rotation(rotation) = &mut model.deformers[0] {
        rotation.keyforms.as_mut().unwrap().forms[2].scale = [1.0, 2.0];
    }
    expect_export_error(&model, &fixture_canvas(), "non-uniform scale");
}

#[test]
fn unexportable_blend_mode_is_a_typed_error_d4() {
    let mut model = fixture_model();
    model.meshes[1].blend_mode = BlendMode::Screen;
    expect_export_error(&model, &fixture_canvas(), "blend mode");
}

#[test]
fn oversized_mesh_is_a_typed_error() {
    let mut model = fixture_model();
    model.meshes[1].vertices = vec![[0.0, 0.0]; 65537];
    model.meshes[1].uvs = vec![[0.0, 0.0]; 65537];
    model.meshes[1].keyforms = None;
    expect_export_error(&model, &fixture_canvas(), "exceed the 65536");
}

#[test]
fn incomplete_lattice_is_a_typed_error() {
    let mut model = fixture_model();
    if let Deformer::Warp(warp) = &mut model.deformers[1] {
        warp.keyforms.as_mut().unwrap().forms.pop();
    }
    expect_export_error(&model, &fixture_canvas(), "incomplete lattice");
}

#[test]
fn duplicate_lattice_coordinate_is_a_typed_error() {
    let mut model = fixture_model();
    if let Deformer::Warp(warp) = &mut model.deformers[1] {
        warp.keyforms.as_mut().unwrap().forms[1].at = vec![0, 0];
    }
    expect_export_error(&model, &fixture_canvas(), "share lattice coordinate");
}

#[test]
fn unknown_clipping_mask_is_a_typed_error() {
    let mut model = fixture_model();
    model.meshes[1].clipping_mask_ids = vec!["NoSuchMesh".to_string()];
    expect_export_error(&model, &fixture_canvas(), "unknown clipping mask");
}

#[test]
fn unknown_texture_is_a_typed_error() {
    let mut model = fixture_model();
    model.meshes[0].texture = "texture_99".to_string();
    expect_export_error(&model, &fixture_canvas(), "unknown texture");
}

#[test]
fn unknown_parent_part_is_a_typed_error() {
    let mut model = fixture_model();
    model.parts[1].parent = Some("NoSuchPart".to_string());
    expect_export_error(&model, &fixture_canvas(), "unknown parent part");
}

#[test]
fn non_finite_canvas_is_a_typed_error() {
    let mut canvas = fixture_canvas();
    canvas.width = f32::NAN;
    expect_export_error(&fixture_model(), &canvas, "canvas width");
}

#[test]
fn unimplemented_version_is_a_typed_error() {
    match export_model(&fixture_model(), &fixture_canvas(), Moc3Version::V5_3) {
        Err(ConvertError::Export { message }) => {
            assert!(message.contains("not implemented"), "got {message:?}");
        }
        other => panic!("expected an Export error, got {other:?}"),
    }
}

#[test]
fn overlong_parameter_id_is_a_typed_error() {
    let mut model = fixture_model();
    model.parameters[2] = scalar(&"P".repeat(65), 0.0, 1.0, 1.0);
    expect_export_error(&model, &fixture_canvas(), "64-byte id block");
}

#[test]
fn writer_rejects_incoherent_section_data() {
    use lumen_rig_import::moc3_write::*;
    let data = Moc3Data {
        canvas: fixture_canvas(),
        parts: vec![PartData {
            id: "PartRoot".to_string(),
            parent: -1,
            binding_set: 0,
            visible_art_meshes: true,
            visible_deformers: true,
        }],
        ..Moc3Data::default()
    };
    // The part has no form-depth entry: the writer must
    // refuse rather than emit a short section.
    match write_moc3(&data) {
        Err(ConvertError::Export { .. }) => {}
        other => panic!("expected an Export error, got {other:?}"),
    }
}
