use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use eframe::egui::{self, RichText, Ui, Vec2};

use super::chat::{ChatClient, ChatEvent, DEFAULT_EFFORT, DEFAULT_MODEL, Model};
use super::preview_panel::Attachment;

pub(super) struct ChatPanel {
    client: Option<ChatClient>,
    models: Vec<Model>,
    model: String,
    effort: String,
    messages: Vec<(String, String)>,
    input: String,
    status: String,
    ready: bool,
    waiting: bool,
    response: Option<usize>,
    attachment: Option<Rc<Attachment>>,
}

impl Default for ChatPanel {
    fn default() -> Self {
        Self {
            client: None,
            models: Vec::new(),
            model: DEFAULT_MODEL.to_owned(),
            effort: DEFAULT_EFFORT.to_owned(),
            messages: Vec::new(),
            input: String::new(),
            status: String::new(),
            ready: false,
            waiting: false,
            response: None,
            attachment: None,
        }
    }
}

impl ChatPanel {
    pub fn open_project(&mut self, directory: &Path) {
        let model = std::mem::take(&mut self.model);
        let effort = std::mem::take(&mut self.effort);
        *self = Self {
            model,
            effort,
            ..Self::default()
        };
        self.connect(directory);
    }

    fn connect(&mut self, directory: &Path) {
        match ChatClient::start(directory) {
            Ok(client) => {
                self.client = Some(client);
                "Connecting to Codex…".clone_into(&mut self.status);
            }
            Err(error) => self.status = error,
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        let Some(client) = self.client.as_mut() else {
            return;
        };
        for event in client.poll() {
            match event {
                ChatEvent::Connected => {
                    self.ready = true;
                    self.status.clear();
                }
                ChatEvent::Models(models) => self.models = models,
                ChatEvent::Delta(delta) => {
                    if let Some((_, text)) =
                        self.response.and_then(|index| self.messages.get_mut(index))
                    {
                        text.push_str(&delta);
                    }
                }
                ChatEvent::Activity(activity) => self.status = activity,
                ChatEvent::Finished(error) => self.finish(error.unwrap_or_default()),
                ChatEvent::Error(error) => self.status = error,
                ChatEvent::Disconnected(error) => {
                    self.client = None;
                    self.ready = false;
                    self.finish(error);
                }
            }
        }
        if self.client.is_some() {
            ctx.request_repaint_after(Duration::from_millis(if self.ready && !self.waiting {
                200
            } else {
                30
            }));
        }
    }

    fn finish(&mut self, status: String) {
        self.waiting = false;
        self.attachment = None;
        if let Some(index) = self.response.take()
            && self
                .messages
                .get(index)
                .is_some_and(|(_, text)| text.is_empty())
        {
            self.messages.remove(index);
        }
        self.status = status;
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        directory: Option<&Path>,
        selections: &[String],
    ) -> ChatAction {
        ui.heading("Codex");
        ui.add_space(3.0);
        self.model_picker(ui);
        ui.separator();

        if directory.is_none() {
            ui.label("Create or open a project to start a chat.");
            return ChatAction::None;
        }
        if let Some(directory) = directory
            && self.client.is_none()
            && ui.button("Connect to Codex").clicked()
        {
            self.connect(directory);
        }
        if !self.status.is_empty() {
            ui.add(egui::Label::new(RichText::new(&self.status).small()).wrap());
        }

        let reserved = if selections.is_empty() { 111.0 } else { 144.0 };
        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - reserved).max(48.0))
            .min_scrolled_height((ui.available_height() - reserved).max(48.0))
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for (role, text) in &self.messages {
                    ui.label(RichText::new(role).strong());
                    ui.add(egui::Label::new(text).wrap().selectable(true));
                    ui.add_space(8.0);
                }
            });

        ui.separator();
        let mut action = ChatAction::None;
        if !selections.is_empty() {
            ui.horizontal(|ui| {
                if ui.small_button("Clear selection").clicked() {
                    action = ChatAction::ClearSelection;
                }
                ui.add(egui::Label::new(RichText::new(selections.join(" · ")).small()).truncate())
                    .on_hover_text(selections.join("\n"));
            });
        }
        let prompt = ui.add_sized(
            Vec2::new(ui.available_width(), 55.0),
            egui::TextEdit::multiline(&mut self.input)
                .hint_text("Ask about this edit…")
                .lock_focus(false),
        );
        let shortcut = prompt.has_focus()
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
        ui.horizontal(|ui| {
            let sendable = self.ready && !self.waiting && !self.input.trim().is_empty();
            if ui
                .add_enabled(sendable, egui::Button::new("Send"))
                .clicked()
                || (shortcut && sendable)
            {
                action = ChatAction::Send;
            }
            if self.waiting
                && ui.button("Stop").clicked()
                && let Some(client) = &mut self.client
                && let Err(error) = client.stop()
            {
                self.status = error;
            }
        });
        action
    }

    fn model_picker(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            let old_model = self.model.clone();
            egui::ComboBox::from_id_salt("codex-model")
                .selected_text(&self.model)
                .width(135.0)
                .show_ui(ui, |ui| {
                    if !self.models.iter().any(|model| model.id == DEFAULT_MODEL) {
                        ui.selectable_value(
                            &mut self.model,
                            DEFAULT_MODEL.to_owned(),
                            DEFAULT_MODEL,
                        );
                    }
                    for model in &self.models {
                        ui.selectable_value(&mut self.model, model.id.clone(), &model.id)
                            .on_hover_text(&model.name);
                    }
                });
            let model = self.models.iter().find(|model| model.id == self.model);
            if self.model != old_model {
                self.effort = model.filter(|model| model.id != DEFAULT_MODEL).map_or_else(
                    || DEFAULT_EFFORT.to_owned(),
                    |model| model.default_effort.clone(),
                );
            }
            egui::ComboBox::from_id_salt("codex-effort")
                .selected_text(format!("{} reasoning", self.effort))
                .width(106.0)
                .show_ui(ui, |ui| {
                    if let Some(model) = model.filter(|model| !model.efforts.is_empty()) {
                        if self.model == DEFAULT_MODEL
                            && !model.efforts.iter().any(|value| value == DEFAULT_EFFORT)
                        {
                            ui.selectable_value(
                                &mut self.effort,
                                DEFAULT_EFFORT.to_owned(),
                                DEFAULT_EFFORT,
                            );
                        }
                        for effort in &model.efforts {
                            ui.selectable_value(&mut self.effort, effort.clone(), effort);
                        }
                    } else {
                        for effort in ["low", "medium", "high", "xhigh", DEFAULT_EFFORT] {
                            ui.selectable_value(&mut self.effort, effort.to_owned(), effort);
                        }
                    }
                });
        });
    }

    pub fn send(
        &mut self,
        context: &str,
        selections: &[String],
        attachment: Option<Rc<Attachment>>,
    ) {
        let Some(client) = self.client.as_mut() else {
            return;
        };
        let text = self.input.trim();
        let image = attachment
            .as_ref()
            .map(|attachment| attachment.path.as_path());
        match client.send(&self.model, &self.effort, text, context, image) {
            Ok(()) => {
                let mut log = std::mem::take(&mut self.input);
                if !selections.is_empty() {
                    log.push_str("\n\nAttached · ");
                    log.push_str(&selections.join(" · "));
                }
                self.messages.push(("You".to_owned(), log));
                self.response = Some(self.messages.len());
                self.messages.push((self.model.clone(), String::new()));
                "Waiting for Codex…".clone_into(&mut self.status);
                self.waiting = true;
                self.attachment = attachment;
            }
            Err(error) => self.status = error,
        }
    }
}

pub(super) enum ChatAction {
    None,
    Send,
    ClearSelection,
}
