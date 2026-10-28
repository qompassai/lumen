//! Format bounds, mirroring SPEC.md section 11 one-for-one. Every
//! limit a validator enforces lives here with its unit in the name
//! or the doc comment; no magic numbers elsewhere in the crate.

/// Maximum asset file size, in bytes (16 MiB).
pub const INPUT_BYTES_MAX: usize = 16_777_216;
/// Maximum id length, in bytes.
pub const ID_BYTES_MAX: usize = 64;
/// Maximum free-text field length, in bytes (generator, names).
pub const NAME_BYTES_MAX: usize = 128;
/// Maximum texture source path length, in bytes.
pub const PATH_BYTES_MAX: usize = 256;

pub const PARAMETERS_MAX: usize = 512;
pub const COMBINED_TERMS_MAX: usize = 16;
pub const PARTS_MAX: usize = 256;
pub const TEXTURES_MAX: usize = 64;
pub const DEFORMERS_MAX: usize = 512;
pub const MESHES_MAX: usize = 1024;
pub const PHYSICS_GROUPS_MAX: usize = 64;
pub const STRANDS_PER_GROUP_MAX: usize = 32;
pub const PARTICLES_PER_STRAND_MAX: usize = 64;

pub const VERTICES_PER_MESH_MAX: usize = 65_536;
pub const INDICES_PER_MESH_MAX: usize = 393_216;
pub const KEYS_PER_AXIS_MAX: usize = 32;
pub const KEYFORMS_PER_OBJECT_MAX: usize = 1024;
pub const TREE_DEPTH_MAX: usize = 64;

/// Maximum absolute value of any model-space coordinate component.
pub const COORD_MAGNITUDE_MAX: f32 = 1e6;
/// Maximum absolute component of gravity / wind accelerations,
/// in model units per second squared.
pub const ACCEL_MAGNITUDE_MAX: f32 = 1e5;
/// Draw order range (inclusive both ends).
pub const DRAW_ORDER_MIN: i32 = -10_000;
pub const DRAW_ORDER_MAX: i32 = 10_000;
