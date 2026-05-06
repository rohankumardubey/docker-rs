mod helpers;
mod render;

use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

use eframe::egui;

use self::helpers::*;
use crate::engine::{
    self, ContainerDetailsInfo, ContainerInfo, ContainerStatsInfo, DockerImageInfo,
    EngineStatusInfo, ImageDetailsInfo, ProjectInfo, RuntimeStatusInfo, WorkerEvent,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkspaceTab {
    Home,
    Projects,
    Containers,
    Images,
    Build,
    Logs,
}

#[derive(Clone, Copy, Debug)]
enum ToastKind {
    Success,
    Error,
    Info,
}

#[derive(Clone, Debug)]
struct ToastMessage {
    message: String,
    kind: ToastKind,
    created_at: Instant,
}

pub struct DockerDesktopApp {
    event_sender: Sender<WorkerEvent>,
    event_receiver: Receiver<WorkerEvent>,
    engine_status: EngineStatusInfo,
    engine_ready: bool,
    runtime_status: RuntimeStatusInfo,
    runtime_ready: bool,
    workspace_tab: WorkspaceTab,
    page_history: Vec<WorkspaceTab>,
    running_task: Option<String>,
    toast: Option<ToastMessage>,
    pull_image_name: String,
    build_context: String,
    build_tag: String,
    dockerfile_path: String,
    run_image_name: String,
    run_container_name: String,
    run_ports: String,
    run_env: String,
    run_volumes: String,
    run_command: String,
    run_restart_policy: String,
    run_auto_remove: bool,
    run_ports_auto: bool,
    exec_command_input: String,
    logs: Vec<String>,
    images: Vec<DockerImageInfo>,
    projects: Vec<ProjectInfo>,
    containers: Vec<ContainerInfo>,
    image_filter: String,
    project_filter: String,
    container_filter: String,
    compose_target: String,
    compose_project_name: String,
    live_log_stream_container: Option<String>,
    live_log_stop: Option<Arc<AtomicBool>>,
    selected_project_name: Option<String>,
    selected_image_ref: Option<String>,
    selected_image_details: Option<ImageDetailsInfo>,
    selected_container_id: Option<String>,
    selected_container_details: Option<ContainerDetailsInfo>,
    selected_container_stats: Option<ContainerStatsInfo>,
}

impl DockerDesktopApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let (event_sender, event_receiver) = mpsc::channel();
        let app = Self {
            event_sender: event_sender.clone(),
            event_receiver,
            engine_status: EngineStatusInfo {
                summary: String::from("Starting native OCI engine..."),
                detail: String::from("Creating the local image store and builder runtime."),
                ..Default::default()
            },
            engine_ready: false,
            runtime_status: RuntimeStatusInfo {
                summary: String::from("Checking runtime bridge..."),
                detail: String::from("Container execution uses Docker Desktop for now."),
                native_ready: false,
                bridge_ready: false,
            },
            runtime_ready: false,
            workspace_tab: WorkspaceTab::Home,
            page_history: Vec::new(),
            running_task: None,
            toast: None,
            pull_image_name: String::from("nginx:latest"),
            build_context: String::new(),
            build_tag: String::from("docker-rs:dev"),
            dockerfile_path: String::new(),
            run_image_name: String::new(),
            run_container_name: String::new(),
            run_ports: String::new(),
            run_env: String::new(),
            run_volumes: String::new(),
            run_command: String::new(),
            run_restart_policy: String::new(),
            run_auto_remove: false,
            run_ports_auto: true,
            exec_command_input: String::from("uname -a"),
            logs: vec![String::from("Docker RS Desktop native engine started.")],
            images: Vec::new(),
            projects: Vec::new(),
            containers: Vec::new(),
            image_filter: String::new(),
            project_filter: String::new(),
            container_filter: String::new(),
            compose_target: String::new(),
            compose_project_name: String::new(),
            live_log_stream_container: None,
            live_log_stop: None,
            selected_project_name: None,
            selected_image_ref: None,
            selected_image_details: None,
            selected_container_id: None,
            selected_container_details: None,
            selected_container_stats: None,
        };

        engine::check_engine_status(event_sender.clone());
        engine::check_runtime_status(event_sender.clone());
        engine::refresh_images(event_sender);
        engine::refresh_projects(app.event_sender.clone());
        engine::refresh_containers(app.event_sender.clone());

        app
    }

    fn poll_events(&mut self) {
        while let Ok(event) = self.event_receiver.try_recv() {
            match event {
                WorkerEvent::EngineStatus(result) => match result {
                    Ok(info) => {
                        self.engine_ready = true;
                        self.engine_status = info;
                    }
                    Err(message) => {
                        self.engine_ready = false;
                        self.logs.push(format!("Engine startup failed: {message}"));
                        self.engine_status.summary = String::from("Native engine unavailable");
                        self.engine_status.detail = message;
                    }
                },
                WorkerEvent::ImageList(result) => match result {
                    Ok(images) => {
                        if let Some(selected_ref) = self.selected_image_ref.as_ref() {
                            if !images.iter().any(|image| {
                                format!("{}:{}", image.repository, image.tag) == *selected_ref
                            }) {
                                self.selected_image_ref = None;
                                self.selected_image_details = None;
                            }
                        }
                        self.images = images;
                    }
                    Err(message) => {
                        self.logs.push(format!("Image refresh failed: {message}"));
                    }
                },
                WorkerEvent::ProjectList(result) => match result {
                    Ok(projects) => {
                        if let Some(selected_name) = self.selected_project_name.as_ref() {
                            if !projects
                                .iter()
                                .any(|project| &project.name == selected_name)
                            {
                                self.selected_project_name = None;
                            }
                        }
                        self.projects = projects;
                    }
                    Err(message) => {
                        self.logs.push(format!("Project refresh failed: {message}"));
                    }
                },
                WorkerEvent::ImageDetails(result) => match result {
                    Ok(details) => {
                        self.selected_image_ref = Some(details.reference.clone());
                        self.selected_image_details = Some(details);
                    }
                    Err(message) => {
                        self.logs.push(format!("Image inspect failed: {message}"));
                    }
                },
                WorkerEvent::RuntimeStatus(result) => match result {
                    Ok(info) => {
                        self.runtime_ready = info.native_ready || info.bridge_ready;
                        self.runtime_status = info;
                    }
                    Err(message) => {
                        self.runtime_ready = false;
                        self.runtime_status.summary = String::from("Runtime unavailable");
                        self.runtime_status.detail = message;
                        self.runtime_status.native_ready = false;
                        self.runtime_status.bridge_ready = false;
                    }
                },
                WorkerEvent::ContainerList(result) => match result {
                    Ok(containers) => {
                        if let Some(selected_id) = self.selected_container_id.as_ref() {
                            if !containers
                                .iter()
                                .any(|container| &container.id == selected_id)
                            {
                                self.selected_container_id = None;
                                self.selected_container_details = None;
                                self.selected_container_stats = None;
                            }
                        }
                        self.containers = containers;
                    }
                    Err(message) => {
                        self.logs
                            .push(format!("Container refresh failed: {message}"));
                    }
                },
                WorkerEvent::ContainerDetails(result) => match result {
                    Ok(details) => {
                        self.selected_container_id = Some(details.id.clone());
                        self.selected_container_details = Some(details);
                    }
                    Err(message) => {
                        self.logs
                            .push(format!("Container inspect failed: {message}"));
                    }
                },
                WorkerEvent::ContainerStats(result) => match result {
                    Ok(stats) => {
                        self.selected_container_stats = Some(stats);
                    }
                    Err(message) => {
                        self.logs.push(format!("Container stats failed: {message}"));
                    }
                },
                WorkerEvent::LogLine(line) => {
                    self.logs.push(line);
                }
                WorkerEvent::ActionFinished(result) => {
                    self.running_task = None;
                    match result {
                        Ok(message) => {
                            self.show_toast(message.clone(), ToastKind::Success);
                            self.logs.push(message);
                            engine::refresh_images(self.event_sender.clone());
                            engine::refresh_projects(self.event_sender.clone());
                            engine::refresh_containers(self.event_sender.clone());
                            engine::check_runtime_status(self.event_sender.clone());
                            self.refresh_selected_image_details();
                            self.refresh_selected_container_details();
                            self.refresh_selected_container_stats();
                        }
                        Err(message) => {
                            self.show_toast(message.clone(), ToastKind::Error);
                            self.logs.push(message);
                            engine::refresh_images(self.event_sender.clone());
                            engine::refresh_projects(self.event_sender.clone());
                            engine::refresh_containers(self.event_sender.clone());
                            engine::check_runtime_status(self.event_sender.clone());
                            self.refresh_selected_image_details();
                            self.refresh_selected_container_details();
                            self.refresh_selected_container_stats();
                        }
                    }
                }
            }
        }
    }

    fn start_pull(&mut self) {
        let image = self.pull_image_name.trim().to_owned();
        if image.is_empty() {
            self.show_toast(
                String::from("Pull aborted: image name is required."),
                ToastKind::Error,
            );
            self.logs
                .push(String::from("Pull aborted: image name is required."));
            return;
        }

        self.running_task = Some(format!("Pulling {image}"));
        engine::pull_image(image, self.event_sender.clone());
    }

    fn start_build(&mut self) {
        let context = self.build_context.trim().to_owned();
        let tag = self.build_tag.trim().to_owned();
        let dockerfile = self.dockerfile_path.trim().to_owned();

        if context.is_empty() {
            self.show_toast(
                String::from("Build aborted: local folder or Git repo URL is required."),
                ToastKind::Error,
            );
            self.logs.push(String::from(
                "Build aborted: local folder or Git repo URL is required.",
            ));
            return;
        }

        if tag.is_empty() {
            self.show_toast(
                String::from("Build aborted: image tag is required."),
                ToastKind::Error,
            );
            self.logs
                .push(String::from("Build aborted: image tag is required."));
            return;
        }

        self.running_task = Some(format!("Building {tag}"));
        let dockerfile = if dockerfile.is_empty() {
            None
        } else {
            Some(dockerfile)
        };

        engine::build_image(context, tag, dockerfile, self.event_sender.clone());
    }

    fn refresh_images(&mut self) {
        engine::refresh_images(self.event_sender.clone());
    }

    fn refresh_projects(&mut self) {
        engine::refresh_projects(self.event_sender.clone());
    }

    fn refresh_engine(&mut self) {
        self.logs
            .push(String::from("Refreshing native OCI engine state..."));
        engine::check_engine_status(self.event_sender.clone());
        engine::refresh_images(self.event_sender.clone());
    }

    fn refresh_runtime(&mut self) {
        self.logs
            .push(String::from("Refreshing runtime bridge state..."));
        engine::check_runtime_status(self.event_sender.clone());
        engine::refresh_containers(self.event_sender.clone());
    }

    fn navigate_to(&mut self, tab: WorkspaceTab) {
        if self.workspace_tab != tab {
            self.page_history.push(self.workspace_tab);
            if self.page_history.len() > 24 {
                let overflow = self.page_history.len() - 24;
                self.page_history.drain(0..overflow);
            }
            self.workspace_tab = tab;
        }
    }

    fn navigate_back(&mut self) {
        if let Some(previous) = self.page_history.pop() {
            self.workspace_tab = previous;
        }
    }

    fn refresh_current_page(&mut self) {
        match self.workspace_tab {
            WorkspaceTab::Home => {
                self.refresh_engine();
                self.refresh_images();
                self.refresh_projects();
                self.refresh_runtime();
            }
            WorkspaceTab::Projects => {
                self.refresh_projects();
                self.refresh_runtime();
            }
            WorkspaceTab::Containers => self.refresh_runtime(),
            WorkspaceTab::Images => self.refresh_images(),
            WorkspaceTab::Build => {
                self.refresh_engine();
                self.refresh_images();
                self.refresh_projects();
                self.refresh_runtime();
            }
            WorkspaceTab::Logs => self.refresh_runtime(),
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.wants_keyboard_input() {
            return;
        }

        let mut go_home = false;
        let mut go_projects = false;
        let mut go_containers = false;
        let mut go_images = false;
        let mut go_build = false;
        let mut go_logs = false;
        let mut go_back = false;
        let mut refresh = false;

        ctx.input(|input| {
            let command = input.modifiers.command;
            if command && input.key_pressed(egui::Key::Num1) {
                go_home = true;
            }
            if command && input.key_pressed(egui::Key::Num2) {
                go_projects = true;
            }
            if command && input.key_pressed(egui::Key::Num3) {
                go_containers = true;
            }
            if command && input.key_pressed(egui::Key::Num4) {
                go_images = true;
            }
            if command && input.key_pressed(egui::Key::Num5) {
                go_build = true;
            }
            if command && input.key_pressed(egui::Key::Num6) {
                go_logs = true;
            }
            if input.key_pressed(egui::Key::Escape) {
                go_back = true;
            }
            if command && input.key_pressed(egui::Key::R) {
                refresh = true;
            }
        });

        if go_back && !self.page_history.is_empty() {
            self.navigate_back();
        }
        if go_home {
            self.navigate_to(WorkspaceTab::Home);
        }
        if go_projects {
            self.navigate_to(WorkspaceTab::Projects);
        }
        if go_containers {
            self.navigate_to(WorkspaceTab::Containers);
        }
        if go_images {
            self.navigate_to(WorkspaceTab::Images);
        }
        if go_build {
            self.navigate_to(WorkspaceTab::Build);
        }
        if go_logs {
            self.navigate_to(WorkspaceTab::Logs);
        }
        if refresh {
            self.refresh_current_page();
            self.show_toast(
                format!("Refreshed {}.", workspace_title(self.workspace_tab)),
                ToastKind::Info,
            );
        }
    }

    fn start_runtime(&mut self) {
        self.running_task = Some(String::from("Starting Docker runtime bridge"));
        engine::start_runtime(self.event_sender.clone());
    }

    fn effective_project_target(&self) -> Option<String> {
        let target = self.compose_target.trim();
        if !target.is_empty() {
            return Some(target.to_string());
        }

        self.selected_project_name
            .as_ref()
            .and_then(|name| self.projects.iter().find(|project| &project.name == name))
            .map(|project| primary_project_target(&project.config_files))
            .filter(|target| !target.is_empty())
    }

    fn effective_project_name(&self) -> Option<String> {
        let name = self.compose_project_name.trim();
        if !name.is_empty() {
            Some(name.to_string())
        } else {
            self.selected_project_name.clone()
        }
    }

    fn select_project(&mut self, project: &ProjectInfo) {
        self.selected_project_name = Some(project.name.clone());
        self.compose_project_name = project.name.clone();
        let target = primary_project_target(&project.config_files);
        if !target.is_empty() {
            self.compose_target = target;
        }
    }

    fn start_compose_up(&mut self) {
        let Some(target) = self.effective_project_target() else {
            self.show_toast(
                String::from("Compose up aborted: choose a compose file or project folder."),
                ToastKind::Error,
            );
            self.logs.push(String::from(
                "Compose up aborted: choose a compose file or project folder.",
            ));
            return;
        };
        let project_name = self.effective_project_name();
        self.running_task = Some(format!(
            "Starting compose project {}",
            project_name.clone().unwrap_or_else(|| target.clone())
        ));
        engine::compose_project_up(target, project_name, self.event_sender.clone());
    }

    fn start_compose_down(&mut self) {
        let Some(target) = self.effective_project_target() else {
            self.show_toast(
                String::from("Compose down aborted: choose a compose file or project folder."),
                ToastKind::Error,
            );
            self.logs.push(String::from(
                "Compose down aborted: choose a compose file or project folder.",
            ));
            return;
        };
        let project_name = self.effective_project_name();
        self.running_task = Some(format!(
            "Stopping compose project {}",
            project_name.clone().unwrap_or_else(|| target.clone())
        ));
        engine::compose_project_down(target, project_name, self.event_sender.clone());
    }

    fn fetch_compose_logs(&mut self) {
        let Some(target) = self.effective_project_target() else {
            self.show_toast(
                String::from("Compose logs aborted: choose a compose file or project folder."),
                ToastKind::Error,
            );
            self.logs.push(String::from(
                "Compose logs aborted: choose a compose file or project folder.",
            ));
            return;
        };
        let project_name = self.effective_project_name();
        self.navigate_to(WorkspaceTab::Logs);
        self.running_task = Some(format!(
            "Fetching compose logs for {}",
            project_name.clone().unwrap_or_else(|| target.clone())
        ));
        engine::fetch_project_logs(target, project_name, self.event_sender.clone());
    }

    fn start_run(&mut self) {
        let image = self.run_image_name.trim().to_owned();
        if image.is_empty() {
            self.show_toast(
                String::from("Run aborted: image name is required."),
                ToastKind::Error,
            );
            self.logs
                .push(String::from("Run aborted: image name is required."));
            return;
        }

        self.running_task = Some(format!("Running {image}"));
        let container_name = if self.run_container_name.trim().is_empty() {
            None
        } else {
            Some(self.run_container_name.trim().to_owned())
        };
        let ports = if self.run_ports.trim().is_empty() {
            None
        } else {
            Some(self.run_ports.trim().to_owned())
        };
        let env_vars = if self.run_env.trim().is_empty() {
            None
        } else {
            Some(self.run_env.trim().to_owned())
        };
        let volume_mounts = if self.run_volumes.trim().is_empty() {
            None
        } else {
            Some(self.run_volumes.trim().to_owned())
        };
        let command_override = if self.run_command.trim().is_empty() {
            None
        } else {
            Some(self.run_command.trim().to_owned())
        };
        let restart_policy = if self.run_restart_policy.trim().is_empty() {
            None
        } else {
            Some(self.run_restart_policy.trim().to_owned())
        };
        if self.run_auto_remove && restart_policy.is_some() {
            self.show_toast(
                String::from(
                    "Run aborted: auto-remove (`--rm`) cannot be combined with a restart policy.",
                ),
                ToastKind::Error,
            );
            self.logs.push(String::from(
                "Run aborted: auto-remove (`--rm`) cannot be combined with a restart policy.",
            ));
            return;
        }

        engine::run_container(
            image,
            container_name,
            ports,
            env_vars,
            volume_mounts,
            command_override,
            restart_policy,
            self.run_auto_remove,
            self.event_sender.clone(),
        );
    }

    fn start_run_from_image(&mut self, image: String) {
        self.apply_run_defaults_for_image(&image);
        self.navigate_to(WorkspaceTab::Build);
        self.start_run();
    }

    fn prefill_run_from_image(&mut self, image: String) {
        self.apply_run_defaults_for_image(&image);
        self.navigate_to(WorkspaceTab::Build);
    }

    fn start_container_action(&mut self, container_id: String) {
        self.running_task = Some(format!(
            "Starting container {}",
            shorten_container_id(&container_id)
        ));
        engine::start_container(container_id, self.event_sender.clone());
    }

    fn stop_container_action(&mut self, container_id: String) {
        self.running_task = Some(format!(
            "Stopping container {}",
            shorten_container_id(&container_id)
        ));
        engine::stop_container(container_id, self.event_sender.clone());
    }

    fn logs_container_action(&mut self, container_id: String) {
        self.navigate_to(WorkspaceTab::Logs);
        self.running_task = Some(format!(
            "Fetching logs for {}",
            shorten_container_id(&container_id)
        ));
        engine::fetch_container_logs(container_id, self.event_sender.clone());
    }

    fn follow_logs_action(&mut self, container_id: String) {
        self.navigate_to(WorkspaceTab::Logs);
        self.stop_live_logs_internal(false);
        let stop_flag = Arc::new(AtomicBool::new(false));
        self.live_log_stream_container = Some(container_id.clone());
        self.live_log_stop = Some(stop_flag.clone());
        self.logs.push(format!(
            "Starting live log stream for {}...",
            shorten_container_id(&container_id)
        ));
        engine::follow_container_logs(container_id, stop_flag, self.event_sender.clone());
    }

    fn stop_live_logs_action(&mut self) {
        self.stop_live_logs_internal(true);
    }

    fn stop_live_logs_internal(&mut self, log_message: bool) {
        if let Some(stop_flag) = self.live_log_stop.take() {
            stop_flag.store(true, Ordering::Relaxed);
        }
        if let Some(container_id) = self.live_log_stream_container.take() {
            if log_message {
                self.logs.push(format!(
                    "Stopping live log stream for {}...",
                    shorten_container_id(&container_id)
                ));
            }
        }
    }

    fn restart_container_action(&mut self, container_id: String) {
        self.running_task = Some(format!(
            "Restarting container {}",
            shorten_container_id(&container_id)
        ));
        engine::restart_container(container_id, self.event_sender.clone());
    }

    fn remove_image_action(&mut self, image: String) {
        self.running_task = Some(format!("Removing image {image}"));
        if self.selected_image_ref.as_deref() == Some(image.as_str()) {
            self.selected_image_ref = None;
            self.selected_image_details = None;
        }
        engine::remove_image(image, self.event_sender.clone());
    }

    fn remove_container_action(&mut self, container_id: String) {
        self.running_task = Some(format!(
            "Removing container {}",
            shorten_container_id(&container_id)
        ));
        if self.selected_container_id.as_deref() == Some(container_id.as_str()) {
            self.selected_container_id = None;
            self.selected_container_details = None;
        }
        engine::remove_container(container_id, self.event_sender.clone());
    }

    fn inspect_container_action(&mut self, container_id: String) {
        self.navigate_to(WorkspaceTab::Containers);
        self.running_task = Some(format!(
            "Inspecting container {}",
            shorten_container_id(&container_id)
        ));
        engine::inspect_container(container_id, self.event_sender.clone());
    }

    fn exec_container_action(&mut self, container_id: String) {
        let command = self.exec_command_input.trim().to_owned();
        if command.is_empty() {
            self.show_toast(
                String::from("Exec aborted: command is required."),
                ToastKind::Error,
            );
            self.logs
                .push(String::from("Exec aborted: command is required."));
            return;
        }

        self.running_task = Some(format!(
            "Executing in {}",
            shorten_container_id(&container_id)
        ));
        engine::exec_in_container(container_id, command, self.event_sender.clone());
    }

    fn refresh_selected_container_details(&mut self) {
        if let Some(container_id) = self.selected_container_id.clone() {
            engine::inspect_container(container_id, self.event_sender.clone());
        }
    }

    fn refresh_selected_container_stats(&mut self) {
        if let Some(container_id) = self.selected_container_id.clone() {
            engine::inspect_container_stats(container_id, self.event_sender.clone());
        }
    }

    fn refresh_container_stats_action(&mut self, container_id: String) {
        self.running_task = Some(format!(
            "Refreshing stats for {}",
            shorten_container_id(&container_id)
        ));
        engine::inspect_container_stats(container_id, self.event_sender.clone());
    }

    fn inspect_image_action(&mut self, image: String) {
        self.navigate_to(WorkspaceTab::Images);
        self.running_task = Some(format!("Inspecting image {image}"));
        engine::inspect_image(image, self.event_sender.clone());
    }

    fn refresh_selected_image_details(&mut self) {
        if let Some(image) = self.selected_image_ref.clone() {
            engine::inspect_image(image, self.event_sender.clone());
        }
    }

    fn open_url_action(&mut self, url: String) {
        match Command::new("open").arg(&url).output() {
            Ok(output) if output.status.success() => {
                self.show_toast(format!("Opened {url}"), ToastKind::Info);
                self.logs.push(format!("Opened {url}"));
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                let message = if stderr.is_empty() {
                    format!("Failed to open {url}")
                } else {
                    format!("Failed to open {url}: {stderr}")
                };
                self.show_toast(message.clone(), ToastKind::Error);
                self.logs.push(message);
            }
            Err(err) => {
                self.show_toast(format!("Failed to open {url}: {err}"), ToastKind::Error);
                self.logs.push(format!("Failed to open {url}: {err}"));
            }
        }
    }

    fn show_toast(&mut self, message: String, kind: ToastKind) {
        self.toast = Some(ToastMessage {
            message,
            kind,
            created_at: Instant::now(),
        });
    }

    fn apply_run_defaults_for_image(&mut self, image: &str) {
        self.run_image_name = image.to_string();
        self.run_container_name.clear();
        self.run_ports = default_ports_for_image(image).to_string();
        self.run_env.clear();
        self.run_volumes.clear();
        self.run_command.clear();
        self.run_restart_policy.clear();
        self.run_auto_remove = false;
        self.run_ports_auto = true;
    }

    fn port_conflict_hint(&self) -> Option<String> {
        let requested_ports = parse_requested_host_ports(&self.run_ports);
        if requested_ports.is_empty() {
            return None;
        }

        let conflicts = self
            .containers
            .iter()
            .flat_map(|container| {
                used_host_ports(&container.ports)
                    .into_iter()
                    .map(move |port| (port, container))
            })
            .filter(|(port, _)| requested_ports.contains(port))
            .map(|(port, container)| format!("{port} ({})", container.name))
            .collect::<Vec<_>>();

        if conflicts.is_empty() {
            None
        } else {
            Some(format!(
                "Port conflict warning: {} already in use.",
                conflicts.join(", ")
            ))
        }
    }
}

impl eframe::App for DockerDesktopApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_events();
        self.handle_shortcuts(ctx);
        ctx.request_repaint_after(std::time::Duration::from_millis(150));

        egui::TopBottomPanel::top("header_panel").show(ctx, |ui| {
            self.render_toolbar(ui);
        });

        egui::SidePanel::left("sidebar_panel")
            .resizable(false)
            .min_width(220.0)
            .show(ctx, |ui| {
                self.render_sidebar(ui);
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_workspace(ui);
        });

        self.render_toast_overlay(ctx);
    }
}
