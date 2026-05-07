use eframe::egui::{self, Align, Color32, Layout, RichText};

use super::{
    ContainerInfo, DockerImageInfo, NetworkInfo, ProjectInfo, RuntimeStatusInfo, VolumeInfo,
    WorkspaceTab,
};

pub(super) fn shorten_container_id(value: &str) -> String {
    value.chars().take(12).collect()
}

pub(super) fn workspace_title(tab: WorkspaceTab) -> &'static str {
    match tab {
        WorkspaceTab::Home => "Home",
        WorkspaceTab::Projects => "Projects",
        WorkspaceTab::Volumes => "Volumes",
        WorkspaceTab::Networks => "Networks",
        WorkspaceTab::Containers => "Containers",
        WorkspaceTab::Images => "Images",
        WorkspaceTab::Build => "Build & Run",
        WorkspaceTab::Logs => "Logs",
    }
}

pub(super) fn runtime_badge_text(status: &RuntimeStatusInfo) -> &'static str {
    match (status.native_ready, status.bridge_ready) {
        (true, true) => "Native + Docker fallback online",
        (true, false) => "Native runtime online",
        (false, true) => "Docker fallback only",
        (false, false) => "Runtime offline",
    }
}

pub(super) fn stat_line(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.small(label);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.monospace(value);
        });
    });
}

pub(super) fn status_chip(ui: &mut egui::Ui, color: Color32, text: &str) {
    ui.colored_label(color, RichText::new(text).strong());
}

pub(super) fn compact_runtime_detail(status: &RuntimeStatusInfo) -> &'static str {
    if status.bridge_ready {
        "Native first, Docker fallback ready."
    } else if status.native_ready {
        "Native first, Docker fallback offline."
    } else {
        "Runtime is currently unavailable."
    }
}

pub(super) fn compact_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let parts = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() <= 3 {
        normalized
    } else {
        format!(
            ".../{}/{}/{}",
            parts[parts.len() - 3],
            parts[parts.len() - 2],
            parts[parts.len() - 1]
        )
    }
}

pub(super) fn detail_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.strong(label);
    ui.label(value);
    ui.end_row();
}

pub(super) fn empty_as_dash(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        String::from("-")
    } else {
        trimmed.to_string()
    }
}

pub(super) fn published_urls(value: &str) -> Vec<String> {
    let mut urls = value
        .split(',')
        .filter_map(|mapping| {
            let left = mapping.split("->").next()?.trim();
            let host_port = left.rsplit(':').next()?.trim();
            if host_port.is_empty() || !host_port.chars().all(|ch| ch.is_ascii_digit()) {
                None
            } else {
                Some(format!("http://localhost:{host_port}/"))
            }
        })
        .collect::<Vec<_>>();
    urls.sort();
    urls.dedup();
    urls
}

pub(super) fn filtered_total_label(filtered: usize, total: usize) -> String {
    if filtered == total {
        format!("{total} total")
    } else {
        format!("{filtered} shown of {total}")
    }
}

pub(super) fn filtered_images(images: &[DockerImageInfo], filter: &str) -> Vec<DockerImageInfo> {
    let needle = normalize_filter(filter);
    images
        .iter()
        .filter(|image| {
            needle.is_empty()
                || matches_filter(
                    &[
                        &image.repository,
                        &image.tag,
                        &image.image_id,
                        &image.size,
                        &image.source,
                    ],
                    &needle,
                )
        })
        .cloned()
        .collect()
}

pub(super) fn filtered_containers(
    containers: &[ContainerInfo],
    filter: &str,
) -> Vec<ContainerInfo> {
    let needle = normalize_filter(filter);
    containers
        .iter()
        .filter(|container| {
            needle.is_empty()
                || matches_filter(
                    &[
                        &container.name,
                        &container.image,
                        &container.runtime,
                        &container.state,
                        &container.status,
                        &container.ports,
                    ],
                    &needle,
                )
        })
        .cloned()
        .collect()
}

pub(super) fn filtered_projects(projects: &[ProjectInfo], filter: &str) -> Vec<ProjectInfo> {
    let needle = normalize_filter(filter);
    projects
        .iter()
        .filter(|project| {
            needle.is_empty()
                || matches_filter(
                    &[
                        &project.name,
                        &project.status,
                        &project.config_files,
                        &project.working_dir,
                    ],
                    &needle,
                )
        })
        .cloned()
        .collect()
}

pub(super) fn filtered_volumes(volumes: &[VolumeInfo], filter: &str) -> Vec<VolumeInfo> {
    let needle = normalize_filter(filter);
    volumes
        .iter()
        .filter(|volume| {
            needle.is_empty()
                || matches_filter(
                    &[
                        &volume.name,
                        &volume.driver,
                        &volume.mountpoint,
                        &volume.scope,
                    ],
                    &needle,
                )
        })
        .cloned()
        .collect()
}

pub(super) fn filtered_networks(networks: &[NetworkInfo], filter: &str) -> Vec<NetworkInfo> {
    let needle = normalize_filter(filter);
    networks
        .iter()
        .filter(|network| {
            needle.is_empty()
                || matches_filter(
                    &[
                        &network.name,
                        &network.driver,
                        &network.subnet,
                        &network.gateway,
                        &network.scope,
                    ],
                    &needle,
                )
        })
        .cloned()
        .collect()
}

pub(super) fn matches_filter(fields: &[&str], needle: &str) -> bool {
    fields
        .iter()
        .any(|field| field.to_ascii_lowercase().contains(needle))
}

pub(super) fn normalize_filter(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

pub(super) fn default_ports_for_image(image: &str) -> &'static str {
    let image = image.to_ascii_lowercase();
    if image.contains("hello-world") || image.contains("alpine") || image.contains("busybox") {
        ""
    } else if image.contains("postgres") {
        "5432:5432"
    } else if image.contains("mysql") {
        "3306:3306"
    } else if image.contains("redis") {
        "6379:6379"
    } else if image.contains("nginx") {
        "8080:80"
    } else {
        ""
    }
}

pub(super) fn parse_requested_host_ports(value: &str) -> Vec<String> {
    value
        .split(',')
        .flat_map(|chunk| chunk.split_whitespace())
        .filter_map(|mapping| {
            let mapping = mapping.trim();
            if mapping.is_empty() {
                return None;
            }
            let parts = mapping.split(':').collect::<Vec<_>>();
            if parts.len() >= 2 {
                Some(parts[parts.len() - 2].to_string())
            } else {
                None
            }
        })
        .collect()
}

pub(super) fn used_host_ports(value: &str) -> Vec<String> {
    value
        .split(',')
        .filter_map(|mapping| {
            let left = mapping.split("->").next()?.trim();
            let port = left.rsplit(':').next()?.trim();
            if port.is_empty() || !port.chars().all(|ch| ch.is_ascii_digit()) {
                None
            } else {
                Some(port.to_string())
            }
        })
        .collect()
}

pub(super) fn state_color(state: &str) -> Color32 {
    match state.to_ascii_lowercase().as_str() {
        "running" => Color32::from_rgb(46, 204, 113),
        "exited" => Color32::from_rgb(231, 76, 60),
        "created" => Color32::from_rgb(241, 196, 15),
        _ => Color32::from_rgb(149, 165, 166),
    }
}

pub(super) fn primary_project_target(config_files: &str) -> String {
    config_files
        .split(',')
        .map(str::trim)
        .find(|value| !value.is_empty())
        .unwrap_or("")
        .to_string()
}
