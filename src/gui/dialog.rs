use std::fs;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Button, Id, Key, Modal, ScrollArea, TextEdit};

#[derive(Clone, Copy)]
pub enum PathAction {
    NewProject,
    OpenProject,
    ImportMedia,
    AddVideo,
    AddAudio,
    Export,
}

impl PathAction {
    const fn title(self) -> &'static str {
        match self {
            Self::NewProject => "New Project",
            Self::OpenProject => "Open Project",
            Self::ImportMedia => "Add Media",
            Self::AddVideo => "Add Video",
            Self::AddAudio => "Add Audio",
            Self::Export => "Export MP4",
        }
    }

    const fn confirm(self) -> &'static str {
        match self {
            Self::NewProject => "Create",
            Self::OpenProject => "Open",
            Self::ImportMedia | Self::AddVideo | Self::AddAudio => "Add",
            Self::Export => "Export",
        }
    }

    const fn selects_directory(self) -> bool {
        matches!(self, Self::NewProject | Self::OpenProject)
    }

    const fn default_path(self) -> &'static str {
        match self {
            Self::Export => "output.mp4",
            _ => "",
        }
    }
}

pub enum PathSelection {
    Canceled,
    One(PathBuf),
    Media(Vec<PathBuf>),
}

struct Entry {
    path: PathBuf,
    directory: bool,
}

pub struct PathDialog {
    pub action: PathAction,
    directory: PathBuf,
    entries: Result<Vec<Entry>, String>,
    path_text: String,
    selected: Vec<PathBuf>,
}

impl PathDialog {
    pub fn new(action: PathAction, directory: PathBuf) -> Self {
        let entries = read_entries(&directory, action);
        Self {
            action,
            directory,
            entries,
            path_text: action.default_path().to_owned(),
            selected: Vec::new(),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<PathSelection> {
        let mut navigate = None;
        let mut confirm = false;
        let mut cancel = false;
        let import = matches!(self.action, PathAction::ImportMedia);
        let modal = Modal::new(Id::unique("path-dialog")).show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 40.0).clamp(0.0, 480.0));
            ui.heading(self.action.title());
            ui.horizontal(|ui| {
                let parent = self.directory.parent();
                if ui
                    .add_enabled(parent.is_some(), Button::new("Up"))
                    .clicked()
                {
                    navigate = parent.map(Path::to_path_buf);
                }
                ui.label(self.directory.display().to_string());
            });

            ui.separator();
            if import {
                ui.label("Select files or folders. Double-click a folder to open it.");
            }
            let height = 260.0_f32.min(ctx.content_rect().height() * 0.5);
            ScrollArea::vertical()
                .id_salt(&self.directory)
                .max_height(height)
                .show(ui, |ui| {
                    ui.set_min_height(height);
                    self.show_entries(ui, &mut navigate, &mut confirm);
                });

            let path = ui.add(
                TextEdit::singleline(&mut self.path_text)
                    .hint_text("Enter a path")
                    .desired_width(f32::INFINITY),
            );
            if path.changed() {
                self.selected.clear();
            }
            let has_path = !self.path_text.is_empty()
                || !self.selected.is_empty()
                || matches!(self.action, PathAction::OpenProject);
            confirm |=
                has_path && path.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter));
            ui.horizontal(|ui| {
                let label = if import && !has_path {
                    "Add This Folder"
                } else {
                    self.action.confirm()
                };
                confirm |= ui
                    .add_enabled(has_path || import, Button::new(label))
                    .clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });

        if cancel || modal.should_close() {
            return Some(PathSelection::Canceled);
        }
        if confirm && navigate.is_none() {
            if import {
                if self.selected.is_empty() {
                    self.selected.push(self.directory.join(&self.path_text));
                }
                return Some(PathSelection::Media(std::mem::take(&mut self.selected)));
            }
            let path = self
                .selected
                .pop()
                .unwrap_or_else(|| self.directory.join(&self.path_text));
            if self.action.selects_directory() || !path.is_dir() {
                return Some(PathSelection::One(path));
            }
            navigate = Some(path);
        }
        if let Some(directory) = navigate {
            self.browse(directory);
        }
        None
    }

    fn show_entries(
        &mut self,
        ui: &mut egui::Ui,
        navigate: &mut Option<PathBuf>,
        confirm: &mut bool,
    ) {
        let entries = match &self.entries {
            Ok(entries) => entries,
            Err(error) => {
                ui.label(error);
                return;
            }
        };
        for entry in entries {
            let Some(name) = entry.path.file_name() else {
                continue;
            };
            let name = name.to_string_lossy();
            let label = if entry.directory {
                format!("{name}/")
            } else {
                name.to_string()
            };
            let mut selected = self.selected.contains(&entry.path);
            if matches!(self.action, PathAction::ImportMedia) {
                let row = ui.checkbox(&mut selected, label);
                if entry.directory && row.double_clicked() {
                    *navigate = Some(entry.path.clone());
                } else if row.changed() {
                    self.path_text.clear();
                    if selected {
                        self.selected.push(entry.path.clone());
                    } else {
                        self.selected.retain(|path| path != &entry.path);
                    }
                }
                continue;
            }
            let row = ui.selectable_label(selected, label);
            if row.double_clicked() && entry.directory {
                *navigate = Some(entry.path.clone());
            } else if row.clicked() || row.double_clicked() {
                self.path_text = name.into_owned();
                self.selected.clear();
                self.selected.push(entry.path.clone());
                *confirm |= row.double_clicked();
            }
        }
    }

    fn browse(&mut self, directory: PathBuf) {
        self.entries = read_entries(&directory, self.action);
        self.directory = directory;
        self.selected.clear();
        self.action.default_path().clone_into(&mut self.path_text);
    }
}

fn read_entries(directory: &Path, action: PathAction) -> Result<Vec<Entry>, String> {
    let listing = fs::read_dir(directory)
        .map_err(|error| format!("Cannot read {}. {error}", directory.display()))?;
    let mut entries: Vec<_> = listing
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let directory = path.is_dir();
            (directory || !action.selects_directory()).then_some(Entry { path, directory })
        })
        .collect();
    entries.sort_unstable_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(entries)
}
