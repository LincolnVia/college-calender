use std::{
    fs,
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use chrono::{
    DateTime, Datelike, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc, Weekday,
};
use chrono_tz::Tz;
use eframe::egui::{self, Color32, CornerRadius, Frame, Margin, RichText, Stroke, Vec2};
use serde::{Deserialize, Serialize};

const STORAGE_KEY: &str = "college_cal_settings";
const AUTO_REFRESH: Duration = Duration::from_secs(15 * 60);

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 760.0])
            .with_min_inner_size([720.0, 560.0]),
        ..Default::default()
    };

    eframe::run_native(
        "College Cal",
        options,
        Box::new(|cc| Ok(Box::new(CollegeCal::new(cc)))),
    )
}

#[derive(Clone, Debug)]
struct CalendarEvent {
    summary: String,
    location: String,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

impl CalendarEvent {
    fn is_current(&self, now: DateTime<Utc>) -> bool {
        self.start <= now && now < self.end
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    source: String,
    weekly_font_scale: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            source: String::new(),
            weekly_font_scale: 1.2,
        }
    }
}

struct CollegeCal {
    settings: Settings,
    events: Vec<CalendarEvent>,
    load_result: Option<Receiver<Result<Vec<CalendarEvent>, String>>>,
    loading: bool,
    status: String,
    last_refresh: Option<Instant>,
    show_settings: bool,
    open_file_picker: bool,
}

impl CollegeCal {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_style(&cc.egui_ctx);
        let settings = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, STORAGE_KEY))
            .unwrap_or_else(|| Settings {
                source: std::env::var("COLLEGE_CAL_ICS").unwrap_or_default(),
                ..Default::default()
            });
        let needs_calendar = settings.source.trim().is_empty();
        let mut app = Self {
            settings,
            events: Vec::new(),
            load_result: None,
            loading: false,
            status: "Add your calendar to get started".into(),
            last_refresh: None,
            show_settings: false,
            open_file_picker: needs_calendar,
        };
        if !app.settings.source.trim().is_empty() {
            app.refresh();
        }
        app
    }

    fn refresh(&mut self) {
        if self.loading {
            return;
        }
        let source = self.settings.source.trim().to_owned();
        if source.is_empty() {
            self.show_settings = true;
            self.status = "Enter an ICS URL or file path".into();
            return;
        }

        let (tx, rx) = mpsc::channel();
        self.load_result = Some(rx);
        self.loading = true;
        self.status = "Syncing calendar…".into();
        thread::spawn(move || {
            let result = load_calendar(&source).and_then(|ics| parse_ics(&ics));
            let _ = tx.send(result);
        });
    }

    fn poll_refresh(&mut self) {
        let result = self
            .load_result
            .as_ref()
            .and_then(|receiver| receiver.try_recv().ok());
        if let Some(result) = result {
            self.loading = false;
            self.load_result = None;
            self.last_refresh = Some(Instant::now());
            match result {
                Ok(events) => {
                    self.status = format!("Synced {} calendar events", events.len());
                    self.events = events;
                }
                Err(error) => self.status = error,
            }
        }

        if !self.loading
            && self
                .last_refresh
                .is_some_and(|last| last.elapsed() >= AUTO_REFRESH)
        {
            self.refresh();
        }
    }

    fn current_and_next(
        &self,
        now: DateTime<Utc>,
    ) -> (Option<&CalendarEvent>, Option<&CalendarEvent>) {
        let current = self.events.iter().find(|event| event.is_current(now));
        let next = self.events.iter().find(|event| event.start > now);
        (current, next)
    }
}

impl eframe::App for CollegeCal {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, STORAGE_KEY, &self.settings);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_refresh();
        ctx.request_repaint_after(Duration::from_millis(250));

        // Defer the native dialog until the first frame has a real window to own it.
        if self.open_file_picker {
            self.open_file_picker = false;
            if let Some(path) = choose_ics_file() {
                self.settings.source = path;
                self.show_settings = false;
                self.refresh();
            }
        }

        let now = Utc::now();
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(12, 17, 25))
                    .inner_margin(24.0),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new("COLLEGE CAL")
                                        .size(13.0)
                                        .strong()
                                        .color(Color32::from_rgb(99, 204, 180)),
                                );
                                ui.label(
                                    RichText::new(Local::now().format("%A, %B %-d").to_string())
                                        .size(24.0)
                                        .strong()
                                        .color(Color32::WHITE),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("⚙  Calendar").clicked() {
                                        self.show_settings = !self.show_settings;
                                    }
                                    if ui
                                        .add_enabled(!self.loading, egui::Button::new("↻  Refresh"))
                                        .clicked()
                                    {
                                        self.refresh();
                                    }
                                },
                            );
                        });

                        ui.add_space(16.0);
                        if self.settings.source.trim().is_empty() && !self.show_settings {
                            Frame::new()
                                .fill(Color32::from_rgb(20, 28, 39))
                                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(39, 52, 67)))
                                .corner_radius(20.0)
                                .inner_margin(38.0)
                                .show(ui, |ui| {
                                    ui.set_min_height(300.0);
                                    ui.vertical_centered(|ui| {
                                        ui.add_space(35.0);
                                        ui.label(
                                            RichText::new("Choose your class calendar")
                                                .size(30.0)
                                                .strong()
                                                .color(Color32::WHITE),
                                        );
                                        ui.add_space(10.0);
                                        ui.label(
                                            RichText::new(
                                                "Select the .ics file exported by your school.",
                                            )
                                            .size(16.0)
                                            .color(Color32::from_gray(160)),
                                        );
                                        ui.add_space(28.0);
                                        if ui
                                            .add_sized(
                                                [190.0, 42.0],
                                                egui::Button::new("Choose .ics file…"),
                                            )
                                            .clicked()
                                        {
                                            self.open_file_picker = true;
                                        }
                                        if ui.link("Use a calendar URL instead").clicked() {
                                            self.show_settings = true;
                                        }
                                    });
                                });
                            return;
                        }

                        if self.show_settings {
                            settings_panel(
                                ui,
                                &mut self.settings.source,
                                &self.status,
                                self.loading,
                            );
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(!self.loading, egui::Button::new("Save & sync"))
                                    .clicked()
                                {
                                    self.refresh();
                                }
                                ui.label(
                                    RichText::new("Use a webcal/https URL or local .ics file")
                                        .small()
                                        .color(Color32::from_gray(140)),
                                );
                            });
                            ui.add_space(20.0);
                        }

                        let (current, next) = self.current_and_next(now);
                        current_card(ui, current, next, now);
                        ui.add_space(12.0);
                        upcoming_card(ui, current, next, now);
                        ui.add_space(20.0);
                        weekly_calendar(
                            ui,
                            &self.events,
                            Local::now().date_naive(),
                            &mut self.settings.weekly_font_scale,
                        );
                        ui.add_space(10.0);

                        ui.label(
                            RichText::new(&self.status)
                                .size(12.0)
                                .color(Color32::from_gray(120)),
                        );
                    });
            });
    }
}

fn settings_panel(ui: &mut egui::Ui, source: &mut String, status: &str, loading: bool) {
    Frame::new()
        .fill(Color32::from_rgb(19, 26, 37))
        .corner_radius(14.0)
        .inner_margin(18.0)
        .show(ui, |ui| {
            ui.label(
                RichText::new("Calendar source")
                    .strong()
                    .color(Color32::WHITE),
            );
            ui.add_space(6.0);
            let row_width = ui.available_width();
            ui.horizontal(|ui| {
                ui.add_sized(
                    [(row_width - 110.0).max(180.0), 34.0],
                    egui::TextEdit::singleline(source)
                        .hint_text("https://…/calendar.ics  or  /path/to/classes.ics"),
                );
                if ui
                    .add_sized([100.0, 34.0], egui::Button::new("Browse…"))
                    .clicked()
                    && let Some(path) = choose_ics_file()
                {
                    *source = path;
                }
            });
            if loading {
                ui.spinner();
            } else if status.to_ascii_lowercase().contains("error") {
                ui.colored_label(Color32::from_rgb(255, 130, 120), status);
            }
        });
}

fn choose_ics_file() -> Option<String> {
    rfd::FileDialog::new()
        .set_title("Choose an ICS calendar")
        .add_filter("iCalendar", &["ics", "ical"])
        .pick_file()
        .map(|path| path.to_string_lossy().into_owned())
}

fn current_card(
    ui: &mut egui::Ui,
    current: Option<&CalendarEvent>,
    next: Option<&CalendarEvent>,
    now: DateTime<Utc>,
) {
    let accent = Color32::from_rgb(99, 204, 180);
    Frame::new()
        .fill(Color32::from_rgb(20, 28, 39))
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(39, 52, 67)))
        .corner_radius(20.0)
        .inner_margin(Margin::same(18))
        .show(ui, |ui| {
            ui.set_min_height(145.0);
            if let Some(event) = current {
                let total = (event.end - event.start).num_milliseconds().max(1) as f32;
                let remaining_ms = (event.end - now).num_milliseconds().max(0);
                let fraction = (1.0 - remaining_ms as f32 / total).clamp(0.0, 1.0);

                ui.label(
                    RichText::new("IN CLASS NOW")
                        .size(12.0)
                        .strong()
                        .color(accent),
                );
                ui.add_space(5.0);
                ui.label(
                    RichText::new(&event.summary)
                        .size(34.0)
                        .strong()
                        .color(Color32::WHITE),
                );
                if !event.location.is_empty() {
                    ui.label(
                        RichText::new(format!("⌖  {}", event.location))
                            .size(16.0)
                            .color(Color32::from_gray(170)),
                    );
                }
                ui.add_space(14.0);

                let desired = Vec2::new(ui.available_width(), 12.0);
                let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
                ui.painter().rect_filled(
                    rect,
                    CornerRadius::same(6),
                    Color32::from_rgb(37, 48, 61),
                );
                let filled = egui::Rect::from_min_size(
                    rect.min,
                    Vec2::new(rect.width() * fraction, rect.height()),
                );
                ui.painter()
                    .rect_filled(filled, CornerRadius::same(6), accent);

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format_duration(remaining_ms))
                            .size(25.0)
                            .strong()
                            .color(Color32::WHITE),
                    );
                    ui.label(
                        RichText::new("remaining")
                            .size(14.0)
                            .color(Color32::from_gray(150)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("ends {}", local_time(event.end)))
                                .color(Color32::from_gray(150)),
                        );
                    });
                });
            } else {
                let (title, subtitle) = match next {
                    Some(event) => (
                        "You’re between classes",
                        format!("Next up: {} at {}", event.summary, local_time(event.start)),
                    ),
                    None => (
                        "You’re all clear",
                        "No more upcoming classes were found.".to_owned(),
                    ),
                };
                ui.add_space(12.0);
                ui.label(RichText::new("FREE TIME").size(12.0).strong().color(accent));
                ui.add_space(5.0);
                ui.label(
                    RichText::new(title)
                        .size(34.0)
                        .strong()
                        .color(Color32::WHITE),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(subtitle)
                        .size(17.0)
                        .color(Color32::from_gray(170)),
                );
            }
        });
}

fn upcoming_card(
    ui: &mut egui::Ui,
    current: Option<&CalendarEvent>,
    next: Option<&CalendarEvent>,
    now: DateTime<Utc>,
) {
    let Some(event) = next else { return };
    Frame::new()
        .fill(Color32::from_rgb(16, 22, 31))
        .corner_radius(14.0)
        .inner_margin(14.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(if current.is_some() {
                            "UP NEXT"
                        } else {
                            "NEXT CLASS"
                        })
                        .size(11.0)
                        .strong()
                        .color(Color32::from_gray(120)),
                    );
                    ui.label(
                        RichText::new(&event.summary)
                            .size(20.0)
                            .strong()
                            .color(Color32::WHITE),
                    );
                    if !event.location.is_empty() {
                        ui.label(RichText::new(&event.location).color(Color32::from_gray(145)));
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(local_time(event.start))
                                .size(19.0)
                                .strong()
                                .color(Color32::from_rgb(99, 204, 180)),
                        );
                        ui.label(
                            RichText::new(format!("in {}", compact_until(event.start - now)))
                                .color(Color32::from_gray(135)),
                        );
                    });
                });
            });
        });
}

fn weekly_calendar(
    ui: &mut egui::Ui,
    events: &[CalendarEvent],
    today: NaiveDate,
    font_scale: &mut f32,
) {
    let accent = Color32::from_rgb(99, 204, 180);
    let monday = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    let friday = monday + chrono::Duration::days(4);

    ui.horizontal(|ui| {
        ui.label(RichText::new("THIS WEEK").size(12.0).strong().color(accent));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(format!(
                    "{} – {}",
                    monday.format("%b %-d"),
                    friday.format("%b %-d")
                ))
                .size(13.0)
                .color(Color32::from_gray(145)),
            );
            ui.label(
                RichText::new(format!("{:.0}%", *font_scale * 100.0))
                    .size(11.0)
                    .color(Color32::from_gray(125)),
            );
            ui.add(
                egui::Slider::new(font_scale, 1.0..=1.6)
                    .show_value(false)
                    .text("Text size"),
            )
            .on_hover_text("Adjust the weekly calendar text size");
        });
    });
    ui.add_space(10.0);

    Frame::new()
        .fill(Color32::from_rgb(16, 22, 31))
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(35, 47, 61)))
        .corner_radius(16.0)
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.columns(5, |columns| {
                for (day_index, column) in columns.iter_mut().enumerate() {
                    let date = monday + chrono::Duration::days(day_index as i64);
                    let is_today = date == today;
                    let day_events: Vec<&CalendarEvent> = events
                        .iter()
                        .filter(|event| event.start.with_timezone(&Local).date_naive() == date)
                        .collect();

                    Frame::new()
                        .fill(if is_today {
                            Color32::from_rgb(25, 43, 48)
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(if is_today {
                            Stroke::new(1.0_f32, Color32::from_rgb(61, 134, 120))
                        } else {
                            Stroke::NONE
                        })
                        .corner_radius(12.0)
                        .inner_margin(8.0)
                        .show(column, |ui| {
                            ui.set_min_height(200.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new(date.format("%a").to_string().to_uppercase())
                                        .size(11.0 * *font_scale)
                                        .strong()
                                        .color(if is_today {
                                            accent
                                        } else {
                                            Color32::from_gray(125)
                                        }),
                                );
                                ui.label(
                                    RichText::new(date.day().to_string())
                                        .size(21.0 * *font_scale)
                                        .strong()
                                        .color(Color32::WHITE),
                                );
                            });
                            ui.add_space(7.0);
                            ui.separator();
                            ui.add_space(5.0);

                            if day_events.is_empty() {
                                ui.vertical_centered(|ui| {
                                    ui.add_space(18.0);
                                    ui.label(
                                        RichText::new("No classes")
                                            .size(11.0 * *font_scale)
                                            .color(Color32::from_gray(90)),
                                    );
                                });
                            } else {
                                for event in day_events {
                                    weekly_event_card(ui, event, accent, *font_scale);
                                    ui.add_space(6.0);
                                }
                            }
                        });
                }
            });
        });
}

fn weekly_event_card(ui: &mut egui::Ui, event: &CalendarEvent, accent: Color32, font_scale: f32) {
    let start = event.start.with_timezone(&Local);
    let end = event.end.with_timezone(&Local);
    Frame::new()
        .fill(Color32::from_rgb(25, 34, 45))
        .corner_radius(8.0)
        .inner_margin(7.0)
        .show(ui, |ui| {
            ui.label(
                RichText::new(format!(
                    "{}–{}",
                    start.format("%-I:%M"),
                    end.format("%-I:%M")
                ))
                .size(10.0 * font_scale)
                .strong()
                .color(accent),
            );
            ui.label(
                RichText::new(&event.summary)
                    .size(12.0 * font_scale)
                    .strong()
                    .color(Color32::WHITE),
            );
            if !event.location.is_empty() {
                ui.label(
                    RichText::new(&event.location)
                        .size(9.0 * font_scale)
                        .color(Color32::from_gray(125)),
                );
            }
        });
}

fn configure_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(10.0, 8.0);
    style.visuals.dark_mode = true;
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(26, 35, 47);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(36, 48, 62);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(45, 60, 74);
    style.visuals.window_corner_radius = CornerRadius::same(14);
    ctx.set_style(style);
}

fn load_calendar(source: &str) -> Result<String, String> {
    if source.starts_with("data:") {
        decode_calendar_data_uri(source)
    } else if source.starts_with("http://")
        || source.starts_with("https://")
        || source.starts_with("webcal://")
    {
        let url = source.replacen("webcal://", "https://", 1);
        let response = reqwest::blocking::Client::builder()
            .user_agent("college-cal/0.1")
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|error| format!("Calendar client error: {error}"))?
            .get(url)
            .send()
            .map_err(|error| format!("Could not download calendar: {error}"))?
            .error_for_status()
            .map_err(|error| format!("Calendar server error: {error}"))?;
        response
            .text()
            .map_err(|error| format!("Could not read calendar: {error}"))
    } else {
        fs::read_to_string(source).map_err(|error| format!("Could not open {source}: {error}"))
    }
}

fn decode_calendar_data_uri(source: &str) -> Result<String, String> {
    let (metadata, payload) = source
        .split_once(',')
        .ok_or_else(|| "Invalid calendar data URI".to_owned())?;
    if metadata.to_ascii_lowercase().contains(";base64") {
        return Err("Base64 calendar data URIs are not supported yet".into());
    }

    let bytes = payload.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err("Invalid percent escape in calendar data URI".into());
            }
            let high = hex_value(bytes[index + 1]);
            let low = hex_value(bytes[index + 2]);
            match (high, low) {
                (Some(high), Some(low)) => decoded.push((high << 4) | low),
                _ => return Err("Invalid percent escape in calendar data URI".into()),
            }
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8(decoded).map_err(|error| format!("Calendar data is not valid UTF-8: {error}"))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse_ics(input: &str) -> Result<Vec<CalendarEvent>, String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in input.replace("\r\n", "\n").lines() {
        if raw.starts_with([' ', '\t']) {
            if let Some(previous) = lines.last_mut() {
                previous.push_str(&raw[1..]);
            }
        } else {
            lines.push(raw.to_owned());
        }
    }

    let mut events = Vec::new();
    let mut in_event = false;
    let mut summary = String::new();
    let mut location = String::new();
    let mut start_spec: Option<(String, String)> = None;
    let mut end_spec: Option<(String, String)> = None;
    let mut recurrence = String::new();

    for line in lines {
        match line.as_str() {
            "BEGIN:VEVENT" => {
                in_event = true;
                summary.clear();
                location.clear();
                start_spec = None;
                end_spec = None;
                recurrence.clear();
            }
            "END:VEVENT" if in_event => {
                if let (Some((start_property, start_value)), Some((end_property, end_value))) =
                    (&start_spec, &end_spec)
                    && let (Some(start), Some(end)) = (
                        parse_ics_datetime(start_property, start_value),
                        parse_ics_datetime(end_property, end_value),
                    )
                    && end > start
                {
                    let event = CalendarEvent {
                        summary: if summary.is_empty() {
                            "Untitled class".into()
                        } else {
                            summary.clone()
                        },
                        location: location.clone(),
                        start,
                        end,
                    };
                    if recurrence.is_empty() {
                        events.push(event);
                    } else {
                        events.extend(expand_weekly_event(
                            event,
                            start_property,
                            start_value,
                            &recurrence,
                        ));
                    }
                }
                in_event = false;
            }
            _ if in_event => {
                let Some((property, value)) = line.split_once(':') else {
                    continue;
                };
                let name = property.split(';').next().unwrap_or(property);
                match name {
                    "SUMMARY" => summary = unescape_ics(value),
                    "LOCATION" => location = unescape_ics(value),
                    "DTSTART" => start_spec = Some((property.to_owned(), value.to_owned())),
                    "DTEND" => end_spec = Some((property.to_owned(), value.to_owned())),
                    "RRULE" => recurrence = value.to_owned(),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    // Keep enough history to render every day in the current week.
    let cutoff = Utc::now() - chrono::Duration::days(7);
    events.retain(|event| event.end >= cutoff);
    events.sort_by_key(|event| event.start);
    if events.is_empty() {
        Err("No current or upcoming timed events found in this calendar".into())
    } else {
        Ok(events)
    }
}

fn expand_weekly_event(
    event: CalendarEvent,
    start_property: &str,
    start_value: &str,
    rule: &str,
) -> Vec<CalendarEvent> {
    let parts: std::collections::HashMap<&str, &str> = rule
        .split(';')
        .filter_map(|part| part.split_once('='))
        .collect();
    if parts.get("FREQ") != Some(&"WEEKLY") {
        return vec![event];
    }

    let Some(base_naive) = parse_naive_ics_datetime(start_value) else {
        return vec![event];
    };
    let weekdays: Vec<Weekday> = parts
        .get("BYDAY")
        .map(|days| days.split(',').filter_map(parse_weekday).collect())
        .unwrap_or_else(|| vec![base_naive.weekday()]);
    let interval = parts
        .get("INTERVAL")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(1)
        .max(1);
    let count = parts
        .get("COUNT")
        .and_then(|value| value.parse::<usize>().ok());
    let until = parts
        .get("UNTIL")
        .and_then(|value| parse_ics_datetime(start_property, value));
    let duration = event.end - event.start;
    let week_anchor = base_naive.date()
        - chrono::Duration::days(base_naive.weekday().num_days_from_monday() as i64);
    let last_date = base_naive.date() + chrono::Duration::days(366 * 5);
    let mut date = base_naive.date();
    let mut occurrences = Vec::new();

    while date <= last_date && occurrences.len() < count.unwrap_or(2_000) {
        let weeks_since_anchor = (date - week_anchor).num_days() / 7;
        if weeks_since_anchor % interval == 0 && weekdays.contains(&date.weekday()) {
            let candidate_naive = date.and_time(base_naive.time());
            if let Some(start) = localize_ics_datetime(start_property, candidate_naive) {
                if until.is_some_and(|limit| start > limit) {
                    break;
                }
                occurrences.push(CalendarEvent {
                    summary: event.summary.clone(),
                    location: event.location.clone(),
                    start,
                    end: start + duration,
                });
            }
        }
        date += chrono::Duration::days(1);
    }

    if occurrences.is_empty() {
        vec![event]
    } else {
        occurrences
    }
}

fn parse_weekday(value: &str) -> Option<Weekday> {
    match value.trim_start_matches(|character: char| {
        character == '+' || character == '-' || character.is_ascii_digit()
    }) {
        "MO" => Some(Weekday::Mon),
        "TU" => Some(Weekday::Tue),
        "WE" => Some(Weekday::Wed),
        "TH" => Some(Weekday::Thu),
        "FR" => Some(Weekday::Fri),
        "SA" => Some(Weekday::Sat),
        "SU" => Some(Weekday::Sun),
        _ => None,
    }
}

fn parse_naive_ics_datetime(value: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value.trim_end_matches('Z'), "%Y%m%dT%H%M%S")
        .or_else(|_| NaiveDateTime::parse_from_str(value.trim_end_matches('Z'), "%Y%m%dT%H%M"))
        .ok()
}

fn parse_ics_datetime(property: &str, value: &str) -> Option<DateTime<Utc>> {
    if value.len() == 8 {
        let date = NaiveDate::parse_from_str(value, "%Y%m%d").ok()?;
        let local = date.and_hms_opt(0, 0, 0)?;
        return local_to_utc(local);
    }
    if value.ends_with('Z') {
        return NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%SZ")
            .ok()
            .map(|datetime| datetime.and_utc());
    }
    let naive = parse_naive_ics_datetime(value)?;
    localize_ics_datetime(property, naive)
}

fn localize_ics_datetime(property: &str, naive: NaiveDateTime) -> Option<DateTime<Utc>> {
    if let Some(tzid) = property
        .split(';')
        .skip(1)
        .find_map(|part| part.strip_prefix("TZID="))
    {
        let tzid = tzid.trim_matches('"');
        let normalized = match tzid {
            "EDT" | "EST" | "US/Eastern" => "America/New_York",
            "CDT" | "CST" | "US/Central" => "America/Chicago",
            "MDT" | "MST" | "US/Mountain" => "America/Denver",
            "PDT" | "PST" | "US/Pacific" => "America/Los_Angeles",
            other => other,
        };
        if let Ok(timezone) = normalized.parse::<Tz>() {
            return match timezone.from_local_datetime(&naive) {
                LocalResult::Single(datetime) => Some(datetime.to_utc()),
                LocalResult::Ambiguous(first, _) => Some(first.to_utc()),
                LocalResult::None => None,
            };
        }
    }
    local_to_utc(naive)
}

fn local_to_utc(naive: NaiveDateTime) -> Option<DateTime<Utc>> {
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(datetime) => Some(datetime.to_utc()),
        LocalResult::Ambiguous(first, _) => Some(first.to_utc()),
        LocalResult::None => None,
    }
}

fn unescape_ics(value: &str) -> String {
    value
        .replace("\\n", " · ")
        .replace("\\N", " · ")
        .replace("\\,", ",")
        .replace("\\;", ";")
        .replace("\\\\", "\\")
}

fn local_time(datetime: DateTime<Utc>) -> String {
    datetime
        .with_timezone(&Local)
        .format("%-I:%M %p")
        .to_string()
}

fn format_duration(milliseconds: i64) -> String {
    let seconds = (milliseconds / 1_000).max(0);
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn compact_until(duration: chrono::Duration) -> String {
    let minutes = duration.num_minutes().max(0);
    if minutes >= 60 {
        format!("{}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_folded_text_and_timezone_dates() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:Operating Sys\r\n tems\r\nLOCATION:Jordan Hall\\, 101\r\nDTSTART;TZID=America/Indiana/Indianapolis:20900825T100000\r\nDTEND;TZID=America/Indiana/Indianapolis:20900825T111500\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let events = parse_ics(ics).expect("calendar should parse");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].summary, "Operating Systems");
        assert_eq!(events[0].location, "Jordan Hall, 101");
        assert_eq!(
            events[0].end - events[0].start,
            chrono::Duration::minutes(75)
        );
    }

    #[test]
    fn parses_utc_dates() {
        let parsed = parse_ics_datetime("DTSTART", "20900102T030405Z").unwrap();
        assert_eq!(
            parsed.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2090-01-02 03:04:05"
        );
    }

    #[test]
    fn formats_countdown() {
        assert_eq!(format_duration(65_000), "1:05");
        assert_eq!(format_duration(3_665_000), "1:01:05");
    }

    #[test]
    fn decodes_percent_encoded_calendar_data_uri() {
        let source = "data:text/calendar;charset=UTF-8,BEGIN%3AVCALENDAR%0D%0ASUMMARY%3ATest%20Class%0D%0AEND%3AVCALENDAR";
        let decoded = decode_calendar_data_uri(source).unwrap();

        assert_eq!(
            decoded,
            "BEGIN:VCALENDAR\r\nSUMMARY:Test Class\r\nEND:VCALENDAR"
        );
    }

    #[test]
    fn expands_weekly_rule_using_byday() {
        let start_property = "DTSTART;TZID=EDT;VALUE=DATE-TIME";
        let start_value = "20260824T120000";
        let start = parse_ics_datetime(start_property, start_value).unwrap();
        let event = CalendarEvent {
            summary: "NEWM-N 115".into(),
            location: "ICTC 270".into(),
            start,
            end: start + chrono::Duration::minutes(75),
        };

        let events = expand_weekly_event(
            event,
            start_property,
            start_value,
            "FREQ=WEEKLY;INTERVAL=1;BYDAY=TU;UNTIL=20260902T235959",
        );

        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| {
            event
                .start
                .with_timezone(&chrono_tz::America::New_York)
                .weekday()
                == Weekday::Tue
        }));
    }
}
