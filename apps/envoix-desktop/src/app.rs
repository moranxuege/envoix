//! Horizontal three-pane shell: navigation rail, transfer activity, composer.
//!
//! The mobile app stacks these vertically (bottom nav, transfer list, "New
//! transfer" sheet). On a wide window they sit side by side instead.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Align, Layout, RichText};

use crate::engine::{Engine, OfferSummary, TransferId, UiEvent};
use crate::qr::{self, QrMatrix};
use crate::theme::{self, DARK, LIGHT, PAD_SCREEN, Palette};
use crate::widgets::{
    card, direction_arrow, ghost_button, human_bytes, pill, primary_button, progress_bar,
    rail_item, section_label, segmented,
};

/// How long "Copy invite" reports back before reverting.
const COPIED_FEEDBACK: Duration = Duration::from_secs(2);

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

/// What a transfer card is currently showing.
#[derive(Debug, PartialEq, Clone, Copy)]
enum Stage {
    Waiting,
    Offered,
    Running,
    Done,
    Failed,
}

/// One transfer's own state. Several run at once, so none of this can live on
/// `App`.
struct Transfer {
    id: TransferId,
    mode: Mode,
    /// Card title until an offer names the payload.
    label: String,
    save_directory: PathBuf,
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
    /// Sampled to show a live rate the way the mobile card does.
    rate: Option<f64>,
    last_sample: Option<(Instant, u64)>,
    copied_at: Option<Instant>,
}

impl Transfer {
    fn new(id: TransferId, mode: Mode, label: String, save_directory: PathBuf) -> Self {
        Self {
            id,
            mode,
            label,
            save_directory,
            stage: Stage::Waiting,
            invite: None,
            room_code: None,
            qr_matrix: None,
            status: "Preparing".to_owned(),
            data_path: None,
            offer: None,
            progress: None,
            result: None,
            error: None,
            rate: None,
            last_sample: None,
            copied_at: None,
        }
    }

    fn busy(&self) -> bool {
        matches!(self.stage, Stage::Waiting | Stage::Offered | Stage::Running)
    }

    fn fraction(&self) -> f32 {
        match self.progress {
            Some((done, total)) if total > 0 => done as f32 / total as f32,
            _ if self.stage == Stage::Done => 1.0,
            _ => 0.0,
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

    fn headline(&self) -> String {
        if let Some(offer) = &self.offer
            && let Some(root) = offer.roots.first()
        {
            return root.clone();
        }
        self.label.clone()
    }

    fn detail(&self) -> String {
        match self.stage {
            Stage::Done if self.mode == Mode::Receive => match self.result {
                Some((entries, _)) => format!(
                    "Saved {entries} entries to {}",
                    self.save_directory.display()
                ),
                None => format!("Saved to {}", self.save_directory.display()),
            },
            Stage::Done => match self.result {
                Some((entries, _)) => format!("Receiver saved and confirmed {entries} entries"),
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
}

/// A card affordance the user pressed. Collected while rendering and applied
/// afterwards, so drawing a card does not need `&mut App`.
enum Action {
    Accept(TransferId),
    Cancel(TransferId),
    Open(PathBuf),
    Copy(TransferId, String),
    Dismiss(TransferId),
}

pub struct App {
    engine: Engine,
    tab: Tab,
    mode: Mode,
    dark: bool,

    save_directory: PathBuf,
    files: Vec<PathBuf>,
    invite_input: String,
    /// Summed once per selection change rather than stat-ing every frame.
    selection_bytes: u64,

    /// Newest first, matching the mobile list order.
    transfers: Vec<Transfer>,
    logs: Vec<String>,

    /// Set by a card action, drained once the borrow on `transfers` is over.
    pending_copy: Option<String>,
    /// Restyling every frame would churn the context, so track what is applied.
    theme_applied: Option<bool>,
    /// Last title pushed to the window manager, so it is only sent on change.
    title: String,
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
            selection_bytes: 0,
            transfers: Vec::new(),
            logs: Vec::new(),
            pending_copy: None,
            theme_applied: None,
            title: String::new(),
        }
    }

    fn palette(&self) -> Palette {
        if self.dark { DARK } else { LIGHT }
    }

    fn transfer_mut(&mut self, id: TransferId) -> Option<&mut Transfer> {
        self.transfers.iter_mut().find(|entry| entry.id == id)
    }

    /// Recomputes the queued total. Directories are walked, since a folder is a
    /// legitimate root and its size is the interesting number.
    fn refresh_selection_size(&mut self) {
        fn size_of(path: &Path) -> u64 {
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

    fn drain_events(&mut self) {
        let events: Vec<(TransferId, UiEvent)> = self.engine.poll().collect();
        for (id, event) in events {
            if let Some(line) = log_line(&event) {
                self.log(line);
            }
            let Some(transfer) = self.transfer_mut(id) else {
                continue;
            };
            match event {
                UiEvent::Invite { payload, room_code } => {
                    transfer.qr_matrix = QrMatrix::encode(&payload);
                    transfer.invite = Some(payload);
                    transfer.room_code = Some(room_code);
                    transfer.stage = Stage::Waiting;
                    transfer.status = "Waiting for a sender".to_owned();
                }
                UiEvent::Status(message) => transfer.status = message,
                UiEvent::Connected(path) => transfer.data_path = Some(path),
                UiEvent::Offer(summary) => {
                    transfer.offer = Some(summary);
                    transfer.stage = Stage::Offered;
                    transfer.status = "Offer received".to_owned();
                }
                UiEvent::Progress { bytes, total } => {
                    transfer.sample_rate(bytes);
                    transfer.progress = Some((bytes, total));
                    transfer.stage = Stage::Running;
                }
                UiEvent::Phase(phase) => {
                    transfer.status = phase;
                    transfer.stage = Stage::Running;
                }
                UiEvent::Finished { entries, bytes } => {
                    transfer.result = Some((entries, bytes));
                    transfer.stage = Stage::Done;
                    transfer.status = "Delivered".to_owned();
                    transfer.rate = None;
                }
                UiEvent::Failed(message) => {
                    transfer.error = Some(message);
                    transfer.stage = Stage::Failed;
                    transfer.status = "Failed".to_owned();
                    transfer.rate = None;
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

    fn busy(&self) -> bool {
        self.transfers.iter().any(Transfer::busy)
    }

    fn start_receive(&mut self) {
        let save_directory = self.save_directory.clone();
        let id = self.engine.start_receive(save_directory.clone());
        self.transfers.insert(
            0,
            Transfer::new(
                id,
                Mode::Receive,
                "Incoming transfer".to_owned(),
                save_directory,
            ),
        );
        self.tab = Tab::Transfers;
    }

    fn start_send(&mut self) {
        let files = std::mem::take(&mut self.files);
        let invite = std::mem::take(&mut self.invite_input);
        let label = match files.first() {
            Some(first) if files.len() > 1 => {
                format!("{} +{}", file_label(first), files.len() - 1)
            }
            Some(first) => file_label(first),
            None => "Outgoing transfer".to_owned(),
        };
        let id = self.engine.start_send(files, invite);
        self.selection_bytes = 0;
        self.transfers.insert(
            0,
            Transfer::new(id, Mode::Send, label, self.save_directory.clone()),
        );
        self.tab = Tab::Transfers;
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::Accept(id) => self.engine.accept_offer(id),
            Action::Cancel(id) => self.engine.cancel(id),
            Action::Open(path) => reveal(&path),
            Action::Copy(id, payload) => {
                if let Some(transfer) = self.transfer_mut(id) {
                    transfer.copied_at = Some(Instant::now());
                }
                self.pending_copy = Some(payload);
            }
            Action::Dismiss(id) => self.transfers.retain(|entry| entry.id != id),
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl App {
    /// The whole surface, needing only a `Ui`, so it can also be rendered
    /// offscreen by the snapshot tests.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        self.drain_events();
        let palette = self.palette();
        if self.theme_applied != Some(self.dark) {
            theme::apply(ui.ctx(), &palette, self.dark);
            self.theme_applied = Some(self.dark);
        }

        self.absorb_dropped_files(ui.ctx());
        self.publish_title(ui.ctx());

        let mut actions = Vec::new();
        self.rail(ui, &palette);
        self.composer(ui, &palette);
        self.activity(ui, &palette, &mut actions);
        self.drop_overlay(ui, &palette);

        for action in actions {
            self.apply(action);
        }
        if let Some(payload) = self.pending_copy.take() {
            ui.ctx().copy_text(payload);
        }

        if self.busy() {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
    }

    /// Mirrors transfer state into the window title, so a minimised or occluded
    /// window still reports from the taskbar.
    fn publish_title(&mut self, ctx: &egui::Context) {
        let active: Vec<&Transfer> = self.transfers.iter().filter(|entry| entry.busy()).collect();
        let wanted = match active.as_slice() {
            [] => "Envoix".to_owned(),
            [only] => title_for(only.stage, only.progress),
            many => format!("Envoix - {} active", many.len()),
        };
        if wanted != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(wanted.clone()));
            self.title = wanted;
        }
    }

    /// Dropping files anywhere on the window queues them for sending, which is
    /// the gesture a desktop user reaches for first.
    fn absorb_dropped_files(&mut self, ctx: &egui::Context) {
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
        if !ui.ctx().input(|input| !input.raw.hovered_files.is_empty()) {
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
                    self.mode = if index == 0 {
                        Mode::Send
                    } else {
                        Mode::Receive
                    };
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
                    if ghost_button(ui, palette, "Change")
                        && let Some(directory) = rfd::FileDialog::new().pick_folder()
                    {
                        self.save_directory = directory;
                    }
                });
            });
        });
        ui.add_space(16.0);

        if primary_button(ui, palette, "Receive", true) {
            self.start_receive();
        }
        ui.add_space(8.0);
        ui.label(
            RichText::new("Its room code appears on the new card.")
                .font(theme::sans(12.0))
                .color(palette.muted),
        );
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
                        RichText::new(file_label(file))
                            .font(theme::mono(12.0))
                            .color(palette.text),
                    );
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "{} item{} · {}",
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
                if ghost_button(ui, palette, "Choose files")
                    && let Some(picked) = rfd::FileDialog::new().pick_files()
                {
                    self.files = picked;
                    self.refresh_selection_size();
                }
                if !self.files.is_empty() && ghost_button(ui, palette, "Clear") {
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
        if primary_button(ui, palette, "Send", ready) {
            self.start_send();
        }
    }

    fn activity(&mut self, ui: &mut egui::Ui, palette: &Palette, actions: &mut Vec<Action>) {
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(palette.bg)
                    .inner_margin(egui::Margin::same(PAD_SCREEN)),
            )
            .show(ui, |ui| match self.tab {
                Tab::Transfers => self.transfers_pane(ui, palette, actions),
                Tab::Logs => self.logs_pane(ui, palette),
            });
    }

    fn transfers_pane(&mut self, ui: &mut egui::Ui, palette: &Palette, actions: &mut Vec<Action>) {
        let count = self.transfers.len();
        ui.label(
            RichText::new(format!(
                "{count} transfer{}",
                if count == 1 { "" } else { "s" }
            ))
            .font(theme::bold(20.0))
            .color(palette.text),
        );
        ui.add_space(16.0);

        if self.transfers.is_empty() {
            ui.label(
                RichText::new("No transfers yet. Use New transfer to start one.")
                    .font(theme::sans(14.0))
                    .color(palette.muted),
            );
            return;
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for transfer in &self.transfers {
                    transfer_card(ui, palette, transfer, actions);
                    ui.add_space(12.0);
                }
            });
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

/// One transfer card, matching the mobile list item: direction, name, status
/// pill, detail line, progress, then whatever action the state affords.
fn transfer_card(
    ui: &mut egui::Ui,
    palette: &Palette,
    transfer: &Transfer,
    actions: &mut Vec<Action>,
) {
    card(ui, palette, |ui| {
        ui.horizontal(|ui| {
            direction_arrow(ui, palette.text, transfer.mode == Mode::Send);
            ui.label(
                RichText::new(transfer.headline())
                    .font(theme::bold(16.0))
                    .color(palette.text),
            );
            ui.add_space(8.0);
            let (text, fg, bg) = transfer.status_pill(palette);
            pill(ui, &text, fg, bg);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !transfer.busy() && ghost_button(ui, palette, "Dismiss") {
                    actions.push(Action::Dismiss(transfer.id));
                }
            });
        });
        ui.add_space(6.0);
        ui.label(
            RichText::new(transfer.detail())
                .font(theme::mono(12.0))
                .color(palette.muted),
        );

        // A receive that is still waiting owns the invitation, so its code and
        // QR belong on the card rather than in the shared composer.
        if transfer.stage == Stage::Waiting
            && let (Some(matrix), Some(code)) = (&transfer.qr_matrix, &transfer.room_code)
        {
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                qr::draw(ui, matrix, 150.0, palette.text, palette.surface);
                ui.add_space(8.0);
                ui.label(
                    RichText::new(code)
                        .font(theme::mono(15.0))
                        .color(palette.text),
                );
                ui.add_space(6.0);
                let copied = transfer
                    .copied_at
                    .is_some_and(|at| at.elapsed() < COPIED_FEEDBACK);
                let label = if copied { "Copied" } else { "Copy invite" };
                if let Some(invite) = &transfer.invite
                    && ghost_button(ui, palette, label)
                {
                    actions.push(Action::Copy(transfer.id, invite.clone()));
                }
                if copied {
                    ui.ctx().request_repaint_after(Duration::from_millis(250));
                }
            });
        }

        ui.add_space(12.0);
        let bar = match transfer.stage {
            Stage::Failed => palette.danger,
            Stage::Done => palette.success,
            _ => palette.accent,
        };
        progress_bar(ui, palette, transfer.fraction(), bar);
        ui.add_space(10.0);

        let total = transfer.progress.map(|(_, total)| total).unwrap_or(0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(transfer.status.clone())
                    .font(theme::sans(13.0))
                    .color(palette.text),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(if total > 0 {
                        human_bytes(total)
                    } else {
                        "-".to_owned()
                    })
                    .font(theme::bold(13.0))
                    .color(palette.text),
                );
                ui.add_space(18.0);
                ui.label(
                    RichText::new(match transfer.rate {
                        Some(rate) if rate > 0.0 => format!("{}/s", human_bytes(rate as u64)),
                        _ => "-".to_owned(),
                    })
                    .font(theme::sans(13.0))
                    .color(palette.muted),
                );
            });
        });

        let offers_action = matches!(transfer.stage, Stage::Offered)
            || (transfer.stage == Stage::Done && transfer.mode == Mode::Receive)
            || transfer.busy();
        if offers_action {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                // Built before rendering so the button is not called from a
                // match guard, where its side effect would be easy to miss.
                let affordance = match transfer.stage {
                    Stage::Offered => Some(("Accept and save", Action::Accept(transfer.id))),
                    Stage::Done if transfer.mode == Mode::Receive => {
                        Some(("Open folder", Action::Open(transfer.save_directory.clone())))
                    }
                    _ => None,
                };
                if let Some((label, action)) = affordance
                    && ghost_button(ui, palette, label)
                {
                    actions.push(action);
                }
                if transfer.busy() && ghost_button(ui, palette, "Cancel") {
                    actions.push(Action::Cancel(transfer.id));
                }
            });
        }

        if let Some(error) = &transfer.error {
            ui.add_space(8.0);
            ui.label(
                RichText::new(error)
                    .font(theme::mono(11.0))
                    .color(palette.danger),
            );
        }
    });
}

/// The log line an event deserves, or `None` for the ones that would flood it.
fn log_line(event: &UiEvent) -> Option<String> {
    match event {
        UiEvent::Invite { room_code, .. } => Some(format!("invite {room_code}")),
        UiEvent::Status(message) => Some(message.clone()),
        UiEvent::Connected(path) => Some(format!("connected via {path}")),
        UiEvent::Offer(summary) => Some(format!(
            "offer: {} files, {}",
            summary.files,
            human_bytes(summary.bytes)
        )),
        UiEvent::Phase(phase) => Some(phase.clone()),
        UiEvent::Finished { entries, bytes } => Some(format!(
            "delivered {entries} entries, {}",
            human_bytes(*bytes)
        )),
        UiEvent::Failed(message) => Some(format!("failed: {message}")),
        UiEvent::Progress { .. } => None,
    }
}

/// The window title for a single transfer. Split out from the viewport call so
/// the wording and the percentage arithmetic can be tested directly.
fn title_for(stage: Stage, progress: Option<(u64, u64)>) -> String {
    match stage {
        Stage::Waiting => "Envoix - waiting for a peer".to_owned(),
        Stage::Offered => "Envoix - needs approval".to_owned(),
        Stage::Running => match progress {
            // Scale in f64: `done * 100` overflows on large byte counts.
            Some((done, total)) if total > 0 => {
                let percent = (done.min(total) as f64 / total as f64 * 100.0).round() as u64;
                format!("Envoix - {percent}%")
            }
            _ => "Envoix - transferring".to_owned(),
        },
        Stage::Done => "Envoix - delivered".to_owned(),
        Stage::Failed => "Envoix - failed".to_owned(),
    }
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Opens `path` in the platform file manager.
///
/// Deliberately fire-and-forget: the file manager is not this app's problem,
/// and a desktop without one is not an error worth surfacing on the card.
fn reveal(path: &Path) {
    #[cfg(target_os = "windows")]
    const OPENER: &str = "explorer";
    #[cfg(target_os = "macos")]
    const OPENER: &str = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    const OPENER: &str = "xdg-open";

    let _ = std::process::Command::new(OPENER).arg(path).spawn();
}

fn default_save_directory() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .map(|home| home.join("Downloads"))
        .unwrap_or_else(std::env::temp_dir)
}

/// The rail shows the broker host without its endpoint id, which is too long to
/// be readable at rail width.
fn broker_host() -> String {
    crate::engine::BROKER
        .split_once('@')
        .map(|(_, host)| host.to_owned())
        .unwrap_or_else(|| crate::engine::BROKER.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a transfer without an engine behind it, so previews can show
    /// states that would otherwise need a live peer.
    fn seed(id: u64, mode: Mode, label: &str, stage: Stage) -> Transfer {
        let mut transfer = Transfer::new(
            TransferId(id),
            mode,
            label.to_owned(),
            PathBuf::from("/home/chkxwlyh/Downloads"),
        );
        transfer.stage = stage;
        transfer
    }

    /// Renders the shell offscreen so the layout can be reviewed without a
    /// display server, and regressions in it show up as an image diff.
    fn preview(name: &str, seed_app: impl FnOnce(&mut App)) {
        let mut app: Option<App> = None;
        let mut seed_app = Some(seed_app);
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1180.0, 720.0))
            .wgpu()
            .build_ui(|ui| {
                let Some(app) = app.as_mut() else {
                    // Fonts only bind on the next frame, so install them and
                    // draw nothing until they are available.
                    theme::install_fonts(ui.ctx());
                    let mut fresh = App::new(ui.ctx());
                    if let Some(seed_app) = seed_app.take() {
                        seed_app(&mut fresh);
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

    const SAMPLE_INVITE: &str = "envoix://invite/v2/eyJyIjoiNDgwOTY2LXU5ZmMtOWM2aCJ9";

    #[test]
    fn idle_light() {
        preview("idle-light", |app| {
            app.dark = false;
            app.mode = Mode::Receive;
        });
    }

    #[test]
    fn waiting_for_a_sender_light() {
        preview("waiting-light", |app| {
            app.dark = false;
            let mut transfer = seed(1, Mode::Receive, "Incoming transfer", Stage::Waiting);
            transfer.qr_matrix = QrMatrix::encode(SAMPLE_INVITE);
            transfer.invite = Some(SAMPLE_INVITE.to_owned());
            transfer.room_code = Some("480966-u9fc-9c6h".to_owned());
            transfer.status = "Waiting for a sender".to_owned();
            app.transfers.push(transfer);
        });
    }

    /// The point of the card list: several transfers at once, in different
    /// states, which is what the mobile client has always done.
    #[test]
    fn several_transfers_light() {
        preview("several-light", |app| {
            app.dark = false;
            let mut running = seed(3, Mode::Send, "quarterly-report.pdf", Stage::Running);
            running.status = "Transferring".to_owned();
            running.progress = Some((6_815_744, 10_485_760));
            running.rate = Some(14_680_064.0);
            running.data_path = Some("direct".to_owned());

            let mut offered = seed(2, Mode::Receive, "Incoming transfer", Stage::Offered);
            offered.status = "Offer received".to_owned();
            offered.data_path = Some("direct".to_owned());
            offered.offer = Some(OfferSummary {
                roots: vec!["photos".to_owned()],
                files: 19,
                directories: 2,
                bytes: 8_388_608,
            });

            let mut done = seed(1, Mode::Receive, "Incoming transfer", Stage::Done);
            done.status = "Delivered".to_owned();
            done.progress = Some((2_097_152, 2_097_152));
            done.result = Some((4, 2_097_152));
            done.offer = Some(OfferSummary {
                roots: vec!["slides.key".to_owned()],
                files: 4,
                directories: 0,
                bytes: 2_097_152,
            });

            app.transfers.extend([running, offered, done]);
        });
    }

    #[test]
    fn transferring_dark() {
        preview("transferring-dark", |app| {
            app.dark = true;
            app.mode = Mode::Send;
            app.files = vec![
                PathBuf::from("/home/demo/quarterly-report.pdf"),
                PathBuf::from("/home/demo/photos"),
            ];
            // These paths do not exist, so the total cannot be measured here.
            app.selection_bytes = 8_912_896;
            app.invite_input = SAMPLE_INVITE.to_owned();

            let mut transfer = seed(1, Mode::Send, "quarterly-report.pdf", Stage::Running);
            transfer.status = "Transferring".to_owned();
            transfer.progress = Some((6_815_744, 10_485_760));
            transfer.rate = Some(14_680_064.0);
            transfer.data_path = Some("direct".to_owned());
            app.transfers.push(transfer);
        });
    }

    #[test]
    fn failed_light() {
        preview("failed-light", |app| {
            app.dark = false;
            let mut transfer = seed(1, Mode::Receive, "Incoming transfer", Stage::Failed);
            transfer.status = "Failed".to_owned();
            transfer.room_code = Some("480966-u9fc-9c6h".to_owned());
            transfer.progress = Some((3_145_728, 8_388_608));
            transfer.error = Some(
                "one-time invitation was consumed after authentication: \
                 transport error: early eof"
                    .to_owned(),
            );
            app.transfers.push(transfer);
        });
    }

    #[test]
    fn logs_dark() {
        preview("logs-dark", |app| {
            app.dark = true;
            app.tab = Tab::Logs;
            app.logs = vec![
                "invite 480966-u9fc-9c6h".to_owned(),
                "Waiting for a sender".to_owned(),
                "Pairing: Joined".to_owned(),
                "connected via direct".to_owned(),
                "offer: 19 files, 8.0 MB".to_owned(),
                "Transferring".to_owned(),
                "Verifying".to_owned(),
                "delivered 21 entries, 8.0 MB".to_owned(),
            ];
        });
    }

    #[test]
    fn title_reports_transfer_state() {
        assert_eq!(
            title_for(Stage::Waiting, None),
            "Envoix - waiting for a peer"
        );
        assert_eq!(
            title_for(Stage::Running, Some((5_242_880, 10_485_760))),
            "Envoix - 50%"
        );
        // A zero total is the state before the manifest lands, not 0%.
        assert_eq!(
            title_for(Stage::Running, Some((0, 0))),
            "Envoix - transferring"
        );
        // Byte counts big enough that scaling by 100 would wrap a u64.
        assert_eq!(
            title_for(Stage::Running, Some((u64::MAX / 2, u64::MAX))),
            "Envoix - 50%"
        );
        assert_eq!(title_for(Stage::Done, None), "Envoix - delivered");
        assert_eq!(title_for(Stage::Failed, None), "Envoix - failed");
    }

    /// Events must land on the transfer they name, not the newest one.
    #[test]
    fn events_route_to_their_own_transfer() {
        let context = egui::Context::default();
        let mut app = App::new(&context);
        app.transfers
            .push(seed(1, Mode::Receive, "first", Stage::Waiting));
        app.transfers
            .push(seed(2, Mode::Send, "second", Stage::Waiting));

        app.transfer_mut(TransferId(2))
            .expect("second transfer")
            .stage = Stage::Running;

        assert_eq!(
            app.transfers[0].stage,
            Stage::Waiting,
            "first was disturbed"
        );
        assert_eq!(app.transfers[1].stage, Stage::Running);
        assert!(app.busy());
    }
}
