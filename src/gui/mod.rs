mod chat;
mod chat_panel;
mod dialog;
mod preview;
mod preview_panel;
mod timeline;

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use eframe::egui::{self, Color32, Context, Id, Pos2, Response, RichText, Ui, Vec2};

use crate::project::{Clip, Level, MediaKind, Placement, Project};
use crate::{media, parse_bounds, parse_gain, parse_seconds, seconds};

use self::chat_panel::{ChatAction, ChatPanel};
use self::dialog::{PathAction, PathDialog, PathSelection};
use self::preview::Preview;
use self::preview_panel::PreviewSelection;
use self::timeline::{ACCENT, Timeline, media_name};

pub fn launch(path: Option<&Path>) -> Result<(), String> {
    let workspace = path.map(|path| Workspace::open(path, false)).transpose()?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1270.0, 830.0])
            .with_min_inner_size([860.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Action",
        options,
        Box::new(move |creation| {
            let mut visuals = egui::Visuals::dark();
            visuals.panel_fill = Color32::from_rgb(22, 24, 31);
            visuals.window_fill = Color32::from_rgb(29, 32, 41);
            visuals.selection.bg_fill = Color32::from_rgb(41, 82, 117);
            creation.egui_ctx.set_visuals(visuals);
            let mut app = Editor::new();
            if let Some(workspace) = workspace {
                app.open_workspace(workspace);
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|error| format!("Cannot open the Action editor. {error}"))
}

struct Workspace {
    directory: PathBuf,
    project: Project,
    base: Option<usize>,
    clip: Option<Clip>,
    dirty: bool,
    library: BTreeMap<PathBuf, MediaKind>,
}

impl Workspace {
    fn open(directory: &Path, create: bool) -> Result<Self, String> {
        if create {
            Project::init(directory)?;
        }
        let directory = directory.canonicalize().map_err(|error| {
            format!(
                "Cannot open project directory {}. {error}",
                directory.display()
            )
        })?;
        let project = Project::load(&directory)?;
        let base = project.clips.len().checked_sub(1);
        let clip = project.clips.last().cloned();
        let mut workspace = Self {
            directory,
            project,
            base,
            clip,
            dirty: false,
            library: BTreeMap::new(),
        };
        workspace.refresh_library();
        Ok(workspace)
    }

    fn refresh_library(&mut self) {
        self.library.clone_from(&self.project.media);
        for clip in self.project.clips.iter().chain(&self.clip) {
            for span in &clip.spans {
                self.library.insert(span.path.clone(), MediaKind::Video);
            }
            for audio in &clip.audio {
                self.library
                    .entry(audio.path.clone())
                    .or_insert(MediaKind::Audio);
            }
        }
    }

    fn label(&self) -> String {
        match (self.base, self.dirty) {
            (Some(index), false) => format!("@c{}", index + 1),
            (Some(index), true) => format!("Draft from @c{}", index + 1),
            (None, _) => "Draft".to_owned(),
        }
    }
}

enum Destination {
    Project(PathBuf, bool),
    Clip(usize),
    Close,
}

enum JobResult {
    Opened(Workspace),
    Changed(Clip),
    Saved(Project),
    Imported(Project),
    Previewed(PathBuf, MediaPreview),
    Exported(PathBuf),
}

type Job = Receiver<Result<JobResult, String>>;

#[derive(PartialEq, Eq)]
enum SidebarTab {
    Media,
    Chat,
}

enum MediaPreview {
    Video(Clip),
    Audio(u64),
}

struct SourcePreview {
    path: PathBuf,
    timeline_position: u64,
}

struct Editor {
    workspace: Option<Workspace>,
    preview: Preview,
    timeline: Timeline,
    selection: PreviewSelection,
    chat: ChatPanel,
    sidebar_tab: SidebarTab,
    source_preview: Option<SourcePreview>,
    path_dialog: Option<PathDialog>,
    audio_dialog: Option<AudioDialog>,
    destination: Option<Destination>,
    job: Option<Job>,
    status: String,
    status_is_error: bool,
    close_confirmed: bool,
}

impl Editor {
    fn new() -> Self {
        Self {
            workspace: None,
            preview: Preview::new(),
            timeline: Timeline::default(),
            selection: PreviewSelection::default(),
            chat: ChatPanel::default(),
            sidebar_tab: SidebarTab::Media,
            source_preview: None,
            path_dialog: None,
            audio_dialog: None,
            destination: None,
            job: None,
            status: String::new(),
            status_is_error: false,
            close_confirmed: false,
        }
    }

    fn clip(&self) -> Option<&Clip> {
        self.workspace
            .as_ref()
            .and_then(|workspace| workspace.clip.as_ref())
    }

    fn open_workspace(&mut self, workspace: Workspace) {
        self.chat.open_project(&workspace.directory);
        self.workspace = Some(workspace);
        self.refresh_preview();
    }

    fn refresh_preview(&mut self) {
        self.source_preview = None;
        self.preview.set_clip(
            self.workspace
                .as_ref()
                .and_then(|workspace| workspace.clip.as_ref()),
        );
        self.timeline.clear();
        self.selection = PreviewSelection::default();
    }

    fn timeline_position(&self) -> u64 {
        self.source_preview.as_ref().map_or_else(
            || self.preview.position(),
            |source| source.timeline_position,
        )
    }

    fn show_timeline_preview(&mut self) {
        if let Some(source) = self.source_preview.take() {
            self.preview.set_clip(
                self.workspace
                    .as_ref()
                    .and_then(|workspace| workspace.clip.as_ref()),
            );
            self.preview.seek(source.timeline_position);
        }
    }

    fn show_source_preview(&mut self, path: PathBuf, source: MediaPreview) {
        let timeline_position = self.timeline_position();
        match source {
            MediaPreview::Video(clip) => self.preview.set_clip(Some(&clip)),
            MediaPreview::Audio(duration) => self.preview.set_audio(path.clone(), duration),
        }
        self.source_preview = Some(SourcePreview {
            path,
            timeline_position,
        });
        self.status("", false);
    }

    fn status(&mut self, text: impl Into<String>, is_error: bool) {
        self.status = text.into();
        self.status_is_error = is_error;
    }

    fn start_job(
        &mut self,
        ctx: &Context,
        label: &str,
        run: impl FnOnce() -> Result<JobResult, String> + Send + 'static,
    ) {
        let (sender, job) = mpsc::channel();
        let ctx = ctx.clone();
        match thread::Builder::new()
            .name("action-project".to_owned())
            .spawn(move || {
                let _ = sender.send(run());
                ctx.request_repaint();
            }) {
            Ok(_) => {
                self.job = Some(job);
                self.status(label, false);
            }
            Err(error) => self.status(format!("Cannot start project operation. {error}"), true),
        }
    }

    fn poll_job(&mut self, ctx: &Context) {
        let Some(job) = &self.job else { return };
        let result = match job.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Err("The project operation ended unexpectedly.".to_owned())
            }
        };
        self.job = None;
        match result {
            Ok(JobResult::Opened(workspace)) => {
                self.open_workspace(workspace);
                self.status("", false);
            }
            Ok(JobResult::Changed(clip)) => self.change_clip(clip),
            Ok(JobResult::Saved(project)) => {
                let clip_id = project.clips.len();
                if let Some(workspace) = &mut self.workspace {
                    workspace.project = project;
                    workspace.base = clip_id.checked_sub(1);
                    workspace.dirty = false;
                    workspace.refresh_library();
                }
                self.status(format!("Saved @c{clip_id}"), false);
                if let Some(destination) = self.destination.take() {
                    self.navigate(ctx, destination);
                }
            }
            Ok(JobResult::Imported(project)) => {
                let mut added = 0;
                if let Some(workspace) = &mut self.workspace {
                    added = project
                        .media
                        .len()
                        .saturating_sub(workspace.project.media.len());
                    workspace.project = project;
                    workspace.refresh_library();
                }
                if added == 0 {
                    self.status("Media already in the project.", false);
                } else {
                    self.status(
                        format!(
                            "Added {added} media {}.",
                            if added == 1 { "file" } else { "files" }
                        ),
                        false,
                    );
                }
            }
            Ok(JobResult::Previewed(path, source)) => self.show_source_preview(path, source),
            Ok(JobResult::Exported(path)) => {
                self.status(format!("Exported {}", path.display()), false);
            }
            Err(error) => self.status(error, true),
        }
    }

    fn change_clip(&mut self, clip: Clip) {
        if let Some(workspace) = &mut self.workspace {
            workspace.clip = Some(clip);
            workspace.dirty = true;
            workspace.refresh_library();
        }
        self.refresh_preview();
        self.status("", false);
    }

    fn save(&mut self, ctx: &Context) {
        if let Some(workspace) = self.workspace.as_ref().filter(|workspace| workspace.dirty)
            && let Some(clip) = workspace.clip.clone()
        {
            let directory = workspace.directory.clone();
            self.start_job(ctx, "Saving clip…", move || {
                Project::append(&directory, |_| Ok(clip)).map(JobResult::Saved)
            });
        }
    }

    fn request_navigation(&mut self, ctx: &Context, destination: Destination) {
        if self
            .workspace
            .as_ref()
            .is_some_and(|workspace| workspace.dirty)
        {
            self.destination = Some(destination);
        } else {
            self.navigate(ctx, destination);
        }
    }

    fn navigate(&mut self, ctx: &Context, destination: Destination) {
        match destination {
            Destination::Project(path, create) => {
                self.start_job(ctx, "Opening project…", move || {
                    Workspace::open(&path, create).map(JobResult::Opened)
                });
            }
            Destination::Clip(index) => {
                if let Some(workspace) = &mut self.workspace
                    && let Some(clip) = workspace.project.clips.get(index)
                {
                    workspace.clip = Some(clip.clone());
                    workspace.base = Some(index);
                    workspace.dirty = false;
                    workspace.refresh_library();
                    self.refresh_preview();
                    self.status("", false);
                }
            }
            Destination::Close => {
                self.close_confirmed = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn import_video(&mut self, ctx: &Context, paths: Vec<PathBuf>) {
        let Some(workspace) = &self.workspace else {
            self.status("Create or open a project before adding video.", true);
            return;
        };
        let directory = workspace.directory.clone();
        let draft = workspace.clip.clone();
        self.start_job(ctx, "Reading video…", move || {
            let clip = paths.iter().try_fold(draft, |clip, path| {
                let path = if path.is_absolute() {
                    path.clone()
                } else {
                    directory.join(path)
                };
                let source = Project::resolve_source(&path)?;
                match clip {
                    Some(clip) => clip.concatenate(source).map(Some),
                    None => Ok(Some(source)),
                }
            })?;
            clip.map(JobResult::Changed)
                .ok_or_else(|| "Select a video file.".to_owned())
        });
    }

    fn import_media(&mut self, ctx: &Context, paths: Vec<PathBuf>) {
        if let Some(workspace) = &self.workspace {
            let directory = workspace.directory.clone();
            self.start_job(ctx, "Adding media…", move || {
                Project::import_media(&directory, paths).map(JobResult::Imported)
            });
        }
    }

    fn preview_media(&mut self, ctx: &Context, path: PathBuf, kind: MediaKind) {
        self.preview.pause();
        self.start_job(ctx, "Opening preview…", move || {
            let source = match kind {
                MediaKind::Video => Project::resolve_source(&path).map(MediaPreview::Video),
                MediaKind::Audio => media::probe_audio(&path).map(MediaPreview::Audio),
            }?;
            Ok(JobResult::Previewed(path, source))
        });
    }

    fn open_dialog(&mut self, action: PathAction) {
        let directory = self
            .workspace
            .as_ref()
            .map(|workspace| workspace.directory.clone())
            .or_else(|| env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        self.path_dialog = Some(PathDialog::new(action, directory));
    }

    fn path_selected(&mut self, ctx: &Context, action: PathAction, path: PathBuf) {
        match action {
            PathAction::NewProject => {
                self.request_navigation(ctx, Destination::Project(path, true));
            }
            PathAction::OpenProject => {
                self.request_navigation(ctx, Destination::Project(path, false));
            }
            PathAction::ImportMedia => self.import_media(ctx, vec![path]),
            PathAction::AddVideo => self.import_video(ctx, vec![path]),
            PathAction::AddAudio => self.audio_form(AudioEdit::Add(path)),
            PathAction::Export => {
                if let Some(clip) = self.clip().cloned() {
                    self.start_job(ctx, "Exporting MP4…", move || {
                        media::render(&clip, &path)?;
                        Ok(JobResult::Exported(path))
                    });
                }
            }
        }
    }

    fn audio_form(&mut self, edit: AudioEdit) {
        if let Some(clip) = self.clip() {
            let bounds = self.timeline.range.clone().unwrap_or_else(|| {
                self.timeline_position().min(clip.duration() - 1)..clip.duration()
            });
            let mut dialog = AudioDialog {
                edit,
                bounds: format!("{}..{}", seconds(bounds.start), seconds(bounds.end)),
                from: "0".to_owned(),
                gain: "0".to_owned(),
                error: String::new(),
            };
            if let AudioEdit::Placement(placement) = &dialog.edit {
                dialog.bounds = format!("{}..{}", seconds(placement.start), seconds(placement.end));
                dialog.from = seconds(placement.from);
                dialog.gain = placement.gain.to_string();
            } else if matches!(dialog.edit, AudioEdit::Original) {
                "mute".clone_into(&mut dialog.gain);
            }
            self.audio_dialog = Some(dialog);
        }
    }

    fn show_dialogs(&mut self, ctx: &Context) {
        if let Some(dialog) = &mut self.path_dialog
            && let Some(selection) = dialog.show(ctx)
        {
            let action = dialog.action;
            self.path_dialog = None;
            match selection {
                PathSelection::Canceled => {}
                PathSelection::One(path) => self.path_selected(ctx, action, path),
                PathSelection::Media(paths) => self.import_media(ctx, paths),
            }
        }

        if let Some(mut dialog) = self.audio_dialog.take() {
            let result = dialog.show(ctx);
            if result == Some(true) {
                if let Some(clip) = self.clip().cloned() {
                    self.start_job(ctx, "Applying audio edit…", move || {
                        dialog.apply(clip).map(JobResult::Changed)
                    });
                }
            } else if result.is_none() {
                self.audio_dialog = Some(dialog);
            }
        }

        if self.destination.is_some() {
            let modal = egui::Modal::new(Id::unique("unsaved-clip")).show(ctx, |ui| {
                ui.heading("Save this clip?");
                ui.label("Save a new clip before leaving this edit.");
                if self.status_is_error {
                    ui.colored_label(ui.visuals().error_fg_color, &self.status);
                }
                ui.add_enabled_ui(self.job.is_none(), |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("Save and Continue").clicked() {
                            self.save(ctx);
                        }
                        if ui.button("Discard").clicked()
                            && let Some(destination) = self.destination.take()
                        {
                            self.navigate(ctx, destination);
                        }
                        if ui.button("Cancel").clicked() {
                            self.destination = None;
                        }
                    });
                });
            });
            if modal.should_close() && self.job.is_none() {
                self.destination = None;
            }
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Action").strong().size(21.0));
            ui.separator();
            ui.add_enabled_ui(self.job.is_none(), |ui| {
                if ui.button("New Project").clicked() {
                    self.open_dialog(PathAction::NewProject);
                }
                if ui.button("Open Project").clicked() {
                    self.open_dialog(PathAction::OpenProject);
                }
                let dirty = self
                    .workspace
                    .as_ref()
                    .is_some_and(|workspace| workspace.dirty);
                let save = ui.add_enabled(dirty, egui::Button::new("Save Clip"));
                let shortcut = dirty
                    && self.path_dialog.is_none()
                    && self.audio_dialog.is_none()
                    && self.destination.is_none()
                    && ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::COMMAND, egui::Key::S)
                    });
                if save.clicked() || shortcut {
                    self.save(&ui.ctx().clone());
                }
                if ui
                    .add_enabled(self.clip().is_some(), egui::Button::new("Export MP4"))
                    .clicked()
                {
                    self.open_dialog(PathAction::Export);
                }
            });
            ui.separator();
            if let Some(workspace) = &self.workspace {
                ui.add(
                    egui::Label::new(RichText::new(media_name(&workspace.directory))).truncate(),
                )
                .on_hover_text(workspace.directory.display().to_string());
            }
        });
    }

    fn timeline_tools(&mut self, ui: &mut Ui) {
        let mut switch = None;
        let mut append = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Timeline").strong());
            if let Some(workspace) = &self.workspace {
                egui::ComboBox::from_id_salt("clip-history")
                    .selected_text(workspace.label())
                    .show_ui(ui, |ui| {
                        for (index, clip) in workspace.project.clips.iter().enumerate().rev() {
                            let active = workspace.base == Some(index) && !workspace.dirty;
                            let label =
                                format!("@c{}  ·  {} s", index + 1, seconds(clip.duration()));
                            if ui.selectable_label(active, label).clicked() && !active {
                                switch = Some(index);
                            }
                        }
                    });
                egui::ComboBox::from_id_salt("append-clip")
                    .selected_text("Append Clip")
                    .show_ui(ui, |ui| {
                        for (index, clip) in workspace.project.clips.iter().enumerate().rev() {
                            if ui
                                .button(format!(
                                    "@c{}  ·  {} s",
                                    index + 1,
                                    seconds(clip.duration())
                                ))
                                .clicked()
                            {
                                append = Some(clip.clone());
                                ui.close();
                            }
                        }
                    });
            }
            self.edit_buttons(ui);
        });
        if let Some(index) = switch {
            self.request_navigation(&ui.ctx().clone(), Destination::Clip(index));
        }
        if let Some(append) = append {
            let result = match self.clip().cloned() {
                Some(clip) => clip.concatenate(append),
                None => Ok(append),
            };
            self.apply_edit(result);
        }
    }

    fn edit_buttons(&mut self, ui: &mut Ui) {
        if ui
            .add_enabled(self.workspace.is_some(), egui::Button::new("Add Video"))
            .clicked()
        {
            self.open_dialog(PathAction::AddVideo);
        }
        let Some(clip) = self.clip() else { return };
        let range = self.timeline.range.clone();
        let placement = self
            .timeline
            .selected_audio()
            .and_then(|id| clip.audio.iter().find(|item| item.id == id))
            .cloned();
        if ui
            .add_enabled(range.is_some(), egui::Button::new("Trim Selection"))
            .on_hover_text("Drag across the ruler to mark the part to keep.")
            .clicked()
            && let Some(range) = range
        {
            self.apply_edit(clip.clone().trim(range));
        }
        if ui.button("Add Audio").clicked() {
            self.open_dialog(PathAction::AddAudio);
        }
        if ui.button("Original Level").clicked() {
            self.audio_form(AudioEdit::Original);
        }
        if let Some(placement) = placement {
            if ui.button("Edit Audio").clicked() {
                self.audio_form(AudioEdit::Placement(placement.clone()));
            }
            if ui.button("Remove Audio").clicked()
                && let Some(clip) = self.clip().cloned()
            {
                self.apply_edit(clip.remove_audio(placement.id));
            }
        }
    }

    fn apply_edit(&mut self, edit: Result<Clip, String>) {
        match edit {
            Ok(clip) => self.change_clip(clip),
            Err(error) => self.status(error, true),
        }
    }

    fn timeline_panel(&mut self, ui: &mut Ui) {
        ui.add_enabled_ui(self.job.is_none(), |ui| self.timeline_tools(ui));
        ui.add_space(4.0);
        let timeline_position = self.timeline_position();
        let Some(clip) = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.clip.as_ref())
        else {
            if self.workspace.is_some() {
                ui.label("Add Video or drop video files here to start the timeline.");
            }
            return;
        };
        if let Some(position) = self.timeline.show(ui, clip, timeline_position) {
            self.show_timeline_preview();
            self.preview.pause();
            if self.preview.position() != position {
                self.preview.seek(position);
                ui.ctx().request_repaint();
            }
        }
    }

    fn preview_panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Preview").strong());
            let playing = self.preview.is_playing();
            let duration = self.preview.duration();
            if ui
                .add_enabled(
                    duration > 0,
                    egui::Button::new(if playing { "Pause" } else { "Play" }),
                )
                .clicked()
            {
                self.preview.toggle_playback();
                ui.ctx().request_repaint();
            }
            if duration > 0 {
                ui.label(
                    RichText::new(format!(
                        "{} / {} s",
                        time_label(self.preview.position()),
                        seconds(duration)
                    ))
                    .monospace(),
                );
            }
            if self.source_preview.is_some() && ui.button("Back to Timeline").clicked() {
                self.show_timeline_preview();
            }
            if let Some(error) = self
                .preview
                .error()
                .or_else(|| self.preview.audio_warning())
            {
                ui.add(
                    egui::Label::new(RichText::new(error).color(ui.visuals().warn_fg_color))
                        .truncate(),
                )
                .on_hover_text(error);
            } else if let Some(source) = &self.source_preview {
                ui.add(egui::Label::new(media_name(&source.path)).truncate())
                    .on_hover_text(source.path.display().to_string());
            }
        });
        ui.add_space(5.0);
        if let Err(error) =
            self.selection
                .show(ui, &mut self.preview, self.source_preview.is_none())
        {
            self.status(error, true);
        }
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.sidebar_tab, SidebarTab::Media, "Media");
            ui.selectable_value(&mut self.sidebar_tab, SidebarTab::Chat, "Chat");
        });
        ui.separator();
        match self.sidebar_tab {
            SidebarTab::Media => self.media_panel(ui),
            SidebarTab::Chat => self.chat_panel(ui),
        }
    }

    fn media_panel(&mut self, ui: &mut Ui) {
        let Some(workspace) = &self.workspace else {
            ui.label("Create or open a project to add media.");
            return;
        };
        let selected = self
            .source_preview
            .as_ref()
            .and_then(|source| workspace.library.get_key_value(&source.path));
        let (mut import, mut append) = (false, false);
        ui.add_enabled_ui(self.job.is_none(), |ui| {
            ui.horizontal(|ui| {
                import = ui.button("Add Media").clicked();
                append = ui
                    .add_enabled(
                        selected.is_some_and(|(_, kind)| {
                            *kind == MediaKind::Video || workspace.clip.is_some()
                        }),
                        egui::Button::new("Add to Timeline"),
                    )
                    .clicked();
            });
        });
        if import {
            self.open_dialog(PathAction::ImportMedia);
            return;
        }
        if append && let Some((path, kind)) = selected {
            let path = path.clone();
            match kind {
                MediaKind::Video => self.import_video(&ui.ctx().clone(), vec![path]),
                MediaKind::Audio => {
                    self.show_timeline_preview();
                    self.audio_form(AudioEdit::Add(path));
                }
            }
            return;
        }
        if workspace.library.is_empty() {
            ui.label("No media in this project.");
            return;
        }
        let mut pick = None;
        egui::ScrollArea::vertical()
            .id_salt("project-media")
            .show(ui, |ui| {
                for (path, kind) in &workspace.library {
                    let active = selected.is_some_and(|(selected, _)| path == selected);
                    let label = match kind {
                        MediaKind::Video => "Video",
                        MediaKind::Audio => "Audio",
                    };
                    let button =
                        egui::Button::selectable(active, format!("{label} · {}", media_name(path)))
                            .truncate();
                    if ui
                        .add_enabled(self.job.is_none(), button)
                        .on_hover_text(path.display().to_string())
                        .clicked()
                        && !active
                    {
                        pick = Some((path.clone(), *kind));
                    }
                }
            });
        if let Some((path, kind)) = pick {
            self.preview_media(&ui.ctx().clone(), path, kind);
        }
    }

    fn chat_panel(&mut self, ui: &mut Ui) {
        let mut selections = self.timeline.summary();
        if let Some(attachment) = &self.selection.attachment {
            selections.push(attachment.summary());
        }
        let directory = self
            .workspace
            .as_ref()
            .map(|workspace| workspace.directory.as_path());
        match self.chat.show(ui, directory, &selections) {
            ChatAction::None => {}
            ChatAction::ClearSelection => {
                self.timeline.clear();
                self.selection = PreviewSelection::default();
            }
            ChatAction::Send => self.send_chat(&selections),
        }
    }

    #[expect(
        clippy::unnecessary_debug_formatting,
        reason = "Escape path characters in chat context."
    )]
    fn send_chat(&mut self, selections: &[String]) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        let mut context = format!("Action video editing project: {:?}.\n", workspace.directory);
        if let Some(clip) = &workspace.clip {
            use std::fmt::Write as _;
            let _ = writeln!(
                context,
                "Active clip: {}. Duration: {} s. Playhead: {} s.",
                workspace.label(),
                seconds(clip.duration()),
                seconds(self.timeline_position())
            );
            context.push_str(&self.timeline.describe(clip, workspace.dirty));
        }
        if let Some(attachment) = &self.selection.attachment {
            context.push_str(&attachment.describe());
        }
        self.chat
            .send(&context, selections, self.selection.attachment.clone());
    }
}

impl eframe::App for Editor {
    fn logic(&mut self, ctx: &Context, _: &mut eframe::Frame) {
        self.poll_job(ctx);
        self.preview.poll(ctx);
        self.chat.poll(ctx);
        if !self.close_confirmed && ctx.input(|input| input.viewport().close_requested()) {
            if self.job.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            } else if self
                .workspace
                .as_ref()
                .is_some_and(|workspace| workspace.dirty)
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.destination = Some(Destination::Close);
            }
        }
        if self.job.is_none()
            && self.path_dialog.is_none()
            && self.audio_dialog.is_none()
            && self.destination.is_none()
        {
            let paths: Vec<_> = ctx.input(|input| {
                input
                    .raw
                    .dropped_files
                    .iter()
                    .map(|file| file.path().to_owned())
                    .collect()
            });
            if !paths.is_empty() {
                self.import_video(ctx, paths);
            }
        }
    }

    fn ui(&mut self, ui: &mut Ui, _: &mut eframe::Frame) {
        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui));
        egui::Panel::right("sidebar")
            .default_size(350.0)
            .size_range(275.0..=525.0)
            .show(ui, |ui| self.sidebar(ui));
        egui::Panel::bottom("timeline")
            .resizable(true)
            .default_size(240.0)
            .size_range(210.0..=420.0)
            .show(ui, |ui| self.timeline_panel(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            if !self.status.is_empty() {
                let color = if self.status_is_error {
                    ui.visuals().error_fg_color
                } else {
                    ACCENT
                };
                ui.add(egui::Label::new(RichText::new(&self.status).color(color)).wrap());
                ui.add_space(3.0);
            }
            self.preview_panel(ui);
        });
        self.show_dialogs(&ui.ctx().clone());
    }
}

enum AudioEdit {
    Add(PathBuf),
    Placement(Placement),
    Original,
}

struct AudioDialog {
    edit: AudioEdit,
    bounds: String,
    from: String,
    gain: String,
    error: String,
}

impl AudioDialog {
    fn show(&mut self, ctx: &Context) -> Option<bool> {
        let modal = egui::Modal::new(Id::unique("audio-edit")).show(ctx, |ui| {
            ui.set_min_width(300.0);
            ui.heading(match &self.edit {
                AudioEdit::Add(_) => "Add Audio",
                AudioEdit::Placement(_) => "Edit Audio",
                AudioEdit::Original => "Original Level",
            });
            if let AudioEdit::Add(path) = &self.edit {
                ui.label(media_name(path));
            }
            ui.add_space(5.0);
            egui::Grid::new("audio-fields")
                .spacing(Vec2::new(12.0, 7.0))
                .show(ui, |ui| {
                    ui.label("Timeline range (s)");
                    ui.add(egui::TextEdit::singleline(&mut self.bounds).desired_width(155.0));
                    ui.end_row();
                    let original = matches!(self.edit, AudioEdit::Original);
                    if !original {
                        ui.label("Source start (s)");
                        ui.add(egui::TextEdit::singleline(&mut self.from).desired_width(155.0));
                        ui.end_row();
                    }
                    ui.label(if original {
                        "Gain (dB or mute)"
                    } else {
                        "Gain (dB)"
                    });
                    ui.add(egui::TextEdit::singleline(&mut self.gain).desired_width(155.0));
                    ui.end_row();
                });
            if !self.error.is_empty() {
                ui.colored_label(ui.visuals().error_fg_color, &self.error);
            }
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                if ui.button("Apply").clicked() {
                    match self.values() {
                        Ok(_) => return Some(true),
                        Err(error) => self.error = error,
                    }
                }
                ui.button("Cancel").clicked().then_some(false)
            })
            .inner
        });
        modal
            .inner
            .or_else(|| modal.should_close().then_some(false))
    }

    fn values(&self) -> Result<(std::ops::Range<u64>, u64, Level), String> {
        let bounds = parse_bounds(OsStr::new(self.bounds.trim()))?;
        let from =
            parse_seconds(self.from.trim()).ok_or("Enter the source start in decimal seconds.")?;
        let gain = self.gain.trim();
        let level = if matches!(self.edit, AudioEdit::Original) && gain == "mute" {
            Level::Mute
        } else {
            Level::Gain(parse_gain(OsStr::new(gain))?)
        };
        Ok((bounds, from, level))
    }

    fn apply(self, clip: Clip) -> Result<Clip, String> {
        let (bounds, from, level) = self.values()?;
        match (self.edit, level) {
            (AudioEdit::Add(path), Level::Gain(gain)) => clip.add_audio(&path, bounds, from, gain),
            (AudioEdit::Placement(item), Level::Gain(gain)) => {
                clip.edit_audio(item.id, Some(bounds), Some(from), Some(gain))
            }
            (AudioEdit::Original, level) => clip.original_level(bounds, level),
            _ => Err("Set a gain in dB for the audio placement.".to_owned()),
        }
    }
}

fn time_label(time: u64) -> String {
    format!("{}.{:02}", time / 1_000_000, time / 10_000 % 100)
}

fn pointer_drag(
    ui: &Ui,
    response: &Response,
    origin: &mut Option<Pos2>,
) -> Option<(Pos2, Pos2, bool)> {
    let primary = egui::PointerButton::Primary;
    if response.drag_started_by(primary) {
        *origin = ui.input(|input| input.pointer.press_origin());
    }
    if let Some(start) = *origin {
        let finished = response.drag_stopped();
        if finished {
            *origin = None;
        }
        return response
            .interact_pointer_pos()
            .map(|end| (start, end, finished));
    }

    // A short gesture can deliver both button events in one egui frame.
    let (start, end) = ui.input(|input| {
        let mut start = None;
        let mut end = None;
        for event in &input.events {
            if let egui::Event::PointerButton {
                pos,
                button,
                pressed,
                ..
            } = event
                && *button == primary
            {
                if *pressed {
                    start = Some(*pos);
                    end = None;
                } else {
                    end = Some(*pos);
                }
            }
        }
        (start, end)
    });
    let (start, end) = (start?, end?);
    (response.enabled()
        && response.interact_rect.contains(start)
        && ui.ctx().layer_id_at(start) == Some(response.layer_id))
    .then_some((start, end, true))
}
