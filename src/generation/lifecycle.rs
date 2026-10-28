//! Engine lifecycle: on-demand start, ownership, and verified stop.
//!
//! Contract:
//! - atelier starts the engine only for a generation job, only when nothing
//!   already listens on the configured loopback port (an engine atelier did
//!   not start is `engine_unowned_present`: never attached, never stopped);
//! - starting requires the host-wide lease (`state_root/engine.lease`,
//!   an exclusive file lock), so two atelier instances under different
//!   profiles cannot both own engines;
//! - before launch, free VRAM on the configured GPU index is checked against
//!   `vram_free_mib_min` and the GPU name against `expected_gpu_name`;
//! - the child is spawned with explicit argv (no shell), the pinned CUDA
//!   environment, and stdout/stderr to `state_root/engine.log`;
//! - stop kills only the owned child (by its handle, never by name), waits
//!   for exit, and verifies the port is free; any failure leaves the
//!   lifecycle `degraded` until a later stop succeeds.

use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::generation::backend::MediaBackend;
use crate::generation::config::EngineConfig;
use crate::generation::error::{AtelierError, ErrorCode, Result, err};

pub const READY_POLL: Duration = Duration::from_millis(500);
pub const PORT_PROBE_TIMEOUT: Duration = Duration::from_millis(300);
pub const STOP_WAIT: Duration = Duration::from_secs(30);
pub const PORT_RELEASE_WAIT: Duration = Duration::from_secs(15);
pub const GPU_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const MIB: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineState {
    Stopped,
    OwnedRunning,
    UnownedPresent,
    Degraded,
}

/// Pre-launch GPU facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GpuSnapshot {
    pub index: u32,
    pub name: String,
    pub memory_used_mib: u64,
    pub memory_free_mib: u64,
}

/// Reads GPU facts without touching any engine.
pub trait GpuProbe: Send + Sync {
    fn snapshot(&self, index: u32) -> impl Future<Output = Result<GpuSnapshot>> + Send;
}

/// `nvidia-smi` query for one index (PCI bus order).
#[derive(Debug, Clone)]
pub struct NvidiaSmi {
    pub program: PathBuf,
}

pub fn parse_nvidia_smi(index: u32, text: &str) -> Result<GpuSnapshot> {
    let line = text.lines().next().unwrap_or_default();
    let fields: Vec<&str> = line.split(',').map(str::trim).collect();
    let [name, used, free] = fields.as_slice() else {
        return err(
            ErrorCode::GpuMismatch,
            format!("unparseable nvidia-smi line: {line}"),
        );
    };
    let number = |text: &str| {
        text.parse::<u64>()
            .map_err(|_| AtelierError::new(ErrorCode::GpuMismatch, "nvidia-smi memory field"))
    };
    Ok(GpuSnapshot {
        index,
        name: (*name).to_string(),
        memory_used_mib: number(used)?,
        memory_free_mib: number(free)?,
    })
}

impl GpuProbe for NvidiaSmi {
    async fn snapshot(&self, index: u32) -> Result<GpuSnapshot> {
        let mut command = tokio::process::Command::new(&self.program);
        command
            .arg("--query-gpu=name,memory.used,memory.free")
            .arg("--format=csv,noheader,nounits")
            .arg(format!("--id={index}"))
            .env("CUDA_DEVICE_ORDER", "PCI_BUS_ID")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        let output = tokio::time::timeout(GPU_PROBE_TIMEOUT, command.output())
            .await
            .map_err(|_| AtelierError::new(ErrorCode::GpuMismatch, "nvidia-smi timed out"))?
            .map_err(|cause| AtelierError::new(ErrorCode::GpuMismatch, cause.to_string()))?;
        if !output.status.success() {
            return err(ErrorCode::GpuMismatch, "nvidia-smi failed");
        }
        parse_nvidia_smi(index, &String::from_utf8_lossy(&output.stdout))
    }
}

/// Spawns the engine child. Real: the pinned launch program. Tests: a fake.
pub trait Launcher: Send + Sync {
    fn launch(&self, engine: &EngineConfig, log_path: &Path) -> Result<tokio::process::Child>;
}

#[derive(Debug, Clone, Copy)]
pub struct CommandLauncher;

impl Launcher for CommandLauncher {
    fn launch(&self, engine: &EngineConfig, log_path: &Path) -> Result<tokio::process::Child> {
        let open_log = || {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path)
        };
        let start_failed = |cause: std::io::Error| {
            AtelierError::new(ErrorCode::EngineStartFailed, cause.to_string())
        };
        let stdout = open_log().map_err(start_failed)?;
        let stderr = open_log().map_err(start_failed)?;
        tokio::process::Command::new(&engine.program)
            .args(&engine.args)
            .current_dir(&engine.working_directory)
            .env("CUDA_DEVICE_ORDER", &engine.cuda_device_order)
            .env("CUDA_VISIBLE_DEVICES", &engine.cuda_visible_devices)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true)
            .spawn()
            .map_err(start_failed)
    }
}

/// Evidence of one engine session, for status and manifests.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SessionEvidence {
    pub pid: Option<u32>,
    pub gpu_before_launch: Option<GpuSnapshot>,
    pub gpu_after_ready: Option<GpuSnapshot>,
    pub gpu_after_collection: Option<GpuSnapshot>,
    pub gpu_after_shutdown: Option<GpuSnapshot>,
    pub engine_device: Option<String>,
    pub engine_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CleanupResult {
    pub ok: bool,
    pub detail: String,
}

pub struct Lifecycle<L: Launcher, G: GpuProbe> {
    engine: EngineConfig,
    state_root: PathBuf,
    launcher: L,
    gpu: G,
    child: Option<tokio::process::Child>,
    lease: Option<std::fs::File>,
    degraded: Option<String>,
    pub evidence: SessionEvidence,
}

pub async fn port_listening(address: SocketAddr) -> bool {
    matches!(
        tokio::time::timeout(PORT_PROBE_TIMEOUT, tokio::net::TcpStream::connect(address)).await,
        Ok(Ok(_))
    )
}

impl<L: Launcher, G: GpuProbe> Lifecycle<L, G> {
    pub fn new(engine: EngineConfig, state_root: PathBuf, launcher: L, gpu: G) -> Self {
        Lifecycle {
            engine,
            state_root,
            launcher,
            gpu,
            child: None,
            lease: None,
            degraded: None,
            evidence: SessionEvidence::default(),
        }
    }

    fn address(&self) -> SocketAddr {
        SocketAddr::new(self.engine.host, self.engine.port)
    }

    pub fn owns_engine(&self) -> bool {
        self.child.is_some()
    }

    pub fn degraded_reason(&self) -> Option<&str> {
        self.degraded.as_deref()
    }

    /// Observe state without starting anything.
    pub async fn state(&self) -> EngineState {
        if self.degraded.is_some() {
            return EngineState::Degraded;
        }
        if self.child.is_some() {
            return EngineState::OwnedRunning;
        }
        if port_listening(self.address()).await {
            return EngineState::UnownedPresent;
        }
        EngineState::Stopped
    }

    pub async fn gpu_snapshot(&self) -> Option<GpuSnapshot> {
        self.gpu.snapshot(self.engine.gpu_index).await.ok()
    }

    fn acquire_lease(&mut self) -> Result<()> {
        let path = self.state_root.join("engine.lease");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|cause| AtelierError::new(ErrorCode::LeaseHeld, cause.to_string()))?;
        match file.try_lock() {
            Ok(()) => {
                self.lease = Some(file);
                Ok(())
            }
            Err(_) => err(
                ErrorCode::LeaseHeld,
                "another atelier instance holds the host-wide engine lease",
            ),
        }
    }

    /// Start an owned engine and wait for readiness. On any failure after
    /// spawn, the child is stopped before the error returns.
    pub async fn start<B: MediaBackend>(&mut self, backend: &B) -> Result<()> {
        assert!(self.child.is_none(), "start called while owning an engine");
        if let Some(reason) = &self.degraded {
            return err(
                ErrorCode::CleanupFailed,
                format!("lifecycle degraded; refusing to start: {reason}"),
            );
        }
        if port_listening(self.address()).await {
            return err(
                ErrorCode::EngineUnownedPresent,
                format!("an engine atelier did not start is on {}", self.address()),
            );
        }
        let gpu = self.gpu.snapshot(self.engine.gpu_index).await?;
        if !gpu.name.contains(&self.engine.expected_gpu_name) {
            return err(
                ErrorCode::GpuMismatch,
                format!(
                    "GPU {} is `{}`, expected {}",
                    gpu.index, gpu.name, self.engine.expected_gpu_name
                ),
            );
        }
        if gpu.memory_free_mib < self.engine.vram_free_mib_min {
            return err(
                ErrorCode::VramInsufficient,
                format!(
                    "{} MiB free < {} MiB required; not launching",
                    gpu.memory_free_mib, self.engine.vram_free_mib_min
                ),
            );
        }
        self.acquire_lease()?;
        self.evidence = SessionEvidence {
            gpu_before_launch: Some(gpu),
            ..SessionEvidence::default()
        };
        let log_path = self.state_root.join("engine.log");
        let child = match self.launcher.launch(&self.engine, &log_path) {
            Ok(child) => child,
            Err(error) => {
                self.lease = None;
                return Err(error);
            }
        };
        self.evidence.pid = child.id();
        self.child = Some(child);
        if let Err(error) = self.await_ready(backend).await {
            let cleanup = self.stop().await;
            return Err(with_cleanup(error, &cleanup));
        }
        self.evidence.gpu_after_ready = self.gpu_snapshot().await;
        Ok(())
    }

    async fn await_ready<B: MediaBackend>(&mut self, backend: &B) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(self.engine.startup_timeout_s);
        loop {
            self.check_alive()?;
            if let Ok(info) = backend.system_info().await {
                let device = info.devices.first().map(|device| device.name.clone());
                self.evidence.engine_version = info.engine_version.clone();
                self.evidence.engine_device = device.clone();
                let matches = device
                    .as_deref()
                    .is_some_and(|name| name.contains(&self.engine.expected_gpu_name));
                if !matches {
                    return err(
                        ErrorCode::GpuMismatch,
                        format!(
                            "engine device is {device:?}, expected {}",
                            self.engine.expected_gpu_name
                        ),
                    );
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                return err(
                    ErrorCode::EngineNotReady,
                    format!(
                        "engine not ready within {} s",
                        self.engine.startup_timeout_s
                    ),
                );
            }
            tokio::time::sleep(READY_POLL).await;
        }
    }

    /// `engine_exited` if the owned child has exited.
    pub fn check_alive(&mut self) -> Result<()> {
        let Some(child) = self.child.as_mut() else {
            return err(ErrorCode::EngineExited, "no owned engine");
        };
        match child.try_wait() {
            Ok(None) => Ok(()),
            Ok(Some(status)) => err(ErrorCode::EngineExited, format!("engine exited: {status}")),
            Err(cause) => err(ErrorCode::EngineExited, cause.to_string()),
        }
    }

    /// Stop the owned child (if any), wait, and verify. Never touches an
    /// engine atelier did not start.
    pub async fn stop(&mut self) -> CleanupResult {
        let Some(mut child) = self.child.take() else {
            return CleanupResult {
                ok: self.degraded.is_none(),
                detail: "no owned engine".into(),
            };
        };
        let mut problems = Vec::new();
        if let Ok(None) = child.try_wait()
            && let Err(cause) = child.start_kill()
        {
            problems.push(format!("kill: {cause}"));
        }
        match tokio::time::timeout(STOP_WAIT, child.wait()).await {
            Ok(Ok(_)) => {}
            Ok(Err(cause)) => problems.push(format!("wait: {cause}")),
            Err(_) => problems.push("child did not exit within the stop wait".into()),
        }
        let port_deadline = Instant::now() + PORT_RELEASE_WAIT;
        while port_listening(self.address()).await {
            if Instant::now() >= port_deadline {
                problems.push(format!(
                    "port {} still listening after stop",
                    self.engine.port
                ));
                break;
            }
            tokio::time::sleep(READY_POLL).await;
        }
        self.lease = None;
        self.evidence.gpu_after_shutdown = self.gpu_snapshot().await;
        if problems.is_empty() {
            self.degraded = None;
            return CleanupResult {
                ok: true,
                detail: format!("owned engine pid {:?} exited; port free", self.evidence.pid),
            };
        }
        let detail = problems.join("; ");
        self.degraded = Some(detail.clone());
        CleanupResult { ok: false, detail }
    }
}

/// Fold a cleanup failure into a job error so neither is lost.
pub fn with_cleanup(error: AtelierError, cleanup: &CleanupResult) -> AtelierError {
    if cleanup.ok {
        return error;
    }
    AtelierError::new(
        error.code,
        format!("{}; cleanup also failed: {}", error.message, cleanup.detail),
    )
}

/// Bytes to MiB, for evidence.
pub fn mib(bytes: u64) -> u64 {
    bytes / MIB
}
