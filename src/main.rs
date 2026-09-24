use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
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
    calendar_index: usize,
    calendar_name: String,
    summary: String,
    location: String,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct CalendarSource {
    name: String,
    source: String,
}

impl CalendarEvent {
    fn is_current(&self, now: DateTime<Utc>) -> bool {
        self.start <= now && now < self.end
    }
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    // Kept for migrating settings saved by versions before multi-calendar support.
    source: String,
    calendars: Vec<CalendarSource>,
    main_calendar: usize,
    weekly_font_scale: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            source: String::new(),
            calendars: Vec::new(),
            main_calendar: 0,
            weekly_font_scale: 1.2,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AppTab {
    #[default]
    Dashboard,
    Compare,
}

struct CollegeCal {
    settings: Settings,
    events: Vec<CalendarEvent>,
    load_result: Option<Receiver<Result<Vec<CalendarEvent>, String>>>,
    loading: bool,
    status: String,
    last_refresh: Option<Instant>,
    show_settings: bool,
    active_tab: AppTab,
}

impl CollegeCal {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_style(&cc.egui_ctx);
        let mut settings = cc
            .storage
            .and_then(|storage| eframe::get_value(storage, STORAGE_KEY))
            .unwrap_or_else(|| Settings {
                source: std::env::var("COLLEGE_CAL_ICS").unwrap_or_default(),
                ..Default::default()
            });
        if settings.calendars.is_empty() && !settings.source.trim().is_empty() {
            settings.calendars.push(CalendarSource {
                name: calendar_name_from_source(&settings.source),
                source: std::mem::take(&mut settings.source),
            });
        }
        sync_ics_directory(&mut settings.calendars);
        let mut app = Self {
            settings,
            events: Vec::new(),
            load_result: None,
            loading: false,
            status: "Add your calendar to get started".into(),
            last_refresh: None,
            show_settings: false,
            active_tab: AppTab::Dashboard,
        };
        if !app.settings.calendars.is_empty() {
            app.refresh();
        }
        app
    }

    fn refresh(&mut self) {
        if self.loading {
            return;
        }
        sync_ics_directory(&mut self.settings.calendars);
        let calendars = self.settings.calendars.clone();
        if !calendars
            .iter()
            .any(|calendar| !calendar.source.trim().is_empty())
        {
            self.show_settings = true;
            self.status = "Add an ICS URL or file".into();
            return;
        }

        let (tx, rx) = mpsc::channel();
        self.load_result = Some(rx);
        self.loading = true;
        self.status = "Syncing calendar…".into();
        thread::spawn(move || {
            let result = load_calendars(&calendars);
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
        calendar_index: usize,
    ) -> (Option<&CalendarEvent>, Option<&CalendarEvent>) {
        let current = self
            .events
            .iter()
            .filter(|event| event.calendar_index == calendar_index)
            .find(|event| event.is_current(now));
        let next = self
            .events
            .iter()
            .filter(|event| event.calendar_index == calendar_index)
            .find(|event| event.start > now);
        (current, next)
    }
}

fn load_calendars(calendars: &[CalendarSource]) -> Result<Vec<CalendarEvent>, String> {
    let mut all_events = Vec::new();
    for (calendar_index, calendar) in calendars.iter().enumerate() {
        if calendar.source.trim().is_empty() {
            continue;
        }
        let name = calendar_display_name(calendar);
        let ics =
            load_calendar(calendar.source.trim()).map_err(|error| format!("{name}: {error}"))?;
        let mut events = parse_ics(&ics).map_err(|error| format!("{name}: {error}"))?;
        for event in &mut events {
            event.calendar_index = calendar_index;
            event.calendar_name.clone_from(&name);
        }
        all_events.extend(events);
    }
    all_events.sort_by_key(|event| event.start);
    Ok(all_events)
}

fn calendar_display_name(calendar: &CalendarSource) -> String {
    if calendar.name.trim().is_empty() {
        calendar_name_from_source(&calendar.source)
    } else {
        calendar.name.trim().to_owned()
    }
}

fn calendar_name_from_source(source: &str) -> String {
    Path::new(source)
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Calendar")
        .to_owned()
}

fn sync_ics_directory(calendars: &mut Vec<CalendarSource>) {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("ics");
    for path in discover_ics_files(&directory) {
        let source = path.to_string_lossy().into_owned();
        if calendars
            .iter()
            .any(|calendar| Path::new(&calendar.source) == path)
        {
            continue;
        }

        let filename = path.file_name();
        if let Some(calendar) = calendars.iter_mut().find(|calendar| {
            !Path::new(&calendar.source).exists()
                && Path::new(&calendar.source).file_name() == filename
        }) {
            calendar.source = source;
            continue;
        }

        calendars.push(CalendarSource {
            name: calendar_name_from_source(&source),
            source,
        });
    }
}

fn discover_ics_files(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("ics") || extension.eq_ignore_ascii_case("ical")
                })
        })
        .collect();
    paths.sort();
    paths
}

impl eframe::App for CollegeCal {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, STORAGE_KEY, &self.settings);
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.poll_refresh();
        ctx.request_repaint_after(Duration::from_millis(250));

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
                        ui.horizontal(|ui| {
                            ui.selectable_value(
                                &mut self.active_tab,
                                AppTab::Dashboard,
                                "Dashboard",
                            );
                            ui.selectable_value(
                                &mut self.active_tab,
                                AppTab::Compare,
                                "Compare timeline",
                            );
                        });
                        ui.add_space(12.0);

                        if self.settings.calendars.is_empty() && !self.show_settings {
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
                                                egui::Button::new("Set up calendar…"),
                                            )
                                            .clicked()
                                        {
                                            self.settings.calendars.push(CalendarSource::default());
                                            self.show_settings = true;
                                        }
                                        if ui.link("Use a calendar URL instead").clicked() {
                                            self.settings.calendars.push(CalendarSource::default());
                                            self.show_settings = true;
                                        }
                                    });
                                });
                            return;
                        }

                        if self.show_settings {
                            let mut browse_index = None;
                            settings_panel(ui, &mut self.settings.calendars, &mut browse_index);
                            if let Some(index) = browse_index
                                && let Some(path) = choose_ics_file()
                                && let Some(calendar) = self.settings.calendars.get_mut(index)
                            {
                                if calendar.name.trim().is_empty() {
                                    calendar.name = calendar_name_from_source(&path);
                                }
                                calendar.source = path;
                            }
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

                        match self.active_tab {
                            AppTab::Dashboard => {
                                self.settings.main_calendar = self
                                    .settings
                                    .main_calendar
                                    .min(self.settings.calendars.len().saturating_sub(1));
                                if self
                                    .settings
                                    .calendars
                                    .get(self.settings.main_calendar)
                                    .is_none_or(|calendar| calendar.source.trim().is_empty())
                                    && let Some(index) = self
                                        .settings
                                        .calendars
                                        .iter()
                                        .position(|calendar| !calendar.source.trim().is_empty())
                                {
                                    self.settings.main_calendar = index;
                                }
                                main_calendar_selector(
                                    ui,
                                    &self.settings.calendars,
                                    &mut self.settings.main_calendar,
                                );
                                ui.add_space(12.0);
                                let main_calendar = self.settings.main_calendar;
                                let (current, next) = self.current_and_next(now, main_calendar);
                                current_card(ui, current, next, now);
                                ui.add_space(12.0);
                                upcoming_card(ui, current, next, now);
                                ui.add_space(20.0);
                                let main_events: Vec<CalendarEvent> = self
                                    .events
                                    .iter()
                                    .filter(|event| event.calendar_index == main_calendar)
                                    .cloned()
                                    .collect();
                                weekly_calendar(
                                    ui,
                                    &main_events,
                                    Local::now().date_naive(),
                                    &mut self.settings.weekly_font_scale,
                                );
                            }
                            AppTab::Compare => comparison_timeline(
                                ui,
                                &self.events,
                                &self.settings.calendars,
                                Local::now().date_naive(),
                            ),
                        }
                        ui.add_space(10.0);

                        ui.label(
                            RichText::new(&self.status)
                                .size(12.0)
                                .color(Color32::from_gray(120)),
                        );
                    });
            });

        // Persist edits and selections immediately. Relying only on App::save means
        // a force-quit or quick restart can lose changes made since the last autosave.
        if let Some(storage) = frame.storage_mut() {
            eframe::set_value(storage, STORAGE_KEY, &self.settings);
            storage.flush();
        }
    }
}

fn settings_panel(
    ui: &mut egui::Ui,
    calendars: &mut Vec<CalendarSource>,
    browse_index: &mut Option<usize>,
) {
    Frame::new()
        .fill(Color32::from_rgb(19, 26, 37))
        .corner_radius(14.0)
        .inner_margin(18.0)
        .show(ui, |ui| {
            ui.label(RichText::new("Calendars").strong().color(Color32::WHITE));
            ui.add_space(6.0);
            let mut remove_index = None;
            for (index, calendar) in calendars.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 34.0],
                        egui::TextEdit::singleline(&mut calendar.name).hint_text("Name"),
                    );
                    let source_width = (ui.available_width() - 132.0).max(180.0);
                    ui.add_sized(
                        [source_width, 34.0],
                        egui::TextEdit::singleline(&mut calendar.source)
                            .hint_text("https://… or /path/to/calendar.ics"),
                    );
                    if ui.button("Browse…").clicked() {
                        *browse_index = Some(index);
                    }
                    if ui.button("×").on_hover_text("Remove calendar").clicked() {
                        remove_index = Some(index);
                    }
                });
            }
            if let Some(index) = remove_index {
                calendars.remove(index);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("+ Add .ics file").clicked() {
                    calendars.push(CalendarSource::default());
                }
                if ui.button("+ Add URL").clicked() {
                    calendars.push(CalendarSource::default());
                }
            });
        });
}

fn main_calendar_selector(ui: &mut egui::Ui, calendars: &[CalendarSource], selected: &mut usize) {
    if calendars.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Main calendar")
                .size(12.0)
                .strong()
                .color(Color32::from_gray(145)),
        );
        egui::ComboBox::from_id_salt("main_calendar_selector")
            .selected_text(calendar_display_name(&calendars[*selected]))
            .show_ui(ui, |ui| {
                for (index, calendar) in calendars.iter().enumerate() {
                    if calendar.source.trim().is_empty() {
                        continue;
                    }
                    ui.selectable_value(selected, index, calendar_display_name(calendar));
                }
            });
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
                if !event.calendar_name.is_empty() {
                    ui.label(RichText::new(&event.calendar_name).size(13.0).color(accent));
                }
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
                    if !event.calendar_name.is_empty() {
                        ui.label(
                            RichText::new(&event.calendar_name)
                                .size(12.0)
                                .color(Color32::from_gray(135)),
                        );
                    }
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

fn comparison_timeline(
    ui: &mut egui::Ui,
    events: &[CalendarEvent],
    calendars: &[CalendarSource],
    today: NaiveDate,
) {
    let calendar_names: BTreeSet<String> = calendars
        .iter()
        .filter(|calendar| !calendar.source.trim().is_empty())
        .map(|calendar| {
            if calendar.name.trim().is_empty() {
                calendar_name_from_source(&calendar.source)
            } else {
                calendar.name.trim().to_owned()
            }
        })
        .collect();

    ui.label(
        RichText::new("SCHEDULE DIFFERENCES")
            .size(12.0)
            .strong()
            .color(Color32::from_rgb(99, 204, 180)),
    );
    ui.label(
        RichText::new("Times when at least one calendar is busy and another is free")
            .size(14.0)
            .color(Color32::from_gray(145)),
    );
    ui.add_space(12.0);

    if calendar_names.len() < 2 {
        empty_timeline_message(ui, "Add at least two named calendars to compare.");
        return;
    }

    let monday = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    let mut found_difference = false;
    Frame::new()
        .fill(Color32::from_rgb(16, 22, 31))
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(35, 47, 61)))
        .corner_radius(16.0)
        .inner_margin(10.0)
        .show(ui, |ui| {
            ui.columns(5, |columns| {
                for (day_offset, column) in columns.iter_mut().enumerate() {
                    let date = monday + chrono::Duration::days(day_offset as i64);
                    let is_today = date == today;
                    let day_events: Vec<&CalendarEvent> = events
                        .iter()
                        .filter(|event| event.start.with_timezone(&Local).date_naive() == date)
                        .collect();
                    let mut boundaries: Vec<DateTime<Utc>> = day_events
                        .iter()
                        .flat_map(|event| [event.start, event.end])
                        .collect();
                    boundaries.sort_unstable();
                    boundaries.dedup();

                    let differences: Vec<_> = boundaries
                        .windows(2)
                        .filter_map(|window| {
                            let start = window[0];
                            let end = window[1];
                            let active_events: Vec<&&CalendarEvent> = day_events
                                .iter()
                                .filter(|event| event.start < end && event.end > start)
                                .collect();
                            let active_names: BTreeSet<&str> = active_events
                                .iter()
                                .map(|event| event.calendar_name.as_str())
                                .collect();
                            if active_names.is_empty() || active_names.len() == calendar_names.len()
                            {
                                return None;
                            }
                            let summaries: BTreeSet<String> = active_events
                                .iter()
                                .map(|event| event.summary.clone())
                                .collect();
                            Some((start, end, active_names, summaries))
                        })
                        .collect();

                    found_difference |= !differences.is_empty();
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
                        .inner_margin(6.0)
                        .show(column, |ui| {
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new(date.format("%a").to_string().to_uppercase())
                                        .size(11.0)
                                        .strong()
                                        .color(if is_today {
                                            Color32::from_rgb(99, 204, 180)
                                        } else {
                                            Color32::from_gray(135)
                                        }),
                                );
                                ui.label(
                                    RichText::new(date.day().to_string())
                                        .size(19.0)
                                        .strong()
                                        .color(Color32::WHITE),
                                );
                            });
                            ui.separator();
                            ui.add_space(4.0);

                            if differences.is_empty() {
                                ui.vertical_centered(|ui| {
                                    ui.label(
                                        RichText::new("No differences")
                                            .size(10.0)
                                            .color(Color32::from_gray(95)),
                                    );
                                });
                            }

                            for (start, end, active_names, summaries) in differences {
                                let free_names: Vec<&str> = calendar_names
                                    .iter()
                                    .map(String::as_str)
                                    .filter(|name| !active_names.contains(name))
                                    .collect();
                                Frame::new()
                                    .fill(Color32::from_rgb(24, 37, 42))
                                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(61, 134, 120)))
                                    .corner_radius(8.0)
                                    .inner_margin(7.0)
                                    .show(ui, |ui| {
                                        ui.label(
                                            RichText::new(format!(
                                                "{} – {}",
                                                short_local_time(start),
                                                short_local_time(end)
                                            ))
                                            .size(9.0)
                                            .color(Color32::from_gray(145)),
                                        );
                                        ui.label(
                                            RichText::new("FREE")
                                                .size(10.0)
                                                .strong()
                                                .color(Color32::from_rgb(99, 204, 180)),
                                        );
                                        ui.label(
                                            RichText::new(free_names.join(", "))
                                                .size(14.0)
                                                .strong()
                                                .color(Color32::WHITE),
                                        );
                                        ui.label(
                                            RichText::new(format!(
                                                "Busy: {}",
                                                summaries
                                                    .into_iter()
                                                    .collect::<Vec<_>>()
                                                    .join(", ")
                                            ))
                                            .size(9.0)
                                            .color(Color32::from_gray(115)),
                                        );
                                    });
                                ui.add_space(5.0);
                            }
                        });
                }
            });
        });

    if !found_difference {
        empty_timeline_message(ui, "No schedule differences were found this week.");
    }
}

fn empty_timeline_message(ui: &mut egui::Ui, message: &str) {
    Frame::new()
        .fill(Color32::from_rgb(20, 28, 39))
        .corner_radius(14.0)
        .inner_margin(24.0)
        .show(ui, |ui| {
            ui.label(RichText::new(message).color(Color32::from_gray(170)));
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
            if !event.calendar_name.is_empty() {
                ui.label(
                    RichText::new(&event.calendar_name)
                        .size(9.0 * font_scale)
                        .color(Color32::from_gray(150)),
                );
            }
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
                        calendar_index: 0,
                        calendar_name: String::new(),
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
                    calendar_index: event.calendar_index,
                    calendar_name: event.calendar_name.clone(),
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

fn short_local_time(datetime: DateTime<Utc>) -> String {
    datetime
        .with_timezone(&Local)
        .format("%-I:%M%P")
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

    #[derive(Default)]
    struct MemoryStorage(std::collections::HashMap<String, String>);

    impl eframe::Storage for MemoryStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }

        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }

        fn flush(&mut self) {}
    }

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
    fn loads_and_names_multiple_calendars() {
        let first = "data:text/calendar,BEGIN:VCALENDAR%0ABEGIN:VEVENT%0ASUMMARY:Math%0ADTSTART:20900102T090000Z%0ADTEND:20900102T100000Z%0AEND:VEVENT%0AEND:VCALENDAR";
        let second = "data:text/calendar,BEGIN:VCALENDAR%0ABEGIN:VEVENT%0ASUMMARY:Art%0ADTSTART:20900102T110000Z%0ADTEND:20900102T120000Z%0AEND:VEVENT%0AEND:VCALENDAR";
        let calendars = vec![
            CalendarSource {
                name: "Lincoln".into(),
                source: first.into(),
            },
            CalendarSource {
                name: "Friend".into(),
                source: second.into(),
            },
        ];

        let events = load_calendars(&calendars).expect("calendars should load");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].calendar_name, "Lincoln");
        assert_eq!(events[1].calendar_name, "Friend");
    }

    #[test]
    fn persists_multi_calendar_settings() {
        let settings = Settings {
            source: String::new(),
            calendars: vec![
                CalendarSource {
                    name: "Lincoln".into(),
                    source: "/calendars/lincoln.ics".into(),
                },
                CalendarSource {
                    name: "Friend".into(),
                    source: "/calendars/friend.ics".into(),
                },
            ],
            main_calendar: 1,
            weekly_font_scale: 1.3,
        };
        let mut storage = MemoryStorage::default();

        eframe::set_value(&mut storage, STORAGE_KEY, &settings);
        let restored: Settings = eframe::get_value(&storage, STORAGE_KEY).unwrap();

        assert_eq!(restored.calendars.len(), 2);
        assert_eq!(restored.calendars[0].name, "Lincoln");
        assert_eq!(restored.calendars[1].source, "/calendars/friend.ics");
        assert_eq!(restored.main_calendar, 1);
        assert_eq!(restored.weekly_font_scale, 1.3);
    }

    #[test]
    fn discovers_calendar_files_in_ics_directory() {
        let directory =
            std::env::temp_dir().join(format!("college-cal-discovery-test-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("second.ICS"), "BEGIN:VCALENDAR").unwrap();
        fs::write(directory.join("first.ical"), "BEGIN:VCALENDAR").unwrap();
        fs::write(directory.join("notes.txt"), "not a calendar").unwrap();

        let files = discover_ics_files(&directory);

        assert_eq!(files.len(), 2);
        assert_eq!(files[0].file_name().unwrap(), "first.ical");
        assert_eq!(files[1].file_name().unwrap(), "second.ICS");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn expands_weekly_rule_using_byday() {
        let start_property = "DTSTART;TZID=EDT;VALUE=DATE-TIME";
        let start_value = "20260824T120000";
        let start = parse_ics_datetime(start_property, start_value).unwrap();
        let event = CalendarEvent {
            calendar_index: 0,
            calendar_name: "Student".into(),
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
