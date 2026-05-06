use std::collections::HashMap;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use directories::ProjectDirs;
use dockerfile_parser::{Dockerfile, Instruction};
use oci_client::client::ClientConfig;
use oci_client::manifest::ImageIndexEntry;
use oci_client::{Client, Reference, secrets::RegistryAuth};
use oci_spec::image::{
    Arch, Config, DescriptorBuilder, Digest as OciDigest, HistoryBuilder, ImageConfiguration,
    ImageManifest, MediaType, Os,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::runtime::Builder;

mod tasks;
mod types;

pub use self::tasks::{
    build_image, check_engine_status, check_runtime_status, compose_project_down,
    compose_project_up, exec_in_container, fetch_container_logs, fetch_project_logs,
    follow_container_logs, inspect_container, inspect_container_stats, inspect_image, pull_image,
    refresh_containers, refresh_images, refresh_projects, remove_container, remove_image,
    restart_container, run_container, start_container, start_runtime, stop_container,
};
use self::types::*;
pub use self::types::{
    ContainerDetailsInfo, ContainerInfo, ContainerStatsInfo, DockerImageInfo, EngineStatusInfo,
    ImageDetailsInfo, ProjectInfo, RuntimeStatusInfo, WorkerEvent,
};

fn engine_status() -> Result<EngineStatusInfo> {
    let paths = ensure_engine_paths()?;
    Ok(EngineStatusInfo {
        summary: String::from("Native OCI engine ready"),
        detail: String::from(
            "Docker daemon is not required for pulls or metadata-only image builds.",
        ),
        store_path: paths.root.display().to_string(),
    })
}

fn list_images() -> Result<Vec<DockerImageInfo>> {
    let paths = ensure_engine_paths()?;
    let state = load_state(&paths)?;
    let mut images = state
        .images
        .into_iter()
        .map(|record| DockerImageInfo {
            repository: record.repository,
            tag: record.tag,
            image_id: shorten_digest(&record.config_digest),
            size: format_bytes(record.size_bytes),
            source: record.source,
        })
        .collect::<Vec<_>>();

    images.sort_by(|left, right| {
        left.repository
            .cmp(&right.repository)
            .then(left.tag.cmp(&right.tag))
    });
    Ok(images)
}

fn inspect_image_entry(image: &str) -> Result<ImageDetailsInfo> {
    let paths = ensure_engine_paths()?;
    let state = load_state(&paths)?;
    let record = state
        .images
        .into_iter()
        .find(|record| record.canonical_reference == image || display_record(record) == image)
        .ok_or_else(|| anyhow!("image `{image}` was not found in the native store"))?;

    let manifest = load_manifest(&paths, &record.manifest_digest)?;
    let config = load_config(&paths, &record.config_digest)?;
    let runtime = config.config().clone().unwrap_or_default();
    let mut labels = runtime
        .labels()
        .clone()
        .unwrap_or_default()
        .into_iter()
        .collect::<Vec<_>>();
    labels.sort_by(|left, right| left.0.cmp(&right.0));

    let mut exposed_ports = runtime.exposed_ports().clone().unwrap_or_default();
    exposed_ports.sort();

    Ok(ImageDetailsInfo {
        reference: display_record(&record),
        image_id: shorten_digest(&record.config_digest),
        manifest_digest: record.manifest_digest.clone(),
        config_digest: record.config_digest.clone(),
        size: format_bytes(record.size_bytes),
        source: record.source,
        architecture: record.architecture,
        os: record.os,
        created: record.created_at_epoch.to_string(),
        layer_count: manifest.layers().len(),
        env: runtime.env().clone().unwrap_or_default(),
        labels,
        exposed_ports,
        user: runtime.user().clone().unwrap_or_default(),
        working_dir: runtime.working_dir().clone().unwrap_or_default(),
        command: runtime.cmd().clone().unwrap_or_default().join(" "),
        entrypoint: runtime.entrypoint().clone().unwrap_or_default().join(" "),
    })
}

fn exec_in_container_entry(
    container_id: &str,
    command: &str,
    sender: &Sender<WorkerEvent>,
) -> Result<()> {
    if is_native_container_id(container_id) {
        return exec_in_native_container_entry(container_id, command, sender);
    }
    let args = vec![
        String::from("exec"),
        container_id.to_string(),
        String::from("sh"),
        String::from("-lc"),
        command.to_string(),
    ];
    let _ = sender.send(WorkerEvent::LogLine(format!(
        "$ {}",
        render_command("docker", &args)
    )));

    let output = Command::new("docker")
        .args(&args)
        .output()
        .context("unable to execute command in container")?;

    emit_command_output(sender, &output);

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!(
            "{}",
            if stderr.is_empty() {
                String::from("docker exec failed")
            } else {
                stderr
            }
        );
    }
}

fn inspect_container_stats_entry(container_id: &str) -> Result<ContainerStatsInfo> {
    if is_native_container_id(container_id) {
        return inspect_native_container_stats_entry(container_id);
    }
    let output = Command::new("docker")
        .args([
            "stats",
            "--no-stream",
            "--format",
            "{{json .}}",
            container_id,
        ])
        .output()
        .context("unable to fetch container stats")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!(
            "{}",
            if stderr.is_empty() {
                String::from("docker stats failed")
            } else {
                stderr
            }
        );
    }

    #[derive(Deserialize)]
    struct DockerStatsRow {
        #[serde(rename = "Container")]
        container: Option<String>,
        #[serde(rename = "CPUPerc")]
        cpu_percent: Option<String>,
        #[serde(rename = "MemUsage")]
        memory_usage: Option<String>,
        #[serde(rename = "MemPerc")]
        memory_percent: Option<String>,
        #[serde(rename = "NetIO")]
        net_io: Option<String>,
        #[serde(rename = "BlockIO")]
        block_io: Option<String>,
        #[serde(rename = "PIDs")]
        pids: Option<String>,
    }

    let line = String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| anyhow!("docker stats returned no rows"))?
        .to_string();
    let parsed: DockerStatsRow =
        serde_json::from_str(&line).context("unable to parse docker stats output")?;

    Ok(ContainerStatsInfo {
        container_id: parsed.container.unwrap_or_else(|| container_id.to_string()),
        cpu_percent: parsed.cpu_percent.unwrap_or_else(|| String::from("-")),
        memory_usage: parsed.memory_usage.unwrap_or_else(|| String::from("-")),
        memory_percent: parsed.memory_percent.unwrap_or_else(|| String::from("-")),
        net_io: parsed.net_io.unwrap_or_else(|| String::from("-")),
        block_io: parsed.block_io.unwrap_or_else(|| String::from("-")),
        pids: parsed.pids.unwrap_or_else(|| String::from("-")),
    })
}

fn runtime_status() -> Result<RuntimeStatusInfo> {
    let paths = ensure_engine_paths()?;
    let _ = refresh_native_runtime_state(&paths)?;
    let bridge = docker_bridge_version().ok();
    let summary = match bridge.as_deref() {
        Some(version) if !version.is_empty() => {
            format!("Native runtime prototype ready; Docker bridge ready ({version})")
        }
        _ => String::from("Native runtime prototype ready"),
    };
    let detail = if bridge.is_some() {
        String::from(
            "Runs host-process containers natively first, with Docker Desktop fallback for Linux-container compatibility on macOS.",
        )
    } else {
        String::from(
            "Runs host-process containers natively. Docker Desktop fallback is currently unavailable, so Linux image commands may not execute on macOS yet.",
        )
    };
    Ok(RuntimeStatusInfo {
        summary,
        detail,
        native_ready: true,
        bridge_ready: bridge.is_some(),
    })
}

fn list_projects() -> Result<Vec<ProjectInfo>> {
    let output = Command::new("docker")
        .args(["compose", "ls", "--format", "json"])
        .output()
        .context("unable to query compose projects")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!(
            "{}",
            if stderr.is_empty() {
                String::from("Docker Compose project listing failed.")
            } else {
                stderr
            }
        );
    }

    #[derive(Deserialize)]
    struct DockerComposeLsRow {
        #[serde(rename = "Name", alias = "name")]
        name: String,
        #[serde(rename = "Status", alias = "status")]
        status: Option<String>,
        #[serde(rename = "ConfigFiles", alias = "config_files")]
        config_files: Option<String>,
    }

    let mut projects = parse_json_list::<DockerComposeLsRow>(&output.stdout)?
        .into_iter()
        .map(|row| {
            let config_files = row.config_files.unwrap_or_default();
            let target = primary_compose_target(&config_files);
            ProjectInfo {
                name: row.name,
                status: row.status.unwrap_or_else(|| String::from("Unknown")),
                working_dir: compose_target_working_dir(&target),
                config_files,
            }
        })
        .collect::<Vec<_>>();

    projects.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(projects)
}

fn list_containers() -> Result<Vec<ContainerInfo>> {
    let paths = ensure_engine_paths()?;
    let native = list_native_containers(&paths)?;
    let docker = list_docker_containers().unwrap_or_default();
    let mut containers = native;
    containers.extend(docker);
    containers.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.runtime.cmp(&right.runtime))
    });
    Ok(containers)
}

fn list_docker_containers() -> Result<Vec<ContainerInfo>> {
    let output = Command::new("docker")
        .args(["ps", "-a", "--format", "{{json .}}"])
        .output()
        .context("unable to query runtime containers")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!(
            "{}",
            if stderr.is_empty() {
                String::from("Docker runtime container listing failed.")
            } else {
                stderr
            }
        );
    }

    #[derive(Deserialize)]
    struct DockerPsRow {
        #[serde(rename = "ID")]
        id: String,
        #[serde(rename = "Image")]
        image: String,
        #[serde(rename = "Names")]
        names: String,
        #[serde(rename = "State")]
        state: String,
        #[serde(rename = "Status")]
        status: String,
        #[serde(rename = "Ports")]
        ports: String,
    }

    let mut containers = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.trim().is_empty() {
            continue;
        }

        let parsed: DockerPsRow = serde_json::from_str(line)
            .with_context(|| format!("unable to parse docker ps row: {line}"))?;
        containers.push(ContainerInfo {
            id: parsed.id,
            name: parsed.names,
            image: parsed.image,
            state: parsed.state,
            status: parsed.status,
            ports: if parsed.ports.trim().is_empty() {
                String::from("-")
            } else {
                parsed.ports
            },
            runtime: String::from("docker"),
        });
    }

    Ok(containers)
}

fn inspect_container_entry(container_id: &str) -> Result<ContainerDetailsInfo> {
    if is_native_container_id(container_id) {
        return inspect_native_container_entry(container_id);
    }
    let output = Command::new("docker")
        .args(["inspect", container_id])
        .output()
        .context("unable to inspect container")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!(
            "{}",
            if stderr.is_empty() {
                String::from("docker inspect failed")
            } else {
                stderr
            }
        );
    }

    #[derive(Deserialize)]
    struct DockerInspectState {
        #[serde(rename = "Status")]
        status: Option<String>,
    }

    #[derive(Deserialize)]
    struct DockerInspectConfig {
        #[serde(rename = "Image")]
        image: Option<String>,
        #[serde(rename = "Cmd")]
        cmd: Option<Vec<String>>,
        #[serde(rename = "Entrypoint")]
        entrypoint: Option<Vec<String>>,
        #[serde(rename = "Env")]
        env: Option<Vec<String>>,
        #[serde(rename = "WorkingDir")]
        working_dir: Option<String>,
        #[serde(rename = "User")]
        user: Option<String>,
        #[serde(rename = "Labels")]
        labels: Option<HashMap<String, String>>,
    }

    #[derive(Deserialize)]
    struct DockerInspectHostConfig {
        #[serde(rename = "RestartPolicy")]
        restart_policy: Option<DockerRestartPolicy>,
    }

    #[derive(Deserialize)]
    struct DockerRestartPolicy {
        #[serde(rename = "Name")]
        name: Option<String>,
    }

    #[derive(Deserialize)]
    struct DockerInspectNetworkSettings {
        #[serde(rename = "IPAddress")]
        ip_address: Option<String>,
        #[serde(rename = "Ports")]
        ports: Option<HashMap<String, Option<Vec<DockerPortBinding>>>>,
    }

    #[derive(Deserialize)]
    struct DockerInspectRow {
        #[serde(rename = "Id")]
        id: String,
        #[serde(rename = "Name")]
        name: String,
        #[serde(rename = "Created")]
        created: Option<String>,
        #[serde(rename = "Path")]
        path: Option<String>,
        #[serde(rename = "Args")]
        args: Option<Vec<String>>,
        #[serde(rename = "State")]
        state: Option<DockerInspectState>,
        #[serde(rename = "Config")]
        config: Option<DockerInspectConfig>,
        #[serde(rename = "HostConfig")]
        host_config: Option<DockerInspectHostConfig>,
        #[serde(rename = "NetworkSettings")]
        network_settings: Option<DockerInspectNetworkSettings>,
    }

    let mut rows: Vec<DockerInspectRow> =
        serde_json::from_slice(&output.stdout).context("unable to parse docker inspect output")?;
    let row = rows
        .pop()
        .ok_or_else(|| anyhow!("docker inspect returned no container details"))?;

    let config = row.config;
    let image = config
        .as_ref()
        .and_then(|item| item.image.clone())
        .unwrap_or_default();
    let command = format_command(
        row.path.as_deref(),
        row.args.as_ref().map(|items| items.as_slice()),
        config.as_ref().and_then(|item| item.cmd.as_deref()),
    );
    let entrypoint = config
        .as_ref()
        .and_then(|item| item.entrypoint.as_ref())
        .map(|items| items.join(" "))
        .unwrap_or_default();
    let env = config
        .as_ref()
        .and_then(|item| item.env.clone())
        .unwrap_or_default();
    let mut labels = config
        .as_ref()
        .and_then(|item| item.labels.clone())
        .unwrap_or_default()
        .into_iter()
        .collect::<Vec<_>>();
    labels.sort_by(|left, right| left.0.cmp(&right.0));

    let ports = row
        .network_settings
        .as_ref()
        .and_then(|item| item.ports.as_ref())
        .map(format_inspect_ports)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| String::from("-"));

    Ok(ContainerDetailsInfo {
        id: row.id,
        name: row.name.trim_start_matches('/').to_string(),
        image,
        command,
        entrypoint,
        created: row.created.unwrap_or_default(),
        status: row
            .state
            .and_then(|item| item.status)
            .unwrap_or_else(|| String::from("unknown")),
        ports,
        ip_address: row
            .network_settings
            .as_ref()
            .and_then(|item| item.ip_address.clone())
            .unwrap_or_default(),
        working_dir: config
            .as_ref()
            .and_then(|item| item.working_dir.clone())
            .unwrap_or_default(),
        user: config
            .as_ref()
            .and_then(|item| item.user.clone())
            .unwrap_or_default(),
        restart_policy: row
            .host_config
            .and_then(|item| item.restart_policy)
            .and_then(|item| item.name)
            .unwrap_or_else(|| String::from("no")),
        runtime: String::from("docker"),
        env,
        labels,
    })
}

fn format_command(path: Option<&str>, args: Option<&[String]>, cmd: Option<&[String]>) -> String {
    let from_path = path
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            let mut parts = vec![value.to_string()];
            if let Some(items) = args {
                parts.extend(
                    items
                        .iter()
                        .map(String::as_str)
                        .map(str::trim)
                        .filter(|item| !item.is_empty())
                        .map(ToOwned::to_owned),
                );
            }
            parts.join(" ")
        });

    let from_cmd = cmd.map(|items| {
        items
            .iter()
            .map(String::as_str)
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>()
            .join(" ")
    });

    from_path
        .or(from_cmd)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| String::from("-"))
}

fn format_inspect_ports(ports: &HashMap<String, Option<Vec<DockerPortBinding>>>) -> String {
    let mut rendered = ports
        .iter()
        .flat_map(|(container_port, bindings)| match bindings {
            Some(bindings) if !bindings.is_empty() => bindings
                .iter()
                .map(|binding| {
                    let host_ip = binding
                        .host_ip
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("0.0.0.0");
                    let host_port = binding
                        .host_port
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("?");
                    format!("{host_ip}:{host_port}->{container_port}")
                })
                .collect::<Vec<_>>(),
            _ => vec![container_port.to_string()],
        })
        .collect::<Vec<_>>();
    rendered.sort();
    rendered.join(", ")
}

fn run_container_entry(
    image: &str,
    container_name: Option<&str>,
    ports: Option<&str>,
    env_vars: Option<&str>,
    volume_mounts: Option<&str>,
    command_override: Option<&str>,
    restart_policy: Option<&str>,
    auto_remove: bool,
    sender: &Sender<WorkerEvent>,
) -> Result<String> {
    if image.trim().is_empty() {
        bail!("image reference is required");
    }
    if auto_remove && restart_policy.is_some() {
        bail!("auto-remove (`--rm`) cannot be combined with a restart policy");
    }

    let native_result = run_native_container_entry(
        image,
        container_name,
        ports,
        env_vars,
        volume_mounts,
        command_override,
        restart_policy,
        auto_remove,
        sender,
    );
    match native_result {
        Ok(container_id) => return Ok(container_id),
        Err(err) => {
            let _ = sender.send(WorkerEvent::LogLine(format!(
                "Native runtime attempt for `{image}` did not stay up cleanly: {err}"
            )));
        }
    }

    let _ = runtime_status()?;
    ensure_runtime_image(image, sender)?;

    let mut args = vec![String::from("run"), String::from("-d")];
    if auto_remove {
        args.push(String::from("--rm"));
    }
    if let Some(name) = container_name {
        args.push(String::from("--name"));
        args.push(name.to_string());
    }
    if let Some(policy) = restart_policy {
        args.push(String::from("--restart"));
        args.push(policy.to_string());
    }

    for mapping in parse_port_mappings(ports) {
        args.push(String::from("-p"));
        args.push(mapping);
    }

    for env_var in parse_env_assignments(env_vars) {
        args.push(String::from("-e"));
        args.push(env_var);
    }

    for mount in parse_volume_mounts(volume_mounts) {
        args.push(String::from("-v"));
        args.push(mount);
    }

    args.push(image.to_string());
    if let Some(command_override) = command_override {
        args.push(String::from("sh"));
        args.push(String::from("-lc"));
        args.push(command_override.to_string());
    }
    let _ = sender.send(WorkerEvent::LogLine(format!(
        "$ {}",
        render_command("docker", &args)
    )));

    let output = Command::new("docker")
        .args(&args)
        .output()
        .context("unable to start docker run command")?;

    emit_command_output(sender, &output);
    let container_id = String::from_utf8_lossy(&output.stdout).trim().to_owned();

    if !output.status.success() {
        if looks_like_container_id(&container_id) {
            cleanup_failed_container(&container_id, sender);
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!(
            "{}",
            if stderr.is_empty() {
                String::from("docker run failed")
            } else {
                stderr
            }
        );
    }

    if container_id.is_empty() {
        bail!("docker run returned no container id");
    }
    Ok(container_id)
}

fn fetch_container_logs_entry(container_id: &str, sender: &Sender<WorkerEvent>) -> Result<()> {
    if is_native_container_id(container_id) {
        return fetch_native_container_logs_entry(container_id, sender);
    }
    let args = ["logs", "--tail", "200", container_id];
    let _ = sender.send(WorkerEvent::LogLine(format!(
        "$ {}",
        render_command(
            "docker",
            &args
                .iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
        )
    )));

    let output = Command::new("docker")
        .args(args)
        .output()
        .context("unable to fetch container logs")?;

    emit_command_output(sender, &output);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!(
            "{}",
            if stderr.is_empty() {
                String::from("docker logs failed")
            } else {
                stderr
            }
        );
    }

    Ok(())
}

fn compose_up_entry(
    compose_target: &str,
    project_name: Option<&str>,
    sender: &Sender<WorkerEvent>,
) -> Result<String> {
    let mut args = compose_command_args(compose_target, project_name)?;
    args.extend([
        String::from("up"),
        String::from("-d"),
        String::from("--remove-orphans"),
    ]);
    let _ = sender.send(WorkerEvent::LogLine(format!(
        "$ {}",
        render_command("docker", &args)
    )));

    let output = Command::new("docker")
        .args(&args)
        .output()
        .context("unable to run docker compose up")?;

    emit_command_output(sender, &output);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!(
            "{}",
            if stderr.is_empty() {
                String::from("docker compose up failed")
            } else {
                stderr
            }
        );
    }

    Ok(compose_project_display_name(compose_target, project_name))
}

fn compose_down_entry(
    compose_target: &str,
    project_name: Option<&str>,
    sender: &Sender<WorkerEvent>,
) -> Result<String> {
    let mut args = compose_command_args(compose_target, project_name)?;
    args.extend([String::from("down"), String::from("--remove-orphans")]);
    let _ = sender.send(WorkerEvent::LogLine(format!(
        "$ {}",
        render_command("docker", &args)
    )));

    let output = Command::new("docker")
        .args(&args)
        .output()
        .context("unable to run docker compose down")?;

    emit_command_output(sender, &output);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!(
            "{}",
            if stderr.is_empty() {
                String::from("docker compose down failed")
            } else {
                stderr
            }
        );
    }

    Ok(compose_project_display_name(compose_target, project_name))
}

fn fetch_project_logs_entry(
    compose_target: &str,
    project_name: Option<&str>,
    sender: &Sender<WorkerEvent>,
) -> Result<String> {
    let mut args = compose_command_args(compose_target, project_name)?;
    args.extend([
        String::from("logs"),
        String::from("--tail"),
        String::from("200"),
        String::from("--no-color"),
    ]);
    let _ = sender.send(WorkerEvent::LogLine(format!(
        "$ {}",
        render_command("docker", &args)
    )));

    let output = Command::new("docker")
        .args(&args)
        .output()
        .context("unable to fetch docker compose logs")?;

    emit_command_output(sender, &output);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!(
            "{}",
            if stderr.is_empty() {
                String::from("docker compose logs failed")
            } else {
                stderr
            }
        );
    }

    Ok(compose_project_display_name(compose_target, project_name))
}

fn follow_container_logs_entry(
    container_id: &str,
    stop_flag: &Arc<AtomicBool>,
    sender: &Sender<WorkerEvent>,
) -> Result<()> {
    if is_native_container_id(container_id) {
        return follow_native_container_logs_entry(container_id, stop_flag, sender);
    }
    let _ = sender.send(WorkerEvent::LogLine(format!(
        "Following live logs for {}.",
        shorten_container_id(container_id)
    )));

    let mut previous_lines = Vec::<String>::new();
    while !stop_flag.load(Ordering::Relaxed) {
        match fetch_container_logs_lines(container_id) {
            Ok(lines) => {
                if previous_lines.is_empty() {
                    for line in &lines {
                        let _ = sender.send(WorkerEvent::LogLine(line.clone()));
                    }
                } else if lines != previous_lines {
                    let shared_prefix = previous_lines
                        .iter()
                        .zip(lines.iter())
                        .take_while(|(left, right)| left == right)
                        .count();
                    for line in lines.iter().skip(shared_prefix) {
                        let _ = sender.send(WorkerEvent::LogLine(line.clone()));
                    }
                }
                previous_lines = lines;
            }
            Err(err) => {
                let _ = sender.send(WorkerEvent::LogLine(format!(
                    "Live log polling failed for {}: {err}",
                    shorten_container_id(container_id)
                )));
                break;
            }
        }

        for _ in 0..10 {
            if stop_flag.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(300));
        }
    }

    let _ = sender.send(WorkerEvent::LogLine(format!(
        "Live log stream ended for {}.",
        shorten_container_id(container_id)
    )));
    Ok(())
}

fn fetch_container_logs_lines(container_id: &str) -> Result<Vec<String>> {
    let output = Command::new("docker")
        .args(["logs", "--tail", "200", container_id])
        .output()
        .context("unable to fetch live container logs")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!(
            "{}",
            if stderr.is_empty() {
                String::from("docker logs failed")
            } else {
                stderr
            }
        );
    }

    let mut lines = Vec::new();
    for line in BufReader::new(output.stdout.as_slice()).lines() {
        let line = line.context("unable to decode container stdout log line")?;
        if !line.trim().is_empty() {
            lines.push(line);
        }
    }
    for line in BufReader::new(output.stderr.as_slice()).lines() {
        let line = line.context("unable to decode container stderr log line")?;
        if !line.trim().is_empty() {
            lines.push(line);
        }
    }
    Ok(lines)
}

fn runtime_simple_action(
    sender: &Sender<WorkerEvent>,
    program: &str,
    args: &[&str],
    success_message: &str,
) -> Result<String> {
    let rendered = render_command(
        program,
        &args
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>(),
    );
    let _ = sender.send(WorkerEvent::LogLine(format!("$ {rendered}")));

    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("unable to launch `{program}`"))?;

    emit_command_output(sender, &output);

    if output.status.success() {
        Ok(success_message.to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!(
            "{}",
            if stderr.is_empty() {
                format!("`{program}` failed")
            } else {
                stderr
            }
        );
    }
}

fn remove_image_entry(image: &str) -> Result<StoredImageRecord> {
    let paths = ensure_engine_paths()?;
    let mut state = load_state(&paths)?;
    let remove_index = state
        .images
        .iter()
        .position(|record| {
            record.canonical_reference == image
                || format!("{}:{}", record.repository, record.tag) == image
        })
        .ok_or_else(|| anyhow!("image `{image}` was not found in the native store"))?;

    let removed = state.images.remove(remove_index);
    save_state(&paths, &state)?;
    garbage_collect_store(&paths, &state, &removed)?;
    Ok(removed)
}

fn garbage_collect_store(
    paths: &EnginePaths,
    state: &EngineState,
    removed: &StoredImageRecord,
) -> Result<()> {
    let mut referenced_manifests = state
        .images
        .iter()
        .map(|record| record.manifest_digest.clone())
        .collect::<std::collections::HashSet<_>>();
    let referenced_configs = state
        .images
        .iter()
        .map(|record| record.config_digest.clone())
        .collect::<std::collections::HashSet<_>>();
    let mut referenced_layers = std::collections::HashSet::new();

    for record in &state.images {
        let manifest = load_manifest(paths, &record.manifest_digest)?;
        for layer in manifest.layers() {
            referenced_layers.insert(layer.digest().to_string());
        }
    }

    if !referenced_manifests.contains(&removed.manifest_digest) {
        let manifest = load_manifest(paths, &removed.manifest_digest)?;
        for layer in manifest.layers() {
            let digest = layer.digest().to_string();
            if !referenced_layers.contains(&digest) {
                remove_digest_file(&paths.blobs, &digest)?;
            }
        }
        remove_digest_file(&paths.manifests, &removed.manifest_digest)?;
        referenced_manifests.insert(removed.manifest_digest.clone());
    }

    if !referenced_configs.contains(&removed.config_digest) {
        remove_digest_file(&paths.configs, &removed.config_digest)?;
    }

    Ok(())
}

fn remove_digest_file(base: &Path, digest: &str) -> Result<()> {
    let path = if base.ends_with("blobs") {
        digest_blob_path(base, digest)?
    } else {
        digest_json_path(base, digest)?
    };

    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("unable to remove {}", path.display())),
    }
}

fn ensure_runtime_image(image: &str, sender: &Sender<WorkerEvent>) -> Result<()> {
    let inspect = Command::new("docker")
        .args(["image", "inspect", image])
        .output()
        .context("unable to inspect runtime image cache")?;

    if inspect.status.success() {
        let _ = sender.send(WorkerEvent::LogLine(format!(
            "Runtime bridge already has image `{image}`."
        )));
        return Ok(());
    }

    let _ = sender.send(WorkerEvent::LogLine(format!(
        "Runtime bridge does not have `{image}` yet. Pulling it into Docker runtime...",
    )));
    let output = Command::new("docker")
        .args(["pull", image])
        .output()
        .context("unable to start docker pull for runtime bridge")?;

    emit_command_output(sender, &output);

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!(
            "{}",
            if stderr.is_empty() {
                String::from("runtime image pull failed")
            } else {
                stderr
            }
        );
    }
}

fn emit_command_output(sender: &Sender<WorkerEvent>, output: &std::process::Output) {
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if !line.trim().is_empty() {
            let _ = sender.send(WorkerEvent::LogLine(line.to_string()));
        }
    }

    for line in String::from_utf8_lossy(&output.stderr).lines() {
        if !line.trim().is_empty() {
            let _ = sender.send(WorkerEvent::LogLine(line.to_string()));
        }
    }
}

fn cleanup_failed_container(container_id: &str, sender: &Sender<WorkerEvent>) {
    let _ = sender.send(WorkerEvent::LogLine(format!(
        "Cleaning up failed container {}...",
        shorten_container_id(container_id)
    )));

    match Command::new("docker")
        .args(["rm", "-f", container_id])
        .output()
    {
        Ok(output) => emit_command_output(sender, &output),
        Err(err) => {
            let _ = sender.send(WorkerEvent::LogLine(format!(
                "Cleanup failed for {}: {err}",
                shorten_container_id(container_id)
            )));
        }
    }
}

fn is_native_container_id(container_id: &str) -> bool {
    container_id.starts_with("native-")
}

fn docker_bridge_version() -> Result<String> {
    let output = Command::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .output()
        .context("unable to start docker cli for runtime check")?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!(
            "{}",
            if stderr.is_empty() {
                String::from("Docker runtime bridge is unavailable.")
            } else {
                stderr
            }
        );
    }
}

fn run_native_container_entry(
    image: &str,
    container_name: Option<&str>,
    ports: Option<&str>,
    env_vars: Option<&str>,
    volume_mounts: Option<&str>,
    command_override: Option<&str>,
    restart_policy: Option<&str>,
    auto_remove: bool,
    sender: &Sender<WorkerEvent>,
) -> Result<String> {
    let paths = ensure_engine_paths()?;
    let mut state = refresh_native_runtime_state(&paths)?;
    let image_spec = resolve_native_image_spec(&paths, image, command_override)?;

    if ports.is_some() {
        let _ = sender.send(WorkerEvent::LogLine(String::from(
            "Native runtime prototype does not virtualize ports yet; published ports are informational and rely on the process binding the host port itself.",
        )));
    }
    if volume_mounts.is_some() {
        let _ = sender.send(WorkerEvent::LogLine(String::from(
            "Native runtime prototype records volume mounts for now but does not isolate or remap them yet.",
        )));
    }

    let id = format!("native-{:x}", now_nanos());
    let name = container_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            format!(
                "native-{}",
                &id["native-".len()..].chars().take(8).collect::<String>()
            )
        });

    let command = command_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| image_spec.command.clone())
        .ok_or_else(|| {
            anyhow!("native runtime needs a command override or an image CMD/ENTRYPOINT")
        })?;

    let log_path = paths.runtime_logs.join(format!("{id}.log"));
    let exit_code_path = paths.runtime_meta.join(format!("{id}.exit"));

    let mut env = image_spec.env.clone();
    env.extend(parse_env_assignments(env_vars));

    let mut record = NativeContainerRecord {
        id: id.clone(),
        name,
        image: image.to_string(),
        state: String::from("created"),
        status: String::from("Created (native)"),
        ports: parse_port_mappings(ports),
        env,
        volumes: parse_volume_mounts(volume_mounts),
        command,
        entrypoint: image_spec.entrypoint,
        working_dir: image_spec.working_dir,
        user: image_spec.user,
        restart_policy: restart_policy.unwrap_or("no").to_string(),
        auto_remove,
        pid: None,
        created_at_epoch: now_epoch_seconds(),
        started_at_epoch: None,
        finished_at_epoch: None,
        last_exit_code: None,
        log_path: log_path.display().to_string(),
        exit_code_path: exit_code_path.display().to_string(),
    };

    start_native_record(&mut record, sender)?;
    state.containers.retain(|item| item.id != record.id);
    state.containers.push(record.clone());
    save_runtime_state(&paths, &state)?;

    thread::sleep(Duration::from_millis(350));
    let refreshed = refresh_native_runtime_state(&paths)?;
    let persisted = refreshed
        .containers
        .iter()
        .find(|item| item.id == id)
        .cloned()
        .ok_or_else(|| anyhow!("native runtime lost container state immediately after launch"))?;
    if persisted.state.eq_ignore_ascii_case("exited") && persisted.last_exit_code.unwrap_or(0) != 0
    {
        let reason = format!(
            "native process exited immediately with code {}. Check logs for `{}`.",
            persisted.last_exit_code.unwrap_or(-1),
            persisted.name
        );
        remove_native_container_record(&paths, &id, false)?;
        bail!("{reason}");
    }

    Ok(id)
}

fn stop_container_entry(container_id: &str, sender: &Sender<WorkerEvent>) -> Result<()> {
    if is_native_container_id(container_id) {
        return stop_native_container_entry(container_id, sender);
    }
    runtime_simple_action(
        sender,
        "docker",
        &["stop", container_id],
        &format!("Stopped container {}", shorten_container_id(container_id)),
    )?;
    Ok(())
}

fn start_container_entry(container_id: &str, sender: &Sender<WorkerEvent>) -> Result<()> {
    if is_native_container_id(container_id) {
        return start_native_container_entry(container_id, sender);
    }
    runtime_simple_action(
        sender,
        "docker",
        &["start", container_id],
        &format!("Started container {}", shorten_container_id(container_id)),
    )?;
    Ok(())
}

fn restart_container_entry(container_id: &str, sender: &Sender<WorkerEvent>) -> Result<()> {
    if is_native_container_id(container_id) {
        stop_native_container_entry(container_id, sender)?;
        return start_native_container_entry(container_id, sender);
    }
    runtime_simple_action(
        sender,
        "docker",
        &["restart", container_id],
        &format!("Restarted container {}", shorten_container_id(container_id)),
    )?;
    Ok(())
}

fn remove_container_entry(container_id: &str, sender: &Sender<WorkerEvent>) -> Result<()> {
    if is_native_container_id(container_id) {
        let paths = ensure_engine_paths()?;
        remove_native_container_record(&paths, container_id, true)?;
        let _ = sender.send(WorkerEvent::LogLine(format!(
            "Removed native container {}.",
            shorten_container_id(container_id)
        )));
        return Ok(());
    }
    runtime_simple_action(
        sender,
        "docker",
        &["rm", container_id],
        &format!("Removed container {}", shorten_container_id(container_id)),
    )?;
    Ok(())
}

fn list_native_containers(paths: &EnginePaths) -> Result<Vec<ContainerInfo>> {
    let state = refresh_native_runtime_state(paths)?;
    Ok(state
        .containers
        .into_iter()
        .map(|record| ContainerInfo {
            id: record.id,
            name: record.name,
            image: record.image,
            state: record.state,
            status: record.status,
            ports: render_native_ports(&record.ports),
            runtime: String::from("native"),
        })
        .collect())
}

fn inspect_native_container_entry(container_id: &str) -> Result<ContainerDetailsInfo> {
    let paths = ensure_engine_paths()?;
    let state = refresh_native_runtime_state(&paths)?;
    let record = state
        .containers
        .into_iter()
        .find(|item| item.id == container_id)
        .ok_or_else(|| anyhow!("native container `{container_id}` was not found"))?;

    Ok(ContainerDetailsInfo {
        id: record.id,
        name: record.name,
        image: record.image,
        command: record.command,
        entrypoint: record.entrypoint,
        created: record.created_at_epoch.to_string(),
        status: record.status,
        ports: render_native_ports(&record.ports),
        ip_address: String::from("host"),
        working_dir: record.working_dir,
        user: record.user,
        restart_policy: record.restart_policy,
        runtime: String::from("native"),
        env: record.env,
        labels: vec![
            (String::from("runtime"), String::from("native-prototype")),
            (
                String::from("ports"),
                String::from("metadata-only unless the host process binds them"),
            ),
        ],
    })
}

fn inspect_native_container_stats_entry(container_id: &str) -> Result<ContainerStatsInfo> {
    let paths = ensure_engine_paths()?;
    let state = refresh_native_runtime_state(&paths)?;
    let record = state
        .containers
        .into_iter()
        .find(|item| item.id == container_id)
        .ok_or_else(|| anyhow!("native container `{container_id}` was not found"))?;

    if let Some(pid) = record.pid.filter(|pid| pid_is_running(*pid)) {
        let output = Command::new("ps")
            .args(["-o", "%cpu=,rss=", "-p", &pid.to_string()])
            .output()
            .context("unable to query native runtime process stats")?;
        let line = String::from_utf8_lossy(&output.stdout)
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_string();
        let mut parts = line.split_whitespace();
        let cpu = parts.next().unwrap_or("-");
        let rss_kb = parts.next().unwrap_or("0").parse::<u64>().unwrap_or(0);
        return Ok(ContainerStatsInfo {
            container_id: container_id.to_string(),
            cpu_percent: if cpu == "-" {
                String::from("-")
            } else {
                format!("{cpu}%")
            },
            memory_usage: format_bytes(rss_kb.saturating_mul(1024)),
            memory_percent: String::from("-"),
            net_io: String::from("-"),
            block_io: String::from("-"),
            pids: String::from("1"),
        });
    }

    Ok(ContainerStatsInfo {
        container_id: container_id.to_string(),
        cpu_percent: String::from("0%"),
        memory_usage: String::from("0 B"),
        memory_percent: String::from("-"),
        net_io: String::from("-"),
        block_io: String::from("-"),
        pids: String::from("0"),
    })
}

fn fetch_native_container_logs_entry(
    container_id: &str,
    sender: &Sender<WorkerEvent>,
) -> Result<()> {
    let lines = fetch_native_container_logs_lines(container_id)?;
    for line in lines {
        let _ = sender.send(WorkerEvent::LogLine(line));
    }
    Ok(())
}

fn follow_native_container_logs_entry(
    container_id: &str,
    stop_flag: &Arc<AtomicBool>,
    sender: &Sender<WorkerEvent>,
) -> Result<()> {
    let _ = sender.send(WorkerEvent::LogLine(format!(
        "Following live logs for native container {}.",
        shorten_container_id(container_id)
    )));
    let mut previous_lines = Vec::<String>::new();
    while !stop_flag.load(Ordering::Relaxed) {
        match fetch_native_container_logs_lines(container_id) {
            Ok(lines) => {
                if previous_lines.is_empty() {
                    for line in &lines {
                        let _ = sender.send(WorkerEvent::LogLine(line.clone()));
                    }
                } else if lines != previous_lines {
                    let shared_prefix = previous_lines
                        .iter()
                        .zip(lines.iter())
                        .take_while(|(left, right)| left == right)
                        .count();
                    for line in lines.iter().skip(shared_prefix) {
                        let _ = sender.send(WorkerEvent::LogLine(line.clone()));
                    }
                }
                previous_lines = lines;
            }
            Err(err) => {
                let _ = sender.send(WorkerEvent::LogLine(format!(
                    "Live log polling failed for native container {}: {err}",
                    shorten_container_id(container_id)
                )));
                break;
            }
        }
        for _ in 0..10 {
            if stop_flag.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(300));
        }
    }
    let _ = sender.send(WorkerEvent::LogLine(format!(
        "Live log stream ended for native container {}.",
        shorten_container_id(container_id)
    )));
    Ok(())
}

fn exec_in_native_container_entry(
    container_id: &str,
    command: &str,
    sender: &Sender<WorkerEvent>,
) -> Result<()> {
    let paths = ensure_engine_paths()?;
    let state = refresh_native_runtime_state(&paths)?;
    let record = state
        .containers
        .into_iter()
        .find(|item| item.id == container_id)
        .ok_or_else(|| anyhow!("native container `{container_id}` was not found"))?;
    let _ = sender.send(WorkerEvent::LogLine(format!(
        "$ native-exec {} {}",
        shorten_container_id(container_id),
        command
    )));
    let output = Command::new("/bin/sh")
        .arg("-lc")
        .arg(command)
        .current_dir(native_working_dir(&record.working_dir)?)
        .envs(parse_env_pairs(&record.env))
        .output()
        .context("unable to execute command in native container")?;
    append_native_log(&record, &format!("$ {command}\n"))?;
    append_native_log(&record, &String::from_utf8_lossy(&output.stdout))?;
    append_native_log(&record, &String::from_utf8_lossy(&output.stderr))?;
    emit_command_output(sender, &output);
    if output.status.success() {
        Ok(())
    } else {
        bail!(
            "{}",
            String::from_utf8_lossy(&output.stderr).trim().to_string()
        )
    }
}

fn fetch_native_container_logs_lines(container_id: &str) -> Result<Vec<String>> {
    let paths = ensure_engine_paths()?;
    let state = refresh_native_runtime_state(&paths)?;
    let record = state
        .containers
        .into_iter()
        .find(|item| item.id == container_id)
        .ok_or_else(|| anyhow!("native container `{container_id}` was not found"))?;
    let text = fs::read_to_string(&record.log_path).unwrap_or_default();
    let mut lines = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if lines.len() > 200 {
        lines = lines.split_off(lines.len() - 200);
    }
    Ok(lines)
}

fn resolve_native_image_spec(
    paths: &EnginePaths,
    image: &str,
    command_override: Option<&str>,
) -> Result<NativeImageSpec> {
    let state = load_state(paths)?;
    let record = state
        .images
        .into_iter()
        .find(|record| record.canonical_reference == image || display_record(record) == image)
        .ok_or_else(|| anyhow!("image `{image}` is not available in the native OCI store yet"))?;
    let config = load_config(paths, &record.config_digest)?;
    let runtime = config.config().clone().unwrap_or_default();
    let entrypoint_items = runtime.entrypoint().clone().unwrap_or_default();
    let cmd_items = runtime.cmd().clone().unwrap_or_default();
    let mut command_items = Vec::new();
    command_items.extend(entrypoint_items.iter().cloned());
    command_items.extend(cmd_items.iter().cloned());
    let command = if command_override.is_some() {
        None
    } else if command_items.is_empty() {
        None
    } else {
        Some(command_items.join(" "))
    };
    Ok(NativeImageSpec {
        command,
        entrypoint: entrypoint_items.join(" "),
        env: runtime.env().clone().unwrap_or_default(),
        working_dir: runtime.working_dir().clone().unwrap_or_default(),
        user: runtime.user().clone().unwrap_or_default(),
    })
}

fn refresh_native_runtime_state(paths: &EnginePaths) -> Result<NativeRuntimeState> {
    let mut state = load_runtime_state(paths)?;
    for record in &mut state.containers {
        sync_native_record(record);
    }
    state
        .containers
        .retain(|record| !(record.auto_remove && record.state.eq_ignore_ascii_case("exited")));
    save_runtime_state(paths, &state)?;
    Ok(state)
}

fn sync_native_record(record: &mut NativeContainerRecord) {
    if let Some(pid) = record.pid {
        if pid_is_running(pid) {
            record.state = String::from("running");
            record.status = if let Some(started) = record.started_at_epoch {
                format!(
                    "Up {}s (native)",
                    now_epoch_seconds().saturating_sub(started)
                )
            } else {
                String::from("Running (native)")
            };
            return;
        }
    }

    if record.started_at_epoch.is_some() {
        record.state = String::from("exited");
        let code = read_exit_code(Path::new(&record.exit_code_path))
            .ok()
            .flatten();
        record.last_exit_code = code;
        if record.finished_at_epoch.is_none() {
            record.finished_at_epoch = Some(now_epoch_seconds());
        }
        record.status = match code {
            Some(value) => format!("Exited ({value}) (native)"),
            None => String::from("Exited (native)"),
        };
        record.pid = None;
    }
}

fn start_native_container_entry(container_id: &str, sender: &Sender<WorkerEvent>) -> Result<()> {
    let paths = ensure_engine_paths()?;
    let mut state = refresh_native_runtime_state(&paths)?;
    let record = state
        .containers
        .iter_mut()
        .find(|item| item.id == container_id)
        .ok_or_else(|| anyhow!("native container `{container_id}` was not found"))?;
    start_native_record(record, sender)?;
    save_runtime_state(&paths, &state)?;
    Ok(())
}

fn stop_native_container_entry(container_id: &str, sender: &Sender<WorkerEvent>) -> Result<()> {
    let paths = ensure_engine_paths()?;
    let mut state = refresh_native_runtime_state(&paths)?;
    let record = state
        .containers
        .iter_mut()
        .find(|item| item.id == container_id)
        .ok_or_else(|| anyhow!("native container `{container_id}` was not found"))?;
    if let Some(pid) = record.pid {
        let _ = sender.send(WorkerEvent::LogLine(format!("$ kill {pid}")));
        let _ = Command::new("kill").arg(pid.to_string()).output();
        thread::sleep(Duration::from_millis(250));
        if pid_is_running(pid) {
            let _ = sender.send(WorkerEvent::LogLine(format!("$ kill -9 {pid}")));
            let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
        }
    }
    record.pid = None;
    record.finished_at_epoch = Some(now_epoch_seconds());
    sync_native_record(record);
    save_runtime_state(&paths, &state)?;
    Ok(())
}

fn start_native_record(
    record: &mut NativeContainerRecord,
    sender: &Sender<WorkerEvent>,
) -> Result<()> {
    let script_path = PathBuf::from(&record.exit_code_path).with_extension("sh");
    let working_dir = native_working_dir(&record.working_dir)?;
    let escaped_command = shell_single_quote(&record.command);
    let escaped_exit = shell_single_quote(&record.exit_code_path);
    let script = format!(
        "#!/bin/sh\n/bin/sh -lc '{escaped_command}'\nstatus=$?\necho \"$status\" > '{escaped_exit}'\nexit \"$status\"\n"
    );
    fs::write(&script_path, script)
        .with_context(|| format!("unable to write {}", script_path.display()))?;

    if let Some(parent) = Path::new(&record.log_path).parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("unable to create {}", parent.display()))?;
    }
    File::create(&record.log_path)
        .with_context(|| format!("unable to create {}", record.log_path))?;
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&record.log_path)
        .with_context(|| format!("unable to open {}", record.log_path))?;
    let stderr = stdout
        .try_clone()
        .context("unable to duplicate native runtime log writer")?;

    let _ = sender.send(WorkerEvent::LogLine(format!(
        "$ native-run {}",
        record.command
    )));
    let mut command = Command::new("/bin/sh");
    command.arg(script_path);
    command.current_dir(&working_dir);
    command.stdout(Stdio::from(stdout));
    command.stderr(Stdio::from(stderr));
    command.stdin(Stdio::null());
    command.envs(parse_env_pairs(&record.env));
    let child = command
        .spawn()
        .context("unable to start native runtime process")?;
    record.pid = Some(child.id());
    record.state = String::from("running");
    record.status = String::from("Up 0s (native)");
    record.started_at_epoch = Some(now_epoch_seconds());
    record.finished_at_epoch = None;
    record.last_exit_code = None;
    Ok(())
}

fn remove_native_container_record(
    paths: &EnginePaths,
    container_id: &str,
    stop_if_running: bool,
) -> Result<()> {
    let mut state = refresh_native_runtime_state(paths)?;
    let remove_index = state
        .containers
        .iter()
        .position(|item| item.id == container_id)
        .ok_or_else(|| anyhow!("native container `{container_id}` was not found"))?;
    let record = state.containers.remove(remove_index);
    if stop_if_running {
        if let Some(pid) = record.pid.filter(|pid| pid_is_running(*pid)) {
            let _ = Command::new("kill").arg(pid.to_string()).output();
        }
    }
    let _ = fs::remove_file(&record.log_path);
    let _ = fs::remove_file(&record.exit_code_path);
    let _ = fs::remove_file(PathBuf::from(&record.exit_code_path).with_extension("sh"));
    save_runtime_state(paths, &state)?;
    Ok(())
}

fn render_native_ports(ports: &[String]) -> String {
    if ports.is_empty() {
        String::from("-")
    } else {
        ports.join(", ")
    }
}

fn parse_env_pairs(values: &[String]) -> Vec<(String, String)> {
    values
        .iter()
        .filter_map(|entry| {
            let (key, value) = entry.split_once('=')?;
            Some((key.trim().to_string(), value.to_string()))
        })
        .collect()
}

fn append_native_log(record: &NativeContainerRecord, text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&record.log_path)
        .with_context(|| format!("unable to open {}", record.log_path))?;
    use std::io::Write;
    file.write_all(text.as_bytes())
        .with_context(|| format!("unable to append {}", record.log_path))?;
    Ok(())
}

fn native_working_dir(value: &str) -> Result<PathBuf> {
    if value.trim().is_empty() {
        std::env::current_dir().context("unable to determine current working directory")
    } else {
        Ok(PathBuf::from(value))
    }
}

fn pid_is_running(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn read_exit_code(path: &Path) -> Result<Option<i32>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text.trim().parse::<i32>().ok()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("unable to read {}", path.display())),
    }
}

fn shell_single_quote(value: &str) -> String {
    value.replace('\'', "'\"'\"'")
}

fn now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[derive(Clone, Debug, Default)]
struct NativeImageSpec {
    command: Option<String>,
    entrypoint: String,
    env: Vec<String>,
    working_dir: String,
    user: String,
}

fn parse_port_mappings(value: Option<&str>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(|text| text.split(','))
        .flat_map(|chunk| chunk.split_whitespace())
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_env_assignments(value: Option<&str>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(str::lines)
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_volume_mounts(value: Option<&str>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(str::lines)
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn render_command(program: &str, args: &[String]) -> String {
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(program.to_string());
    parts.extend(args.iter().cloned());
    parts.join(" ")
}

fn compose_command_args(compose_target: &str, project_name: Option<&str>) -> Result<Vec<String>> {
    let target = compose_target.trim();
    if target.is_empty() {
        bail!("compose file or project folder is required");
    }

    let target_path = PathBuf::from(target);
    let mut args = vec![String::from("compose")];
    if target_path.is_dir() {
        args.push(String::from("--project-directory"));
        args.push(target.to_string());
    } else {
        args.push(String::from("-f"));
        args.push(target.to_string());
        if let Some(parent) = target_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            args.push(String::from("--project-directory"));
            args.push(parent.display().to_string());
        }
    }

    if let Some(name) = project_name.map(str::trim).filter(|name| !name.is_empty()) {
        args.push(String::from("-p"));
        args.push(name.to_string());
    }

    Ok(args)
}

fn compose_project_display_name(compose_target: &str, project_name: Option<&str>) -> String {
    if let Some(name) = project_name.map(str::trim).filter(|name| !name.is_empty()) {
        return name.to_string();
    }

    let target_path = Path::new(compose_target.trim());
    if target_path.is_dir() {
        target_path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| compose_target.to_string())
    } else {
        target_path
            .parent()
            .and_then(|parent| parent.file_name())
            .map(|value| value.to_string_lossy().to_string())
            .or_else(|| {
                target_path
                    .file_stem()
                    .map(|value| value.to_string_lossy().to_string())
            })
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| compose_target.to_string())
    }
}

fn primary_compose_target(config_files: &str) -> String {
    config_files
        .split(',')
        .map(str::trim)
        .find(|value| !value.is_empty())
        .unwrap_or("")
        .to_string()
}

fn compose_target_working_dir(target: &str) -> String {
    let target_path = Path::new(target);
    if target_path.is_dir() {
        target.to_string()
    } else {
        target_path
            .parent()
            .map(|parent| parent.display().to_string())
            .unwrap_or_default()
    }
}

fn parse_json_list<T>(bytes: &[u8]) -> Result<Vec<T>>
where
    T: for<'de> Deserialize<'de>,
{
    let text = String::from_utf8_lossy(bytes).trim().to_string();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    if text.starts_with('[') {
        serde_json::from_str(&text).context("unable to parse docker json array output")
    } else {
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line)
                    .with_context(|| format!("unable to parse docker json row: {line}"))
            })
            .collect()
    }
}

fn native_pull_entry(image: &str, sender: &Sender<WorkerEvent>) -> Result<StoredImageRecord> {
    let paths = ensure_engine_paths()?;
    let reference: Reference = image.parse().context("invalid image reference")?;
    let _ = sender.send(WorkerEvent::LogLine(format!(
        "Native pull requested for `{}`.",
        canonical_reference(&reference)
    )));
    let _ = sender.send(WorkerEvent::LogLine(String::from(
        "Using anonymous registry auth for now. Public images work without Docker.",
    )));
    let record = pull_reference_into_store(&paths, &reference, sender)?;
    let _ = sender.send(WorkerEvent::ImageList(
        list_images().map_err(|err| err.to_string()),
    ));
    Ok(record)
}

fn native_build_entry(
    context: &str,
    tag: &str,
    dockerfile_override: Option<&str>,
    sender: &Sender<WorkerEvent>,
) -> Result<StoredImageRecord> {
    let paths = ensure_engine_paths()?;
    let output_reference: Reference = tag.parse().context("invalid output image tag")?;
    let prepared = prepare_build_context(context, sender)?;
    let dockerfile_path = resolve_dockerfile_path(prepared.root(), dockerfile_override)?;
    let dockerfile_text = fs::read_to_string(&dockerfile_path)
        .with_context(|| format!("unable to read Dockerfile at {}", dockerfile_path.display()))?;
    let parsed = Dockerfile::parse(&dockerfile_text).context("unable to parse Dockerfile")?;

    let froms = parsed
        .instructions
        .iter()
        .filter_map(|instruction| instruction.as_from())
        .collect::<Vec<_>>();
    if froms.len() != 1 {
        bail!("native builder currently supports exactly one FROM stage");
    }

    let base_image = froms[0]
        .image_parsed
        .resolve_vars(&parsed)
        .unwrap_or_else(|| froms[0].image_parsed.clone())
        .to_string();
    let base_reference: Reference = base_image
        .parse()
        .with_context(|| format!("invalid FROM image reference `{base_image}`"))?;

    let _ = sender.send(WorkerEvent::LogLine(format!(
        "Preparing native build for `{}` from base `{}`.",
        canonical_reference(&output_reference),
        canonical_reference(&base_reference)
    )));

    let base_record = ensure_local_image(&paths, &base_reference, sender)?;
    let mut manifest = load_manifest(&paths, &base_record.manifest_digest)?;
    let mut image_config = load_config(&paths, &base_record.config_digest)?;

    apply_supported_instructions(&parsed, &mut image_config)?;

    let config_json = image_config
        .to_string()
        .context("unable to serialize built image config")?;
    let config_bytes = config_json.into_bytes();
    let config_digest = sha256_digest(&config_bytes);
    write_digest_text(&paths.configs, &config_digest, &config_bytes)?;

    let new_descriptor = DescriptorBuilder::default()
        .media_type(MediaType::ImageConfig)
        .digest(
            config_digest
                .parse::<OciDigest>()
                .context("invalid generated config digest")?,
        )
        .size(config_bytes.len() as u64)
        .build()
        .context("unable to create config descriptor")?;
    manifest.set_config(new_descriptor);

    let manifest_json = manifest
        .to_string()
        .context("unable to serialize built manifest")?;
    let manifest_bytes = manifest_json.into_bytes();
    let manifest_digest = sha256_digest(&manifest_bytes);
    write_digest_text(&paths.manifests, &manifest_digest, &manifest_bytes)?;

    let record = StoredImageRecord {
        canonical_reference: canonical_reference(&output_reference),
        repository: output_reference.repository().to_string(),
        tag: output_reference.tag().unwrap_or("latest").to_string(),
        manifest_digest,
        config_digest,
        size_bytes: manifest
            .layers()
            .iter()
            .map(|layer| layer.size())
            .sum::<u64>()
            + config_bytes.len() as u64,
        source: String::from("built-native"),
        architecture: format!("{:?}", image_config.architecture()),
        os: format!("{:?}", image_config.os()),
        created_at_epoch: now_epoch(),
    };
    upsert_record(&paths, record.clone())?;

    let _ = sender.send(WorkerEvent::LogLine(String::from(
        "Native build completed. Reused base layers and emitted a new OCI config + manifest.",
    )));
    let _ = sender.send(WorkerEvent::ImageList(
        list_images().map_err(|err| err.to_string()),
    ));
    Ok(record)
}

fn ensure_local_image(
    paths: &EnginePaths,
    reference: &Reference,
    sender: &Sender<WorkerEvent>,
) -> Result<StoredImageRecord> {
    if let Some(record) = find_record(paths, &canonical_reference(reference))? {
        let _ = sender.send(WorkerEvent::LogLine(format!(
            "Base image `{}` already exists in the native store.",
            canonical_reference(reference)
        )));
        return Ok(record);
    }

    let _ = sender.send(WorkerEvent::LogLine(format!(
        "Base image `{}` not found locally. Pulling it natively first...",
        canonical_reference(reference)
    )));
    pull_reference_into_store(paths, reference, sender)
}

fn pull_reference_into_store(
    paths: &EnginePaths,
    reference: &Reference,
    sender: &Sender<WorkerEvent>,
) -> Result<StoredImageRecord> {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .context("unable to create async runtime")?;

    runtime.block_on(async {
        let client = native_client();
        let auth = RegistryAuth::Anonymous;

        let _ = sender.send(WorkerEvent::LogLine(format!(
            "Fetching manifest for `{}`...",
            canonical_reference(reference)
        )));
        let (manifest, manifest_digest, config_json) = client
            .pull_manifest_and_config(reference, &auth)
            .await
            .context("registry manifest pull failed")?;

        let manifest_text =
            serde_json::to_string_pretty(&manifest).context("unable to encode manifest json")?;
        write_digest_text(&paths.manifests, &manifest_digest, manifest_text.as_bytes())?;

        let config_digest = manifest.config.digest.clone();
        write_digest_text(&paths.configs, &config_digest, config_json.as_bytes())?;

        for (index, layer) in manifest.layers.iter().enumerate() {
            let layer_path = digest_blob_path(&paths.blobs, &layer.digest)?;
            if layer_path.exists() {
                let _ = sender.send(WorkerEvent::LogLine(format!(
                    "Layer {}/{} {} already cached.",
                    index + 1,
                    manifest.layers.len(),
                    shorten_digest(&layer.digest)
                )));
                continue;
            }

            if let Some(parent) = layer_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("unable to create {}", parent.display()))?;
            }

            let _ = sender.send(WorkerEvent::LogLine(format!(
                "Downloading layer {}/{} {} ({})...",
                index + 1,
                manifest.layers.len(),
                shorten_digest(&layer.digest),
                format_bytes(layer.size as u64)
            )));

            let mut file = tokio::fs::File::create(&layer_path)
                .await
                .with_context(|| format!("unable to create {}", layer_path.display()))?;
            client
                .pull_blob(reference, layer, &mut file)
                .await
                .with_context(|| format!("failed to download layer {}", layer.digest))?;
        }

        let image_config: ImageConfiguration =
            serde_json::from_str(&config_json).context("unable to parse image config")?;
        let record = StoredImageRecord {
            canonical_reference: canonical_reference(reference),
            repository: reference.repository().to_string(),
            tag: reference.tag().unwrap_or("latest").to_string(),
            manifest_digest,
            config_digest,
            size_bytes: manifest
                .layers
                .iter()
                .map(|layer| layer.size.max(0) as u64)
                .sum::<u64>()
                + config_json.len() as u64,
            source: String::from("pulled-native"),
            architecture: format!("{:?}", image_config.architecture()),
            os: format!("{:?}", image_config.os()),
            created_at_epoch: now_epoch(),
        };
        upsert_record(paths, record.clone())?;
        Ok(record)
    })
}

fn native_client() -> Client {
    let mut config = ClientConfig::default();
    config.platform_resolver = Some(Box::new(native_platform_resolver));
    Client::new(config)
}

fn native_platform_resolver(manifests: &[ImageIndexEntry]) -> Option<String> {
    let preferred_arch = match std::env::consts::ARCH {
        "aarch64" => Arch::ARM64,
        "x86_64" => Arch::Amd64,
        _ => Arch::Amd64,
    };

    manifests
        .iter()
        .find(|entry| {
            entry.platform.as_ref().is_some_and(|platform| {
                platform.os == Os::Linux && platform.architecture == preferred_arch
            })
        })
        .or_else(|| {
            manifests.iter().find(|entry| {
                entry.platform.as_ref().is_some_and(|platform| {
                    platform.os == Os::Linux && platform.architecture == Arch::Amd64
                })
            })
        })
        .map(|entry| entry.digest.clone())
}

fn apply_supported_instructions(
    dockerfile: &Dockerfile,
    image_config: &mut ImageConfiguration,
) -> Result<()> {
    let mut saw_first_from = false;
    let mut unsupported = Vec::new();
    let mut runtime_config = image_config.config().clone().unwrap_or_default();
    let mut history = image_config.history().clone().unwrap_or_default();

    for instruction in &dockerfile.instructions {
        match instruction {
            Instruction::From(_) if !saw_first_from => {
                saw_first_from = true;
            }
            Instruction::From(_) => unsupported.push(String::from("multi-stage FROM")),
            Instruction::Arg(arg) if saw_first_from => {
                unsupported.push(format!("ARG {}", arg.name.content));
            }
            Instruction::Env(env) if saw_first_from => {
                apply_env(&mut runtime_config, env);
                history.push(history_entry(&format!("ENV {}", env_to_string(env))));
            }
            Instruction::Label(label) if saw_first_from => {
                apply_labels(&mut runtime_config, label);
                history.push(history_entry(&format!("LABEL {}", label_to_string(label))));
            }
            Instruction::Cmd(cmd) if saw_first_from => {
                runtime_config.set_cmd(Some(command_value(cmd)));
                history.push(history_entry(&format!("CMD {}", command_to_string(cmd))));
            }
            Instruction::Entrypoint(entrypoint) if saw_first_from => {
                runtime_config.set_entrypoint(Some(command_value(entrypoint)));
                history.push(history_entry(&format!(
                    "ENTRYPOINT {}",
                    entrypoint_to_string(entrypoint)
                )));
            }
            Instruction::Misc(misc) if saw_first_from => {
                apply_misc(&mut runtime_config, &mut history, misc, &mut unsupported);
            }
            Instruction::Run(run) if saw_first_from => {
                unsupported.push(format!("RUN {}", run_to_string(run)))
            }
            Instruction::Copy(copy) if saw_first_from => {
                unsupported.push(format!("COPY {}", copy.sources.len()))
            }
            _ => {}
        }
    }

    if !unsupported.is_empty() {
        bail!(
            "native builder currently supports only metadata-changing Dockerfiles. Unsupported instructions: {}",
            unsupported.join(", ")
        );
    }

    image_config.set_config(Some(runtime_config));
    image_config.set_history(Some(history));
    Ok(())
}

fn apply_env(config: &mut Config, instruction: &dockerfile_parser::EnvInstruction) {
    let mut env_map = HashMap::<String, String>::new();
    let mut order = Vec::<String>::new();

    if let Some(existing) = config.env().clone() {
        for entry in existing {
            if let Some((key, value)) = entry.split_once('=') {
                order.push(key.to_string());
                env_map.insert(key.to_string(), value.to_string());
            }
        }
    }

    for item in &instruction.vars {
        let key = item.key.content.clone();
        if !env_map.contains_key(&key) {
            order.push(key.clone());
        }
        env_map.insert(key, item.value.to_string());
    }

    let values = order
        .into_iter()
        .filter_map(|key| env_map.get(&key).map(|value| format!("{key}={value}")))
        .collect::<Vec<_>>();
    config.set_env(Some(values));
}

fn apply_labels(config: &mut Config, instruction: &dockerfile_parser::LabelInstruction) {
    let mut labels = config.labels().clone().unwrap_or_default();
    for item in &instruction.labels {
        labels.insert(item.name.content.clone(), item.value.content.clone());
    }
    config.set_labels(Some(labels));
}

fn apply_misc(
    config: &mut Config,
    history: &mut Vec<oci_spec::image::History>,
    misc: &dockerfile_parser::MiscInstruction,
    unsupported: &mut Vec<String>,
) {
    let name = misc.instruction.content.to_uppercase();
    let args = misc.arguments.to_string();

    match name.as_str() {
        "EXPOSE" => {
            let ports = args
                .split_whitespace()
                .map(|value| {
                    if value.contains('/') {
                        value.to_string()
                    } else {
                        format!("{value}/tcp")
                    }
                })
                .collect::<Vec<_>>();
            config.set_exposed_ports(Some(ports));
            history.push(history_entry(&format!("EXPOSE {}", args.trim())));
        }
        "USER" => {
            config.set_user(Some(args.trim().to_string()));
            history.push(history_entry(&format!("USER {}", args.trim())));
        }
        "WORKDIR" => {
            config.set_working_dir(Some(args.trim().to_string()));
            history.push(history_entry(&format!("WORKDIR {}", args.trim())));
        }
        _ => unsupported.push(format!("{} {}", name, args.trim())),
    }
}

fn command_value<T>(instruction: &T) -> Vec<String>
where
    T: CommandForm,
{
    if let Some(exec) = instruction.exec_form() {
        exec
    } else {
        vec![
            String::from("/bin/sh"),
            String::from("-c"),
            instruction.shell_form().unwrap_or_default(),
        ]
    }
}

fn command_to_string(instruction: &dockerfile_parser::CmdInstruction) -> String {
    if let Some(exec) = instruction.as_exec() {
        exec.elements
            .iter()
            .map(|item| format!("{:?}", item.content))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        instruction
            .as_shell()
            .map(|value| value.to_string())
            .unwrap_or_default()
    }
}

fn entrypoint_to_string(instruction: &dockerfile_parser::EntrypointInstruction) -> String {
    if let Some(exec) = instruction.as_exec() {
        exec.elements
            .iter()
            .map(|item| format!("{:?}", item.content))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        instruction
            .as_shell()
            .map(|value| value.to_string())
            .unwrap_or_default()
    }
}

fn env_to_string(instruction: &dockerfile_parser::EnvInstruction) -> String {
    instruction
        .vars
        .iter()
        .map(|item| format!("{}={}", item.key.content, item.value))
        .collect::<Vec<_>>()
        .join(" ")
}

fn label_to_string(instruction: &dockerfile_parser::LabelInstruction) -> String {
    instruction
        .labels
        .iter()
        .map(|item| format!("{}={}", item.name.content, item.value.content))
        .collect::<Vec<_>>()
        .join(" ")
}

fn history_entry(command: &str) -> oci_spec::image::History {
    HistoryBuilder::default()
        .created_by(command.to_string())
        .empty_layer(true)
        .build()
        .expect("history entry build should not fail")
}

fn run_to_string(instruction: &dockerfile_parser::RunInstruction) -> String {
    if let Some(exec) = instruction.as_exec() {
        exec.elements
            .iter()
            .map(|item| format!("{:?}", item.content))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        instruction
            .as_shell()
            .map(|value| value.to_string())
            .unwrap_or_default()
    }
}

fn ensure_engine_paths() -> Result<EnginePaths> {
    let project_dirs = ProjectDirs::from("rs", "docker-rs", "DockerRSDesktop")
        .ok_or_else(|| anyhow!("unable to determine application data directory"))?;
    let root = project_dirs.data_dir().join("engine");
    let blobs = root.join("blobs");
    let manifests = root.join("manifests");
    let configs = root.join("configs");
    let metadata = root.join("images.json");
    let runtime_metadata = root.join("runtime.json");
    let runtime_logs = root.join("runtime-logs");
    let runtime_meta = root.join("runtime-meta");

    fs::create_dir_all(&blobs).with_context(|| format!("unable to create {}", blobs.display()))?;
    fs::create_dir_all(&manifests)
        .with_context(|| format!("unable to create {}", manifests.display()))?;
    fs::create_dir_all(&configs)
        .with_context(|| format!("unable to create {}", configs.display()))?;
    fs::create_dir_all(&runtime_logs)
        .with_context(|| format!("unable to create {}", runtime_logs.display()))?;
    fs::create_dir_all(&runtime_meta)
        .with_context(|| format!("unable to create {}", runtime_meta.display()))?;

    if !metadata.exists() {
        fs::write(
            &metadata,
            serde_json::to_vec_pretty(&EngineState::default())?,
        )
        .with_context(|| format!("unable to create {}", metadata.display()))?;
    }
    if !runtime_metadata.exists() {
        fs::write(
            &runtime_metadata,
            serde_json::to_vec_pretty(&NativeRuntimeState::default())?,
        )
        .with_context(|| format!("unable to create {}", runtime_metadata.display()))?;
    }

    Ok(EnginePaths {
        root,
        blobs,
        manifests,
        configs,
        metadata,
        runtime_metadata,
        runtime_logs,
        runtime_meta,
    })
}

fn load_state(paths: &EnginePaths) -> Result<EngineState> {
    let text = fs::read_to_string(&paths.metadata)
        .with_context(|| format!("unable to read {}", paths.metadata.display()))?;
    Ok(serde_json::from_str(&text).context("invalid engine metadata file")?)
}

fn save_state(paths: &EnginePaths, state: &EngineState) -> Result<()> {
    let content = serde_json::to_vec_pretty(state).context("unable to encode engine state")?;
    fs::write(&paths.metadata, content)
        .with_context(|| format!("unable to write {}", paths.metadata.display()))?;
    Ok(())
}

fn load_runtime_state(paths: &EnginePaths) -> Result<NativeRuntimeState> {
    let text = fs::read_to_string(&paths.runtime_metadata)
        .with_context(|| format!("unable to read {}", paths.runtime_metadata.display()))?;
    Ok(serde_json::from_str(&text).context("invalid native runtime metadata file")?)
}

fn save_runtime_state(paths: &EnginePaths, state: &NativeRuntimeState) -> Result<()> {
    let content =
        serde_json::to_vec_pretty(state).context("unable to encode native runtime state")?;
    fs::write(&paths.runtime_metadata, content)
        .with_context(|| format!("unable to write {}", paths.runtime_metadata.display()))?;
    Ok(())
}

fn upsert_record(paths: &EnginePaths, record: StoredImageRecord) -> Result<()> {
    let mut state = load_state(paths)?;
    state.images.retain(|item| {
        !(item.repository == record.repository && item.tag == record.tag)
            && item.canonical_reference != record.canonical_reference
    });
    state.images.push(record);
    save_state(paths, &state)
}

fn find_record(
    paths: &EnginePaths,
    canonical_reference: &str,
) -> Result<Option<StoredImageRecord>> {
    let state = load_state(paths)?;
    Ok(state
        .images
        .into_iter()
        .find(|record| record.canonical_reference == canonical_reference))
}

fn load_manifest(paths: &EnginePaths, digest: &str) -> Result<ImageManifest> {
    let path = digest_json_path(&paths.manifests, digest)?;
    ImageManifest::from_file(&path).with_context(|| format!("unable to load {}", path.display()))
}

fn load_config(paths: &EnginePaths, digest: &str) -> Result<ImageConfiguration> {
    let path = digest_json_path(&paths.configs, digest)?;
    ImageConfiguration::from_file(&path)
        .with_context(|| format!("unable to load {}", path.display()))
}

fn write_digest_text(base: &Path, digest: &str, bytes: &[u8]) -> Result<()> {
    let path = digest_json_path(base, digest)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("unable to create {}", parent.display()))?;
    }
    fs::write(&path, bytes).with_context(|| format!("unable to write {}", path.display()))?;
    Ok(())
}

fn digest_json_path(base: &Path, digest: &str) -> Result<PathBuf> {
    let (algo, hex) = split_digest(digest)?;
    Ok(base.join(algo).join(format!("{hex}.json")))
}

fn digest_blob_path(base: &Path, digest: &str) -> Result<PathBuf> {
    let (algo, hex) = split_digest(digest)?;
    Ok(base.join(algo).join(hex))
}

fn split_digest(digest: &str) -> Result<(&str, &str)> {
    digest
        .split_once(':')
        .ok_or_else(|| anyhow!("invalid OCI digest `{digest}`"))
}

fn canonical_reference(reference: &Reference) -> String {
    reference.to_string()
}

fn display_record(record: &StoredImageRecord) -> String {
    format!("{}:{}", record.repository, record.tag)
}

fn prepare_build_context(context: &str, sender: &Sender<WorkerEvent>) -> Result<PreparedContext> {
    if looks_like_git_url(context) {
        let tempdir = TempDir::new().context("unable to create temporary build directory")?;
        let _ = sender.send(WorkerEvent::LogLine(format!(
            "Cloning remote repository `{context}` into {}...",
            tempdir.path().display()
        )));
        let output = Command::new("git")
            .args([
                "clone",
                "--depth",
                "1",
                context,
                tempdir.path().to_string_lossy().as_ref(),
            ])
            .output()
            .context("failed to launch git")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            bail!(
                "git clone failed{}",
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            );
        }
        Ok(PreparedContext::Temporary(tempdir))
    } else {
        let path = PathBuf::from(context);
        if !path.is_dir() {
            bail!("build context `{context}` is not a directory or supported Git URL");
        }
        Ok(PreparedContext::Local(path))
    }
}

fn resolve_dockerfile_path(root: &Path, override_path: Option<&str>) -> Result<PathBuf> {
    let candidate = match override_path
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(path) => {
            let override_path = PathBuf::from(path);
            if override_path.is_absolute() {
                override_path
            } else {
                root.join(override_path)
            }
        }
        None => root.join("Dockerfile"),
    };

    if !candidate.is_file() {
        bail!("Dockerfile not found at {}", candidate.display());
    }
    Ok(candidate)
}

fn looks_like_git_url(value: &str) -> bool {
    value.starts_with("https://")
        || value.starts_with("http://")
        || value.starts_with("git@")
        || value.starts_with("ssh://")
        || value.ends_with(".git")
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit_index = 0usize;

    while value >= 1024.0 && unit_index + 1 < UNITS.len() {
        value /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{bytes} {}", UNITS[unit_index])
    } else {
        format!("{value:.1} {}", UNITS[unit_index])
    }
}

fn shorten_digest(digest: &str) -> String {
    if let Some((algo, hex)) = digest.split_once(':') {
        let short = hex.chars().take(12).collect::<String>();
        format!("{algo}:{short}")
    } else {
        digest.chars().take(18).collect()
    }
}

fn shorten_container_id(value: &str) -> String {
    value.chars().take(12).collect()
}

fn looks_like_container_id(value: &str) -> bool {
    value.len() >= 12 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

trait CommandForm {
    fn exec_form(&self) -> Option<Vec<String>>;
    fn shell_form(&self) -> Option<String>;
}

impl CommandForm for dockerfile_parser::CmdInstruction {
    fn exec_form(&self) -> Option<Vec<String>> {
        self.as_exec().map(|items| {
            items
                .elements
                .iter()
                .map(|item| item.content.clone())
                .collect()
        })
    }

    fn shell_form(&self) -> Option<String> {
        self.as_shell().map(|value| value.to_string())
    }
}

impl CommandForm for dockerfile_parser::EntrypointInstruction {
    fn exec_form(&self) -> Option<Vec<String>> {
        self.as_exec().map(|items| {
            items
                .elements
                .iter()
                .map(|item| item.content.clone())
                .collect()
        })
    }

    fn shell_form(&self) -> Option<String> {
        self.as_shell().map(|value| value.to_string())
    }
}

enum PreparedContext {
    Local(PathBuf),
    Temporary(TempDir),
}

impl PreparedContext {
    fn root(&self) -> &Path {
        match self {
            Self::Local(path) => path.as_path(),
            Self::Temporary(path) => path.path(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn metadata_only_builder_updates_runtime_config() {
        let dockerfile = Dockerfile::parse(
            r#"
FROM alpine:3.20
ENV APP_ENV=prod PORT=8080
LABEL org.opencontainers.image.title="demo"
EXPOSE 8080
USER app
WORKDIR /srv/app
CMD ["echo", "hello"]
ENTRYPOINT ["/bin/demo"]
"#,
        )
        .unwrap();

        let mut image_config: ImageConfiguration = serde_json::from_str(
            r#"{
  "architecture": "amd64",
  "os": "linux",
  "config": {
    "Env": ["PATH=/usr/bin"]
  },
  "rootfs": {
    "type": "layers",
    "diff_ids": []
  }
}"#,
        )
        .unwrap();

        apply_supported_instructions(&dockerfile, &mut image_config).unwrap();

        let runtime = image_config.config().clone().unwrap();
        assert_eq!(
            runtime.env().clone().unwrap(),
            vec![
                String::from("PATH=/usr/bin"),
                String::from("APP_ENV=prod"),
                String::from("PORT=8080"),
            ]
        );
        assert_eq!(
            runtime.labels().clone().unwrap()["org.opencontainers.image.title"],
            "demo"
        );
        assert_eq!(
            runtime.exposed_ports().clone().unwrap(),
            vec![String::from("8080/tcp")]
        );
        assert_eq!(runtime.user().clone().unwrap(), "app");
        assert_eq!(runtime.working_dir().clone().unwrap(), "/srv/app");
        assert_eq!(runtime.cmd().clone().unwrap(), vec!["echo", "hello"]);
        assert_eq!(runtime.entrypoint().clone().unwrap(), vec!["/bin/demo"]);
    }

    #[test]
    fn metadata_only_builder_rejects_run() {
        let dockerfile = Dockerfile::parse(
            r#"
FROM alpine:3.20
RUN apk add curl
"#,
        )
        .unwrap();

        let mut image_config: ImageConfiguration = serde_json::from_str(
            r#"{
  "architecture": "amd64",
  "os": "linux",
  "rootfs": {
    "type": "layers",
    "diff_ids": []
  }
}"#,
        )
        .unwrap();

        let error = apply_supported_instructions(&dockerfile, &mut image_config)
            .unwrap_err()
            .to_string();
        assert!(error.contains("RUN"));
    }

    #[test]
    #[ignore = "requires network access to a public OCI registry"]
    fn native_pull_public_image_smoke() {
        let (sender, _receiver) = mpsc::channel();
        let record = native_pull_entry("hello-world:latest", &sender).unwrap();
        assert_eq!(record.repository, "library/hello-world");
        assert_eq!(record.tag, "latest");
        assert!(record.config_digest.starts_with("sha256:"));
    }
}
