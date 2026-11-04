//! The bounded value-expression form records use to name
//! tuned quantities. A pose parameter or placement input is
//! either a constant or a small arithmetic form over the
//! tune's named outputs (`g`, `cd`, `turn_lift`, `b_x`, ...):
//! a name, a name plus a constant, or a name times a
//! constant. There is deliberately no general expression
//! language — records stay declarative data, and every form
//! here evaluates in one step with no allocation beyond the
//! name lookup (SPEC section 4.1: bounded work, no hidden
//! behaviour).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::EngineError;

/// A source of named tune outputs.
pub trait NameSource {
    /// The value bound to `name`, if any.
    fn lookup(&self, name: &str) -> Option<f64>;
}

impl NameSource for BTreeMap<String, f64> {
    fn lookup(&self, name: &str) -> Option<f64> {
        self.get(name).copied()
    }
}

impl NameSource for &BTreeMap<String, f64> {
    fn lookup(&self, name: &str) -> Option<f64> {
        self.get(name).copied()
    }
}

/// One bounded value form. In RON: `Const(0.9)`,
/// `Ref("g")`, `Add("g", 0.05)`, `Mul("cd", 0.3125)`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum ValueExpr {
    /// A literal constant.
    Const(f64),
    /// A tune output by name.
    Ref(String),
    /// A tune output plus a constant offset.
    Add(String, f64),
    /// A tune output times a constant factor.
    Mul(String, f64),
}

impl ValueExpr {
    /// Evaluate against a name source. An unresolved name
    /// is a typed refusal (LE-103), never a silent zero.
    pub fn eval(&self, names: &dyn NameSource) -> Result<f64, EngineError> {
        match self {
            ValueExpr::Const(value) => Ok(*value),
            ValueExpr::Ref(name) => resolve(name, names),
            ValueExpr::Add(name, offset) => resolve(name, names).map(|v| v + offset),
            ValueExpr::Mul(name, factor) => resolve(name, names).map(|v| v * factor),
        }
    }
}

fn resolve(name: &str, names: &dyn NameSource) -> Result<f64, EngineError> {
    names
        .lookup(name)
        .ok_or_else(|| EngineError::UnknownName(name.to_string()))
}
