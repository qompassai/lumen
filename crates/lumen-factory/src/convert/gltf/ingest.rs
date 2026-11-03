//! The `torogao-mmd-ingest/1` record builder and the
//! `mmd-profile/1` RON emitter. See the module docs in `mod.rs`.
//!
//! Determinism: the record serializes from ordered structs
//! (field order is declaration order), arrays stay in GLB file
//! order, and no timestamps or hash-map iteration reach the
//! output — the same GLB always yields byte-identical JSON.

use serde::Serialize;
use serde_json::Value;

use crate::FactoryError;
use crate::convert::pmx::profile::{
    BONE_MAPPINGS, FAC_CHANNELS, MORPH_MAPPINGS, MmdProfile, bone_target, fac_channel,
    morph_channel,
};
use crate::gltf::parse_glb_parts;
use crate::sha256_hex;

/// The ingest record's schema id.
pub const INGEST_SCHEMA: &str = "torogao-mmd-ingest/1";
/// The profile id every record is built against.
pub const PROFILE_ID: &str = "mmd-profile/1";

/// Provenance anchor: which censused source, which GLB, which profile.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Provenance {
    /// SHA-256 of the source PMX file (the census `file_sha256`),
    /// carried through the GLB's `pmx_provenance` extras. `null`
    /// when the Stage 1 conversion ran without a census hash.
    pub source_pmx_sha256: Option<String>,
    /// SHA-256 of the GLB bytes this record was derived from.
    pub glb_sha256: String,
    /// Mapping profile id (`mmd-profile/1`).
    pub profile_id: String,
    /// Converter id (`gltf-import/1`).
    pub converter: String,
}

/// One skeleton joint in mapped space.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SkeletonEntry {
    /// Joint index (GLB skin order == PMX bone order).
    pub index: usize,
    /// PMX local bone name, verbatim.
    pub name_local: String,
    /// PMX universal bone name, verbatim.
    pub name_universal: String,
    /// torogao-humanoid/1 target, or `null` when unmapped.
    pub torogao: Option<String>,
    /// Why the bone is unmapped (present iff `torogao` is null).
    pub unmapped_reason: Option<String>,
    /// Parent joint index in this record's skeleton space,
    /// or `null` for a root.
    pub parent: Option<usize>,
    /// Rest position in Torogao space (metres, right-handed):
    /// the global position accumulated from GLB node translations.
    pub rest_position: [f32; 3],
}

/// One morph's face-parameter row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FaceEntry {
    /// PMX local morph name, verbatim.
    pub name_local: String,
    /// PMX universal morph name, verbatim.
    pub name_universal: String,
    /// Census class (group/vertex/bone/uv/material/flip/impulse).
    pub class: String,
    /// PMX panel type byte.
    pub panel: u8,
    /// Disposition: `morph_target`, `recipe`, or `ledgered`.
    pub route: String,
    /// Canonical D-630 `Fac_` channel, or `null` when unmapped.
    pub fac_channel: Option<String>,
    /// SPEC §6.2 governed channel, or `null` when unmapped.
    pub governed_channel: Option<String>,
    /// Why the morph is unmapped/ledgered, when it is.
    pub unmapped_reason: Option<String>,
}

/// Mesh tallies from the GLB itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MeshSummary {
    /// Vertex count (POSITION accessor count).
    pub vertices: usize,
    /// Triangle count (summed index counts / 3).
    pub triangles: usize,
    /// Material count in the GLB document.
    pub materials: usize,
    /// Morph target count (global target table length).
    pub morph_targets: usize,
}

/// One strand particle (SPEC §6.4: a rigid body on a bone chain).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Particle {
    /// PMX rigid-body index.
    pub rigid_index: usize,
    /// Rigid-body name (universal when present, else local).
    pub name: String,
    /// Related bone's local name.
    pub bone_name: String,
    /// Particle radius (metres), per the stated shape formula.
    pub radius: f32,
    /// Particle position (metres, Torogao space).
    pub position: [f32; 3],
    /// Mass, carried verbatim (PMX mass units).
    pub mass: f32,
    /// Strand mobility: `1.0 - clamp(move_attenuation, 0, 1)`.
    pub mobility: f32,
    /// Strand damping: `rotation_damping` carried direct.
    pub damping: f32,
    /// Strand friction: `friction` carried direct.
    pub friction: f32,
}

/// One strand: a chain of particles along a bone chain.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Strand {
    /// Strand index in emission order.
    pub id: usize,
    /// Local name of the chain's root bone.
    pub root_bone: String,
    /// Physics mode of the chain (`physics` / `physics_plus_bone`).
    pub mode: String,
    /// Particles in chain order (root first).
    pub particles: Vec<Particle>,
}

/// A follow-bone rigid body lowered to a static strand anchor.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StaticAnchor {
    /// PMX rigid-body index.
    pub rigid_index: usize,
    /// Rigid-body name (universal when present, else local).
    pub name: String,
    /// Anchored bone's local name.
    pub bone_name: String,
    /// Semantics note (follow-bone → static anchor).
    pub semantics: String,
}

/// The strand-group record for physics-mode strands (SPEC §6.4:
/// PMX carries no gravity/wind, so Torogao's scene defaults are
/// substituted — the substitution is recorded, never silent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhysicsGroup {
    /// Strand ids in this group.
    pub strands: Vec<usize>,
    /// Gravity source: `torogao_scene_default (substituted)`.
    pub gravity: String,
    /// Wind source: `torogao_scene_default (substituted)`.
    pub wind: String,
    /// The substitution note, verbatim.
    pub substitution_note: String,
}

/// The converted physics section.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Physics {
    /// The scalar conversion formulas used, stated verbatim.
    pub conversion_formulas: Vec<String>,
    /// Strands from physics-mode rigid-body chains.
    pub strands: Vec<Strand>,
    /// Static anchors from follow-bone rigid bodies.
    pub static_anchors: Vec<StaticAnchor>,
    /// Strand groups (gravity/wind substitution noted).
    pub groups: Vec<PhysicsGroup>,
}

/// One joint's per-axis limits, ledgered verbatim (SPEC §6.4:
/// strand particles have no per-axis angular limits).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JointLimits {
    /// Joint name (universal when present, else local).
    pub name: String,
    /// Position limit minimum, PMX space.
    pub position_min: [f32; 3],
    /// Position limit maximum, PMX space.
    pub position_max: [f32; 3],
    /// Rotation limit minimum (radians), PMX space.
    pub rotation_min: [f32; 3],
    /// Rotation limit maximum (radians), PMX space.
    pub rotation_max: [f32; 3],
}

/// One joint's spring constants, ledgered verbatim.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JointSprings {
    /// Joint name (universal when present, else local).
    pub name: String,
    /// Position spring constants per axis.
    pub position_spring: [f32; 3],
    /// Rotation spring constants per axis.
    pub rotation_spring: [f32; 3],
}

/// One rigid body's collision grouping, ledgered verbatim
/// (Torogao physics has no collision-group construct).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CollisionEntry {
    /// Rigid-body name (universal when present, else local).
    pub name: String,
    /// PMX collision group id (0..16).
    pub group_id: u8,
    /// PMX non-collision mask.
    pub non_collision_mask: i16,
}

/// Everything the conversion did not carry across — verbatim
/// counts and names, never silence (SPEC §6.4's gate).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DroppedLedger {
    /// Per-axis joint limits, one entry per PMX joint.
    pub joint_axis_limits: Vec<JointLimits>,
    /// Per-axis joint springs, one entry per PMX joint.
    pub joint_springs: Vec<JointSprings>,
    /// Collision groups/masks, one entry per PMX rigid body.
    pub collision: Vec<CollisionEntry>,
    /// Impulse morph names (physics kicks; census-only).
    pub impulse_morphs: Vec<String>,
    /// Bone morph names (no glTF morph-target form).
    pub bone_morphs: Vec<String>,
    /// Material morph names (no glTF morph-target form).
    pub material_morphs: Vec<String>,
    /// Group/flip recipes ledgered whole (incomplete members).
    pub incomplete_recipes: Vec<String>,
    /// Soft-body count (PMX 2.1; no conversion target exists).
    pub soft_bodies: usize,
    /// IK bone names (preserved in GLB skin provenance only).
    pub ik_bones: Vec<String>,
    /// Inherit (grant) bone names (provenance only).
    pub inherit_bones: Vec<String>,
    /// Bones with no torogao-humanoid/1 target (local names).
    pub unmapped_bones: Vec<String>,
    /// Morph targets with no canonical `Fac_` channel (names).
    pub unmapped_morphs: Vec<String>,
    /// Texture paths that did not embed at Stage 1.
    pub missing_textures: Vec<String>,
}

/// One zero-silent-drop accounting row: for every feature class
/// the census counts, `mapped_count + ledgered_count` must equal
/// `census_count`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountingRow {
    /// Feature class (`bones`, `morphs:vertex`, `joints`, ...).
    pub feature: String,
    /// Count in the source census (from GLB provenance).
    pub census_count: usize,
    /// Count converted into the record's mapped sections.
    pub mapped_count: usize,
    /// Count carried in the dropped ledger / unmapped entries.
    pub ledgered_count: usize,
}

/// The `torogao-mmd-ingest/1` record.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IngestRecord {
    /// Schema id (`torogao-mmd-ingest/1`).
    pub schema: String,
    /// Provenance anchor block.
    pub provenance: Provenance,
    /// Skeleton joints in GLB skin order.
    pub skeleton: Vec<SkeletonEntry>,
    /// Face parameters in PMX file order (all morph classes).
    pub face_parameters: Vec<FaceEntry>,
    /// Mesh tallies from the GLB.
    pub mesh_summary: MeshSummary,
    /// Converted physics (SPEC §6.4).
    pub physics: Physics,
    /// The dropped ledger (SPEC §6.4's zero-silent-drop gate).
    pub dropped_ledger: DroppedLedger,
    /// Zero-silent-drop accounting rows, fixed order.
    pub accounting: Vec<AccountingRow>,
}

impl IngestRecord {
    /// Deterministic JSON form (struct field order, no timestamps).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("ingest record always serializes")
    }
}

fn display_name(local: &str, universal: &str) -> String {
    if universal.is_empty() {
        local.to_string()
    } else {
        universal.to_string()
    }
}

fn str_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn usize_field(value: &Value, key: &str) -> usize {
    value.get(key).and_then(Value::as_u64).unwrap_or(0) as usize
}

fn i64_field(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn vec3_field(value: &Value, key: &str) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    if let Some(items) = value.get(key).and_then(Value::as_array) {
        for (slot, item) in out.iter_mut().zip(items.iter()) {
            *slot = item.as_f64().unwrap_or(0.0) as f32;
        }
    }
    out
}

fn f32_field(value: &Value, key: &str) -> f32 {
    value.get(key).and_then(Value::as_f64).unwrap_or(0.0) as f32
}

/// Build the ingest record from Stage 1 GLB bytes. Fails typed
/// (`FactoryError::InvalidGlb`) when the GLB is not a pmx-import/1
/// output (no `pmx_provenance` extras) or is structurally invalid.
pub fn ingest_glb(glb_bytes: &[u8]) -> Result<IngestRecord, FactoryError> {
    let parts = parse_glb_parts(glb_bytes)?;
    let doc = &parts.json;
    let root_prov = doc
        .get("extras")
        .and_then(|extras| extras.get("pmx_provenance"))
        .ok_or_else(|| {
            FactoryError::InvalidGlb(
                "glb carries no pmx_provenance extras: not a pmx-import/1 output".to_string(),
            )
        })?
        .clone();
    let scale = {
        let value = root_prov
            .get("scale_to_meters")
            .and_then(Value::as_f64)
            .unwrap_or(0.0) as f32;
        if value > 0.0 && value.is_finite() {
            value
        } else {
            MmdProfile::builtin().proportions.scale_to_meters
        }
    };

    // ---- Skeleton ---------------------------------------------------
    let skin_bones = doc
        .get("skins")
        .and_then(Value::as_array)
        .and_then(|skins| skins.first())
        .and_then(|skin| skin.get("extras"))
        .and_then(|extras| extras.get("pmx_provenance"))
        .and_then(|prov| prov.get("bones"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let nodes = doc
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let bone_count = skin_bones.len();
    // Global rest positions: accumulate node translations along the
    // parent chain (bone nodes are the first `bone_count` nodes, in
    // bone order — the Stage 1 emitter's construction).
    let mut globals: Vec<Option<[f32; 3]>> = vec![None; bone_count];
    fn global_position(
        index: usize,
        skin_bones: &[Value],
        nodes: &[Value],
        globals: &mut [Option<[f32; 3]>],
        depth: usize,
    ) -> [f32; 3] {
        if let Some(done) = globals.get(index).copied().flatten() {
            return done;
        }
        let local = nodes
            .get(index)
            .map(|node| vec3_field(node, "translation"))
            .unwrap_or([0.0; 3]);
        let parent = skin_bones
            .get(index)
            .map(|bone| i64_field(bone, "parent"))
            .unwrap_or(-1);
        let out = if parent >= 0 && (parent as usize) < skin_bones.len() && depth < skin_bones.len()
        {
            let base = global_position(parent as usize, skin_bones, nodes, globals, depth + 1);
            [base[0] + local[0], base[1] + local[1], base[2] + local[2]]
        } else {
            local
        };
        if let Some(slot) = globals.get_mut(index) {
            *slot = Some(out);
        }
        out
    }
    let mut skeleton: Vec<SkeletonEntry> = Vec::with_capacity(bone_count);
    let mut ik_bones: Vec<String> = Vec::new();
    let mut inherit_bones: Vec<String> = Vec::new();
    for (index, bone) in skin_bones.iter().enumerate() {
        let name_local = str_field(bone, "name_local");
        let target = bone_target(&name_local);
        if bone.get("ik").is_some_and(|ik| !ik.is_null()) {
            ik_bones.push(name_local.clone());
        }
        if bone.get("inherit").is_some_and(|v| !v.is_null()) {
            inherit_bones.push(name_local.clone());
        }
        let parent_raw = i64_field(bone, "parent");
        skeleton.push(SkeletonEntry {
            index,
            unmapped_reason: if target.is_none() {
                Some(
                    "no torogao-humanoid/1 target in mmd-profile/1 for this PMX bone name"
                        .to_string(),
                )
            } else {
                None
            },
            torogao: target.map(str::to_string),
            name_universal: str_field(bone, "name_universal"),
            name_local,
            parent: if parent_raw >= 0 && (parent_raw as usize) < bone_count {
                Some(parent_raw as usize)
            } else {
                None
            },
            rest_position: global_position(index, &skin_bones, &nodes, &mut globals, 0),
        });
    }

    // ---- Face parameters ---------------------------------------------
    // Target order == the morph table filtered to vertex/UV classes
    // (the Stage 1 emitter's global target table construction), so
    // the mesh extras' Fac_ rows line up by position.
    let fac_rows = doc
        .get("meshes")
        .and_then(Value::as_array)
        .and_then(|meshes| meshes.first())
        .and_then(|mesh| mesh.get("extras"))
        .and_then(|extras| extras.get("pmx_fac_mapping"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let recipes = root_prov
        .get("recipes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let morphs = root_prov
        .get("morphs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut face_parameters: Vec<FaceEntry> = Vec::with_capacity(morphs.len());
    let mut target_cursor = 0usize;
    for morph in &morphs {
        let name_local = str_field(morph, "name_local");
        let name_universal = str_field(morph, "name_universal");
        let class = str_field(morph, "type");
        let panel = usize_field(morph, "panel") as u8;
        let governed = morph_channel(&name_local).map(str::to_string);
        let (route, fac, reason) = match class.as_str() {
            "vertex" | "uv" => {
                let fac = fac_rows
                    .get(target_cursor)
                    .and_then(|row| row.get("fac_channel"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| fac_channel(&name_local, &name_universal).map(str::to_string));
                target_cursor += 1;
                let reason = if fac.is_none() {
                    Some(
                        "morph target carried in the GLB but its name is outside the \
                         D-630 Fac_ vocabulary (profile grows by amendment, never fuzzy \
                         matching)"
                            .to_string(),
                    )
                } else {
                    None
                };
                ("morph_target".to_string(), fac, reason)
            }
            "group" | "flip" => {
                let display = display_name(&name_local, &name_universal);
                let recipe = recipes.iter().find(|row| str_field(row, "name") == display);
                match recipe {
                    Some(row) if row.get("complete").and_then(Value::as_bool) == Some(true) => {
                        ("recipe".to_string(), None, None)
                    }
                    _ => (
                        "ledgered".to_string(),
                        None,
                        Some(
                            "group/flip recipe is incomplete (a driven morph is missing); \
                             ledgered whole per SPEC 6.3, never half-applied"
                                .to_string(),
                        ),
                    ),
                }
            }
            "bone" => (
                "ledgered".to_string(),
                fac_channel(&name_local, &name_universal).map(str::to_string),
                Some(
                    "bone morphs have no glTF morph-target form; offsets preserved in GLB \
                     provenance (SPEC 5.4)"
                        .to_string(),
                ),
            ),
            "material" => (
                "ledgered".to_string(),
                fac_channel(&name_local, &name_universal).map(str::to_string),
                Some(
                    "material morphs have no glTF morph-target form; offsets preserved in \
                     GLB provenance (SPEC 5.4)"
                        .to_string(),
                ),
            ),
            "impulse" => (
                "ledgered".to_string(),
                None,
                Some("impulse morphs are census-only at this stage (SPEC 5.4/6.4)".to_string()),
            ),
            _ => (
                "ledgered".to_string(),
                None,
                Some(format!(
                    "unknown morph class {class}; ledgered, never dropped"
                )),
            ),
        };
        face_parameters.push(FaceEntry {
            name_local,
            name_universal,
            class,
            panel,
            route,
            fac_channel: fac,
            governed_channel: governed,
            unmapped_reason: reason,
        });
    }

    // ---- Mesh summary --------------------------------------------------
    let accessors = doc
        .get("accessors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut vertices = 0usize;
    let mut triangles = 0usize;
    if let Some(meshes) = doc.get("meshes").and_then(Value::as_array) {
        for mesh in meshes {
            if let Some(prims) = mesh.get("primitives").and_then(Value::as_array) {
                for prim in prims {
                    if vertices == 0
                        && let Some(pos) = prim
                            .get("attributes")
                            .and_then(|attrs| attrs.get("POSITION"))
                            .and_then(Value::as_u64)
                    {
                        vertices = accessors
                            .get(pos as usize)
                            .map(|acc| usize_field(acc, "count"))
                            .unwrap_or(0);
                    }
                    if let Some(idx) = prim.get("indices").and_then(Value::as_u64) {
                        triangles += accessors
                            .get(idx as usize)
                            .map(|acc| usize_field(acc, "count"))
                            .unwrap_or(0)
                            / 3;
                    }
                }
            }
        }
    }
    let mesh_summary = MeshSummary {
        vertices,
        triangles,
        materials: doc
            .get("materials")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        morph_targets: fac_rows.len(),
    };

    // ---- Physics (SPEC §6.4) -------------------------------------------
    let rigids = root_prov
        .get("rigid_bodies")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let joints = root_prov
        .get("joints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let bone_name = |bone: i64| -> String {
        if bone >= 0 && (bone as usize) < bone_count {
            skeleton[bone as usize].name_local.clone()
        } else {
            String::new()
        }
    };
    let radius_of = |rigid: &Value| -> f32 {
        let size = vec3_field(rigid, "size");
        match str_field(rigid, "shape").as_str() {
            "box" => size[0].min(size[1]).min(size[2]) / 2.0,
            _ => size[0],
        }
    };
    let particle_of = |rigid: &Value| -> Particle {
        let position = vec3_field(rigid, "position_pmx");
        Particle {
            rigid_index: usize_field(rigid, "index"),
            name: display_name(
                &str_field(rigid, "name_local"),
                &str_field(rigid, "name_universal"),
            ),
            bone_name: bone_name(i64_field(rigid, "bone")),
            radius: radius_of(rigid) * scale,
            position: [
                position[0] * scale,
                position[1] * scale,
                -position[2] * scale,
            ],
            mass: f32_field(rigid, "mass"),
            mobility: (1.0 - f32_field(rigid, "move_attenuation")).clamp(0.0, 1.0),
            damping: f32_field(rigid, "rotation_damping"),
            friction: f32_field(rigid, "friction"),
        }
    };
    // Chains: a particle rigid starts a strand when no ancestor bone
    // carries a particle rigid; strands walk child bones in order.
    let particle_bones: Vec<i64> = rigids
        .iter()
        .filter(|r| {
            matches!(
                str_field(r, "physics_mode").as_str(),
                "physics" | "physics_plus_bone"
            ) && i64_field(r, "bone") >= 0
                && (i64_field(r, "bone") as usize) < bone_count
        })
        .map(|r| i64_field(r, "bone"))
        .collect();
    let rigid_for_bone = |bone: i64| -> Option<&Value> {
        rigids.iter().find(|r| {
            i64_field(r, "bone") == bone
                && matches!(
                    str_field(r, "physics_mode").as_str(),
                    "physics" | "physics_plus_bone"
                )
        })
    };
    let mut strands: Vec<Strand> = Vec::new();
    let mut claimed: Vec<i64> = Vec::new();
    for rigid in &rigids {
        let bone = i64_field(rigid, "bone");
        if !particle_bones.contains(&bone) || claimed.contains(&bone) {
            continue;
        }
        // Root check: walk ancestors; if any carries a particle
        // rigid, this bone belongs to that ancestor's strand.
        let mut ancestor = skeleton[bone as usize].parent;
        let mut is_root = true;
        while let Some(parent) = ancestor {
            if particle_bones.contains(&(parent as i64)) {
                is_root = false;
                break;
            }
            ancestor = skeleton[parent].parent;
        }
        if !is_root {
            continue;
        }
        // Walk the chain: repeatedly take the lowest-index child
        // bone carrying a particle rigid (chains in PMX models are
        // linear in practice; branching children start their own
        // strands on later root passes because they stay unclaimed
        // only when their ancestor chain was not walked — the
        // claimed set keeps every particle in exactly one strand).
        let mut particles: Vec<Particle> = Vec::new();
        let mut modes: Vec<String> = Vec::new();
        let mut current = Some(bone);
        while let Some(current_bone) = current {
            if claimed.contains(&current_bone) {
                break;
            }
            if let Some(r) = rigid_for_bone(current_bone) {
                claimed.push(current_bone);
                particles.push(particle_of(r));
                modes.push(str_field(r, "physics_mode"));
            }
            current = skeleton
                .iter()
                .filter(|entry| entry.parent == Some(current_bone as usize))
                .map(|entry| entry.index as i64)
                .find(|child| particle_bones.contains(child) && !claimed.contains(child));
        }
        if !particles.is_empty() {
            let mode = if modes.iter().all(|m| m == &modes[0]) {
                modes[0].clone()
            } else {
                "mixed".to_string()
            };
            strands.push(Strand {
                id: strands.len(),
                root_bone: bone_name(bone),
                mode,
                particles,
            });
        }
    }
    // Any particle rigid not claimed by a chain walk (a branching
    // child whose parent chain took a sibling) becomes its own
    // single-particle strand — mapped, never dropped.
    for rigid in &rigids {
        let bone = i64_field(rigid, "bone");
        if particle_bones.contains(&bone) && !claimed.contains(&bone) {
            claimed.push(bone);
            strands.push(Strand {
                id: strands.len(),
                root_bone: bone_name(bone),
                mode: str_field(rigid, "physics_mode"),
                particles: vec![particle_of(rigid)],
            });
        }
    }
    let mut static_anchors: Vec<StaticAnchor> = Vec::new();
    let mut ledgered_rigids = 0usize;
    for rigid in &rigids {
        let bone = i64_field(rigid, "bone");
        let valid_bone = bone >= 0 && (bone as usize) < bone_count;
        match str_field(rigid, "physics_mode").as_str() {
            "follow_bone" if valid_bone => static_anchors.push(StaticAnchor {
                rigid_index: usize_field(rigid, "index"),
                name: display_name(
                    &str_field(rigid, "name_local"),
                    &str_field(rigid, "name_universal"),
                ),
                bone_name: bone_name(bone),
                semantics: "follow-bone mode -> static strand anchor (SPEC 6.4)".to_string(),
            }),
            "physics" | "physics_plus_bone" if valid_bone => {}
            _ => ledgered_rigids += 1,
        }
    }
    let groups = if strands.is_empty() {
        Vec::new()
    } else {
        vec![PhysicsGroup {
            strands: strands.iter().map(|strand| strand.id).collect(),
            gravity: "torogao_scene_default (substituted)".to_string(),
            wind: "torogao_scene_default (substituted)".to_string(),
            substitution_note: "PMX carries no gravity/wind fields (they are MMD scene \
                 settings); the strand group takes Torogao's authored scene defaults. \
                 Substitution recorded per SPEC 6.4, not a parity claim."
                .to_string(),
        }]
    };
    let physics = Physics {
        conversion_formulas: vec![
            "particle_radius: sphere -> size[0]; capsule -> size[0]; box -> min(size[0], size[1], size[2]) / 2; result scaled by scale_to_meters".to_string(),
            "particle_position: position_pmx * scale_to_meters with z negated (SPEC 5.1 basis change)".to_string(),
            "mobility: 1.0 - clamp(move_attenuation, 0.0, 1.0) (PMX attenuation inverts to strand mobility)".to_string(),
            "damping: rotation_damping carried direct (both are per-step damping coefficients in 0..=1)".to_string(),
            "friction: friction carried direct".to_string(),
            "mass: mass carried verbatim in PMX mass units (starting point for tuning, not a parity claim — SPEC 6.4)".to_string(),
        ],
        strands,
        static_anchors,
        groups,
    };

    // ---- Dropped ledger --------------------------------------------------
    let dropped_ledger = DroppedLedger {
        joint_axis_limits: joints
            .iter()
            .map(|joint| JointLimits {
                name: display_name(
                    &str_field(joint, "name_local"),
                    &str_field(joint, "name_universal"),
                ),
                position_min: vec3_field(joint, "position_min"),
                position_max: vec3_field(joint, "position_max"),
                rotation_min: vec3_field(joint, "rotation_min"),
                rotation_max: vec3_field(joint, "rotation_max"),
            })
            .collect(),
        joint_springs: joints
            .iter()
            .map(|joint| JointSprings {
                name: display_name(
                    &str_field(joint, "name_local"),
                    &str_field(joint, "name_universal"),
                ),
                position_spring: vec3_field(joint, "position_spring"),
                rotation_spring: vec3_field(joint, "rotation_spring"),
            })
            .collect(),
        collision: rigids
            .iter()
            .map(|rigid| CollisionEntry {
                name: display_name(
                    &str_field(rigid, "name_local"),
                    &str_field(rigid, "name_universal"),
                ),
                group_id: usize_field(rigid, "group_id") as u8,
                non_collision_mask: i64_field(rigid, "non_collision_mask") as i16,
            })
            .collect(),
        impulse_morphs: face_parameters
            .iter()
            .filter(|entry| entry.class == "impulse")
            .map(|entry| display_name(&entry.name_local, &entry.name_universal))
            .collect(),
        bone_morphs: face_parameters
            .iter()
            .filter(|entry| entry.class == "bone")
            .map(|entry| display_name(&entry.name_local, &entry.name_universal))
            .collect(),
        material_morphs: face_parameters
            .iter()
            .filter(|entry| entry.class == "material")
            .map(|entry| display_name(&entry.name_local, &entry.name_universal))
            .collect(),
        incomplete_recipes: face_parameters
            .iter()
            .filter(|entry| {
                (entry.class == "group" || entry.class == "flip") && entry.route == "ledgered"
            })
            .map(|entry| display_name(&entry.name_local, &entry.name_universal))
            .collect(),
        soft_bodies: root_prov
            .get("census_counts")
            .map(|counts| usize_field(counts, "soft_bodies"))
            .unwrap_or(0),
        ik_bones,
        inherit_bones,
        unmapped_bones: skeleton
            .iter()
            .filter(|entry| entry.torogao.is_none())
            .map(|entry| entry.name_local.clone())
            .collect(),
        unmapped_morphs: face_parameters
            .iter()
            .filter(|entry| entry.route == "morph_target" && entry.fac_channel.is_none())
            .map(|entry| display_name(&entry.name_local, &entry.name_universal))
            .collect(),
        missing_textures: root_prov
            .get("missing_textures")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    };

    // ---- Accounting (zero silent drops) ----------------------------------
    let counts = root_prov
        .get("census_counts")
        .cloned()
        .unwrap_or(Value::Null);
    let class_count = |class: &str| -> usize {
        counts
            .get("morphs_by_class")
            .map(|by_class| usize_field(by_class, class))
            .unwrap_or(0)
    };
    let morph_row = |class: &str| -> AccountingRow {
        let mapped = face_parameters
            .iter()
            .filter(|entry| entry.class == class && entry.route != "ledgered")
            .count();
        let ledgered = face_parameters
            .iter()
            .filter(|entry| entry.class == class && entry.route == "ledgered")
            .count();
        AccountingRow {
            feature: format!("morphs:{class}"),
            census_count: class_count(class),
            mapped_count: mapped,
            ledgered_count: ledgered,
        }
    };
    let strand_particles: usize = physics
        .strands
        .iter()
        .map(|strand| strand.particles.len())
        .sum();
    let accounting = vec![
        AccountingRow {
            feature: "bones".to_string(),
            census_count: usize_field(&counts, "bones"),
            mapped_count: skeleton.iter().filter(|e| e.torogao.is_some()).count(),
            ledgered_count: skeleton.iter().filter(|e| e.torogao.is_none()).count(),
        },
        morph_row("group"),
        morph_row("vertex"),
        morph_row("bone"),
        morph_row("uv"),
        morph_row("material"),
        morph_row("flip"),
        morph_row("impulse"),
        AccountingRow {
            feature: "morphs:total".to_string(),
            census_count: usize_field(&counts, "morphs_total"),
            mapped_count: face_parameters
                .iter()
                .filter(|e| e.route != "ledgered")
                .count(),
            ledgered_count: face_parameters
                .iter()
                .filter(|e| e.route == "ledgered")
                .count(),
        },
        AccountingRow {
            feature: "rigid_bodies".to_string(),
            census_count: usize_field(&counts, "rigid_bodies"),
            mapped_count: strand_particles + physics.static_anchors.len(),
            ledgered_count: ledgered_rigids,
        },
        AccountingRow {
            feature: "joints".to_string(),
            census_count: usize_field(&counts, "joints"),
            mapped_count: 0,
            ledgered_count: dropped_ledger.joint_axis_limits.len(),
        },
        AccountingRow {
            feature: "textures".to_string(),
            census_count: usize_field(&counts, "textures"),
            mapped_count: root_prov
                .get("embedded_textures")
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
            ledgered_count: dropped_ledger.missing_textures.len(),
        },
        AccountingRow {
            feature: "materials".to_string(),
            census_count: usize_field(&counts, "materials"),
            mapped_count: usize_field(&counts, "materials"),
            ledgered_count: 0,
        },
    ];

    Ok(IngestRecord {
        schema: INGEST_SCHEMA.to_string(),
        provenance: Provenance {
            source_pmx_sha256: root_prov
                .get("source_file_sha256")
                .and_then(Value::as_str)
                .map(str::to_string),
            glb_sha256: sha256_hex(glb_bytes),
            profile_id: PROFILE_ID.to_string(),
            converter: "gltf-import/1".to_string(),
        },
        skeleton,
        face_parameters,
        mesh_summary,
        physics,
        dropped_ledger,
        accounting,
    })
}

// ---------------------------------------------------------------------
// mmd-profile/1 RON emission — machine-derived from the profile.rs
// tables (the single source), never hand-duplicated.
// ---------------------------------------------------------------------

fn ron_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn ron_option(value: Option<&str>) -> String {
    match value {
        Some(inner) => format!("Some({})", ron_string(inner)),
        None => "None".to_string(),
    }
}

/// Render `mmd-profile/1.ron` from the `profile.rs` tables: bone
/// rows, morph rows (with `Fac_` channels), the `Fac_` vocabulary,
/// and the SPEC §6.4 physics posture. Byte-deterministic.
pub fn emit_profile_ron() -> String {
    let profile = MmdProfile::builtin();
    let mut out = String::new();
    out.push_str("// mmd-profile/1 — machine-derived from lumen-factory's profile.rs\n");
    out.push_str("// tables by `gltf_ingest --emit-profile` (gltf-import/1, D-633).\n");
    out.push_str("// Do not edit by hand: amend the profile.rs tables and re-emit.\n");
    out.push_str("(\n");
    out.push_str(&format!("    id: {},\n", ron_string(profile.id)));
    out.push_str(&format!(
        "    scale_to_meters: {},\n",
        profile.proportions.scale_to_meters
    ));
    out.push_str("    bones: [\n");
    for row in BONE_MAPPINGS {
        out.push_str(&format!(
            "        (pmx_local: {}, gloss: {}, torogao: {}),\n",
            ron_string(row.pmx_local),
            ron_string(row.gloss),
            ron_option(if row.torogao.is_empty() {
                None
            } else {
                Some(row.torogao)
            }),
        ));
    }
    out.push_str("    ],\n");
    out.push_str("    morphs: [\n");
    for row in MORPH_MAPPINGS {
        out.push_str(&format!(
            "        (pmx_local: {}, gloss: {}, torogao_channel: {}, fac_channel: {}),\n",
            ron_string(row.pmx_local),
            ron_string(row.gloss),
            ron_option(Some(row.torogao_channel)),
            ron_option(fac_channel(row.pmx_local, "")),
        ));
    }
    out.push_str("    ],\n");
    out.push_str("    fac_channels: [");
    out.push_str(
        &FAC_CHANNELS
            .iter()
            .map(|channel| ron_string(channel))
            .collect::<Vec<_>>()
            .join(", "),
    );
    out.push_str("],\n");
    out.push_str("    physics: [\n");
    for (record, home, posture) in [
        (
            "rigid body on a bone chain",
            Some("Strand particles along the chain"),
            "mapped_shape",
        ),
        (
            "mass, move attenuation, rotation damping, friction",
            Some("Particle mobility / damping / friction scalars"),
            "approximate_formula_stated",
        ),
        (
            "joint position/rotation limits (per axis)",
            None,
            "unmapped_ledgered_verbatim",
        ),
        (
            "joint spring constants (per axis)",
            None,
            "unmapped_ledgered_verbatim",
        ),
        (
            "collision groups + non-collision masks (16 groups)",
            None,
            "unmapped_ledgered_verbatim",
        ),
        (
            "physics mode: follow-bone",
            Some("static strand anchor"),
            "mapped_semantics",
        ),
        (
            "physics modes: physics / physics + bone",
            Some("strand group under gravity/wind"),
            "partial_gravity_wind_substituted",
        ),
        ("impulse morphs", None, "unmapped_ledgered_verbatim"),
    ] {
        out.push_str(&format!(
            "        (pmx_record: {}, torogao_home: {}, posture: {}),\n",
            ron_string(record),
            ron_option(home),
            ron_string(posture),
        ));
    }
    out.push_str("    ],\n");
    out.push_str(")\n");
    out
}
