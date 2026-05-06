use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use super::*;

pub fn check_engine_status(sender: Sender<WorkerEvent>) {
    thread::spawn(move || {
        let result = engine_status().map_err(|err| err.to_string());
        let _ = sender.send(WorkerEvent::EngineStatus(result));
    });
}

pub fn refresh_images(sender: Sender<WorkerEvent>) {
    thread::spawn(move || {
        let result = list_images().map_err(|err| err.to_string());
        let _ = sender.send(WorkerEvent::ImageList(result));
    });
}

pub fn refresh_projects(sender: Sender<WorkerEvent>) {
    thread::spawn(move || {
        let result = list_projects().map_err(|err| err.to_string());
        let _ = sender.send(WorkerEvent::ProjectList(result));
    });
}

pub fn inspect_image(image: String, sender: Sender<WorkerEvent>) {
    thread::spawn(move || {
        let result = inspect_image_entry(&image).map_err(|err| err.to_string());
        let _ = sender.send(WorkerEvent::ImageDetails(result));
    });
}

pub fn inspect_container_stats(container_id: String, sender: Sender<WorkerEvent>) {
    thread::spawn(move || {
        let result = inspect_container_stats_entry(&container_id).map_err(|err| err.to_string());
        let _ = sender.send(WorkerEvent::ContainerStats(result));
    });
}

pub fn check_runtime_status(sender: Sender<WorkerEvent>) {
    thread::spawn(move || {
        let result = runtime_status().map_err(|err| err.to_string());
        let _ = sender.send(WorkerEvent::RuntimeStatus(result));
    });
}

pub fn refresh_containers(sender: Sender<WorkerEvent>) {
    thread::spawn(move || {
        let result = list_containers().map_err(|err| err.to_string());
        let _ = sender.send(WorkerEvent::ContainerList(result));
    });
}

pub fn start_runtime(sender: Sender<WorkerEvent>) {
    thread::spawn(move || {
        let _ = sender.send(WorkerEvent::LogLine(String::from(
            "Launching Docker Desktop runtime bridge...",
        )));

        let output = Command::new("open").args(["-a", "Docker"]).output();
        match output {
            Ok(result) if result.status.success() => {
                let _ = sender.send(WorkerEvent::LogLine(String::from(
                    "Docker Desktop launch request sent. Waiting for runtime...",
                )));
            }
            Ok(result) => {
                let stderr = String::from_utf8_lossy(&result.stderr).trim().to_owned();
                let message = if stderr.is_empty() {
                    String::from("Failed to launch Docker Desktop runtime.")
                } else {
                    format!("Failed to launch Docker Desktop runtime: {stderr}")
                };
                let _ = sender.send(WorkerEvent::ActionFinished(Err(message)));
                return;
            }
            Err(err) => {
                let _ = sender.send(WorkerEvent::ActionFinished(Err(format!(
                    "Failed to launch Docker Desktop runtime: {err}"
                ))));
                return;
            }
        }

        for _ in 0..45 {
            thread::sleep(Duration::from_secs(1));
            match runtime_status() {
                Ok(info) => {
                    let _ = sender.send(WorkerEvent::RuntimeStatus(Ok(info)));
                    let _ = sender.send(WorkerEvent::ContainerList(
                        list_containers().map_err(|err| err.to_string()),
                    ));
                    let _ = sender.send(WorkerEvent::ActionFinished(Ok(String::from(
                        "Docker runtime bridge is ready.",
                    ))));
                    return;
                }
                Err(message) => {
                    let _ = sender.send(WorkerEvent::RuntimeStatus(Err(message.to_string())));
                }
            }
        }

        let _ = sender.send(WorkerEvent::ActionFinished(Err(String::from(
            "Docker runtime bridge did not become ready within 45 seconds.",
        ))));
    });
}

pub fn pull_image(image: String, sender: Sender<WorkerEvent>) {
    thread::spawn(move || {
        let result = native_pull_entry(&image, &sender)
            .map(|record| {
                format!(
                    "Pulled `{}` into the native OCI store.",
                    display_record(&record)
                )
            })
            .map_err(|err| format!("Native pull failed for `{image}`: {err}"));
        let _ = sender.send(WorkerEvent::ActionFinished(result));
    });
}

pub fn build_image(
    context: String,
    tag: String,
    dockerfile: Option<String>,
    sender: Sender<WorkerEvent>,
) {
    thread::spawn(move || {
        let result = native_build_entry(&context, &tag, dockerfile.as_deref(), &sender)
            .map(|record| {
                format!(
                    "Built `{}` with the native metadata-only builder.",
                    display_record(&record)
                )
            })
            .map_err(|err| format!("Native build failed for `{tag}`: {err}"));
        let _ = sender.send(WorkerEvent::ActionFinished(result));
    });
}

pub fn compose_project_up(
    compose_target: String,
    project_name: Option<String>,
    sender: Sender<WorkerEvent>,
) {
    thread::spawn(move || {
        let trimmed_target = compose_target.trim().to_owned();
        let trimmed_project_name = project_name.and_then(|value| {
            let value = value.trim().to_owned();
            if value.is_empty() { None } else { Some(value) }
        });
        let result = compose_up_entry(&trimmed_target, trimmed_project_name.as_deref(), &sender)
            .map(|name| format!("Compose project `{name}` is up."))
            .map_err(|err| format!("Compose up failed for `{trimmed_target}`: {err}"));
        let _ = sender.send(WorkerEvent::ActionFinished(result));
    });
}

pub fn compose_project_down(
    compose_target: String,
    project_name: Option<String>,
    sender: Sender<WorkerEvent>,
) {
    thread::spawn(move || {
        let trimmed_target = compose_target.trim().to_owned();
        let trimmed_project_name = project_name.and_then(|value| {
            let value = value.trim().to_owned();
            if value.is_empty() { None } else { Some(value) }
        });
        let result = compose_down_entry(&trimmed_target, trimmed_project_name.as_deref(), &sender)
            .map(|name| format!("Compose project `{name}` is down."))
            .map_err(|err| format!("Compose down failed for `{trimmed_target}`: {err}"));
        let _ = sender.send(WorkerEvent::ActionFinished(result));
    });
}

pub fn fetch_project_logs(
    compose_target: String,
    project_name: Option<String>,
    sender: Sender<WorkerEvent>,
) {
    thread::spawn(move || {
        let trimmed_target = compose_target.trim().to_owned();
        let trimmed_project_name = project_name.and_then(|value| {
            let value = value.trim().to_owned();
            if value.is_empty() { None } else { Some(value) }
        });
        let result =
            fetch_project_logs_entry(&trimmed_target, trimmed_project_name.as_deref(), &sender)
                .map(|name| format!("Fetched logs for compose project `{name}`."))
                .map_err(|err| format!("Compose logs failed for `{trimmed_target}`: {err}"));
        let _ = sender.send(WorkerEvent::ActionFinished(result));
    });
}

pub fn run_container(
    image: String,
    container_name: Option<String>,
    ports: Option<String>,
    env_vars: Option<String>,
    volume_mounts: Option<String>,
    command_override: Option<String>,
    restart_policy: Option<String>,
    auto_remove: bool,
    sender: Sender<WorkerEvent>,
) {
    thread::spawn(move || {
        let trimmed_image = image.trim().to_owned();
        let trimmed_name = container_name.and_then(|value| {
            let value = value.trim().to_owned();
            if value.is_empty() { None } else { Some(value) }
        });
        let trimmed_ports = ports.and_then(|value| {
            let value = value.trim().to_owned();
            if value.is_empty() { None } else { Some(value) }
        });
        let trimmed_env_vars = env_vars.and_then(|value| {
            let value = value.trim().to_owned();
            if value.is_empty() { None } else { Some(value) }
        });
        let trimmed_volume_mounts = volume_mounts.and_then(|value| {
            let value = value.trim().to_owned();
            if value.is_empty() { None } else { Some(value) }
        });
        let trimmed_command = command_override.and_then(|value| {
            let value = value.trim().to_owned();
            if value.is_empty() { None } else { Some(value) }
        });
        let trimmed_restart_policy = restart_policy.and_then(|value| {
            let value = value.trim().to_owned();
            if value.is_empty() { None } else { Some(value) }
        });

        let result = run_container_entry(
            &trimmed_image,
            trimmed_name.as_deref(),
            trimmed_ports.as_deref(),
            trimmed_env_vars.as_deref(),
            trimmed_volume_mounts.as_deref(),
            trimmed_command.as_deref(),
            trimmed_restart_policy.as_deref(),
            auto_remove,
            &sender,
        )
        .map(|container_id| {
            format!(
                "Started container {} from `{trimmed_image}`.",
                shorten_container_id(&container_id)
            )
        })
        .map_err(|err| format!("Container run failed for `{trimmed_image}`: {err}"));
        let _ = sender.send(WorkerEvent::ActionFinished(result));
    });
}

pub fn stop_container(container_id: String, sender: Sender<WorkerEvent>) {
    thread::spawn(move || {
        let result = stop_container_entry(&container_id, &sender)
            .map(|_| format!("Stopped container {}", shorten_container_id(&container_id)))
            .map_err(|err| format!("Stop failed for `{container_id}`: {err}"));
        let _ = sender.send(WorkerEvent::ActionFinished(result));
    });
}

pub fn start_container(container_id: String, sender: Sender<WorkerEvent>) {
    thread::spawn(move || {
        let result = start_container_entry(&container_id, &sender)
            .map(|_| format!("Started container {}", shorten_container_id(&container_id)))
            .map_err(|err| format!("Start failed for `{container_id}`: {err}"));
        let _ = sender.send(WorkerEvent::ActionFinished(result));
    });
}

pub fn fetch_container_logs(container_id: String, sender: Sender<WorkerEvent>) {
    thread::spawn(move || {
        let result = fetch_container_logs_entry(&container_id, &sender)
            .map(|_| {
                format!(
                    "Fetched logs for container {}.",
                    shorten_container_id(&container_id)
                )
            })
            .map_err(|err| format!("Log fetch failed for `{container_id}`: {err}"));
        let _ = sender.send(WorkerEvent::ActionFinished(result));
    });
}

pub fn follow_container_logs(
    container_id: String,
    stop_flag: Arc<AtomicBool>,
    sender: Sender<WorkerEvent>,
) {
    thread::spawn(move || {
        if let Err(err) = follow_container_logs_entry(&container_id, &stop_flag, &sender) {
            let _ = sender.send(WorkerEvent::LogLine(format!(
                "Live log stream failed for {}: {err}",
                shorten_container_id(&container_id)
            )));
        }
    });
}

pub fn restart_container(container_id: String, sender: Sender<WorkerEvent>) {
    thread::spawn(move || {
        let result = restart_container_entry(&container_id, &sender)
            .map(|_| {
                format!(
                    "Restarted container {}",
                    shorten_container_id(&container_id)
                )
            })
            .map_err(|err| format!("Restart failed for `{container_id}`: {err}"));
        let _ = sender.send(WorkerEvent::ActionFinished(result));
    });
}

pub fn remove_image(image: String, sender: Sender<WorkerEvent>) {
    thread::spawn(move || {
        let trimmed_image = image.trim().to_owned();
        let result = remove_image_entry(&trimmed_image)
            .map(|record| {
                format!(
                    "Removed native image `{}` from the local store.",
                    display_record(&record)
                )
            })
            .map_err(|err| format!("Image delete failed for `{trimmed_image}`: {err}"));
        let _ = sender.send(WorkerEvent::ActionFinished(result));
    });
}

pub fn remove_container(container_id: String, sender: Sender<WorkerEvent>) {
    thread::spawn(move || {
        let result = remove_container_entry(&container_id, &sender)
            .map(|_| format!("Removed container {}", shorten_container_id(&container_id)))
            .map_err(|err| format!("Remove failed for `{container_id}`: {err}"));
        let _ = sender.send(WorkerEvent::ActionFinished(result));
    });
}

pub fn inspect_container(container_id: String, sender: Sender<WorkerEvent>) {
    thread::spawn(move || {
        let result = inspect_container_entry(&container_id).map_err(|err| err.to_string());
        let _ = sender.send(WorkerEvent::ContainerDetails(result));
    });
}

pub fn exec_in_container(container_id: String, command: String, sender: Sender<WorkerEvent>) {
    thread::spawn(move || {
        let result = exec_in_container_entry(&container_id, &command, &sender)
            .map(|_| {
                format!(
                    "Executed command in container {}.",
                    shorten_container_id(&container_id)
                )
            })
            .map_err(|err| format!("Exec failed for `{container_id}`: {err}"));
        let _ = sender.send(WorkerEvent::ActionFinished(result));
    });
}
