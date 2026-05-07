use std::time::Duration;

use eframe::egui::{self, Align, Color32, Layout, RichText, ScrollArea, TextEdit};
use rfd::FileDialog;

use super::*;

impl DockerDesktopApp {
    fn render_pull_card(&mut self, ui: &mut egui::Ui) {
        ui.heading("Pull Image");
        ui.label("Fetch an OCI image directly into the native local store without Docker.");
        ui.add_space(8.0);
        ui.add(
            TextEdit::singleline(&mut self.pull_image_name)
                .hint_text("ubuntu:24.04 or ghcr.io/org/image:tag"),
        );
        ui.add_space(8.0);
        if ui.button("Pull Image").clicked() {
            self.start_pull();
        }
    }

    fn render_build_card(&mut self, ui: &mut egui::Ui) {
        ui.heading("Build Image");
        ui.label("Build from a local project folder or a remote Git repository URL.");
        ui.add_space(8.0);
        ui.add(
            TextEdit::singleline(&mut self.build_context)
                .hint_text("/path/to/repo or https://github.com/org/repo.git"),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Choose Folder").clicked() {
                if let Some(path) = FileDialog::new().pick_folder() {
                    self.build_context = path.display().to_string();
                }
            }
            ui.label("or paste a Git URL above");
        });
        ui.add_space(8.0);
        ui.add(TextEdit::singleline(&mut self.build_tag).hint_text("my-app:latest"));
        ui.add_space(8.0);
        ui.add(
            TextEdit::singleline(&mut self.dockerfile_path)
                .hint_text("Optional Dockerfile path, e.g. ./Dockerfile"),
        );
        ui.add_space(8.0);
        ui.small(
            "Native builder currently supports FROM, ENV, LABEL, EXPOSE, USER, WORKDIR, CMD, and ENTRYPOINT. RUN, COPY, and multi-stage builds are the next engine step.",
        );
        ui.add_space(8.0);
        if ui.button("Build Image").clicked() {
            self.start_build();
        }
    }

    fn render_run_card(&mut self, ui: &mut egui::Ui) {
        ui.heading("Run Container");
        ui.label(
            "Launch a container through the native runtime first, with Docker fallback for Linux-container compatibility on macOS.",
        );
        ui.add_space(8.0);
        let previous_image = self.run_image_name.clone();
        let image_response = ui
            .add(TextEdit::singleline(&mut self.run_image_name).hint_text("library/nginx:latest"));
        if image_response.changed() && self.run_ports_auto {
            self.run_ports = default_ports_for_image(&self.run_image_name).to_string();
        }
        if image_response.changed() && previous_image != self.run_image_name {
            self.run_container_name.clear();
        }
        ui.add_space(8.0);
        ui.add(
            TextEdit::singleline(&mut self.run_container_name).hint_text("Optional container name"),
        );
        ui.add_space(8.0);
        let ports_response = ui.add(
            TextEdit::singleline(&mut self.run_ports)
                .hint_text("Optional ports, e.g. 8080:80, 8443:443"),
        );
        if ports_response.changed() {
            self.run_ports_auto = false;
        }
        ui.add_space(8.0);
        let suggested_ports = default_ports_for_image(&self.run_image_name);
        if suggested_ports.is_empty() {
            ui.small("Suggested ports: none for this image.");
        } else {
            ui.small(format!("Suggested ports: {suggested_ports}"));
        }
        if let Some(warning) = self.port_conflict_hint() {
            ui.colored_label(Color32::from_rgb(241, 196, 15), warning);
        }
        ui.add_space(8.0);
        ui.label("Environment");
        ui.add(
            TextEdit::multiline(&mut self.run_env)
                .desired_rows(3)
                .hint_text("KEY=value\nLOG_LEVEL=debug"),
        );
        ui.small("Use one environment variable per line.");
        ui.add_space(8.0);
        ui.label("Volume Mounts");
        ui.add(
            TextEdit::multiline(&mut self.run_volumes)
                .desired_rows(3)
                .hint_text("/absolute/host/path:/container/path\n./data:/var/lib/app:ro"),
        );
        ui.small("Use one bind mount per line in `host:container[:ro]` form.");
        ui.add_space(8.0);
        ui.label("Command Override");
        ui.add(
            TextEdit::singleline(&mut self.run_command)
                .hint_text("Optional shell command, e.g. nginx -g 'daemon off;'"),
        );
        ui.small("When set, the selected runtime backend runs this through `sh -lc`.");
        ui.add_space(8.0);
        ui.label("Restart Policy");
        ui.add(
            TextEdit::singleline(&mut self.run_restart_policy)
                .hint_text("Optional: no, on-failure, unless-stopped, or always"),
        );
        ui.small("Leave this blank for Docker's default behavior.");
        ui.add_space(8.0);
        ui.checkbox(
            &mut self.run_auto_remove,
            "Auto-remove container on exit (`--rm`)",
        );
        if self.run_auto_remove {
            ui.small(
                "Best for short-lived jobs. Disable it for long-running services you want to restart.",
            );
        }
        if self.run_auto_remove && !self.run_restart_policy.trim().is_empty() {
            ui.colored_label(
                Color32::from_rgb(231, 76, 60),
                "Auto-remove and restart policy cannot be used together.",
            );
        }
        ui.add_space(8.0);
        ui.small("The native runtime prototype executes host processes directly. Docker is used only as a fallback when a Linux image cannot execute natively on macOS yet.");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.runtime_ready, egui::Button::new("Run Container"))
                .clicked()
            {
                self.start_run();
            }
            if !self.runtime_status.bridge_ready && self.running_task.is_none() {
                if ui.button("Start Docker Fallback").clicked() {
                    self.start_runtime();
                }
            }
        });
    }

    fn render_logs_panel(&mut self, ui: &mut egui::Ui) {
        let max_height = (ui.available_height() - 24.0).max(220.0);
        ui.horizontal(|ui| {
            ui.heading("Activity");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Clear").clicked() {
                    self.logs.clear();
                }
            });
        });
        ui.label("Build and pull logs stream here in real time.");
        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ScrollArea::vertical()
                .id_salt("activity_scroll")
                .max_height(max_height)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &self.logs {
                        ui.monospace(line);
                    }
                });
        });
    }

    fn render_images_table(&mut self, ui: &mut egui::Ui) {
        let max_height = (ui.available_height() - 88.0).max(220.0);
        ui.horizontal(|ui| {
            ui.heading("Local Images");
            ui.label(filtered_total_label(
                filtered_images(&self.images, &self.image_filter).len(),
                self.images.len(),
            ));
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(
                TextEdit::singleline(&mut self.image_filter)
                    .hint_text("repo, tag, image id, size, or source"),
            );
            if ui.button("Clear").clicked() {
                self.image_filter.clear();
            }
        });
        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ScrollArea::vertical()
                .id_salt("images_scroll")
                .max_height(max_height)
                .show(ui, |ui| {
                    egui::Grid::new("images_grid")
                        .striped(true)
                        .min_col_width(80.0)
                        .show(ui, |ui| {
                            ui.strong("Repository");
                            ui.strong("Tag");
                            ui.strong("Image ID");
                            ui.strong("Size");
                            ui.strong("Source");
                            ui.strong("Actions");
                            ui.end_row();

                            let images = filtered_images(&self.images, &self.image_filter);
                            for image in images {
                                let image_ref = format!("{}:{}", image.repository, image.tag);
                                ui.label(&image.repository);
                                ui.label(&image.tag);
                                ui.monospace(&image.image_id);
                                ui.label(&image.size);
                                ui.label(&image.source);
                                ui.horizontal(|ui| {
                                    if ui
                                        .add_enabled(
                                            self.running_task.is_none(),
                                            egui::Button::new("Inspect"),
                                        )
                                        .clicked()
                                    {
                                        self.inspect_image_action(image_ref.clone());
                                    }
                                    if ui
                                        .add_enabled(
                                            self.running_task.is_none(),
                                            egui::Button::new("Run"),
                                        )
                                        .clicked()
                                    {
                                        self.start_run_from_image(image_ref.clone());
                                    }
                                    ui.menu_button("More", |ui| {
                                        if ui.button("Use").clicked() {
                                            self.prefill_run_from_image(image_ref.clone());
                                            ui.close_menu();
                                        }
                                        if ui
                                            .add_enabled(
                                                self.running_task.is_none(),
                                                egui::Button::new("Delete"),
                                            )
                                            .clicked()
                                        {
                                            self.remove_image_action(image_ref.clone());
                                            ui.close_menu();
                                        }
                                    });
                                });
                                ui.end_row();
                            }
                        });
                });
        });
    }

    fn render_projects_panel(&mut self, ui: &mut egui::Ui) {
        let max_height = (ui.available_height() - 360.0).max(180.0);
        ui.horizontal(|ui| {
            ui.heading("Compose Projects");
            ui.label(filtered_total_label(
                filtered_projects(&self.projects, &self.project_filter).len(),
                self.projects.len(),
            ));
        });
        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.strong("Open Compose Project");
            ui.add_space(8.0);
            ui.add(
                TextEdit::singleline(&mut self.compose_target)
                    .hint_text("/path/to/docker-compose.yml or /path/to/project"),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Choose Compose File").clicked() {
                    if let Some(path) = FileDialog::new()
                        .add_filter("Compose", &["yml", "yaml"])
                        .pick_file()
                    {
                        self.compose_target = path.display().to_string();
                    }
                }
                if ui.button("Choose Project Folder").clicked() {
                    if let Some(path) = FileDialog::new().pick_folder() {
                        self.compose_target = path.display().to_string();
                    }
                }
            });
            ui.add_space(8.0);
            ui.add(
                TextEdit::singleline(&mut self.compose_project_name)
                    .hint_text("Optional project name override"),
            );
            ui.small("Leave the project name blank to use Docker Compose defaults.");
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(self.running_task.is_none(), egui::Button::new("Up"))
                    .clicked()
                {
                    self.start_compose_up();
                }
                if ui
                    .add_enabled(self.running_task.is_none(), egui::Button::new("Down"))
                    .clicked()
                {
                    self.start_compose_down();
                }
                if ui
                    .add_enabled(self.running_task.is_none(), egui::Button::new("Logs"))
                    .clicked()
                {
                    self.fetch_compose_logs();
                }
                if ui.button("Clear").clicked() {
                    self.compose_target.clear();
                    self.compose_project_name.clear();
                    self.selected_project_name = None;
                }
            });
        });

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(
                TextEdit::singleline(&mut self.project_filter)
                    .hint_text("name, status, config path, or working dir"),
            );
            if ui.button("Clear").clicked() {
                self.project_filter.clear();
            }
        });
        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ScrollArea::vertical()
                .id_salt("projects_scroll")
                .max_height(max_height)
                .show(ui, |ui| {
                    egui::Grid::new("projects_grid")
                        .striped(true)
                        .min_col_width(80.0)
                        .show(ui, |ui| {
                            ui.strong("Name");
                            ui.strong("Status");
                            ui.strong("Config");
                            ui.strong("Actions");
                            ui.end_row();

                            let projects = filtered_projects(&self.projects, &self.project_filter);
                            for project in projects {
                                let selected = self.selected_project_name.as_deref()
                                    == Some(project.name.as_str());
                                if selected {
                                    ui.colored_label(
                                        Color32::from_rgb(52, 152, 219),
                                        RichText::new(&project.name).strong(),
                                    );
                                } else {
                                    ui.label(&project.name);
                                }
                                ui.label(&project.status);
                                ui.label(compact_path(&project.config_files));
                                ui.horizontal(|ui| {
                                    if ui.button("Select").clicked() {
                                        self.select_project(&project);
                                    }
                                    if ui
                                        .add_enabled(
                                            self.running_task.is_none(),
                                            egui::Button::new("Up"),
                                        )
                                        .clicked()
                                    {
                                        self.select_project(&project);
                                        self.start_compose_up();
                                    }
                                    ui.menu_button("More", |ui| {
                                        if ui
                                            .add_enabled(
                                                self.running_task.is_none(),
                                                egui::Button::new("Down"),
                                            )
                                            .clicked()
                                        {
                                            self.select_project(&project);
                                            self.start_compose_down();
                                            ui.close_menu();
                                        }
                                        if ui
                                            .add_enabled(
                                                self.running_task.is_none(),
                                                egui::Button::new("Logs"),
                                            )
                                            .clicked()
                                        {
                                            self.select_project(&project);
                                            self.fetch_compose_logs();
                                            ui.close_menu();
                                        }
                                    });
                                });
                                ui.end_row();
                            }
                        });
                });
        });
    }

    fn render_volumes_panel(&mut self, ui: &mut egui::Ui) {
        let max_height = (ui.available_height() - 280.0).max(180.0);
        ui.horizontal(|ui| {
            ui.heading("Volumes");
            ui.label(filtered_total_label(
                filtered_volumes(&self.volumes, &self.volume_filter).len(),
                self.volumes.len(),
            ));
        });
        ui.add_space(8.0);
        ui.colored_label(
            Color32::from_rgb(46, 204, 113),
            "Volumes are stored natively inside Docker RS Desktop.",
        );
        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.strong("Create Volume");
            ui.add_space(8.0);
            ui.add(TextEdit::singleline(&mut self.volume_name_input).hint_text("my-volume"));
            ui.add_space(6.0);
            ui.add(
                TextEdit::singleline(&mut self.volume_driver_input).hint_text("Driver, e.g. local"),
            );
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        self.running_task.is_none(),
                        egui::Button::new("Create Volume"),
                    )
                    .clicked()
                {
                    self.create_volume_action();
                }
                if ui.button("Clear").clicked() {
                    self.volume_name_input.clear();
                    self.volume_driver_input = String::from("local");
                }
            });
        });

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(
                TextEdit::singleline(&mut self.volume_filter)
                    .hint_text("name, driver, mountpoint, or scope"),
            );
            if ui.button("Clear").clicked() {
                self.volume_filter.clear();
            }
        });
        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ScrollArea::vertical()
                .id_salt("volumes_scroll")
                .max_height(max_height)
                .show(ui, |ui| {
                    egui::Grid::new("volumes_grid")
                        .striped(true)
                        .min_col_width(80.0)
                        .show(ui, |ui| {
                            ui.strong("Name");
                            ui.strong("Driver");
                            ui.strong("Mountpoint");
                            ui.strong("Scope");
                            ui.strong("Actions");
                            ui.end_row();

                            let volumes = filtered_volumes(&self.volumes, &self.volume_filter);
                            for volume in volumes {
                                let selected = self.selected_volume_name.as_deref()
                                    == Some(volume.name.as_str());
                                if selected {
                                    ui.colored_label(
                                        Color32::from_rgb(52, 152, 219),
                                        RichText::new(&volume.name).strong(),
                                    );
                                } else {
                                    ui.label(&volume.name);
                                }
                                ui.label(&volume.driver);
                                ui.label(compact_path(&volume.mountpoint));
                                ui.label(&volume.scope);
                                ui.horizontal(|ui| {
                                    if ui
                                        .add_enabled(
                                            self.running_task.is_none(),
                                            egui::Button::new("Inspect"),
                                        )
                                        .clicked()
                                    {
                                        self.inspect_volume_action(volume.name.clone());
                                    }
                                    ui.menu_button("More", |ui| {
                                        if ui
                                            .add_enabled(
                                                self.running_task.is_none(),
                                                egui::Button::new("Delete"),
                                            )
                                            .clicked()
                                        {
                                            self.remove_volume_action(volume.name.clone());
                                            ui.close_menu();
                                        }
                                    });
                                });
                                ui.end_row();
                            }
                        });
                });
        });
    }

    fn render_volume_details(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Volume Details");
            if let Some(details) = self.selected_volume_details.as_ref() {
                let volume_name = details.name.clone();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(self.running_task.is_none(), egui::Button::new("Delete"))
                        .clicked()
                    {
                        self.remove_volume_action(volume_name.clone());
                    }
                    if ui
                        .add_enabled(
                            self.running_task.is_none(),
                            egui::Button::new("Refresh Details"),
                        )
                        .clicked()
                    {
                        self.inspect_volume_action(volume_name.clone());
                    }
                });
            }
        });
        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            if let Some(details) = self.selected_volume_details.as_ref() {
                egui::Grid::new("volume_details_meta")
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        detail_row(ui, "Name", &details.name);
                        detail_row(ui, "Driver", &details.driver);
                        detail_row(ui, "Mountpoint", &details.mountpoint);
                        detail_row(ui, "Scope", &details.scope);
                        detail_row(ui, "Created", &details.created_at);
                    });

                ui.add_space(12.0);
                ui.heading("Labels");
                if details.labels.is_empty() {
                    ui.label("No labels reported.");
                } else {
                    ScrollArea::vertical()
                        .id_salt("volume_labels_scroll")
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for (key, value) in &details.labels {
                                ui.horizontal_wrapped(|ui| {
                                    ui.monospace(key);
                                    ui.label(value);
                                });
                            }
                        });
                }

                ui.add_space(12.0);
                ui.heading("Options");
                if details.options.is_empty() {
                    ui.label("No driver options reported.");
                } else {
                    ScrollArea::vertical()
                        .id_salt("volume_options_scroll")
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for (key, value) in &details.options {
                                ui.horizontal_wrapped(|ui| {
                                    ui.monospace(key);
                                    ui.label(value);
                                });
                            }
                        });
                }
            } else {
                ui.label("Choose `Inspect` on a volume row to load full details here.");
            }
        });
    }

    fn render_networks_panel(&mut self, ui: &mut egui::Ui) {
        let max_height = (ui.available_height() - 280.0).max(180.0);
        ui.horizontal(|ui| {
            ui.heading("Networks");
            ui.label(filtered_total_label(
                filtered_networks(&self.networks, &self.network_filter).len(),
                self.networks.len(),
            ));
        });
        ui.add_space(8.0);
        ui.colored_label(
            Color32::from_rgb(46, 204, 113),
            "Networks are stored natively inside Docker RS Desktop.",
        );
        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.strong("Create Network");
            ui.add_space(8.0);
            ui.add(TextEdit::singleline(&mut self.network_name_input).hint_text("app-network"));
            ui.add_space(6.0);
            ui.add(
                TextEdit::singleline(&mut self.network_driver_input)
                    .hint_text("Driver, e.g. bridge"),
            );
            ui.add_space(6.0);
            ui.add(
                TextEdit::singleline(&mut self.network_subnet_input)
                    .hint_text("Subnet, e.g. 172.30.0.0/24"),
            );
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        self.running_task.is_none(),
                        egui::Button::new("Create Network"),
                    )
                    .clicked()
                {
                    self.create_network_action();
                }
                if ui.button("Clear").clicked() {
                    self.network_name_input.clear();
                    self.network_driver_input = String::from("bridge");
                    self.network_subnet_input = String::from("172.30.0.0/24");
                }
            });
        });

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(
                TextEdit::singleline(&mut self.network_filter)
                    .hint_text("name, driver, subnet, gateway, or scope"),
            );
            if ui.button("Clear").clicked() {
                self.network_filter.clear();
            }
        });
        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ScrollArea::vertical()
                .id_salt("networks_scroll")
                .max_height(max_height)
                .show(ui, |ui| {
                    egui::Grid::new("networks_grid")
                        .striped(true)
                        .min_col_width(72.0)
                        .show(ui, |ui| {
                            ui.strong("Name");
                            ui.strong("Driver");
                            ui.strong("Subnet");
                            ui.strong("Gateway");
                            ui.strong("Scope");
                            ui.strong("Actions");
                            ui.end_row();

                            let networks = filtered_networks(&self.networks, &self.network_filter);
                            for network in networks {
                                let selected = self.selected_network_name.as_deref()
                                    == Some(network.name.as_str());
                                if selected {
                                    ui.colored_label(
                                        Color32::from_rgb(52, 152, 219),
                                        RichText::new(&network.name).strong(),
                                    );
                                } else {
                                    ui.label(&network.name);
                                }
                                ui.label(&network.driver);
                                ui.label(&network.subnet);
                                ui.label(&network.gateway);
                                ui.label(&network.scope);
                                ui.horizontal(|ui| {
                                    if ui
                                        .add_enabled(
                                            self.running_task.is_none(),
                                            egui::Button::new("Inspect"),
                                        )
                                        .clicked()
                                    {
                                        self.inspect_network_action(network.name.clone());
                                    }
                                    ui.menu_button("More", |ui| {
                                        if ui
                                            .add_enabled(
                                                self.running_task.is_none(),
                                                egui::Button::new("Delete"),
                                            )
                                            .clicked()
                                        {
                                            self.remove_network_action(network.name.clone());
                                            ui.close_menu();
                                        }
                                    });
                                });
                                ui.end_row();
                            }
                        });
                });
        });
    }

    fn render_network_details(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Network Details");
            if let Some(details) = self.selected_network_details.as_ref() {
                let network_name = details.name.clone();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(self.running_task.is_none(), egui::Button::new("Delete"))
                        .clicked()
                    {
                        self.remove_network_action(network_name.clone());
                    }
                    if ui
                        .add_enabled(
                            self.running_task.is_none(),
                            egui::Button::new("Refresh Details"),
                        )
                        .clicked()
                    {
                        self.inspect_network_action(network_name.clone());
                    }
                });
            }
        });
        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            if let Some(details) = self.selected_network_details.as_ref() {
                egui::Grid::new("network_details_meta")
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        detail_row(ui, "Name", &details.name);
                        detail_row(ui, "Driver", &details.driver);
                        detail_row(ui, "Subnet", &details.subnet);
                        detail_row(ui, "Gateway", &details.gateway);
                        detail_row(ui, "Scope", &details.scope);
                        detail_row(ui, "Created", &details.created_at);
                    });

                ui.add_space(12.0);
                ui.heading("Labels");
                if details.labels.is_empty() {
                    ui.label("No labels reported.");
                } else {
                    ScrollArea::vertical()
                        .id_salt("network_labels_scroll")
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for (key, value) in &details.labels {
                                ui.horizontal_wrapped(|ui| {
                                    ui.monospace(key);
                                    ui.label(value);
                                });
                            }
                        });
                }
            } else {
                ui.label("Choose `Inspect` on a network row to load full details here.");
            }
        });
    }

    fn render_project_details(&mut self, ui: &mut egui::Ui) {
        let selected_project = self
            .selected_project_name
            .as_ref()
            .and_then(|name| self.projects.iter().find(|project| &project.name == name))
            .cloned();

        ui.horizontal(|ui| {
            ui.heading("Project Details");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Refresh Projects").clicked() {
                    self.refresh_projects();
                }
                if ui
                    .add_enabled(self.running_task.is_none(), egui::Button::new("Logs"))
                    .clicked()
                {
                    self.fetch_compose_logs();
                }
                if ui
                    .add_enabled(self.running_task.is_none(), egui::Button::new("Down"))
                    .clicked()
                {
                    self.start_compose_down();
                }
                if ui
                    .add_enabled(self.running_task.is_none(), egui::Button::new("Up"))
                    .clicked()
                {
                    self.start_compose_up();
                }
            });
        });
        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            if let Some(project) = selected_project {
                egui::Grid::new("project_details_meta")
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        detail_row(ui, "Name", &project.name);
                        detail_row(ui, "Status", &project.status);
                        detail_row(ui, "Compose", &project.config_files);
                        detail_row(ui, "Working Dir", &project.working_dir);
                    });
            } else if !self.compose_target.trim().is_empty() {
                egui::Grid::new("project_draft_meta")
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        detail_row(ui, "Compose", self.compose_target.trim());
                        detail_row(
                            ui,
                            "Project Name",
                            &empty_as_dash(self.compose_project_name.trim()),
                        );
                    });
            } else {
                ui.label("Choose a discovered project or point the form at a compose file or project folder.");
            }
        });
    }

    fn render_image_details(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Image Details");
            if let Some(details) = self.selected_image_details.as_ref() {
                let image_ref = details.reference.clone();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(self.running_task.is_none(), egui::Button::new("Delete"))
                        .clicked()
                    {
                        self.remove_image_action(image_ref.clone());
                    }
                    if ui
                        .add_enabled(self.running_task.is_none(), egui::Button::new("Run"))
                        .clicked()
                    {
                        self.start_run_from_image(image_ref.clone());
                    }
                    if ui.button("Use").clicked() {
                        self.prefill_run_from_image(image_ref.clone());
                    }
                    if ui
                        .add_enabled(
                            self.running_task.is_none(),
                            egui::Button::new("Refresh Details"),
                        )
                        .clicked()
                    {
                        self.inspect_image_action(image_ref.clone());
                    }
                });
            }
        });
        if let Some(details) = self.selected_image_details.as_ref() {
            ui.add_space(4.0);
            ui.label(RichText::new(&details.reference).monospace().weak());
        }
        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            if let Some(details) = self.selected_image_details.as_ref() {
                egui::Grid::new("image_details_meta")
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        detail_row(ui, "Reference", &details.reference);
                        detail_row(ui, "Image ID", &details.image_id);
                        detail_row(ui, "Source", &details.source);
                        detail_row(ui, "Size", &details.size);
                        detail_row(ui, "Architecture", &details.architecture);
                        detail_row(ui, "OS", &details.os);
                        detail_row(ui, "Created", &details.created);
                        detail_row(ui, "Layers", &details.layer_count.to_string());
                        detail_row(ui, "User", &empty_as_dash(&details.user));
                        detail_row(ui, "Working Dir", &empty_as_dash(&details.working_dir));
                        detail_row(ui, "Entrypoint", &empty_as_dash(&details.entrypoint));
                        detail_row(ui, "Command", &empty_as_dash(&details.command));
                        detail_row(ui, "Manifest", &details.manifest_digest);
                        detail_row(ui, "Config", &details.config_digest);
                    });

                ui.add_space(12.0);
                ui.heading("Environment");
                if details.env.is_empty() {
                    ui.label("No environment variables recorded.");
                } else {
                    ScrollArea::vertical()
                        .id_salt("image_env_scroll")
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for entry in &details.env {
                                ui.monospace(entry);
                            }
                        });
                }

                ui.add_space(12.0);
                ui.heading("Exposed Ports");
                if details.exposed_ports.is_empty() {
                    ui.label("No exposed ports recorded.");
                } else {
                    ui.label(details.exposed_ports.join(", "));
                }

                ui.add_space(12.0);
                ui.heading("Labels");
                if details.labels.is_empty() {
                    ui.label("No labels recorded.");
                } else {
                    ScrollArea::vertical()
                        .id_salt("image_labels_scroll")
                        .max_height(140.0)
                        .show(ui, |ui| {
                            for (key, value) in &details.labels {
                                ui.horizontal_wrapped(|ui| {
                                    ui.monospace(key);
                                    ui.label(value);
                                });
                            }
                        });
                }
            } else {
                ui.label("Choose `Inspect` on an image row to load native OCI details here.");
            }
        });
    }

    fn render_containers_table(&mut self, ui: &mut egui::Ui) {
        let max_height = (ui.available_height() - 88.0).max(220.0);
        ui.horizontal(|ui| {
            ui.heading("Containers");
            ui.label(filtered_total_label(
                filtered_containers(&self.containers, &self.container_filter).len(),
                self.containers.len(),
            ));
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(
                TextEdit::singleline(&mut self.container_filter)
                    .hint_text("name, image, runtime, state, status, or port"),
            );
            if ui.button("Clear").clicked() {
                self.container_filter.clear();
            }
        });
        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ScrollArea::vertical()
                .id_salt("containers_scroll")
                .max_height(max_height)
                .show(ui, |ui| {
                    egui::Grid::new("containers_grid")
                        .striped(true)
                        .min_col_width(80.0)
                        .show(ui, |ui| {
                            ui.strong("Name");
                            ui.strong("Image");
                            ui.strong("Runtime");
                            ui.strong("State");
                            ui.strong("Status");
                            ui.strong("Ports");
                            ui.strong("Actions");
                            ui.end_row();

                            let containers =
                                filtered_containers(&self.containers, &self.container_filter);
                            for container in containers {
                                let streaming = self.live_log_stream_container.as_deref()
                                    == Some(container.id.as_str());
                                ui.label(&container.name);
                                ui.label(&container.image);
                                ui.monospace(&container.runtime);
                                ui.colored_label(state_color(&container.state), &container.state);
                                ui.colored_label(state_color(&container.state), &container.status);
                                ui.vertical(|ui| {
                                    ui.label(&container.ports);
                                    for url in published_urls(&container.ports) {
                                        if ui.link(url.as_str()).clicked() {
                                            self.open_url_action(url.clone());
                                        }
                                    }
                                });
                                ui.horizontal(|ui| {
                                    let running = container.state.eq_ignore_ascii_case("running");
                                    let removable = !running;
                                    if ui
                                        .add_enabled(
                                            self.running_task.is_none() && !running,
                                            egui::Button::new("Start"),
                                        )
                                        .clicked()
                                    {
                                        self.start_container_action(container.id.clone());
                                    }
                                    if ui
                                        .add_enabled(
                                            self.running_task.is_none() && running,
                                            egui::Button::new("Stop"),
                                        )
                                        .clicked()
                                    {
                                        self.stop_container_action(container.id.clone());
                                    }
                                    if ui
                                        .add_enabled(
                                            self.running_task.is_none(),
                                            egui::Button::new("Restart"),
                                        )
                                        .clicked()
                                    {
                                        self.restart_container_action(container.id.clone());
                                    }
                                    if ui
                                        .add_enabled(
                                            self.running_task.is_none(),
                                            egui::Button::new("Logs"),
                                        )
                                        .clicked()
                                    {
                                        self.logs_container_action(container.id.clone());
                                    }
                                    if streaming {
                                        if ui.button("Stop Stream").clicked() {
                                            self.stop_live_logs_action();
                                        }
                                    } else if ui
                                        .add_enabled(
                                            self.running_task.is_none(),
                                            egui::Button::new("Follow"),
                                        )
                                        .clicked()
                                    {
                                        self.follow_logs_action(container.id.clone());
                                    }
                                    if ui
                                        .add_enabled(
                                            self.running_task.is_none(),
                                            egui::Button::new("Inspect"),
                                        )
                                        .clicked()
                                    {
                                        self.inspect_container_action(container.id.clone());
                                    }
                                    if ui
                                        .add_enabled(
                                            self.running_task.is_none() && removable,
                                            egui::Button::new("Delete"),
                                        )
                                        .clicked()
                                    {
                                        self.remove_container_action(container.id.clone());
                                    }
                                });
                                ui.end_row();
                            }
                        });
                });
        });
    }

    fn render_container_details(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Container Details");
            if let Some(details) = self.selected_container_details.as_ref() {
                ui.label(&details.name);
                let status = details.status.clone();
                let container_id = details.id.clone();
                let urls = published_urls(&details.ports);
                let running = status.eq_ignore_ascii_case("running");
                let streaming =
                    self.live_log_stream_container.as_deref() == Some(container_id.as_str());
                if ui
                    .add_enabled(
                        self.running_task.is_none(),
                        egui::Button::new("Refresh Details"),
                    )
                    .clicked()
                {
                    self.inspect_container_action(container_id.clone());
                }
                if ui
                    .add_enabled(
                        self.running_task.is_none() && running,
                        egui::Button::new("Refresh Stats"),
                    )
                    .clicked()
                {
                    self.refresh_container_stats_action(container_id.clone());
                }
                if ui
                    .add_enabled(
                        self.running_task.is_none() && !running,
                        egui::Button::new("Start"),
                    )
                    .clicked()
                {
                    self.start_container_action(container_id.clone());
                }
                if ui
                    .add_enabled(
                        self.running_task.is_none() && running,
                        egui::Button::new("Stop"),
                    )
                    .clicked()
                {
                    self.stop_container_action(container_id.clone());
                }
                if ui
                    .add_enabled(self.running_task.is_none(), egui::Button::new("Restart"))
                    .clicked()
                {
                    self.restart_container_action(container_id.clone());
                }
                if ui
                    .add_enabled(self.running_task.is_none(), egui::Button::new("Logs"))
                    .clicked()
                {
                    self.logs_container_action(container_id.clone());
                }
                if streaming {
                    if ui.button("Stop Stream").clicked() {
                        self.stop_live_logs_action();
                    }
                } else if ui
                    .add_enabled(self.running_task.is_none(), egui::Button::new("Follow"))
                    .clicked()
                {
                    self.follow_logs_action(container_id.clone());
                }
                for url in urls {
                    if ui.link(url.as_str()).clicked() {
                        self.open_url_action(url);
                    }
                }
            }
        });
        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            if let Some(details) = self.selected_container_details.as_ref() {
                egui::Grid::new("container_details_meta")
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        detail_row(ui, "Name", &details.name);
                        detail_row(ui, "Image", &details.image);
                        detail_row(ui, "Runtime", &details.runtime);
                        detail_row(ui, "Status", &details.status);
                        detail_row(ui, "Ports", &details.ports);
                        detail_row(ui, "IP Address", &empty_as_dash(&details.ip_address));
                        detail_row(ui, "User", &empty_as_dash(&details.user));
                        detail_row(ui, "Working Dir", &empty_as_dash(&details.working_dir));
                        detail_row(ui, "Restart", &details.restart_policy);
                        detail_row(ui, "Created", &details.created);
                        detail_row(ui, "Entrypoint", &empty_as_dash(&details.entrypoint));
                        detail_row(ui, "Command", &empty_as_dash(&details.command));
                    });

                ui.add_space(12.0);
                ui.heading("Environment");
                if details.env.is_empty() {
                    ui.label("No environment variables reported.");
                } else {
                    ScrollArea::vertical()
                        .id_salt("container_env_scroll")
                        .max_height(140.0)
                        .show(ui, |ui| {
                            for entry in &details.env {
                                ui.monospace(entry);
                            }
                        });
                }

                ui.add_space(12.0);
                ui.heading("Labels");
                if details.labels.is_empty() {
                    ui.label("No labels reported.");
                } else {
                    ScrollArea::vertical()
                        .id_salt("container_labels_scroll")
                        .max_height(140.0)
                        .show(ui, |ui| {
                            for (key, value) in &details.labels {
                                ui.horizontal_wrapped(|ui| {
                                    ui.monospace(key);
                                    ui.label(value);
                                });
                            }
                        });
                }

                ui.add_space(12.0);
                ui.heading("Stats");
                if let Some(stats) = self.selected_container_stats.as_ref() {
                    if stats.container_id == details.id {
                        egui::Grid::new("container_stats_meta")
                            .num_columns(2)
                            .spacing([16.0, 8.0])
                            .show(ui, |ui| {
                                detail_row(ui, "CPU", &stats.cpu_percent);
                                detail_row(ui, "Memory", &stats.memory_usage);
                                detail_row(ui, "Memory %", &stats.memory_percent);
                                detail_row(ui, "Network I/O", &stats.net_io);
                                detail_row(ui, "Block I/O", &stats.block_io);
                                detail_row(ui, "PIDs", &stats.pids);
                            });
                    } else if details.status.eq_ignore_ascii_case("running") {
                        ui.label("Stats are available once you refresh the selected container.");
                    } else {
                        ui.label("Stats are available only for running containers.");
                    }
                } else if details.status.eq_ignore_ascii_case("running") {
                    ui.label("Stats are available once you refresh the selected container.");
                } else {
                    ui.label("Stats are available only for running containers.");
                }

                ui.add_space(12.0);
                ui.heading("Exec");
                ui.label("Run a one-off command inside the selected running container.");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add(
                        TextEdit::singleline(&mut self.exec_command_input)
                            .hint_text("env | sort | head"),
                    );
                    if ui.button("Sample Env").clicked() {
                        self.exec_command_input = String::from("env | sort | head -50");
                    }
                    if ui.button("Sample PS").clicked() {
                        self.exec_command_input = String::from("ps aux");
                    }
                });
                ui.add_space(6.0);
                let running = details.status.eq_ignore_ascii_case("running");
                let exec_container_id = details.id.clone();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            self.running_task.is_none() && running,
                            egui::Button::new("Exec In Container"),
                        )
                        .clicked()
                    {
                        self.exec_container_action(exec_container_id.clone());
                    }
                    if !running {
                        ui.colored_label(
                            Color32::from_rgb(241, 196, 15),
                            "Start the container before using exec.",
                        );
                    }
                });
            } else {
                ui.label("Choose `Inspect` on a container row to load full details here.");
            }
        });
    }

    pub(super) fn render_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.heading("Docker RS");
        ui.label("Desktop");
        ui.add_space(12.0);

        self.render_nav_button(ui, WorkspaceTab::Home, "Home", "Overview and quick actions");
        self.render_nav_button(
            ui,
            WorkspaceTab::Projects,
            "Projects",
            &format!("{} compose", self.projects.len()),
        );
        self.render_nav_button(
            ui,
            WorkspaceTab::Volumes,
            "Volumes",
            &format!("{} local", self.volumes.len()),
        );
        self.render_nav_button(
            ui,
            WorkspaceTab::Networks,
            "Networks",
            &format!("{} local", self.networks.len()),
        );
        self.render_nav_button(
            ui,
            WorkspaceTab::Containers,
            "Containers",
            &format!("{} total", self.containers.len()),
        );
        self.render_nav_button(
            ui,
            WorkspaceTab::Images,
            "Images",
            &format!("{} local", self.images.len()),
        );
        self.render_nav_button(
            ui,
            WorkspaceTab::Build,
            "Build & Run",
            "Pull, build, launch",
        );
        self.render_nav_button(ui, WorkspaceTab::Logs, "Logs", "Live events and logs");

        ui.add_space(18.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.strong("Runtime");
            ui.add_space(6.0);
            ui.label(runtime_badge_text(&self.runtime_status));
            ui.small(compact_runtime_detail(&self.runtime_status));
            if !self.engine_status.store_path.is_empty() {
                ui.add_space(6.0);
                ui.small("Store");
                ui.small(compact_path(&self.engine_status.store_path));
            }
            ui.add_space(8.0);
            if ui.button("Refresh").clicked() {
                self.refresh_runtime();
            }
            if !self.runtime_status.bridge_ready
                && self.running_task.is_none()
                && ui.button("Start Docker Fallback").clicked()
            {
                self.start_runtime();
            }
        });

        ui.add_space(12.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.strong("Quick Stats");
            ui.add_space(6.0);
            stat_line(
                ui,
                "Running",
                &self
                    .containers
                    .iter()
                    .filter(|container| container.state.eq_ignore_ascii_case("running"))
                    .count()
                    .to_string(),
            );
            stat_line(
                ui,
                "Native",
                &self
                    .containers
                    .iter()
                    .filter(|container| container.runtime == "native")
                    .count()
                    .to_string(),
            );
            stat_line(
                ui,
                "Docker",
                &self
                    .containers
                    .iter()
                    .filter(|container| container.runtime == "docker")
                    .count()
                    .to_string(),
            );
            stat_line(ui, "Projects", &self.projects.len().to_string());
            stat_line(ui, "Volumes", &self.volumes.len().to_string());
            stat_line(ui, "Networks", &self.networks.len().to_string());
            stat_line(ui, "Images", &self.images.len().to_string());
        });

        ui.add_space(12.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.strong("Shortcuts");
            ui.add_space(6.0);
            ui.small("Cmd/Ctrl+1..8 switch pages");
            ui.small("Esc goes back");
            ui.small("Cmd/Ctrl+R refreshes");
        });
    }

    fn render_nav_button(
        &mut self,
        ui: &mut egui::Ui,
        tab: WorkspaceTab,
        label: &str,
        caption: &str,
    ) {
        let selected = self.workspace_tab == tab;
        let button = egui::Button::new(RichText::new(label).strong()).selected(selected);
        let response = ui.add_sized([ui.available_width(), 44.0], button);
        if response.clicked() {
            self.navigate_to(tab);
        }
        ui.small(caption);
        ui.add_space(8.0);
    }

    pub(super) fn render_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if !self.page_history.is_empty() && ui.button("Back").clicked() {
                self.navigate_back();
            }
            self.render_toolbar_tab(ui, WorkspaceTab::Home, "Home");
            self.render_toolbar_tab(ui, WorkspaceTab::Projects, "Projects");
            self.render_toolbar_tab(ui, WorkspaceTab::Volumes, "Volumes");
            self.render_toolbar_tab(ui, WorkspaceTab::Networks, "Networks");
            self.render_toolbar_tab(ui, WorkspaceTab::Containers, "Containers");
            self.render_toolbar_tab(ui, WorkspaceTab::Images, "Images");
            self.render_toolbar_tab(ui, WorkspaceTab::Build, "Build & Run");
            self.render_toolbar_tab(ui, WorkspaceTab::Logs, "Logs");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Refresh Runtime").clicked() {
                    self.refresh_runtime();
                }
                if ui.button("Refresh Images").clicked() {
                    self.refresh_images();
                }
                if ui.button("Refresh Engine").clicked() {
                    self.refresh_engine();
                }
            });
        });
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            status_chip(
                ui,
                if self.engine_ready {
                    Color32::from_rgb(46, 204, 113)
                } else {
                    Color32::from_rgb(231, 76, 60)
                },
                &self.engine_status.summary,
            );
            status_chip(
                ui,
                if self.runtime_status.native_ready {
                    Color32::from_rgb(52, 152, 219)
                } else {
                    Color32::from_rgb(241, 196, 15)
                },
                if self.runtime_status.native_ready {
                    "Native Runtime Online"
                } else {
                    "Native Runtime Unavailable"
                },
            );
            status_chip(
                ui,
                if self.runtime_status.bridge_ready {
                    Color32::from_rgb(52, 152, 219)
                } else {
                    Color32::from_rgb(149, 165, 166)
                },
                if self.runtime_status.bridge_ready {
                    "Docker Fallback Ready"
                } else {
                    "Docker Fallback Offline"
                },
            );
            if let Some(task) = &self.running_task {
                status_chip(
                    ui,
                    Color32::from_rgb(241, 196, 15),
                    &format!("Running: {task}"),
                );
            }
        });
    }

    fn render_toolbar_tab(&mut self, ui: &mut egui::Ui, tab: WorkspaceTab, label: &str) {
        let selected = self.workspace_tab == tab;
        let button = egui::Button::new(RichText::new(label).strong()).selected(selected);
        if ui.add(button).clicked() && !selected {
            self.navigate_to(tab);
        }
    }

    pub(super) fn render_toast_overlay(&mut self, ctx: &egui::Context) {
        let Some(toast) = self.toast.clone() else {
            return;
        };
        if toast.created_at.elapsed() > Duration::from_secs(4) {
            self.toast = None;
            return;
        }

        let (bg_fill, border, title) = match toast.kind {
            ToastKind::Success => (
                Color32::from_rgb(22, 48, 36),
                Color32::from_rgb(46, 204, 113),
                "Success",
            ),
            ToastKind::Error => (
                Color32::from_rgb(56, 24, 24),
                Color32::from_rgb(231, 76, 60),
                "Error",
            ),
            ToastKind::Info => (
                Color32::from_rgb(23, 38, 58),
                Color32::from_rgb(52, 152, 219),
                "Info",
            ),
        };

        egui::Area::new("toast_overlay".into())
            .anchor(egui::Align2::RIGHT_TOP, [-20.0, 76.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(bg_fill)
                    .stroke(egui::Stroke::new(1.0, border))
                    .corner_radius(10.0)
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_max_width(360.0);
                        ui.horizontal(|ui| {
                            ui.colored_label(border, RichText::new(title).strong());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Dismiss").clicked() {
                                    self.toast = None;
                                }
                            });
                        });
                        ui.add_space(4.0);
                        ui.label(&toast.message);
                    });
            });
    }

    pub(super) fn render_workspace(&mut self, ui: &mut egui::Ui) {
        match self.workspace_tab {
            WorkspaceTab::Home => self.render_home_page(ui),
            WorkspaceTab::Projects => self.render_projects_page(ui),
            WorkspaceTab::Volumes => self.render_volumes_page(ui),
            WorkspaceTab::Networks => self.render_networks_page(ui),
            WorkspaceTab::Containers => self.render_containers_page(ui),
            WorkspaceTab::Images => self.render_images_page(ui),
            WorkspaceTab::Build => self.render_build_page(ui),
            WorkspaceTab::Logs => self.render_logs_page(ui),
        }
    }

    fn render_home_page(&mut self, ui: &mut egui::Ui) {
        let available_height = ui.available_height();
        ui.columns(2, |columns| {
            egui::Frame::group(columns[0].style()).show(&mut columns[0], |ui| {
                ui.heading("Overview");
                ui.add_space(8.0);
                stat_line(
                    ui,
                    "Running containers",
                    &self
                        .containers
                        .iter()
                        .filter(|item| item.state.eq_ignore_ascii_case("running"))
                        .count()
                        .to_string(),
                );
                stat_line(ui, "Total containers", &self.containers.len().to_string());
                stat_line(ui, "Compose projects", &self.projects.len().to_string());
                stat_line(ui, "Named volumes", &self.volumes.len().to_string());
                stat_line(ui, "Native networks", &self.networks.len().to_string());
                stat_line(ui, "Local images", &self.images.len().to_string());
                stat_line(
                    ui,
                    "Native runtime",
                    if self.runtime_status.native_ready {
                        "online"
                    } else {
                        "offline"
                    },
                );
                stat_line(
                    ui,
                    "Docker fallback",
                    if self.runtime_status.bridge_ready {
                        "ready"
                    } else {
                        "offline"
                    },
                );
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Open Projects").clicked() {
                        self.navigate_to(WorkspaceTab::Projects);
                    }
                    if ui.button("Open Volumes").clicked() {
                        self.navigate_to(WorkspaceTab::Volumes);
                    }
                    if ui.button("Open Networks").clicked() {
                        self.navigate_to(WorkspaceTab::Networks);
                    }
                    if ui.button("Open Containers").clicked() {
                        self.navigate_to(WorkspaceTab::Containers);
                    }
                    if ui.button("Open Images").clicked() {
                        self.navigate_to(WorkspaceTab::Images);
                    }
                    if ui.button("Open Build & Run").clicked() {
                        self.navigate_to(WorkspaceTab::Build);
                    }
                    if ui.button("Open Logs").clicked() {
                        self.navigate_to(WorkspaceTab::Logs);
                    }
                });
            });
            egui::Frame::group(columns[1].style()).show(&mut columns[1], |ui| {
                ui.heading("Status");
                ui.add_space(8.0);
                ui.label(&self.engine_status.detail);
                ui.label(&self.runtime_status.detail);
                if let Some(task) = &self.running_task {
                    ui.colored_label(
                        Color32::from_rgb(241, 196, 15),
                        format!("Current task: {task}"),
                    );
                } else {
                    ui.label("No active task.");
                }
                if !self.engine_status.store_path.is_empty() {
                    ui.add_space(8.0);
                    ui.small("Native store");
                    ui.monospace(&self.engine_status.store_path);
                }
            });
        });
        ui.add_space(14.0);
        ui.columns(2, |columns| {
            egui::Frame::group(columns[0].style()).show(&mut columns[0], |ui| {
                ui.heading("Recent Containers");
                ui.add_space(8.0);
                ScrollArea::vertical()
                    .id_salt("home_containers")
                    .max_height((available_height * 0.45).max(220.0))
                    .show(ui, |ui| {
                        for container in self.containers.iter().take(12) {
                            ui.horizontal(|ui| {
                                ui.label(&container.name);
                                ui.monospace(&container.runtime);
                                ui.colored_label(state_color(&container.state), &container.status);
                            });
                        }
                        if self.containers.is_empty() {
                            ui.label("No containers yet.");
                        }
                    });
            });
            egui::Frame::group(columns[1].style()).show(&mut columns[1], |ui| {
                ui.heading("Recent Images");
                ui.add_space(8.0);
                ScrollArea::vertical()
                    .id_salt("home_images")
                    .max_height((available_height * 0.45).max(220.0))
                    .show(ui, |ui| {
                        for image in self.images.iter().take(12) {
                            ui.horizontal(|ui| {
                                ui.label(format!("{}:{}", image.repository, image.tag));
                                ui.monospace(&image.image_id);
                                ui.label(&image.size);
                            });
                        }
                        if self.images.is_empty() {
                            ui.label("No images in the native store yet.");
                        }
                    });
            });
        });
    }

    fn render_projects_page(&mut self, ui: &mut egui::Ui) {
        ui.columns(2, |columns| {
            egui::Frame::group(columns[0].style()).show(&mut columns[0], |ui| {
                self.render_projects_panel(ui);
            });
            egui::Frame::group(columns[1].style()).show(&mut columns[1], |ui| {
                ScrollArea::vertical()
                    .id_salt("project_details_page")
                    .show(ui, |ui| {
                        self.render_project_details(ui);
                    });
            });
        });
    }

    fn render_volumes_page(&mut self, ui: &mut egui::Ui) {
        ui.columns(2, |columns| {
            egui::Frame::group(columns[0].style()).show(&mut columns[0], |ui| {
                self.render_volumes_panel(ui);
            });
            egui::Frame::group(columns[1].style()).show(&mut columns[1], |ui| {
                ScrollArea::vertical()
                    .id_salt("volume_details_page")
                    .show(ui, |ui| {
                        self.render_volume_details(ui);
                    });
            });
        });
    }

    fn render_networks_page(&mut self, ui: &mut egui::Ui) {
        ui.columns(2, |columns| {
            egui::Frame::group(columns[0].style()).show(&mut columns[0], |ui| {
                self.render_networks_panel(ui);
            });
            egui::Frame::group(columns[1].style()).show(&mut columns[1], |ui| {
                ScrollArea::vertical()
                    .id_salt("network_details_page")
                    .show(ui, |ui| {
                        self.render_network_details(ui);
                    });
            });
        });
    }

    fn render_containers_page(&mut self, ui: &mut egui::Ui) {
        ui.columns(2, |columns| {
            egui::Frame::group(columns[0].style()).show(&mut columns[0], |ui| {
                self.render_containers_table(ui);
            });
            egui::Frame::group(columns[1].style()).show(&mut columns[1], |ui| {
                ScrollArea::vertical()
                    .id_salt("container_details_page")
                    .show(ui, |ui| {
                        self.render_container_details(ui);
                    });
            });
        });
    }

    fn render_images_page(&mut self, ui: &mut egui::Ui) {
        ui.columns(2, |columns| {
            egui::Frame::group(columns[0].style()).show(&mut columns[0], |ui| {
                self.render_images_table(ui);
            });
            egui::Frame::group(columns[1].style()).show(&mut columns[1], |ui| {
                ScrollArea::vertical()
                    .id_salt("image_details_page")
                    .show(ui, |ui| {
                        self.render_image_details(ui);
                    });
            });
        });
    }

    fn render_build_page(&mut self, ui: &mut egui::Ui) {
        ui.columns(2, |columns| {
            egui::Frame::group(columns[0].style()).show(&mut columns[0], |ui| {
                ui.add_enabled_ui(self.running_task.is_none() && self.engine_ready, |ui| {
                    self.render_pull_card(ui);
                });
                ui.add_space(14.0);
                ui.add_enabled_ui(self.running_task.is_none() && self.engine_ready, |ui| {
                    self.render_build_card(ui);
                });
            });
            egui::Frame::group(columns[1].style()).show(&mut columns[1], |ui| {
                ui.add_enabled_ui(self.running_task.is_none(), |ui| {
                    self.render_run_card(ui);
                });
            });
        });
    }

    fn render_logs_page(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            self.render_logs_panel(ui);
        });
    }
}
