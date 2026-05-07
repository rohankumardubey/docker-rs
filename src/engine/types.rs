use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default)]
pub struct DockerImageInfo {
    pub repository: String,
    pub tag: String,
    pub image_id: String,
    pub size: String,
    pub source: String,
}

#[derive(Clone, Debug, Default)]
pub struct EngineStatusInfo {
    pub summary: String,
    pub detail: String,
    pub store_path: String,
}

#[derive(Clone, Debug, Default)]
pub struct RuntimeStatusInfo {
    pub summary: String,
    pub detail: String,
    pub native_ready: bool,
    pub bridge_ready: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub ports: String,
    pub runtime: String,
}

#[derive(Clone, Debug, Default)]
pub struct ProjectInfo {
    pub name: String,
    pub status: String,
    pub config_files: String,
    pub working_dir: String,
}

#[derive(Clone, Debug, Default)]
pub struct VolumeInfo {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
    pub scope: String,
}

#[derive(Clone, Debug, Default)]
pub struct VolumeDetailsInfo {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
    pub scope: String,
    pub created_at: String,
    pub labels: Vec<(String, String)>,
    pub options: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default)]
pub struct NetworkInfo {
    pub name: String,
    pub driver: String,
    pub subnet: String,
    pub gateway: String,
    pub scope: String,
}

#[derive(Clone, Debug, Default)]
pub struct NetworkDetailsInfo {
    pub name: String,
    pub driver: String,
    pub subnet: String,
    pub gateway: String,
    pub scope: String,
    pub created_at: String,
    pub labels: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default)]
pub struct ContainerDetailsInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    pub command: String,
    pub entrypoint: String,
    pub created: String,
    pub status: String,
    pub ports: String,
    pub ip_address: String,
    pub networks: String,
    pub working_dir: String,
    pub user: String,
    pub restart_policy: String,
    pub runtime: String,
    pub env: Vec<String>,
    pub labels: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default)]
pub struct ContainerStatsInfo {
    pub container_id: String,
    pub cpu_percent: String,
    pub memory_usage: String,
    pub memory_percent: String,
    pub net_io: String,
    pub block_io: String,
    pub pids: String,
}

#[derive(Clone, Debug, Default)]
pub struct ImageDetailsInfo {
    pub reference: String,
    pub image_id: String,
    pub manifest_digest: String,
    pub config_digest: String,
    pub size: String,
    pub source: String,
    pub architecture: String,
    pub os: String,
    pub created: String,
    pub layer_count: usize,
    pub env: Vec<String>,
    pub labels: Vec<(String, String)>,
    pub exposed_ports: Vec<String>,
    pub user: String,
    pub working_dir: String,
    pub command: String,
    pub entrypoint: String,
}

#[derive(Debug)]
pub enum WorkerEvent {
    EngineStatus(Result<EngineStatusInfo, String>),
    RuntimeStatus(Result<RuntimeStatusInfo, String>),
    ImageList(Result<Vec<DockerImageInfo>, String>),
    ProjectList(Result<Vec<ProjectInfo>, String>),
    VolumeList(Result<Vec<VolumeInfo>, String>),
    VolumeDetails(Result<VolumeDetailsInfo, String>),
    NetworkList(Result<Vec<NetworkInfo>, String>),
    NetworkDetails(Result<NetworkDetailsInfo, String>),
    ImageDetails(Result<ImageDetailsInfo, String>),
    ContainerList(Result<Vec<ContainerInfo>, String>),
    ContainerDetails(Result<ContainerDetailsInfo, String>),
    ContainerStats(Result<ContainerStatsInfo, String>),
    LogLine(String),
    ActionFinished(Result<String, String>),
}

#[derive(Debug)]
pub(super) struct EnginePaths {
    pub(super) root: PathBuf,
    pub(super) blobs: PathBuf,
    pub(super) manifests: PathBuf,
    pub(super) configs: PathBuf,
    pub(super) metadata: PathBuf,
    pub(super) volume_root: PathBuf,
    pub(super) volume_metadata: PathBuf,
    pub(super) network_root: PathBuf,
    pub(super) network_metadata: PathBuf,
    pub(super) runtime_metadata: PathBuf,
    pub(super) runtime_logs: PathBuf,
    pub(super) runtime_meta: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub(super) struct EngineState {
    pub(super) images: Vec<StoredImageRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct StoredImageRecord {
    pub(super) canonical_reference: String,
    pub(super) repository: String,
    pub(super) tag: String,
    pub(super) manifest_digest: String,
    pub(super) config_digest: String,
    pub(super) size_bytes: u64,
    pub(super) source: String,
    pub(super) architecture: String,
    pub(super) os: String,
    pub(super) created_at_epoch: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub(super) struct NativeRuntimeState {
    pub(super) containers: Vec<NativeContainerRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub(super) struct NativeVolumeState {
    pub(super) volumes: Vec<NativeVolumeRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct NativeVolumeRecord {
    pub(super) name: String,
    pub(super) driver: String,
    pub(super) mountpoint: String,
    pub(super) scope: String,
    pub(super) created_at: String,
    pub(super) labels: Vec<(String, String)>,
    pub(super) options: Vec<(String, String)>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub(super) struct NativeNetworkState {
    pub(super) networks: Vec<NativeNetworkRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct NativeNetworkRecord {
    pub(super) name: String,
    pub(super) driver: String,
    pub(super) subnet: String,
    pub(super) gateway: String,
    pub(super) scope: String,
    pub(super) created_at: String,
    pub(super) labels: Vec<(String, String)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct NativeContainerRecord {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) image: String,
    pub(super) state: String,
    pub(super) status: String,
    pub(super) ports: Vec<String>,
    pub(super) env: Vec<String>,
    pub(super) volumes: Vec<String>,
    #[serde(default)]
    pub(super) networks: Vec<String>,
    pub(super) command: String,
    pub(super) entrypoint: String,
    pub(super) working_dir: String,
    pub(super) user: String,
    pub(super) restart_policy: String,
    pub(super) auto_remove: bool,
    pub(super) pid: Option<u32>,
    pub(super) created_at_epoch: u64,
    pub(super) started_at_epoch: Option<u64>,
    pub(super) finished_at_epoch: Option<u64>,
    pub(super) last_exit_code: Option<i32>,
    pub(super) log_path: String,
    pub(super) exit_code_path: String,
}

#[derive(Deserialize)]
pub(super) struct DockerPortBinding {
    #[serde(rename = "HostIp")]
    pub(super) host_ip: Option<String>,
    #[serde(rename = "HostPort")]
    pub(super) host_port: Option<String>,
}
