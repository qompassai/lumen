//! Adversarial tests for the VRMC_springBone evaluator:
//! every construction failure is a typed `SpringError`,
//! produced before any state is built.

use lumen_rig_core::spring_bone::{
    ColliderDef, ColliderShape, Frame, JointDef, JointParams, Quat, SPRING_CHAINS_MAX,
    SpringChainDef, SpringConfig, SpringError, SpringSystem, SpringSystemDef, TimeMode, Vec3,
};

fn joint(node: usize, translation: Vec3, params: JointParams) -> JointDef {
    JointDef {
        node,
        params,
        rest_local_translation: translation,
        rest_local_rotation: Quat::IDENTITY,
    }
}

fn valid_params() -> JointParams {
    JointParams {
        hit_radius: 0.01,
        stiffness: 1.0,
        gravity_power: 0.5,
        gravity_dir: Vec3::new(0.0, -1.0, 0.0),
        drag_force: 0.5,
    }
}

fn valid_chain() -> SpringChainDef {
    SpringChainDef {
        joints: vec![
            joint(0, Vec3::ZERO, valid_params()),
            joint(1, Vec3::new(0.1, 0.0, 0.0), valid_params()),
        ],
        collider_groups: Vec::new(),
        center_node: None,
    }
}

fn build(def: SpringSystemDef) -> Result<SpringSystem, SpringError> {
    SpringSystem::new(def, &Frame::IDENTITY)
}

fn one_chain(chain: SpringChainDef) -> SpringSystemDef {
    SpringSystemDef {
        springs: vec![chain],
        ..SpringSystemDef::default()
    }
}

#[test]
fn empty_system_is_rejected() {
    assert_eq!(
        build(SpringSystemDef::default()).unwrap_err(),
        SpringError::EmptySystem
    );
}

#[test]
fn too_many_chains_is_rejected() {
    let def = SpringSystemDef {
        springs: vec![valid_chain(); SPRING_CHAINS_MAX + 1],
        ..SpringSystemDef::default()
    };
    assert_eq!(
        build(def).unwrap_err(),
        SpringError::TooManyChains {
            count: SPRING_CHAINS_MAX + 1,
            max: SPRING_CHAINS_MAX,
        }
    );
}

#[test]
fn empty_chain_is_rejected() {
    let chain = SpringChainDef {
        joints: Vec::new(),
        collider_groups: Vec::new(),
        center_node: None,
    };
    assert_eq!(
        build(one_chain(chain)).unwrap_err(),
        SpringError::EmptyChain { chain: 0 }
    );
}

#[test]
fn single_joint_chain_is_rejected() {
    let chain = SpringChainDef {
        joints: vec![joint(0, Vec3::ZERO, valid_params())],
        collider_groups: Vec::new(),
        center_node: None,
    };
    assert_eq!(
        build(one_chain(chain)).unwrap_err(),
        SpringError::ChainTooShort { chain: 0, count: 1 }
    );
}

#[test]
fn center_node_is_rejected_not_ignored() {
    let chain = SpringChainDef {
        center_node: Some(0),
        ..valid_chain()
    };
    assert_eq!(
        build(one_chain(chain)).unwrap_err(),
        SpringError::CenterUnsupported { chain: 0 }
    );
}

#[test]
fn unknown_collider_group_is_rejected() {
    let chain = SpringChainDef {
        collider_groups: vec![7],
        ..valid_chain()
    };
    assert_eq!(
        build(one_chain(chain)).unwrap_err(),
        SpringError::UnknownColliderGroup { chain: 0, group: 7 }
    );
}

#[test]
fn unknown_collider_in_group_is_rejected() {
    let def = SpringSystemDef {
        collider_groups: vec![vec![3]],
        springs: vec![valid_chain()],
        ..SpringSystemDef::default()
    };
    assert_eq!(
        build(def).unwrap_err(),
        SpringError::UnknownCollider {
            group: 0,
            collider: 3
        }
    );
}

#[test]
fn invalid_joint_params_are_rejected() {
    let cases: Vec<(JointParams, &'static str)> = vec![
        (
            JointParams {
                drag_force: 1.5,
                ..valid_params()
            },
            "drag_force",
        ),
        (
            JointParams {
                drag_force: f64::NAN,
                ..valid_params()
            },
            "drag_force",
        ),
        (
            JointParams {
                stiffness: -0.5,
                ..valid_params()
            },
            "stiffness",
        ),
        (
            JointParams {
                gravity_power: f64::INFINITY,
                ..valid_params()
            },
            "gravity_power",
        ),
        (
            JointParams {
                gravity_dir: Vec3::ZERO,
                ..valid_params()
            },
            "gravity_dir",
        ),
        (
            JointParams {
                hit_radius: -0.01,
                ..valid_params()
            },
            "hit_radius",
        ),
    ];
    for (params, field) in cases {
        let chain = SpringChainDef {
            joints: vec![
                joint(0, Vec3::ZERO, params),
                joint(1, Vec3::new(0.1, 0.0, 0.0), valid_params()),
            ],
            collider_groups: Vec::new(),
            center_node: None,
        };
        assert_eq!(
            build(one_chain(chain)).unwrap_err(),
            SpringError::InvalidJointParam {
                chain: 0,
                joint: 0,
                field,
            },
            "case {field}"
        );
    }
}

#[test]
fn non_finite_rest_transform_is_rejected() {
    let chain = SpringChainDef {
        joints: vec![
            joint(0, Vec3::new(f64::NAN, 0.0, 0.0), valid_params()),
            joint(1, Vec3::new(0.1, 0.0, 0.0), valid_params()),
        ],
        collider_groups: Vec::new(),
        center_node: None,
    };
    assert_eq!(
        build(one_chain(chain)).unwrap_err(),
        SpringError::InvalidJointTransform { chain: 0, joint: 0 }
    );
}

#[test]
fn degenerate_bone_is_rejected() {
    let chain = SpringChainDef {
        joints: vec![
            joint(0, Vec3::ZERO, valid_params()),
            joint(1, Vec3::ZERO, valid_params()),
        ],
        collider_groups: Vec::new(),
        center_node: None,
    };
    assert_eq!(
        build(one_chain(chain)).unwrap_err(),
        SpringError::DegenerateBone { chain: 0, joint: 0 }
    );
}

#[test]
fn negative_collider_radius_is_rejected() {
    let def = SpringSystemDef {
        colliders: vec![ColliderDef {
            node: 90,
            shape: ColliderShape::Sphere {
                offset: Vec3::ZERO,
                radius: -1.0,
            },
        }],
        springs: vec![valid_chain()],
        ..SpringSystemDef::default()
    };
    assert_eq!(
        build(def).unwrap_err(),
        SpringError::InvalidCollider {
            collider: 0,
            field: "radius",
        }
    );
}

#[test]
fn non_convention_step_dt_is_rejected_in_fixed_step_mode() {
    let config = SpringConfig {
        mode: TimeMode::FixedStep,
        step_dt_seconds: 1.0 / 90.0,
    };
    let result = SpringSystem::with_config(one_chain(valid_chain()), &Frame::IDENTITY, config);
    assert_eq!(
        result.unwrap_err(),
        SpringError::InvalidStepDt { found: 1.0 / 90.0 }
    );
}

#[test]
fn step_dt_is_not_validated_in_per_frame_mode() {
    // The fixed step is unused in per-frame mode; the
    // configuration is inert there, not an error.
    let config = SpringConfig {
        mode: TimeMode::PerFrame,
        step_dt_seconds: 1.0 / 90.0,
    };
    let result = SpringSystem::with_config(one_chain(valid_chain()), &Frame::IDENTITY, config);
    assert!(result.is_ok());
}
