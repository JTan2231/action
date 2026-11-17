use std::env;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Color32, Pos2, Rect, Ui, Vec2};

use super::preview::{Frame, Preview};
use super::timeline::ACCENT;
use crate::seconds;

#[derive(Default)]
pub(super) struct PreviewSelection {
    drag: Option<Pos2>,
    pub attachment: Option<Rc<Attachment>>,
}

impl PreviewSelection {
    pub fn show(&mut self, ui: &mut Ui, preview: &mut Preview, loaded: bool) -> Result<(), String> {
        let (area, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
        ui.painter()
            .rect_filled(area, 6, Color32::from_rgb(12, 14, 18));
        let Some(frame) = preview.frame() else {
            let text = preview.error().unwrap_or(if loaded {
                "Loading preview…"
            } else {
                "Add a video to preview it here."
            });
            ui.painter().text(
                area.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(14.0),
                ui.visuals().weak_text_color(),
            );
            return Ok(());
        };

        let size = frame.texture.size_vec2();
        let display = Rect::from_center_size(
            area.center(),
            size * (area.width() / size.x).min(area.height() / size.y),
        );
        let image_uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        ui.painter()
            .image(frame.texture.id(), display, image_uv, Color32::WHITE);
        let timestamp = frame.timestamp;
        let response = ui
            .interact(
                display,
                ui.scope_id().with("preview-frame"),
                egui::Sense::drag(),
            )
            .on_hover_cursor(egui::CursorIcon::Crosshair);
        if let Some((origin, pointer, finished)) =
            super::pointer_drag(ui, &response, &mut self.drag)
        {
            preview.pause();
            let region = Rect::from_two_pos(display.clamp(origin), display.clamp(pointer));
            draw_region(ui, region);
            if finished
                && region.width() >= 3.0
                && region.height() >= 3.0
                && let Some(frame) = preview.frame()
            {
                let min = (region.min - display.min) / display.size();
                let max = (region.max - display.min) / display.size();
                self.attachment = Some(Rc::new(Attachment::create(
                    frame,
                    [min.x, min.y, max.x, max.y],
                )?));
                ui.ctx().request_repaint();
            }
        } else if let Some(attachment) = &self.attachment
            && attachment.timestamp == timestamp
        {
            let [x1, y1, x2, y2] = attachment.region;
            draw_region(
                ui,
                Rect::from_min_max(
                    display.min + Vec2::new(x1, y1) * display.size(),
                    display.min + Vec2::new(x2, y2) * display.size(),
                ),
            );
        }
        Ok(())
    }
}

fn draw_region(ui: &Ui, rect: Rect) {
    ui.painter()
        .rect_filled(rect, 0, Color32::from_rgba_unmultiplied(142, 210, 255, 40));
    ui.painter().rect_stroke(
        rect,
        0,
        egui::Stroke::new(2.0, ACCENT),
        egui::StrokeKind::Inside,
    );
}

pub(super) struct Attachment {
    directory: PathBuf,
    pub path: PathBuf,
    pub timestamp: u64,
    region: [f32; 4],
}

impl Attachment {
    fn create(frame: &Frame, region: [f32; 4]) -> Result<Self, String> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let directory =
            env::temp_dir().join(format!("action-frame-{}-{stamp}", std::process::id()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(0o700);
        }
        builder
            .create(&directory)
            .map_err(|error| format!("Cannot create frame attachment. {error}"))?;
        let attachment = Self {
            path: directory.join("selection.png"),
            directory,
            timestamp: frame.timestamp,
            region,
        };
        frame.save_region(region, &attachment.path)?;
        Ok(attachment)
    }

    pub fn summary(&self) -> String {
        format!("Frame at {} s", seconds(self.timestamp))
    }

    pub fn describe(&self) -> String {
        let [x1, y1, x2, y2] = self.region;
        format!(
            "Attached cropped video frame at {} s. Normalized frame bounds (left, top, right, bottom): {x1:.3}, {y1:.3}, {x2:.3}, {y2:.3}.",
            seconds(self.timestamp)
        )
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_dir(&self.directory);
    }
}
