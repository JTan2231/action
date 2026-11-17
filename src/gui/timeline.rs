use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::ops::Range;
use std::path::Path;
use std::time::Duration;

use eframe::egui::{self, Color32, Pos2, Rect, Response, Ui, Vec2};

use crate::project::{Clip, Level};
use crate::seconds;

const LABEL_WIDTH: f32 = 100.0;
const RULER_HEIGHT: f32 = 28.0;
const TRACK_HEIGHT: f32 = 34.0;
pub(super) const ACCENT: Color32 = Color32::from_rgb(142, 210, 255);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Item {
    Video(usize),
    Audio(u64),
    Original(usize),
}

#[derive(Default)]
pub(super) struct Timeline {
    items: BTreeSet<Item>,
    pub range: Option<Range<u64>>,
    drag_start: Option<Pos2>,
}

impl Timeline {
    pub fn selected_audio(&self) -> Option<u64> {
        match (self.items.len(), self.items.first()) {
            (1, Some(Item::Audio(id))) => Some(*id),
            _ => None,
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn summary(&self) -> Vec<String> {
        let mut labels: Vec<_> = self
            .items
            .iter()
            .map(|item| match item {
                Item::Video(index) => format!("Video {}", index + 1),
                Item::Audio(id) => format!("Audio a{id}"),
                Item::Original(index) => format!("Original level {}", index + 1),
            })
            .collect();
        if let Some(range) = &self.range {
            labels.push(format!("{}–{} s", seconds(range.start), seconds(range.end)));
        }
        labels
    }

    pub fn show(&mut self, ui: &mut Ui, clip: &Clip, position: u64) -> Option<u64> {
        let width = ui.available_width().max(180.0);
        let duration = clip.duration();
        let mut seek = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .scroll_source(
                egui::scroll_area::ScrollSource::SCROLL_BAR
                    | egui::scroll_area::ScrollSource::MOUSE_WHEEL,
            )
            .show(ui, |ui| {
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(width, RULER_HEIGHT), egui::Sense::hover());
                let rect = content_rect(rect);
                let response = ui
                    .interact(rect, ui.scope_id().with("ruler"), egui::Sense::drag())
                    .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                self.ruler_input(ui, &response, duration, &mut seek);
                draw_ruler(ui, rect, duration);
                let ruler = rect;

                let video = track(ui, width, "Video");
                let mut start = 0;
                for (index, span) in clip.spans.iter().enumerate() {
                    let end = start + (span.end - span.start);
                    let item = Item::Video(index);
                    let block = time_rect(video, duration, start..end);
                    if self.block(ui, item, block, media_name(&span.path).as_ref()) {
                        seek = Some(start);
                    }
                    start = end;
                }

                let original = track(ui, width, "Original audio");
                ui.painter()
                    .rect_filled(original, 3, Color32::from_rgb(39, 47, 48));
                for (index, adjustment) in clip.original.iter().enumerate() {
                    let block = time_rect(original, duration, adjustment.start..adjustment.end);
                    let label = match adjustment.level {
                        Level::Gain(gain) => format!("{gain} dB"),
                        Level::Mute => "Mute".to_owned(),
                    };
                    if self.block(ui, Item::Original(index), block, &label) {
                        seek = Some(adjustment.start);
                    }
                }

                let mut bottom = original.bottom();
                for audio in &clip.audio {
                    let track = track(ui, width, &format!("Audio a{}", audio.id));
                    let block = time_rect(track, duration, audio.start..audio.end);
                    if self.block(
                        ui,
                        Item::Audio(audio.id),
                        block,
                        media_name(&audio.path).as_ref(),
                    ) {
                        seek = Some(audio.start);
                    }
                    bottom = track.bottom();
                }
                let content = Rect::from_min_max(ruler.min, Pos2::new(ruler.right(), bottom));
                self.draw_markers(ui, content, duration, seek.unwrap_or(position));
            });
        seek
    }

    fn ruler_input(&mut self, ui: &Ui, response: &Response, duration: u64, seek: &mut Option<u64>) {
        if let Some((origin, pointer, _)) = super::pointer_drag(ui, response, &mut self.drag_start)
        {
            let position = time_at(response.rect, pointer.x, duration);
            if (pointer.x - origin.x).abs() > 3.0 {
                let start = time_at(response.rect, origin.x, duration);
                let bounds = start.min(position)..start.max(position);
                if !bounds.is_empty() && self.range.as_ref() != Some(&bounds) {
                    self.range = Some(bounds);
                    ui.ctx().request_repaint();
                }
            }
            *seek = Some(position);
        }
    }

    fn block(&mut self, ui: &Ui, item: Item, rect: Rect, label: &str) -> bool {
        let response = ui.interact(
            rect,
            ui.scope_id().with((
                match item {
                    Item::Video(_) => "video",
                    Item::Audio(_) => "audio",
                    Item::Original(_) => "original",
                },
                match item {
                    Item::Video(index) | Item::Original(index) => index as u64,
                    Item::Audio(id) => id,
                },
            )),
            egui::Sense::click(),
        );
        let clicked = response.clicked();
        if clicked {
            ui.ctx().request_repaint();
            if ui.input(|input| input.modifiers.shift) {
                if !self.items.insert(item) {
                    self.items.remove(&item);
                }
            } else {
                self.items.clear();
                self.items.insert(item);
            }
        }
        let fill = match item {
            Item::Video(_) => Color32::from_rgb(52, 76, 116),
            Item::Audio(_) => Color32::from_rgb(33, 94, 83),
            Item::Original(_) => Color32::from_rgb(64, 100, 89),
        };
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        painter.rect_filled(
            rect,
            4,
            if response.hovered() {
                fill.gamma_multiply(1.25)
            } else {
                fill
            },
        );
        if self.items.contains(&item) {
            painter.rect_stroke(
                rect,
                4,
                egui::Stroke::new(2.0, ACCENT),
                egui::StrokeKind::Inside,
            );
        }
        painter.text(
            rect.left_center() + Vec2::new(7.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(12.0),
            Color32::WHITE,
        );
        response.on_hover_text(label);
        clicked
    }

    fn draw_markers(&self, ui: &Ui, rect: Rect, duration: u64, position: u64) {
        let painter = ui.painter();
        if let Some(range) = &self.range {
            let band = time_rect(rect, duration, range.clone());
            painter.rect_filled(band, 0, Color32::from_rgba_unmultiplied(142, 210, 255, 30));
            for x in [band.left(), band.right()] {
                painter.vline(x, rect.y_range(), egui::Stroke::new(1.0, ACCENT));
            }
        }
        let x = x_at(rect, position, duration).min(rect.right() - 1.0);
        painter.vline(x, rect.y_range(), egui::Stroke::new(1.5, Color32::WHITE));
    }

    #[expect(
        clippy::unnecessary_debug_formatting,
        reason = "Escape path characters in chat context."
    )]
    pub fn describe(&self, clip: &Clip, include_draft: bool) -> String {
        let mut text = String::new();
        if let Some(range) = &self.range {
            let _ = writeln!(
                text,
                "Highlighted time range: {}..{} s.",
                seconds(range.start),
                seconds(range.end)
            );
        }
        let mut start = 0;
        for (index, span) in clip.spans.iter().enumerate() {
            let end = start + (span.end - span.start);
            let highlighted = self.items.contains(&Item::Video(index));
            if include_draft || highlighted || self.intersects(start, end) {
                let mark = if highlighted {
                    "Highlighted video"
                } else {
                    "Video"
                };
                let _ = writeln!(
                    text,
                    "{mark} {}: {:?}, timeline {}..{} s, source {}..{} s.",
                    index + 1,
                    span.path,
                    seconds(start),
                    seconds(end),
                    seconds(span.start),
                    seconds(span.end),
                );
            }
            start = end;
        }
        for audio in &clip.audio {
            let highlighted = self.items.contains(&Item::Audio(audio.id));
            if include_draft || highlighted || self.intersects(audio.start, audio.end) {
                let mark = if highlighted {
                    "Highlighted audio"
                } else {
                    "Audio"
                };
                let _ = writeln!(
                    text,
                    "{mark} a{}: {:?}, timeline {}..{} s, source starts at {} s, gain {} dB.",
                    audio.id,
                    audio.path,
                    seconds(audio.start),
                    seconds(audio.end),
                    seconds(audio.from),
                    audio.gain,
                );
            }
        }
        for (index, original) in clip.original.iter().enumerate() {
            let highlighted = self.items.contains(&Item::Original(index));
            if include_draft || highlighted || self.intersects(original.start, original.end) {
                let mark = if highlighted {
                    "Highlighted original sound"
                } else {
                    "Original sound"
                };
                let level = match original.level {
                    Level::Gain(gain) => format!("{gain} dB"),
                    Level::Mute => "muted".to_owned(),
                };
                let _ = writeln!(
                    text,
                    "{mark}: {}..{} s, {level}.",
                    seconds(original.start),
                    seconds(original.end)
                );
            }
        }
        text
    }

    fn intersects(&self, start: u64, end: u64) -> bool {
        self.range
            .as_ref()
            .is_some_and(|range| range.start < end && start < range.end)
    }
}

fn track(ui: &mut Ui, width: f32, label: &str) -> Rect {
    let (row, _) = ui.allocate_exact_size(Vec2::new(width, TRACK_HEIGHT), egui::Sense::hover());
    ui.painter().text(
        row.left_center() + Vec2::new(5.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.0),
        ui.visuals().text_color(),
    );
    let rect = content_rect(row);
    ui.painter()
        .rect_filled(rect, 4, Color32::from_rgb(28, 31, 38));
    rect
}

fn content_rect(rect: Rect) -> Rect {
    Rect::from_min_max(rect.min + Vec2::new(LABEL_WIDTH, 0.0), rect.max)
}

fn draw_ruler(ui: &Ui, rect: Rect, duration: u64) {
    let painter = ui.painter();
    let mut step = 100_000_u64;
    let spacing = 74.0 / f64::from(rect.width());
    let minimum_step = Duration::from_micros(duration).mul_f64(spacing);
    while u128::from(step) < minimum_step.as_micros() {
        step = step.saturating_mul(2);
    }
    let mut time = 0;
    while time < duration {
        let x = x_at(rect, time, duration);
        painter.vline(
            x,
            rect.bottom() - 5.0..=rect.bottom(),
            egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
        );
        painter.text(
            Pos2::new(x + 4.0, rect.center().y - 2.0),
            egui::Align2::LEFT_CENTER,
            format!("{} s", seconds(time)),
            egui::FontId::monospace(11.0),
            ui.visuals().weak_text_color(),
        );
        let Some(next) = time.checked_add(step) else {
            break;
        };
        time = next;
    }
}

fn x_at(rect: Rect, time: u64, duration: u64) -> f32 {
    egui::lerp(
        rect.x_range(),
        Duration::from_micros(time).as_secs_f32() / Duration::from_micros(duration).as_secs_f32(),
    )
}

fn time_at(rect: Rect, x: f32, duration: u64) -> u64 {
    let fraction = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    let time = Duration::from_micros(duration).mul_f64(f64::from(fraction));
    u64::try_from(time.as_micros())
        .unwrap_or(duration)
        .min(duration)
}

fn time_rect(rect: Rect, duration: u64, bounds: Range<u64>) -> Rect {
    let start = x_at(rect, bounds.start, duration);
    let end = x_at(rect, bounds.end, duration);
    Rect::from_min_max(
        Pos2::new(start, rect.top()),
        Pos2::new(
            (end - 1.0).max(start + 1.0).min(rect.right()),
            rect.bottom(),
        ),
    )
}

pub(super) fn media_name(path: &Path) -> std::borrow::Cow<'_, str> {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
}
