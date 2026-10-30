//! The runtime rig: a model, its parameter state, its
//! physics simulation, and its current evaluated pose, kept
//! in step.
//!
//! This is the object a host (renderer, game, batch tool)
//! drives: set parameters, advance physics in fixed steps,
//! read deformed meshes. The step discipline is section
//! 5.2's: a step reads the anchors of the pose at step
//! entry, integrates every strand, writes physics outputs
//! into the parameter state, and only then re-evaluates the
//! pose — so a frame is always a consistent (parameters,
//! geometry) pair, and replaying the same call sequence
//! reproduces it bit for bit.

use crate::model::Model;
use crate::physics::{PhysicsSim, anchor_positions};
use crate::pose::{EvaluatedMesh, ParameterValues, evaluate_pose};

/// A live rig instance. Owns its model; construct with
/// [`Rig::new`], drive with [`Rig::set_parameter`] and
/// [`Rig::step`], render from [`Rig::meshes`].
#[derive(Debug)]
pub struct Rig {
    model: Model,
    values: ParameterValues,
    physics: PhysicsSim,
    meshes: Vec<EvaluatedMesh>,
}

impl Rig {
    /// A rig at rest: scalar defaults, derived combined
    /// parameters, physics strands laid out along +x from
    /// their rest anchors (section 5.1), pose evaluated.
    /// `run_seed` is the turbulence stream seed (section
    /// 5.3) — a simulation input, recorded by the host's
    /// session, never read from a clock.
    pub fn new(model: Model, run_seed: u64) -> Rig {
        let values = ParameterValues::rest(&model);
        let physics = PhysicsSim::new(&model, &values, run_seed);
        let meshes = evaluate_pose(&model, &values);
        Rig {
            model,
            values,
            physics,
            meshes,
        }
    }

    /// The rig's model.
    pub fn model(&self) -> &Model {
        &self.model
    }

    /// The current parameter state (physics outputs
    /// included, after any steps taken).
    pub fn values(&self) -> &ParameterValues {
        &self.values
    }

    /// The current evaluated pose.
    pub fn meshes(&self) -> &[EvaluatedMesh] {
        &self.meshes
    }

    /// Write one scalar parameter (section 9.3 clamp
    /// applies) and re-evaluate the pose. Physics state is
    /// untouched — strands respond on the next [`Rig::step`].
    /// Returns `false` for an unknown id or a combined
    /// parameter.
    pub fn set_parameter(&mut self, id: &str, value: f32) -> bool {
        if !self.values.set(&self.model, id, value) {
            return false;
        }
        self.meshes = evaluate_pose(&self.model, &self.values);
        true
    }

    /// Blend a motion's contributions at its current player
    /// state onto the parameter state, with section 4.3's
    /// host law exactly as the format's trace harness
    /// (`examples/evaluate_motion.rs`) performs it: each
    /// contribution blends onto the value its parameter
    /// currently holds, in contribution order, through the
    /// sanctioned clamp. Then re-evaluate the pose.
    pub fn apply_contributions(
        &mut self,
        contributions: &[crate::evaluate::EvaluatedContribution],
    ) {
        for contribution in contributions {
            self.values.write_blended(
                &self.model,
                &contribution.parameter,
                contribution.weight,
                contribution.value,
            );
        }
        self.meshes = evaluate_pose(&self.model, &self.values);
    }

    /// Advance physics one fixed step of `dt_seconds`
    /// (section 5.1: conforming hosts use 1/120, or a
    /// declared 1/30 or 1/60) and re-evaluate the pose under
    /// the updated parameter state.
    pub fn step(&mut self, dt_seconds: f64) {
        let anchors = anchor_positions(&self.model, &self.values, &self.meshes);
        self.physics
            .step(&self.model, &mut self.values, &anchors, dt_seconds);
        self.meshes = evaluate_pose(&self.model, &self.values);
    }

    /// The physics simulation (step counter, strand states).
    pub fn physics(&self) -> &PhysicsSim {
        &self.physics
    }
}
