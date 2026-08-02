//! Horizontal three-pane shell: navigation rail, transfer activity, composer.
//!
//! The mobile app stacks these vertically (bottom nav, transfer list, "New
//! transfer" sheet). On a wide window they sit side by side instead.

use std::path::PathBuf;
use std::time::Instant;

use egui::{Align, Layout, RichText};

use crate::engine::{Engine, OfferSummary, UiEvent};
use crate::qr::{self, QrMatrix};
use crate::theme::{self, DARK, LIGHT, PAD_SCREEN, Palette};
use crate::widgets::{
    card, direction_arrow, ghost_button, human_bytes, pill, primary_button, progress_bar,
    rail_item, section_label, segmented,
};

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Transfers,
    Logs,
}

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Send,
    Receive,
}

/// What the activity pane is currently showing.
#[derive(PartialEq)]
enum Stage {
    Idle,
    Waiting,
    Offered,
    Running,
    Done,
    Failed,
}

pub struct App {
    engine: Engine,
    tab: Tab,
    mode: Mode,
    dark: bool,

    save_directory: PathBuf,
    files: Vec<PathBuf>,
    invite_input: String,

    stage: Stage,
    invite: Option<String>,
    room_code: Option<String>,
    qr_matrix: Option<QrMatrix>,
    status: String,
    data_path: Option<String>,
    offer: Option<OfferSummary>,
    progress: Option<(u64, u64)>,
    result: Option<(usize, u64)>,
    error: Option<String>,
    logs: Vec<String>,

    /// Sampled to show a live rate the way the mobile card does.
    rate: Option<f64>,
    last_sample: Option<(Instant, u64)>,

    /// Restyling every frame would churn the context, so track what is applied.
    theme_applied: Option<bool>,

    /// Clicking copy is silent otherwise, so the button reports back briefly.
    copied_at: Option<Instant>,
    /// Summed once per selection change rather than stat-ing every frame.
    selection_bytes: u64,
}

impl App {
    /// `theme::install_fonts` must already have run on `context`: `set_fonts`
    /// only takes effect on the following frame, so it cannot happen here.
    pub fn new(context: &egui::Context) -> Self {
        Self {
            engine: Engine::new(context.clone()),
            tab: Tab::Transfers,
            mode: Mode::Receive,
            dark: context.theme() == egui::Theme::Dark,
            save_directory: default_save_directory(),
            files: Vec::new(),
            invite_input: String::new(),
            stage: Stage::Idle,
            invite: None,
            room_code: None,
            qr_matrix: None,
            status: "Ready".to_owned(),
            data_path: None,
            offer: None,
            progress: None,
            result: None,
            error: None,
            logs: Vec::new(),
            rate: None,
            last_sample: None,
            theme_applied: None,
            copied_at: None,
            selection_bytes: 0,
        }
    }

    /// Recomputes the queued total. Directories are walked, since a folder is a
    /// legitimate root and its size is the interesting number.
    fn refresh_selection_size(&mut self) {
        fn size_of(path: &std::path::Path) -> u64 {
            let Ok(metadata) = std::fs::symlink_metadata(path) else {
                return 0;
            };
            if metadata.is_file() {
                return metadata.len();
            }
            if !metadata.is_dir() {
                return 0;
            }
            let Ok(entries) = std::fs::read_dir(path) else {
                return 0;
            };
            entries.flatten().map(|entry| size_of(&entry.path())).sum()
        }
        self.selection_bytes = self.files.iter().map(|path| size_of(path)).sum();
    }

    fn palette(&self) -> Palette {
        if self.dark { DARK } else { LIGHT }
    }

    fn drain_events(&mut self) {
        let events: Vec<UiEvent> = self.engine.poll().collect();
        for event in events {
            match event {
                UiEvent::Invite { payload, room_code } => {
                    self.log(format!("invite {room_code}"));
                    self.qr_matrix = QrMatrix::encode(&payload);
                    self.invite = Some(payload);
                    self.room_code = Some(room_code);
                    self.stage = Stage::Waiting;
                    self.status = "Waiting for a sender".to_owned();
                }
                UiEvent::Status(message) => {
                    self.log(message.clone());
                    self.status = message;
                    if self.stage == Stage::Idle {
                        self.stage = Stage::Waiting;
                    }
                }
                UiEvent::Connected(path) => {
                    self.log(format!("connected via {path}"));
                    self.data_path = Some(path);
                }
                UiEvent::Offer(summary) => {
                    self.log(format!(
                        "offer: {} files, {}",
                        summary.files,
                        human_bytes(summary.bytes)
                    ));
                    self.offer = Some(summary);
                    self.stage = Stage::Offered;
                    self.status = "Offer received".to_owned();
                }
                UiEvent::Progress { bytes, total } => {
                    self.sample_rate(bytes);
                    self.progress = Some((bytes, total));
                    self.stage = Stage::Running;
                }
                UiEvent::Phase(phase) => {
                    self.log(phase.clone());
                    self.status = phase;
                    self.stage = Stage::Running;
                }
                UiEvent::Finished { entries, bytes } => {
                    self.log(format!(
                        "delivered {entries} entries, {}",
                        human_bytes(bytes)
                    ));
                    self.result = Some((entries, bytes));
                    self.stage = Stage::Done;
                    self.status = "Delivered".to_owned();
                    self.rate = None;
                }
                UiEvent::Failed(message) => {
                    self.log(format!("failed: {message}"));
                    self.error = Some(message);
                    self.stage = Stage::Failed;
                    self.status = "Failed".to_owned();
                }
            }
        }
    }

    fn log(&mut self, line: String) {
        self.logs.push(line);
        if self.logs.len() > 500 {
            self.logs.remove(0);
        }
    }

    fn sample_rate(&mut self, bytes: u64) {
        let now = Instant::now();
        if let Some((previous, previous_bytes)) = self.last_sample {
            let elapsed = now.duration_since(previous).as_secs_f64();
            if elapsed >= 0.4 {
                self.rate = Some((bytes.saturating_sub(previous_bytes) as f64) / elapsed);
                self.last_sample = Some((now, bytes));
            }
        } else {
            self.last_sample = Some((now, bytes));
        }
    }

    fn reset_run(&mut self) {
        self.stage = Stage::Idle;
        self.invite = None;
        self.room_code = None;
        self.qr_matrix = None;
        self.data_path = None;
        self.offer = None;
        self.progress = None;
        self.result = None;
        self.error = None;
        self.rate = None;
        self.last_sample = None;
        self.status = "Ready".to_owned();
    }

    fn busy(&self) -> bool {
        matches!(self.stage, Stage::Waiting | Stage::Offered | Stage::Running)
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl App {
    /// The whole surface, needing only a `Ui`, so it can also be rendered
    /// offscreen by the snapshot test.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        self.drain_events();
        let palette = self.palette();
        if self.theme_applied != Some(self.dark) {
            theme::apply(ui.ctx(), &palette, self.dark);
            self.theme_applied = Some(self.dark);
        }

        self.absorb_dropped_files(ui.ctx());

        self.rail(ui, &palette);
        self.composer(ui, &palette);
        self.activity(ui, &palette);
        self.drop_overlay(ui, &palette);

        if self.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// Dropping files anywhere on the window queues them for sending, which is
    /// the gesture a desktop user reaches for first.
    fn absorb_dropped_files(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let dropped: Vec<PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        self.mode = Mode::Send;
        self.tab = Tab::Transfers;
        for path in dropped {
            if !self.files.contains(&path) {
                self.files.push(path);
            }
        }
        self.refresh_selection_size();
    }

    fn drop_overlay(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        if self.busy() || !ui.ctx().input(|input| !input.raw.hovered_files.is_empty()) {
            return;
        }
        let screen = ui.ctx().viewport_rect();
        let painter = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("drop-overlay"),
        ));
        painter.rect_filled(screen, 0, palette.accent_soft.gamma_multiply(0.92));
        painter.text(
            screen.center(),
            egui::Align2::CENTER_CENTER,
            "Drop files to send",
            theme::bold(24.0),
            palette.accent_strong,
        );
    }
}

impl App {
    fn rail(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        egui::Panel::left("rail")
            .exact_size(208.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(palette.surface)
                    .inner_margin(egui::Margin::same(PAD_SCREEN)),
            )
            .show(ui, |ui| {
                ui.label(
                    RichText::new("Envoix")
                        .font(theme::bold(26.0))
                        .color(palette.text),
                );
                ui.add_space(24.0);

                if rail_item(ui, palette, "Transfers", self.tab == Tab::Transfers) {
                    self.tab = Tab::Transfers;
                }
                if rail_item(ui, palette, "Logs", self.tab == Tab::Logs) {
                    self.tab = Tab::Logs;
                }

                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    let toggle = if self.dark {
                        "Light theme"
                    } else {
                        "Dark theme"
                    };
                    if ghost_button(ui, palette, toggle) {
                        self.dark = !self.dark;
                    }
                    ui.add_space(12.0);

                    // bottom_up emits upwards, so each value precedes its label.
                    ui.label(
                        RichText::new(broker_host())
                            .font(theme::mono(11.0))
                            .color(palette.muted),
                    );
                    section_label(ui, palette, "Rendezvous");
                    if let Some(path) = &self.data_path {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(path)
                                .font(theme::mono(11.0))
                                .color(palette.success),
                        );
                        section_label(ui, palette, "Data path");
                    }
                });
            });
    }

    fn composer(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        egui::Panel::right("composer")
            .exact_size(396.0)
            .resizable(false)
            .frame(
                // The composer stands in for the mobile "New transfer" sheet,
                // which sits above the page rather than flush with it.
                egui::Frame::new()
                    .fill(palette.surface_raised)
                    .inner_margin(egui::Margin::same(PAD_SCREEN)),
            )
            .show(ui, |ui| {
                ui.label(
                    RichText::new("New transfer")
                        .font(theme::bold(20.0))
                        .color(palette.text),
                );
                ui.add_space(14.0);

                if let Some(index) =
                    segmented(ui, palette, &["Send", "Receive"], self.mode as usize)
                {
                    let next = if index == 0 {
                        Mode::Send
                    } else {
                        Mode::Receive
                    };
                    if next != self.mode && !self.busy() {
                        self.mode = next;
                        self.reset_run();
                    }
                }
                ui.add_space(18.0);

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.mode {
                        Mode::Receive => self.receive_form(ui, palette),
                        Mode::Send => self.send_form(ui, palette),
                    });
            });
    }

    fn receive_form(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        // Once a peer has joined, the invitation is spent. Leaving the QR up
        // would crowd out the approval action that now owns this panel.
        if self.stage == Stage::Waiting
            && let (Some(matrix), Some(code)) = (&self.qr_matrix, &self.room_code)
        {
            ui.vertical_centered(|ui| {
                qr::draw(ui, matrix, 210.0, palette.text, palette.surface);
                ui.add_space(12.0);
                ui.label(
                    RichText::new(code)
                        .font(theme::mono(16.0))
                        .color(palette.text),
                );
            });
            ui.add_space(10.0);
            let just_copied = self
                .copied_at
                .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(2));
            let label = if just_copied { "Copied" } else { "Copy invite" };
            let mut copied = false;
            ui.vertical_centered(|ui| {
                if let Some(invite) = &self.invite
                    && ghost_button(ui, palette, label)
                {
                    ui.ctx().copy_text(invite.clone());
                    copied = true;
                }
            });
            if copied {
                self.copied_at = Some(Instant::now());
            }
            if just_copied {
                // Repaint so the label reverts without needing a mouse move.
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(250));
            }
            ui.add_space(16.0);
        }

        section_label(ui, palette, "Save to");
        ui.add_space(6.0);
        card(ui, palette, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(self.save_directory.display().to_string())
                        .font(theme::mono(12.0))
                        .color(palette.text),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if !self.busy()
                        && ghost_button(ui, palette, "Change")
                        && let Some(directory) = rfd::FileDialog::new().pick_folder()
                    {
                        self.save_directory = directory;
                    }
                });
            });
        });
        ui.add_space(16.0);

        match self.stage {
            Stage::Offered => {
                if primary_button(ui, palette, "Accept and save", true) {
                    self.engine.accept_offer();
                }
            }
            Stage::Idle | Stage::Done | Stage::Failed => {
                if primary_button(ui, palette, "Receive", true) {
                    self.reset_run();
                    self.engine.start_receive(self.save_directory.clone());
                }
            }
            _ => {
                primary_button(ui, palette, "Waiting for a sender", false);
            }
        }
    }

    fn send_form(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        section_label(ui, palette, "Files");
        ui.add_space(6.0);
        card(ui, palette, |ui| {
            if self.files.is_empty() {
                ui.label(
                    RichText::new("Nothing selected yet.")
                        .font(theme::sans(13.0))
                        .color(palette.muted),
                );
            } else {
                for file in &self.files {
                    ui.label(
                        RichText::new(
                            file.file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_else(|| file.display().to_string()),
                        )
                        .font(theme::mono(12.0))
                        .color(palette.text),
                    );
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "{} item{} \u{b7} {}",
                        self.files.len(),
                        if self.files.len() == 1 { "" } else { "s" },
                        human_bytes(self.selection_bytes)
                    ))
                    .font(theme::sans(12.0))
                    .color(palette.muted),
                );
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if !self.busy()
                    && ghost_button(ui, palette, "Choose files")
                    && let Some(picked) = rfd::FileDialog::new().pick_files()
                {
                    self.files = picked;
                    self.refresh_selection_size();
                }
                if !self.files.is_empty() && !self.busy() && ghost_button(ui, palette, "Clear") {
                    self.files.clear();
                    self.refresh_selection_size();
                }
            });
        });
        ui.add_space(16.0);

        section_label(ui, palette, "Invite from the receiver");
        ui.add_space(6.0);
        ui.add(
            egui::TextEdit::multiline(&mut self.invite_input)
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .font(theme::mono(11.0))
                .hint_text("envoix://invite/v2/..."),
        );
        ui.add_space(16.0);

        let ready = !self.files.is_empty() && !self.invite_input.trim().is_empty();
        match self.stage {
            Stage::Idle | Stage::Done | Stage::Failed => {
                if primary_button(ui, palette, "Send", ready) {
                    self.reset_run();
                    self.engine
                        .start_send(self.files.clone(), self.invite_input.clone());
                }
            }
            _ => {
                primary_button(ui, palette, "Sending", false);
            }
        }
    }

    fn activity(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(palette.bg)
                    .inner_margin(egui::Margin::same(PAD_SCREEN)),
            )
            .show(ui, |ui| match self.tab {
                Tab::Transfers => self.transfers(ui, palette),
                Tab::Logs => self.logs_pane(ui, palette),
            });
    }

    fn transfers(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(if self.stage == Stage::Idle {
                    "0 transfers".to_owned()
                } else {
                    "1 transfer".to_owned()
                })
                .font(theme::bold(20.0))
                .color(palette.text),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.busy() && ghost_button(ui, palette, "Cancel") {
                    self.engine.cancel();
                }
            });
        });
        ui.add_space(16.0);

        if self.stage == Stage::Idle {
            ui.label(
                RichText::new("No transfers yet. Use New transfer to start one.")
                    .font(theme::sans(14.0))
                    .color(palette.muted),
            );
            return;
        }

        card(ui, palette, |ui| {
            ui.horizontal(|ui| {
                direction_arrow(ui, palette.text, self.mode == Mode::Send);
                ui.label(
                    RichText::new(self.headline())
                        .font(theme::bold(16.0))
                        .color(palette.text),
                );
                ui.add_space(8.0);
                let (text, fg, bg) = self.status_pill(palette);
                pill(ui, &text, fg, bg);
            });
            ui.add_space(6.0);
            ui.label(
                RichText::new(self.detail_line())
                    .font(theme::mono(12.0))
                    .color(palette.muted),
            );
            ui.add_space(12.0);

            let (transferred, total) = self.progress.unwrap_or((0, 0));
            let fraction = if total > 0 {
                transferred as f32 / total as f32
            } else if self.stage == Stage::Done {
                1.0
            } else {
                0.0
            };
            progress_bar(ui, palette, fraction);
            ui.add_space(10.0);

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(self.status.clone())
                        .font(theme::sans(13.0))
                        .color(palette.text),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(if total > 0 {
                            human_bytes(total)
                        } else {
                            "—".to_owned()
                        })
                        .font(theme::bold(13.0))
                        .color(palette.text),
                    );
                    ui.add_space(18.0);
                    ui.label(
                        RichText::new(match self.rate {
                            Some(rate) if rate > 0.0 => format!("{}/s", human_bytes(rate as u64)),
                            _ => "—".to_owned(),
                        })
                        .font(theme::sans(13.0))
                        .color(palette.muted),
                    );
                });
            });
        });

        if let Some(error) = self.error.clone() {
            ui.add_space(14.0);
            card(ui, palette, |ui| {
                ui.label(
                    RichText::new("Failed")
                        .font(theme::bold(14.0))
                        .color(palette.danger),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(error)
                        .font(theme::mono(12.0))
                        .color(palette.muted),
                );
            });
        }
    }

    fn headline(&self) -> String {
        if let Some(offer) = &self.offer {
            return offer
                .roots
                .first()
                .cloned()
                .unwrap_or_else(|| "transfer".to_owned());
        }
        if let Some(first) = self.files.first() {
            let name = first
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| first.display().to_string());
            return if self.files.len() > 1 {
                format!("{name} +{}", self.files.len() - 1)
            } else {
                name
            };
        }
        // Before an offer arrives the receiver has no name to show, and the
        // room code already appears on the detail line.
        match self.mode {
            Mode::Receive => "Incoming transfer".to_owned(),
            Mode::Send => "Outgoing transfer".to_owned(),
        }
    }

    fn detail_line(&self) -> String {
        match self.stage {
            Stage::Done if self.mode == Mode::Receive => match self.result {
                Some((entries, _)) => format!(
                    "Saved {entries} entries to {}",
                    self.save_directory.display()
                ),
                None => format!("Saved to {}", self.save_directory.display()),
            },
            Stage::Done => match self.result {
                Some((entries, _)) => {
                    format!("Receiver saved and confirmed {entries} entries")
                }
                None => "Receiver saved and confirmed".to_owned(),
            },
            Stage::Offered => self
                .offer
                .as_ref()
                .map(|offer| {
                    format!(
                        "{} files · {} directories · {}",
                        offer.files,
                        offer.directories,
                        human_bytes(offer.bytes)
                    )
                })
                .unwrap_or_default(),
            _ => match (&self.room_code, &self.data_path) {
                (_, Some(path)) => format!("via {path}"),
                (Some(code), None) => format!("room {code}"),
                _ => self.status.clone(),
            },
        }
    }

    fn status_pill(&self, palette: &Palette) -> (String, egui::Color32, egui::Color32) {
        match self.stage {
            Stage::Done => ("Done".to_owned(), palette.success, palette.success_soft),
            Stage::Failed => (
                "Failed".to_owned(),
                palette.danger,
                palette.danger.gamma_multiply(0.16),
            ),
            Stage::Offered => (
                "Needs approval".to_owned(),
                palette.warning,
                palette.warning.gamma_multiply(0.16),
            ),
            _ => ("Active".to_owned(), palette.accent, palette.accent_soft),
        }
    }

    fn logs_pane(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        ui.label(
            RichText::new("Logs")
                .font(theme::bold(20.0))
                .color(palette.text),
        );
        ui.add_space(16.0);
        card(ui, palette, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    if self.logs.is_empty() {
                        ui.label(
                            RichText::new("Nothing logged yet.")
                                .font(theme::sans(13.0))
                                .color(palette.muted),
                        );
                    }
                    for line in &self.logs {
                        ui.label(
                            RichText::new(line)
                                .font(theme::mono(12.0))
                                .color(palette.muted),
                        );
                    }
                });
        });
    }
}

fn default_save_directory() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .map(|home| home.join("Downloads"))
        .unwrap_or_else(std::env::temp_dir)
}

/// The rail shows the broker host without its endpoint id, which is too long
/// to be readable at rail width.
fn broker_host() -> String {
    crate::engine::BROKER
        .split_once('@')
        .map(|(_, host)| host.to_owned())
        .unwrap_or_else(|| crate::engine::BROKER.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders the shell offscreen so the layout can be reviewed without a
    /// display server, and regressions in it show up as an image diff.
    fn preview(name: &str, seed: impl FnOnce(&mut App)) {
        let mut app: Option<App> = None;
        let mut seed = Some(seed);
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1180.0, 720.0))
            .wgpu()
            .build_ui(|ui| {
                let Some(app) = app.as_mut() else {
                    // Fonts only bind on the next frame, so install them and
                    // draw nothing until they are available.
                    theme::install_fonts(ui.ctx());
                    let mut fresh = App::new(ui.ctx());
                    if let Some(seed) = seed.take() {
                        seed(&mut fresh);
                    }
                    app = Some(fresh);
                    return;
                };
                app.draw(ui);
            });
        harness.run_steps(3);

        let image = harness.render().expect("offscreen render");
        let directory =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-preview");
        std::fs::create_dir_all(&directory).expect("preview directory");
        image
            .save(directory.join(format!("{name}.png")))
            .expect("write preview");
    }

    #[test]
    fn waiting_for_a_sender_light() {
        preview("waiting-light", |app| {
            let invite = "envoix://invite/v2/eyJyIjoiMDc1Mjg3LWluZGlnby1vcGFsIiwiYiI6ImU5NDZhMzFhMjIwN2VmY2Q2OGI5ZGJmNDA5YzRiZjI0MWFhMDJhMGNiYzAwMjhhZjJlMWVkMTE0NzIwNjRlZmYifQ";
            app.dark = false;
            app.mode = Mode::Receive;
            app.qr_matrix = QrMatrix::encode(invite);
            app.invite = Some(invite.to_owned());
            app.room_code = Some("075287-indigo-opal".to_owned());
            app.stage = Stage::Waiting;
            app.status = "Waiting for a sender".to_owned();
        });
    }

    #[test]
    fn transferring_dark() {
        preview("transferring-dark", |app| {
            app.dark = true;
            app.mode = Mode::Send;
            app.files = vec![PathBuf::from("/home/demo/quarterly-report.pdf")];
            app.invite_input = "envoix://invite/v2/eyJyIjoiMDc1Mjg3LWluZGlnby1vcGFsIn0".to_owned();
            app.stage = Stage::Running;
            app.status = "Transferring".to_owned();
            app.progress = Some((6_815_744, 10_485_760));
            app.rate = Some(14_680_064.0);
            app.data_path = Some("direct".to_owned());
        });
    }

    #[test]
    fn idle_light() {
        preview("idle-light", |app| {
            app.dark = false;
            app.mode = Mode::Receive;
        });
    }

    #[test]
    fn offer_awaiting_approval_light() {
        preview("offer-light", |app| {
            app.dark = false;
            app.mode = Mode::Receive;
            app.stage = Stage::Offered;
            app.status = "Offer received".to_owned();
            app.data_path = Some("direct".to_owned());
            app.room_code = Some("075287-indigo-opal".to_owned());
            // Seeded so the preview proves the QR is withdrawn once a peer has
            // joined, rather than passing because none was ever built.
            app.qr_matrix = QrMatrix::encode("envoix://invite/v2/eyJyIjoiMDc1Mjg3In0");
            app.offer = Some(OfferSummary {
                roots: vec!["quarterly-report.pdf".to_owned(), "photos".to_owned()],
                files: 19,
                directories: 2,
                bytes: 8_388_608,
            });
        });
    }

    #[test]
    fn delivered_light() {
        preview("delivered-light", |app| {
            app.dark = false;
            app.mode = Mode::Receive;
            app.stage = Stage::Done;
            app.status = "Delivered".to_owned();
            app.data_path = Some("direct".to_owned());
            app.progress = Some((8_388_608, 8_388_608));
            app.result = Some((21, 8_388_608));
            app.offer = Some(OfferSummary {
                roots: vec!["quarterly-report.pdf".to_owned()],
                files: 19,
                directories: 2,
                bytes: 8_388_608,
            });
        });
    }

    #[test]
    fn send_composer_light() {
        preview("send-light", |app| {
            app.dark = false;
            app.mode = Mode::Send;
            app.files = vec![
                PathBuf::from("/home/demo/quarterly-report.pdf"),
                PathBuf::from("/home/demo/photos"),
            ];
            // These paths do not exist, so the total cannot be measured here.
            app.selection_bytes = 8_912_896;
            app.invite_input =
                "envoix://invite/v2/eyJyIjoiMDc1Mjg3LWluZGlnby1vcGFsIiwiYiI6ImU5NDZhMzFhIn0"
                    .to_owned();
        });
    }

    #[test]
    fn logs_dark() {
        preview("logs-dark", |app| {
            app.dark = true;
            app.tab = Tab::Logs;
            app.mode = Mode::Receive;
            app.logs = vec![
                "invite 075287-indigo-opal".to_owned(),
                "Waiting for a sender".to_owned(),
                "Pairing: Joined".to_owned(),
                "Pairing: Confirmed".to_owned(),
                "connected via direct".to_owned(),
                "offer: 19 files, 8.0 MB".to_owned(),
                "Transferring".to_owned(),
                "Verifying".to_owned(),
                "Saving".to_owned(),
                "delivered 21 entries, 8.0 MB".to_owned(),
            ];
        });
    }
}
