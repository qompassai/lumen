//! Structured errors with stable machine-readable codes.
//!
//! Contract: every refusal atelier returns carries a `code` from the fixed
//! set below and a `class` naming which boundary failed (configuration,
//! profile, model admission, source path, engine lifecycle, workflow
//! validation, execution, output, cleanup, or tool arguments). Codes are
//! part of the public contract; renaming one is a breaking change.

use serde::Serialize;

/// Failure classes. Mutually exclusive; each code belongs to exactly one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    Configuration,
    Profile,
    ModelAdmission,
    SourcePath,
    EngineLifecycle,
    WorkflowValidation,
    Execution,
    Output,
    Cleanup,
    Argument,
}

/// Stable error codes. `as_str` is the wire form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    ConfigMissing,
    ConfigUnreadable,
    ConfigInvalid,
    ProfileMissing,
    ProfileUnknown,
    ProfileBlockMissing,
    ProfileRootInvalid,
    ProfileRootOverlap,
    ModelUnknown,
    ModelNotAdmitted,
    CheckpointMissing,
    CheckpointHashMismatch,
    CheckpointNameMismatch,
    CheckpointNotVisible,
    SourceOutsideRoots,
    SourceNotRegularFile,
    SourceUnsupportedFormat,
    SourceTooLarge,
    SourceUnreadable,
    EngineAddressNotLoopback,
    EngineUnownedPresent,
    EngineStartFailed,
    EngineExited,
    EngineNotReady,
    EngineBusy,
    GpuMismatch,
    VramInsufficient,
    LeaseHeld,
    TemplateNotApiFormat,
    TemplateUnexpectedNode,
    TemplatePatchPointMissing,
    TemplateLinkInvalid,
    NodeClassMissing,
    ComfyValidationFailed,
    ExecutionFailed,
    JobTimeout,
    HistoryMissing,
    HistoryWrongPrompt,
    BackendProtocol,
    OutputDescriptorMissing,
    OutputEscapeRefused,
    OutputFetchFailed,
    OutputInvalidImage,
    OutputWriteFailed,
    CleanupFailed,
    ArgumentInvalid,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::ConfigMissing => "config_missing",
            ErrorCode::ConfigUnreadable => "config_unreadable",
            ErrorCode::ConfigInvalid => "config_invalid",
            ErrorCode::ProfileMissing => "profile_missing",
            ErrorCode::ProfileUnknown => "profile_unknown",
            ErrorCode::ProfileBlockMissing => "profile_block_missing",
            ErrorCode::ProfileRootInvalid => "profile_root_invalid",
            ErrorCode::ProfileRootOverlap => "profile_root_overlap",
            ErrorCode::ModelUnknown => "model_unknown",
            ErrorCode::ModelNotAdmitted => "model_not_admitted",
            ErrorCode::CheckpointMissing => "checkpoint_missing",
            ErrorCode::CheckpointHashMismatch => "checkpoint_hash_mismatch",
            ErrorCode::CheckpointNameMismatch => "checkpoint_name_mismatch",
            ErrorCode::CheckpointNotVisible => "checkpoint_not_visible",
            ErrorCode::SourceOutsideRoots => "source_outside_roots",
            ErrorCode::SourceNotRegularFile => "source_not_regular_file",
            ErrorCode::SourceUnsupportedFormat => "source_unsupported_format",
            ErrorCode::SourceTooLarge => "source_too_large",
            ErrorCode::SourceUnreadable => "source_unreadable",
            ErrorCode::EngineAddressNotLoopback => "engine_address_not_loopback",
            ErrorCode::EngineUnownedPresent => "engine_unowned_present",
            ErrorCode::EngineStartFailed => "engine_start_failed",
            ErrorCode::EngineExited => "engine_exited",
            ErrorCode::EngineNotReady => "engine_not_ready",
            ErrorCode::EngineBusy => "engine_busy",
            ErrorCode::GpuMismatch => "gpu_mismatch",
            ErrorCode::VramInsufficient => "vram_insufficient",
            ErrorCode::LeaseHeld => "lease_held",
            ErrorCode::TemplateNotApiFormat => "template_not_api_format",
            ErrorCode::TemplateUnexpectedNode => "template_unexpected_node",
            ErrorCode::TemplatePatchPointMissing => "template_patch_point_missing",
            ErrorCode::TemplateLinkInvalid => "template_link_invalid",
            ErrorCode::NodeClassMissing => "node_class_missing",
            ErrorCode::ComfyValidationFailed => "comfy_validation_failed",
            ErrorCode::ExecutionFailed => "execution_failed",
            ErrorCode::JobTimeout => "job_timeout",
            ErrorCode::HistoryMissing => "history_missing",
            ErrorCode::HistoryWrongPrompt => "history_wrong_prompt",
            ErrorCode::BackendProtocol => "backend_protocol",
            ErrorCode::OutputDescriptorMissing => "output_descriptor_missing",
            ErrorCode::OutputEscapeRefused => "output_escape_refused",
            ErrorCode::OutputFetchFailed => "output_fetch_failed",
            ErrorCode::OutputInvalidImage => "output_invalid_image",
            ErrorCode::OutputWriteFailed => "output_write_failed",
            ErrorCode::CleanupFailed => "cleanup_failed",
            ErrorCode::ArgumentInvalid => "argument_invalid",
        }
    }

    pub fn class(self) -> ErrorClass {
        use ErrorCode as C;
        match self {
            C::ConfigMissing | C::ConfigUnreadable | C::ConfigInvalid => ErrorClass::Configuration,
            C::ProfileMissing
            | C::ProfileUnknown
            | C::ProfileBlockMissing
            | C::ProfileRootInvalid
            | C::ProfileRootOverlap => ErrorClass::Profile,
            C::ModelUnknown
            | C::ModelNotAdmitted
            | C::CheckpointMissing
            | C::CheckpointHashMismatch
            | C::CheckpointNameMismatch
            | C::CheckpointNotVisible => ErrorClass::ModelAdmission,
            C::SourceOutsideRoots
            | C::SourceNotRegularFile
            | C::SourceUnsupportedFormat
            | C::SourceTooLarge
            | C::SourceUnreadable => ErrorClass::SourcePath,
            C::EngineAddressNotLoopback
            | C::EngineUnownedPresent
            | C::EngineStartFailed
            | C::EngineExited
            | C::EngineNotReady
            | C::EngineBusy
            | C::GpuMismatch
            | C::VramInsufficient
            | C::LeaseHeld => ErrorClass::EngineLifecycle,
            C::TemplateNotApiFormat
            | C::TemplateUnexpectedNode
            | C::TemplatePatchPointMissing
            | C::TemplateLinkInvalid
            | C::NodeClassMissing => ErrorClass::WorkflowValidation,
            C::ComfyValidationFailed
            | C::ExecutionFailed
            | C::JobTimeout
            | C::HistoryMissing
            | C::HistoryWrongPrompt
            | C::BackendProtocol => ErrorClass::Execution,
            C::OutputDescriptorMissing
            | C::OutputEscapeRefused
            | C::OutputFetchFailed
            | C::OutputInvalidImage
            | C::OutputWriteFailed => ErrorClass::Output,
            C::CleanupFailed => ErrorClass::Cleanup,
            C::ArgumentInvalid => ErrorClass::Argument,
        }
    }
}

/// Longest message carried by an error, in bytes; bounds echoed engine text.
pub const ERROR_MESSAGE_BYTES_MAX: usize = 2048;

/// One refusal: a stable code plus bounded human context.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("[{}] {message}", code.as_str())]
pub struct AtelierError {
    pub code: ErrorCode,
    pub message: String,
}

impl AtelierError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        let mut message: String = message.into();
        if message.len() > ERROR_MESSAGE_BYTES_MAX {
            let mut cut = ERROR_MESSAGE_BYTES_MAX;
            while !message.is_char_boundary(cut) {
                cut -= 1;
            }
            message.truncate(cut);
            message.push('…');
        }
        Self { code, message }
    }

    /// Wire form returned to MCP callers.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "error": {
                "code": self.code.as_str(),
                "class": self.code.class(),
                "message": self.message,
            }
        })
    }
}

pub type Result<T> = std::result::Result<T, AtelierError>;

pub fn err<T>(code: ErrorCode, message: impl Into<String>) -> Result<T> {
    Err(AtelierError::new(code, message))
}
