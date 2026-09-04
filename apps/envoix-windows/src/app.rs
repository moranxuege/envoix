use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, RichText};
use envoix_client::model::TransferState;
use envoix_client::product::{
    AgentOfferDecision, AgentPathKind, AgentRequest, AgentResponse, AgentTransferTelemetry,
    PairingInvitation,
};

use crate::controller::{
    AgentController, ControllerCommand, ControllerEvent, Dashboard, Operation, join_request,
    offer_decision_request,
};
use crate::presentation::{
    direction_text, human_bytes, transfer_eta, transfer_path_text, transfer_phase_text,
    transfer_rate, transfer_state_text,
};
use crate::theme::{
    ACCENT, ACCENT_DARK, ACCENT_SOFT, BACKGROUND, BORDER, DANGER, DANGER_SOFT, MUTED, SUCCESS,
    SUCCESS_SOFT, SURFACE, SURFACE_RAISED, TEXT, WARNING, WARNING_SOFT,
};

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const TOAST_DURATION: Duration = Duration::from_secs(4);
const SIDEBAR_WIDTH: f32 = 226.0;
const CARD_RADIUS: u8 = 20;
const CARD_PADDING: i8 = 22;
const BMP_DIB_HEADER_BYTES: u32 = 40;
const BMP_PIXEL_OFFSET: u32 = 54;
const BMP_BYTES_PER_PIXEL: u32 = 4;
const BMP_PLANES: u16 = 1;
const BMP_BITS_PER_PIXEL: u16 = 32;
const BMP_PIXELS_PER_METER: i32 = 2_835;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Page {
    Devices,
    Activity,
    Inbox,
    Settings,
}

pub(crate) struct EnvoixWindowsApp {
    controller: AgentController,
    page: Page,
    dashboard: Option<Dashboard>,
    refresh_pending: bool,
    last_refresh: Instant,
    active_operation: Option<Operation>,
    error: Option<String>,
    toast: Option<(String, Instant)>,
    selected_device: Option<String>,
    selected_paths: Vec<PathBuf>,
    show_pair: bool,
    pair_label: String,
    pairing: Option<PairingInvitation>,
    show_join: bool,
    join_label: String,
    join_invitation: String,
    join_verification: String,
    revoke_device: Option<String>,
    screenshot_target: Option<PathBuf>,
    screenshot_requested: bool,
}

impl EnvoixWindowsApp {
    pub(crate) fn new(context: &egui::Context) -> Self {
        configure_style(context);
        let controller = AgentController::start(context.clone());
        let screenshot_target = std::env::var_os("ENVOIX_UI_SCREENSHOT").map(PathBuf::from);
        let page = if screenshot_target.is_some() {
            match std::env::var("ENVOIX_UI_PAGE").as_deref() {
                Ok("activity") => Page::Activity,
                Ok("inbox") => Page::Inbox,
                Ok("settings") => Page::Settings,
                _ => Page::Devices,
            }
        } else {
            Page::Devices
        };
        if let Some(target) = &screenshot_target {
            let _ = std::fs::write(target.with_extension("pending"), b"waiting for screenshot");
        }
        let mut app = Self {
            controller,
            page,
            dashboard: None,
            refresh_pending: false,
            last_refresh: Instant::now() - REFRESH_INTERVAL,
            active_operation: None,
            error: None,
            toast: None,
            selected_device: None,
            selected_paths: Vec::new(),
            show_pair: false,
            pair_label: String::new(),
            pairing: None,
            show_join: false,
            join_label: String::new(),
            join_invitation: String::new(),
            join_verification: String::new(),
            revoke_device: None,
            screenshot_target,
            screenshot_requested: false,
        };
        app.request_refresh();
        if app.screenshot_target.is_some() {
            let deadline = Instant::now() + Duration::from_secs(2);
            while app.dashboard.is_none() && app.error.is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(40));
                app.drain_events();
            }
        }
        app
    }

    fn request_refresh(&mut self) {
        if self.refresh_pending || self.active_operation.is_some() {
            return;
        }
        match self.controller.send(ControllerCommand::Refresh) {
            Ok(()) => self.refresh_pending = true,
            Err(error) => self.error = Some(error),
        }
    }

    fn start_agent_operation(&mut self, operation: Operation, request: AgentRequest) {
        if self.active_operation.is_some() {
            return;
        }
        match self
            .controller
            .send(ControllerCommand::Agent { operation, request })
        {
            Ok(()) => self.active_operation = Some(operation),
            Err(error) => self.error = Some(error),
        }
    }

    fn start_lifecycle(&mut self, operation: Operation) {
        if self.active_operation.is_some() {
            return;
        }
        match self
            .controller
            .send(ControllerCommand::Lifecycle(operation))
        {
            Ok(()) => self.active_operation = Some(operation),
            Err(error) => self.error = Some(error),
        }
    }

    fn drain_events(&mut self) {
        while let Some(event) = self.controller.try_event() {
            match event {
                ControllerEvent::Dashboard(result) => {
                    self.refresh_pending = false;
                    self.last_refresh = Instant::now();
                    match result {
                        Ok(dashboard) => {
                            self.error = None;
                            if self.selected_device.as_ref().is_none_or(|selected| {
                                !dashboard
                                    .devices
                                    .iter()
                                    .any(|device| &device.id == selected)
                            }) {
                                self.selected_device =
                                    dashboard.devices.first().map(|device| device.id.clone());
                            }
                            self.dashboard = Some(dashboard);
                        }
                        Err(error) => {
                            self.error = Some(error);
                            self.dashboard = None;
                        }
                    }
                }
                ControllerEvent::Operation { operation, result } => {
                    self.active_operation = None;
                    match result {
                        Ok(response) => self.handle_operation_response(operation, response),
                        Err(error) => self.error = Some(error),
                    }
                    self.request_refresh();
                }
            }
        }
    }

    fn handle_operation_response(&mut self, operation: Operation, response: Option<AgentResponse>) {
        match response {
            Some(AgentResponse::Pairing { pairing }) if operation == Operation::Pair => {
                self.pairing = Some(pairing);
                self.show_toast("配对房间已建立，正在等待另一台设备");
            }
            Some(AgentResponse::DevicePaired { device }) if operation == Operation::JoinPairing => {
                self.show_join = false;
                self.selected_device = Some(device.id);
                self.join_invitation.clear();
                self.join_verification.clear();
                self.show_toast("设备验证完成并已安全保存");
            }
            Some(AgentResponse::TransferCreated { .. })
                if operation == Operation::CreateTransfer =>
            {
                self.selected_paths.clear();
                self.page = Page::Activity;
                self.show_toast("文件已加入发送队列");
            }
            Some(AgentResponse::PendingOfferDecided { .. })
                if matches!(operation, Operation::ApproveOffer | Operation::RejectOffer) =>
            {
                self.show_toast(if operation == Operation::ApproveOffer {
                    "已允许接收，传输即将开始"
                } else {
                    "已拒绝本次接收"
                });
            }
            Some(AgentResponse::Transfer { .. })
                if matches!(
                    operation,
                    Operation::PauseTransfer
                        | Operation::ResumeTransfer
                        | Operation::RetryTransfer
                        | Operation::CancelTransfer
                ) =>
            {
                self.show_toast(match operation {
                    Operation::PauseTransfer => "正在暂停传输",
                    Operation::ResumeTransfer => "传输已继续",
                    Operation::RetryTransfer => "已重新尝试传输",
                    Operation::CancelTransfer => "传输已取消",
                    _ => unreachable!(),
                });
            }
            Some(AgentResponse::TransferRemoved { .. })
                if operation == Operation::RemoveTransfer =>
            {
                self.show_toast("传输记录已移除");
            }
            Some(AgentResponse::DeviceRevoked { device })
                if operation == Operation::RevokeDevice =>
            {
                self.selected_device = None;
                self.revoke_device = None;
                self.show_toast(&format!("已忘记设备 {}", device.label));
            }
            Some(AgentResponse::PreferencesUpdated { .. })
                if operation == Operation::SetInboxDirectory =>
            {
                self.show_toast("收件位置已更新");
            }
            Some(AgentResponse::Error { code, message }) => {
                self.error = Some(format!("操作失败（{code}）：{message}"));
            }
            None if matches!(
                operation,
                Operation::InstallAgent | Operation::StartAgent | Operation::RestartAgent
            ) =>
            {
                self.show_toast(match operation {
                    Operation::InstallAgent => "后台传输已安装并启动",
                    Operation::StartAgent => "后台传输已启动",
                    Operation::RestartAgent => "后台传输已重新启动",
                    _ => unreachable!(),
                });
            }
            _ => self.error = Some("后台服务返回了无法识别的结果".to_owned()),
        }
    }

    fn show_toast(&mut self, message: &str) {
        self.error = None;
        self.toast = Some((message.to_owned(), Instant::now()));
    }

    fn handle_screenshot_result(&mut self, context: &egui::Context) {
        let screenshot = context.input(|input| {
            input.raw.events.iter().find_map(|event| {
                if let egui::Event::Screenshot { image, .. } = event {
                    Some(std::sync::Arc::clone(image))
                } else {
                    None
                }
            })
        });
        let Some(image) = screenshot else {
            return;
        };
        let Some(target) = self.screenshot_target.take() else {
            return;
        };
        if let Err(error) = write_bmp(&target, &image) {
            self.error = Some(format!("无法写入 UI 截图：{error}"));
        }
        let _ = std::fs::remove_file(target.with_extension("pending"));
        let _ = std::fs::remove_file(target.with_extension("requested"));
        context.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn maybe_request_screenshot(&mut self, context: &egui::Context) {
        if self.screenshot_target.is_some() && !self.screenshot_requested {
            if let Some(target) = &self.screenshot_target {
                let _ = std::fs::write(target.with_extension("requested"), b"screenshot requested");
            }
            self.screenshot_requested = true;
            context.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
    }

    fn busy(&self) -> bool {
        self.active_operation.is_some()
    }

    fn render_header(&mut self, root: &mut egui::Ui) {
        egui::Panel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(BACKGROUND)
                    .inner_margin(egui::Margin::symmetric(28, 18)),
            )
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    let (title, subtitle) = page_heading(self.page);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(title).size(28.0).strong().color(TEXT));
                        ui.label(RichText::new(subtitle).size(13.5).color(MUTED));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !self.refresh_pending && !self.busy(),
                                quiet_button("刷新"),
                            )
                            .clicked()
                        {
                            self.request_refresh();
                        }
                        if self.busy() {
                            ui.spinner();
                        }
                        match &self.dashboard {
                            Some(dashboard) => status_pill(
                                ui,
                                &format!("●  后台传输已就绪 · {}", dashboard.status.device_name),
                                SUCCESS,
                                SUCCESS_SOFT,
                            ),
                            None => status_pill(ui, "●  后台传输未就绪", DANGER, DANGER_SOFT),
                        }
                    });
                });
            });
    }

    fn render_sidebar(&mut self, root: &mut egui::Ui) {
        egui::Panel::left("navigation")
            .exact_size(SIDEBAR_WIDTH)
            .frame(
                egui::Frame::new()
                    .fill(SURFACE)
                    .stroke(egui::Stroke::new(1.0, BORDER))
                    .inner_margin(egui::Margin::symmetric(18, 20)),
            )
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    brand_mark(ui);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Envoix").size(21.0).strong().color(TEXT));
                        ui.label(RichText::new("安全文件传输").size(11.0).color(MUTED));
                    });
                });
                ui.add_space(34.0);
                ui.label(RichText::new("传输").size(10.0).strong().color(MUTED));
                ui.add_space(8.0);
                nav_button(ui, &mut self.page, Page::Devices, "01", "设备");
                nav_button(ui, &mut self.page, Page::Activity, "02", "传输活动");
                nav_button(ui, &mut self.page, Page::Inbox, "03", "收件箱");
                ui.add_space(18.0);
                ui.label(RichText::new("应用").size(10.0).strong().color(MUTED));
                ui.add_space(8.0);
                nav_button(ui, &mut self.page, Page::Settings, "04", "设置");
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    egui::Frame::new()
                        .fill(ACCENT_SOFT)
                        .stroke(egui::Stroke::new(1.0, BORDER))
                        .corner_radius(14)
                        .inner_margin(12.0)
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("后台传输已启用")
                                    .size(12.0)
                                    .strong()
                                    .color(ACCENT_DARK),
                            );
                            ui.label(
                                RichText::new("关闭窗口后仍会继续发送")
                                    .size(10.5)
                                    .color(MUTED),
                            );
                        });
                });
            });
    }

    fn render_unavailable(&mut self, ui: &mut egui::Ui) {
        empty_state(ui, "!", "后台传输尚未就绪", |ui| {
            ui.label(
                RichText::new("启动后台传输后，已配对设备和传输记录会自动恢复。").color(MUTED),
            );
            if let Some(error) = &self.error {
                ui.colored_label(DANGER, error);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.busy(), primary_button("安装并启动"))
                    .clicked()
                {
                    self.start_lifecycle(Operation::InstallAgent);
                }
                if ui
                    .add_enabled(!self.busy(), quiet_button("重新连接"))
                    .clicked()
                {
                    self.start_lifecycle(Operation::StartAgent);
                }
            });
        });
    }

    fn render_devices(&mut self, ui: &mut egui::Ui) {
        let devices = self
            .dashboard
            .as_ref()
            .map(|dashboard| dashboard.devices.clone())
            .unwrap_or_default();

        let dropped_paths = ui.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect::<Vec<_>>()
        });
        append_unique_paths(&mut self.selected_paths, dropped_paths);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} 台已验证设备", devices.len()))
                    .size(13.0)
                    .strong()
                    .color(MUTED),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(primary_button("＋  添加设备")).clicked() {
                    self.show_pair = true;
                    self.pairing = None;
                }
                if ui.add(quiet_button("使用配对码加入")).clicked() {
                    self.show_join = true;
                }
            });
        });
        ui.add_space(18.0);

        if devices.is_empty() {
            empty_state(ui, "+", "还没有已验证的设备", |ui| {
                ui.label(
                    RichText::new("创建一个一次性配对房间，或输入另一台设备给出的配对信息。")
                        .color(MUTED),
                );
            });
            return;
        }

        ui.horizontal_wrapped(|ui| {
            for device in &devices {
                let selected = self.selected_device.as_deref() == Some(device.id.as_str());
                if device_button(ui, &device.label, device.generation, selected).clicked() {
                    self.selected_device = Some(device.id.clone());
                }
            }
        });
        ui.add_space(20.0);

        let Some(device) = devices
            .iter()
            .find(|device| Some(device.id.as_str()) == self.selected_device.as_deref())
            .cloned()
        else {
            return;
        };
        room_card(ui, |ui| {
            ui.horizontal(|ui| {
                device_avatar(ui, &device.label, 48.0, ACCENT);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&device.label).size(21.0).strong().color(TEXT));
                    ui.label(
                        RichText::new("已验证的私人房间 · 离线时也可以排队")
                            .size(12.5)
                            .color(MUTED),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(danger_quiet_button("忘记设备")).clicked() {
                        self.revoke_device = Some(device.id.clone());
                    }
                    status_pill(ui, "●  房间就绪", SUCCESS, SUCCESS_SOFT);
                });
            });
            ui.add_space(18.0);

            let hovering_files = ui.input(|input| !input.raw.hovered_files.is_empty());
            egui::Frame::new()
                .fill(if hovering_files {
                    ACCENT_SOFT
                } else {
                    BACKGROUND
                })
                .stroke(egui::Stroke::new(
                    if hovering_files { 2.0 } else { 1.0 },
                    if hovering_files { ACCENT } else { BORDER },
                ))
                .corner_radius(18)
                .inner_margin(22.0)
                .show(ui, |ui| {
                    ui.set_min_height(if self.selected_paths.is_empty() {
                        190.0
                    } else {
                        228.0
                    });
                    ui.vertical_centered(|ui| {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(if hovering_files { "↓" } else { "＋" })
                                .size(34.0)
                                .strong()
                                .color(ACCENT),
                        );
                        ui.label(
                            RichText::new(if hovering_files {
                                "松开即可添加"
                            } else {
                                "把文件拖到这里"
                            })
                            .size(18.0)
                            .strong()
                            .color(TEXT),
                        );
                        ui.label(
                            RichText::new("也可以从系统文件选择器添加多个文件或整个文件夹")
                                .size(12.5)
                                .color(MUTED),
                        );
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui.add(quiet_button("选择文件")).clicked()
                                && let Some(paths) = rfd::FileDialog::new().pick_files()
                            {
                                append_unique_paths(&mut self.selected_paths, paths);
                            }
                            if ui.add(quiet_button("选择文件夹")).clicked()
                                && let Some(path) = rfd::FileDialog::new().pick_folder()
                            {
                                append_unique_paths(&mut self.selected_paths, vec![path]);
                            }
                        });
                    });

                    if !self.selected_paths.is_empty() {
                        ui.add_space(14.0);
                        ui.separator();
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "已选择 {} 个项目",
                                    self.selected_paths.len()
                                ))
                                .strong()
                                .color(TEXT),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.add(danger_quiet_button("清空")).clicked() {
                                        self.selected_paths.clear();
                                    }
                                },
                            );
                        });
                        for path in self.selected_paths.iter().take(4) {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("●").size(8.0).color(ACCENT));
                                ui.label(
                                    RichText::new(path_display_name(path))
                                        .size(12.5)
                                        .color(TEXT),
                                );
                                ui.label(
                                    RichText::new(path.display().to_string())
                                        .size(10.5)
                                        .color(MUTED),
                                );
                            });
                        }
                        if self.selected_paths.len() > 4 {
                            ui.label(
                                RichText::new(format!(
                                    "另外 {} 个项目",
                                    self.selected_paths.len() - 4
                                ))
                                .size(11.0)
                                .color(MUTED),
                            );
                        }
                    }
                });

            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("关闭窗口后仍会继续发送")
                        .size(11.5)
                        .color(MUTED),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_send = !self.selected_paths.is_empty() && !self.busy();
                    if ui
                        .add_enabled(can_send, primary_button("发送到这台设备  →"))
                        .clicked()
                    {
                        self.start_agent_operation(
                            Operation::CreateTransfer,
                            AgentRequest::CreateTransfer {
                                device: device.id,
                                paths: self.selected_paths.clone(),
                            },
                        );
                    }
                });
            });
        });

        self.render_pending_offers(ui);
    }

    fn render_pending_offers(&mut self, ui: &mut egui::Ui) {
        let offers = self
            .dashboard
            .as_ref()
            .map(|dashboard| dashboard.pending_offers.clone())
            .unwrap_or_default();
        if offers.is_empty() {
            return;
        }
        ui.add_space(22.0);
        ui.label(RichText::new("等待你确认").size(16.0).strong().color(TEXT));
        ui.label(
            RichText::new("对方只会在你允许后开始发送内容。")
                .size(12.0)
                .color(MUTED),
        );
        ui.add_space(10.0);
        for offer in offers {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    device_avatar(ui, &offer.from_device_label, 42.0, ACCENT);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(format!("来自 {}", offer.from_device_label))
                                .strong()
                                .color(TEXT),
                        );
                        ui.label(
                            RichText::new(format!(
                                "{} 个项目 · {} 个文件夹 · {}",
                                offer.item_count,
                                offer.directory_count,
                                human_bytes(offer.total_bytes)
                            ))
                            .size(12.0)
                            .color(MUTED),
                        );
                        ui.label(
                            RichText::new(offer.root_names.join("、"))
                                .size(11.0)
                                .color(MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!self.busy(), primary_button("允许接收"))
                            .clicked()
                        {
                            self.start_agent_operation(
                                Operation::ApproveOffer,
                                offer_decision_request(
                                    offer.offer_id.clone(),
                                    AgentOfferDecision::Approve,
                                ),
                            );
                        }
                        if ui
                            .add_enabled(!self.busy(), danger_quiet_button("拒绝"))
                            .clicked()
                        {
                            self.start_agent_operation(
                                Operation::RejectOffer,
                                offer_decision_request(
                                    offer.offer_id.clone(),
                                    AgentOfferDecision::Reject,
                                ),
                            );
                        }
                    });
                });
            });
            ui.add_space(10.0);
        }
    }

    fn render_activity(&mut self, ui: &mut egui::Ui) {
        let Some(dashboard) = self.dashboard.clone() else {
            return;
        };
        let delivered = dashboard
            .transfers
            .iter()
            .filter(|transfer| transfer.state == TransferState::Delivered)
            .count();
        let attention = dashboard
            .transfers
            .iter()
            .filter(|transfer| {
                matches!(
                    transfer.state,
                    TransferState::Failed | TransferState::Rejected | TransferState::Canceled
                )
            })
            .count();
        let durable_active = dashboard
            .transfers
            .len()
            .saturating_sub(delivered + attention);
        let transfer_ids = dashboard
            .transfers
            .iter()
            .map(|transfer| transfer.id.to_string())
            .collect::<std::collections::HashSet<_>>();
        let orphan_telemetry = dashboard
            .telemetry
            .iter()
            .filter(|value| !transfer_ids.contains(&value.transfer_id))
            .collect::<Vec<_>>();
        let active = durable_active + orphan_telemetry.len();

        ui.columns(3, |columns| {
            metric_card(
                &mut columns[0],
                "已送达",
                delivered,
                SUCCESS,
                "接收方已确认保存",
            );
            metric_card(
                &mut columns[1],
                "进行中",
                active,
                ACCENT,
                "含排队与等待确认",
            );
            metric_card(
                &mut columns[2],
                "需要留意",
                attention,
                WARNING,
                "失败、拒绝或取消",
            );
        });
        ui.add_space(22.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("最近传输").size(16.0).strong().color(TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new("“已送达”表示接收方已经安全保存")
                        .size(11.5)
                        .color(MUTED),
                );
            });
        });
        ui.add_space(10.0);
        if dashboard.transfers.is_empty() && dashboard.telemetry.is_empty() {
            empty_state(ui, "↗", "暂无传输记录", |ui| {
                ui.label(
                    RichText::new("从设备页面选择内容，第一笔传输会显示在这里。").color(MUTED),
                );
            });
            return;
        }
        for telemetry in orphan_telemetry {
            let device = dashboard
                .devices
                .iter()
                .find(|device| device.id == telemetry.relationship_id)
                .map(|device| device.label.as_str())
                .unwrap_or("已配对设备");
            let path = dashboard
                .active_paths
                .iter()
                .find(|path| path.transfer_id == telemetry.transfer_id)
                .map(|path| path.path);
            render_live_transfer_card(ui, telemetry, device, path);
            ui.add_space(10.0);
        }
        for transfer in dashboard.transfers.iter().rev() {
            let device = dashboard
                .devices
                .iter()
                .find(|device| device.id == transfer.relationship_id.to_string())
                .map(|device| device.label.as_str())
                .unwrap_or("已配对设备");
            let state_color = match transfer.state {
                TransferState::Delivered => SUCCESS,
                TransferState::Failed | TransferState::Rejected | TransferState::Canceled => DANGER,
                TransferState::Paused => WARNING,
                _ => ACCENT,
            };
            let telemetry = dashboard
                .telemetry
                .iter()
                .find(|value| value.transfer_id == transfer.id.to_string());
            let transferred_bytes = telemetry
                .map(|value| value.transferred_bytes)
                .unwrap_or(transfer.transferred_bytes);
            let total_bytes = telemetry
                .map(|value| value.total_bytes)
                .unwrap_or(transfer.total_bytes);
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    let direction = direction_text(transfer.direction);
                    transfer_icon(ui, direction, state_color);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(format!("{} · {}", direction, device))
                                .size(14.0)
                                .strong()
                                .color(TEXT),
                        );
                        ui.label(
                            RichText::new(transfer_content_summary(
                                telemetry,
                                transferred_bytes,
                                total_bytes,
                            ))
                            .size(11.5)
                            .color(MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        state_pill(
                            ui,
                            telemetry
                                .map(|value| transfer_phase_text(value.phase))
                                .unwrap_or_else(|| transfer_state_text(transfer.state)),
                            state_color,
                        );
                    });
                });
                let progress = if total_bytes == 0 {
                    0.0
                } else {
                    transferred_bytes as f32 / total_bytes as f32
                };
                ui.add_space(8.0);
                ui.add(
                    egui::ProgressBar::new(progress.clamp(0.0, 1.0))
                        .fill(state_color)
                        .desired_height(9.0)
                        .corner_radius(8),
                );
                if let Some(telemetry) = telemetry {
                    render_transfer_metrics(ui, telemetry);
                }
                if let Some(path) = dashboard
                    .active_paths
                    .iter()
                    .find(|path| path.transfer_id == transfer.id.to_string())
                {
                    ui.label(
                        RichText::new(format!("连接方式：{}", transfer_path_text(path.path)))
                            .color(MUTED),
                    );
                }
                if let Some(failure) = &transfer.failure {
                    ui.colored_label(
                        DANGER,
                        format!(
                            "{:?} · {}",
                            failure.code,
                            if failure.retryable {
                                "可以重试"
                            } else {
                                "不可重试"
                            }
                        ),
                    );
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let transfer_id = transfer.id.to_string();
                    match transfer.state {
                        TransferState::Connecting | TransferState::Transferring => {
                            if ui.add_enabled(!self.busy(), quiet_button("暂停")).clicked() {
                                self.start_agent_operation(
                                    Operation::PauseTransfer,
                                    AgentRequest::PauseTransfer {
                                        transfer_id: transfer_id.clone(),
                                    },
                                );
                            }
                        }
                        TransferState::Paused => {
                            if ui
                                .add_enabled(!self.busy(), primary_button("继续"))
                                .clicked()
                            {
                                self.start_agent_operation(
                                    Operation::ResumeTransfer,
                                    AgentRequest::ResumeTransfer {
                                        transfer_id: transfer_id.clone(),
                                    },
                                );
                            }
                        }
                        TransferState::Failed
                            if transfer
                                .failure
                                .as_ref()
                                .is_some_and(|failure| failure.retryable) =>
                        {
                            if ui
                                .add_enabled(!self.busy(), primary_button("重试"))
                                .clicked()
                            {
                                self.start_agent_operation(
                                    Operation::RetryTransfer,
                                    AgentRequest::RecoverTransfer {
                                        transfer_id: transfer_id.clone(),
                                    },
                                );
                            }
                        }
                        _ => {}
                    }
                    if transfer.state.can_cancel()
                        && ui
                            .add_enabled(!self.busy(), danger_quiet_button("取消"))
                            .clicked()
                    {
                        self.start_agent_operation(
                            Operation::CancelTransfer,
                            AgentRequest::CancelTransfer {
                                transfer_id: transfer_id.clone(),
                            },
                        );
                    }
                    if transfer.state.is_terminal()
                        && ui
                            .add_enabled(!self.busy(), danger_quiet_button("移除记录"))
                            .clicked()
                    {
                        self.start_agent_operation(
                            Operation::RemoveTransfer,
                            AgentRequest::RemoveTransfer { transfer_id },
                        );
                    }
                });
                ui.label(
                    RichText::new(format!("传输 ID  {}", transfer.id))
                        .size(9.5)
                        .color(Color32::from_rgb(151, 160, 174)),
                );
            });
            ui.add_space(10.0);
        }
    }

    fn render_inbox(&mut self, ui: &mut egui::Ui) {
        let items = self
            .dashboard
            .as_ref()
            .map(|dashboard| dashboard.inbox.clone())
            .unwrap_or_default();
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(format!("{} 批已保存内容", items.len()))
                            .size(18.0)
                            .strong()
                            .color(TEXT),
                    );
                    ui.label(
                        RichText::new("这里只显示完整校验、已经安全写入磁盘的内容。")
                            .size(12.0)
                            .color(MUTED),
                    );
                });
                if let Some(inbox) = self
                    .dashboard
                    .as_ref()
                    .map(|dashboard| dashboard.status.inbox_directory.clone())
                {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(primary_button("打开收件箱  →")).clicked() {
                            open_in_explorer(PathBuf::from(inbox));
                        }
                    });
                }
            });
        });
        ui.add_space(18.0);
        if items.is_empty() {
            empty_state(ui, "↓", "尚未收到文件", |ui| {
                ui.label(
                    RichText::new("对方发送并完成校验后，文件会自动出现在这里。").color(MUTED),
                );
            });
            return;
        }
        for item in items {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    transfer_icon(ui, "接收", SUCCESS);
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(format!("来自 {}", item.from_device_label))
                                .size(14.0)
                                .strong()
                                .color(TEXT),
                        );
                        ui.label(
                            RichText::new(format!(
                                "{} 个文件 · {} 个文件夹 · {}",
                                item.file_count,
                                item.directory_count,
                                human_bytes(item.total_bytes)
                            ))
                            .size(11.5)
                            .color(MUTED),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        state_pill(ui, "已保存", SUCCESS);
                    });
                });
                ui.add_space(10.0);
                for root in &item.roots {
                    egui::Frame::new()
                        .fill(BACKGROUND)
                        .corner_radius(12)
                        .inner_margin(egui::Margin::symmetric(14, 9))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&root.name).strong().color(TEXT));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.add(quiet_button("在资源管理器中显示")).clicked()
                                        {
                                            open_in_explorer(PathBuf::from(&root.path));
                                        }
                                    },
                                );
                            });
                        });
                }
            });
            ui.add_space(10.0);
        }
    }

    fn render_settings(&mut self, ui: &mut egui::Ui) {
        let Some(dashboard) = &self.dashboard else {
            self.render_unavailable(ui);
            return;
        };
        let status = dashboard.status.clone();
        let diagnostics = dashboard.diagnostics.clone();
        ui.columns(2, |columns| {
            card(&mut columns[0], |ui| {
                ui.label(RichText::new("隐私与安全").size(16.0).strong().color(TEXT));
                ui.add_space(8.0);
                security_line(ui, "●", "设备连接信息由 Windows 安全存储保护");
                security_line(ui, "●", "只有当前 Windows 用户可以访问");
                security_line(ui, "●", "界面不会读取或显示连接密钥");
            });
            card(&mut columns[1], |ui| {
                ui.label(RichText::new("后台传输").size(16.0).strong().color(TEXT));
                ui.add_space(8.0);
                key_value(ui, "设备", &status.device_name);
                key_value(ui, "状态", "运行中");
                ui.add_space(8.0);
                if ui
                    .add_enabled(!self.busy(), quiet_button("重新启动后台传输"))
                    .clicked()
                {
                    self.start_lifecycle(Operation::RestartAgent);
                }
            });
        });
        ui.add_space(18.0);
        card(ui, |ui| {
            ui.label(RichText::new("连接与存储").size(16.0).strong().color(TEXT));
            ui.label(
                RichText::new("一般情况下无需修改这些地址。")
                    .size(11.5)
                    .color(MUTED),
            );
            ui.add_space(10.0);
            key_value(ui, "协调服务", &status.broker);
            key_value(ui, "中继服务", status.relay.as_deref().unwrap_or("未启用"));
            key_value(ui, "接收文件夹", &status.inbox_directory);
            if ui
                .add_enabled(!self.busy(), quiet_button("更改收件位置"))
                .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_directory(&status.inbox_directory)
                    .pick_folder()
            {
                self.start_agent_operation(
                    Operation::SetInboxDirectory,
                    AgentRequest::SetInboxDirectory { path },
                );
            }
            ui.label(
                RichText::new("之后收到的文件会保存到新位置；进行中的传输不受影响。")
                    .size(11.0)
                    .color(MUTED),
            );
            ui.add_space(8.0);
            ui.collapsing("技术信息", |ui| {
                key_value(ui, "后台接口版本", &format!("v{}", status.protocol_version));
                key_value(
                    ui,
                    "数据版本",
                    &format!("v{}", diagnostics.engine_schema_version),
                );
                key_value(
                    ui,
                    "系统保护",
                    &format!(
                        "{:?} · {:?}",
                        diagnostics.credential_protection, diagnostics.control_transport
                    ),
                );
            });
        });
    }

    fn render_pair_window(&mut self, context: &egui::Context) {
        if !self.show_pair {
            return;
        }
        let mut open = self.show_pair;
        egui::Window::new("添加设备")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .show(context, |ui| {
                if let Some(pairing) = &self.pairing {
                    ui.colored_label(SUCCESS, "房间已建立，等待另一台设备加入");
                    ui.label("在另一台设备选择“加入配对”，输入下面的房间码和验证码。");
                    ui.separator();
                    ui.label(RichText::new("房间码").color(MUTED));
                    ui.horizontal(|ui| {
                        ui.monospace(&pairing.room_code);
                        if ui.button("复制").clicked() {
                            ui.ctx().copy_text(pairing.room_code.clone());
                        }
                    });
                    ui.label(RichText::new("验证码").color(MUTED));
                    ui.monospace(RichText::new(&pairing.verification_code).size(26.0));
                    ui.label(
                        RichText::new("验证码仅用于本次面对面核对，请勿写入日志。").color(MUTED),
                    );
                } else {
                    ui.label("给即将加入的设备起一个名称：");
                    ui.text_edit_singleline(&mut self.pair_label);
                    let valid = !self.pair_label.trim().is_empty()
                        && self.pair_label.trim() == self.pair_label
                        && !self.busy();
                    if ui
                        .add_enabled(valid, primary_button("创建配对房间"))
                        .clicked()
                    {
                        self.start_agent_operation(
                            Operation::Pair,
                            AgentRequest::Pair {
                                label: self.pair_label.clone(),
                            },
                        );
                    }
                }
            });
        self.show_pair = open;
    }

    fn render_join_window(&mut self, context: &egui::Context) {
        if !self.show_join {
            return;
        }
        let mut open = self.show_join;
        egui::Window::new("加入配对")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(500.0)
            .show(context, |ui| {
                ui.label("设备名称");
                ui.text_edit_singleline(&mut self.join_label);
                ui.label("房间码或完整邀请");
                ui.text_edit_singleline(&mut self.join_invitation);
                ui.label("六位验证码");
                ui.add(
                    egui::TextEdit::singleline(&mut self.join_verification)
                        .char_limit(6)
                        .password(true),
                );
                let valid = !self.join_label.trim().is_empty()
                    && self.join_label.trim() == self.join_label
                    && !self.join_invitation.is_empty()
                    && self.join_invitation.trim() == self.join_invitation
                    && self.join_verification.len() == 6
                    && self
                        .join_verification
                        .bytes()
                        .all(|byte| byte.is_ascii_digit())
                    && !self.busy();
                if ui
                    .add_enabled(valid, primary_button("验证并加入"))
                    .clicked()
                {
                    self.start_agent_operation(
                        Operation::JoinPairing,
                        join_request(
                            self.join_label.clone(),
                            self.join_invitation.clone(),
                            self.join_verification.clone(),
                        ),
                    );
                }
            });
        self.show_join = open;
    }

    fn render_revoke_window(&mut self, context: &egui::Context) {
        let Some(device_id) = self.revoke_device.clone() else {
            return;
        };
        let mut open = true;
        egui::Window::new("忘记设备？")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(context, |ui| {
                ui.label(
                    "这会撤销双方关系，并删除此设备对应的本地连接信息。已接收文件不会被删除。",
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.busy(), danger_button("确认忘记"))
                        .clicked()
                    {
                        self.start_agent_operation(
                            Operation::RevokeDevice,
                            AgentRequest::RevokeDevice {
                                device: device_id.clone(),
                            },
                        );
                    }
                    if ui.add(quiet_button("取消")).clicked() {
                        self.revoke_device = None;
                    }
                });
            });
        if !open {
            self.revoke_device = None;
        }
    }

    fn render_notices(&mut self, root: &mut egui::Ui) {
        let now = Instant::now();
        if self
            .toast
            .as_ref()
            .is_some_and(|(_, created)| now.duration_since(*created) >= TOAST_DURATION)
        {
            self.toast = None;
        }
        let message = self
            .error
            .as_ref()
            .map(|message| (message.as_str(), DANGER))
            .or_else(|| {
                self.toast
                    .as_ref()
                    .map(|(message, _)| (message.as_str(), SUCCESS))
            });
        if let Some((message, color)) = message {
            egui::Panel::bottom("notice")
                .frame(
                    egui::Frame::new()
                        .fill(SURFACE)
                        .inner_margin(egui::Margin::symmetric(22, 10)),
                )
                .show(root, |ui| {
                    ui.colored_label(color, message);
                });
        }
    }
}

impl eframe::App for EnvoixWindowsApp {
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_screenshot_result(context);
        self.drain_events();
        self.maybe_request_screenshot(context);
        if self.last_refresh.elapsed() >= REFRESH_INTERVAL {
            self.request_refresh();
        }
        context.request_repaint_after(Duration::from_millis(250));
    }

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = root.ctx().clone();
        self.handle_screenshot_result(&context);
        self.render_header(root);
        self.render_sidebar(root);
        self.render_notices(root);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND).inner_margin(24.0))
            .show(root, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if self.dashboard.is_none() && self.page != Page::Settings {
                        self.render_unavailable(ui);
                        return;
                    }
                    match self.page {
                        Page::Devices => self.render_devices(ui),
                        Page::Activity => self.render_activity(ui),
                        Page::Inbox => self.render_inbox(ui),
                        Page::Settings => self.render_settings(ui),
                    }
                });
            });
        self.render_pair_window(&context);
        self.render_join_window(&context);
        self.render_revoke_window(&context);
        self.maybe_request_screenshot(&context);
    }
}

fn configure_style(context: &egui::Context) {
    configure_system_font(context);
    context.set_theme(egui::ThemePreference::Light);
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let mut style = (*context.style_of(theme)).clone();
        style.spacing.item_spacing = egui::vec2(10.0, 9.0);
        style.spacing.button_padding = egui::vec2(15.0, 8.0);
        style.spacing.interact_size.y = 38.0;
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(14.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(13.0, egui::FontFamily::Proportional),
        );
        style.visuals.panel_fill = BACKGROUND;
        style.visuals.window_fill = SURFACE;
        style.visuals.override_text_color = Some(TEXT);
        style.visuals.selection.bg_fill = ACCENT;
        style.visuals.selection.stroke.color = Color32::WHITE;
        style.visuals.widgets.inactive.bg_fill = SURFACE;
        style.visuals.widgets.inactive.weak_bg_fill = SURFACE;
        style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, BORDER);
        style.visuals.widgets.hovered.bg_fill = ACCENT_SOFT;
        style.visuals.widgets.hovered.weak_bg_fill = ACCENT_SOFT;
        style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT);
        style.visuals.widgets.active.bg_fill = ACCENT_SOFT;
        style.visuals.widgets.active.weak_bg_fill = ACCENT_SOFT;
        style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT_DARK);
        style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(9);
        style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(9);
        style.visuals.widgets.active.corner_radius = egui::CornerRadius::same(9);
        style.visuals.window_corner_radius = egui::CornerRadius::same(18);
        style.visuals.window_shadow = egui::epaint::Shadow {
            offset: [0, 6],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(32),
        };
        context.set_style_of(theme, style);
    }
}

fn configure_system_font(context: &egui::Context) {
    const PRIMARY_FONT_NAME: &str = "envoix-windows-primary";
    const CJK_FONT_NAME: &str = "envoix-windows-cjk";
    const PRIMARY_FONT: &str = "segoeui.ttf";
    const WINDOWS_FONT_CANDIDATES: [&str; 2] = ["msyh.ttc", "simsun.ttc"];

    let windows_directory = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let fonts_directory = windows_directory.join("Fonts");
    let Ok(primary_font_data) = std::fs::read(fonts_directory.join(PRIMARY_FONT)) else {
        return;
    };
    let cjk_font_data = WINDOWS_FONT_CANDIDATES
        .iter()
        .map(|name| fonts_directory.join(name))
        .find_map(|path| std::fs::read(path).ok());

    let mut fonts = egui::FontDefinitions::empty();
    fonts.font_data.insert(
        PRIMARY_FONT_NAME.to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(primary_font_data)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        if let Some(names) = fonts.families.get_mut(&family) {
            names.push(PRIMARY_FONT_NAME.to_owned());
        }
    }
    if let Some(cjk_font_data) = cjk_font_data {
        fonts.font_data.insert(
            CJK_FONT_NAME.to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(cjk_font_data)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            if let Some(names) = fonts.families.get_mut(&family) {
                names.push(CJK_FONT_NAME.to_owned());
            }
        }
    }
    context.set_fonts(fonts);
}

fn nav_button(ui: &mut egui::Ui, page: &mut Page, target: Page, icon: &str, label: &str) {
    let selected = *page == target;
    let response = ui.add_sized(
        [ui.available_width(), 44.0],
        egui::Button::new(
            RichText::new(format!("{icon}    {label}"))
                .size(13.5)
                .strong()
                .color(if selected { ACCENT_DARK } else { TEXT }),
        )
        .fill(if selected {
            ACCENT_SOFT
        } else {
            Color32::TRANSPARENT
        })
        .stroke(egui::Stroke::NONE)
        .corner_radius(12),
    );
    if response.clicked() {
        *page = target;
    }
}

fn card(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(CARD_RADIUS)
        .inner_margin(CARD_PADDING)
        .shadow(egui::epaint::Shadow {
            offset: [0, 2],
            blur: 10,
            spread: 0,
            color: Color32::from_black_alpha(10),
        })
        .show(ui, content);
}

fn render_live_transfer_card(
    ui: &mut egui::Ui,
    telemetry: &AgentTransferTelemetry,
    device_label: &str,
    path: Option<AgentPathKind>,
) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            let direction = direction_text(telemetry.direction);
            transfer_icon(ui, direction, ACCENT);
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(format!("{} · {}", direction, device_label))
                        .size(14.0)
                        .strong()
                        .color(TEXT),
                );
                ui.label(
                    RichText::new(transfer_content_summary(
                        Some(telemetry),
                        telemetry.transferred_bytes,
                        telemetry.total_bytes,
                    ))
                    .size(11.5)
                    .color(MUTED),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                state_pill(ui, transfer_phase_text(telemetry.phase), ACCENT);
            });
        });
        let progress = if telemetry.total_bytes == 0 {
            0.0
        } else {
            telemetry.transferred_bytes as f32 / telemetry.total_bytes as f32
        };
        ui.add_space(8.0);
        ui.add(
            egui::ProgressBar::new(progress.clamp(0.0, 1.0))
                .fill(ACCENT)
                .desired_height(9.0)
                .corner_radius(8),
        );
        render_transfer_metrics(ui, telemetry);
        if let Some(path) = path {
            ui.label(RichText::new(format!("连接方式：{}", transfer_path_text(path))).color(MUTED));
        }
    });
}

fn transfer_content_summary(
    telemetry: Option<&AgentTransferTelemetry>,
    transferred_bytes: u64,
    total_bytes: u64,
) -> String {
    let progress = format!(
        "{} / {}",
        human_bytes(transferred_bytes),
        human_bytes(total_bytes)
    );
    let names = telemetry
        .map(|value| value.root_names.join("、"))
        .unwrap_or_default();
    if names.is_empty() {
        progress
    } else {
        format!("{names} · {progress}")
    }
}

fn render_transfer_metrics(ui: &mut egui::Ui, telemetry: &AgentTransferTelemetry) {
    let mut metrics = Vec::new();
    if telemetry.current_bytes_per_second > 0 {
        metrics.push(format!(
            "当前 {}",
            transfer_rate(telemetry.current_bytes_per_second)
        ));
    }
    if telemetry.average_bytes_per_second > 0 {
        metrics.push(format!(
            "平均 {}",
            transfer_rate(telemetry.average_bytes_per_second)
        ));
    }
    if let Some(seconds) = telemetry.eta_seconds {
        metrics.push(transfer_eta(seconds));
    }
    if !metrics.is_empty() {
        ui.label(RichText::new(metrics.join(" · ")).size(11.5).color(MUTED));
    }
}

fn room_card(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(SURFACE_RAISED)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(24)
        .inner_margin(26.0)
        .shadow(egui::epaint::Shadow {
            offset: [0, 5],
            blur: 20,
            spread: 0,
            color: Color32::from_black_alpha(18),
        })
        .show(ui, content);
}

fn key_value(ui: &mut egui::Ui, key: &str, value: &str) {
    egui::Frame::new()
        .fill(BACKGROUND)
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(key).size(11.0).color(MUTED).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(value).size(11.0).color(TEXT));
                });
            });
        });
}

fn page_heading(page: Page) -> (&'static str, &'static str) {
    match page {
        Page::Devices => ("你的设备", "选择一个私人房间，发送文件或文件夹"),
        Page::Activity => ("传输活动", "清楚区分排队、传输和真正送达"),
        Page::Inbox => ("收件箱", "查看已经校验并保存到此电脑的内容"),
        Page::Settings => ("设置", "偏好、后台传输与存储位置"),
    }
}

fn primary_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(
        RichText::new(label)
            .size(13.0)
            .strong()
            .color(Color32::WHITE),
    )
    .fill(ACCENT)
    .stroke(egui::Stroke::NONE)
    .corner_radius(11)
    .min_size(egui::vec2(132.0, 42.0))
}

fn quiet_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).size(12.5).strong().color(TEXT))
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(11)
        .min_size(egui::vec2(104.0, 40.0))
}

fn danger_quiet_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(label).size(12.0).strong().color(DANGER))
        .fill(Color32::TRANSPARENT)
        .stroke(egui::Stroke::NONE)
        .corner_radius(9)
}

fn danger_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(
        RichText::new(label)
            .size(12.5)
            .strong()
            .color(Color32::WHITE),
    )
    .fill(DANGER)
    .stroke(egui::Stroke::NONE)
    .corner_radius(11)
    .min_size(egui::vec2(110.0, 40.0))
}

fn brand_mark(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(42.0, 42.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 13.0, ACCENT);
    ui.painter()
        .circle_stroke(rect.center(), 10.0, egui::Stroke::new(2.2, Color32::WHITE));
    ui.painter()
        .circle_filled(rect.center() + egui::vec2(7.0, -7.0), 3.5, Color32::WHITE);
}

fn status_pill(ui: &mut egui::Ui, text: &str, color: Color32, fill: Color32) {
    egui::Frame::new()
        .fill(fill)
        .corner_radius(20)
        .inner_margin(egui::Margin::symmetric(12, 7))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.5).strong().color(color));
        });
}

fn state_pill(ui: &mut egui::Ui, text: &str, color: Color32) {
    let fill = if color == SUCCESS {
        SUCCESS_SOFT
    } else if color == DANGER {
        DANGER_SOFT
    } else if color == WARNING {
        WARNING_SOFT
    } else {
        ACCENT_SOFT
    };
    status_pill(ui, text, color, fill);
}

fn device_avatar(ui: &mut egui::Ui, label: &str, size: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), size / 2.0, color);
    let initial = label.chars().next().unwrap_or('E').to_string();
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        initial,
        egui::FontId::proportional(size * 0.38),
        Color32::WHITE,
    );
}

fn device_button(
    ui: &mut egui::Ui,
    label: &str,
    generation: u64,
    selected: bool,
) -> egui::Response {
    ui.add(
        egui::Button::new(
            RichText::new(format!("●   {label}\n     已验证 · 第 {generation} 代"))
                .size(12.5)
                .strong()
                .color(if selected { ACCENT_DARK } else { TEXT }),
        )
        .fill(if selected { ACCENT_SOFT } else { SURFACE })
        .stroke(egui::Stroke::new(
            if selected { 1.5 } else { 1.0 },
            if selected { ACCENT } else { BORDER },
        ))
        .corner_radius(16)
        .min_size(egui::vec2(218.0, 66.0)),
    )
}

fn metric_card(ui: &mut egui::Ui, label: &str, value: usize, color: Color32, detail: &str) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("●").size(11.0).color(color));
            ui.label(RichText::new(label).size(12.0).strong().color(MUTED));
        });
        ui.label(
            RichText::new(value.to_string())
                .size(30.0)
                .strong()
                .color(TEXT),
        );
        ui.label(RichText::new(detail).size(10.5).color(MUTED));
    });
}

fn transfer_icon(ui: &mut egui::Ui, direction: &str, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(38.0, 38.0), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), 19.0, color.gamma_multiply(0.12));
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        if direction.contains("发送") {
            "↗"
        } else {
            "↓"
        },
        egui::FontId::proportional(18.0),
        color,
    );
}

fn empty_state(ui: &mut egui::Ui, icon: &str, title: &str, content: impl FnOnce(&mut egui::Ui)) {
    card(ui, |ui| {
        ui.set_min_height(220.0);
        ui.vertical_centered(|ui| {
            ui.add_space(32.0);
            ui.label(RichText::new(icon).size(34.0).strong().color(ACCENT));
            ui.label(RichText::new(title).size(18.0).strong().color(TEXT));
            content(ui);
        });
    });
}

fn security_line(ui: &mut egui::Ui, icon: &str, text: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon).strong().color(SUCCESS));
        ui.label(RichText::new(text).size(12.0).color(TEXT));
    });
}

fn path_display_name(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

fn write_bmp(path: &std::path::Path, image: &egui::ColorImage) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};

    let width = u32::try_from(image.width())
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "screenshot width is too large"))?;
    let height = u32::try_from(image.height())
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "screenshot height is too large"))?;
    let signed_width = i32::try_from(width)
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "screenshot width is too large"))?;
    let signed_height = i32::try_from(height)
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "screenshot height is too large"))?;
    let pixel_bytes = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(BMP_BYTES_PER_PIXEL))
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "screenshot is too large"))?;
    let file_size = BMP_PIXEL_OFFSET
        .checked_add(pixel_bytes)
        .ok_or_else(|| Error::new(ErrorKind::InvalidInput, "screenshot is too large"))?;

    let mut output = Vec::with_capacity(file_size as usize);
    output.extend_from_slice(b"BM");
    output.extend_from_slice(&file_size.to_le_bytes());
    output.extend_from_slice(&[0; 4]);
    output.extend_from_slice(&BMP_PIXEL_OFFSET.to_le_bytes());
    output.extend_from_slice(&BMP_DIB_HEADER_BYTES.to_le_bytes());
    output.extend_from_slice(&signed_width.to_le_bytes());
    output.extend_from_slice(&signed_height.to_le_bytes());
    output.extend_from_slice(&BMP_PLANES.to_le_bytes());
    output.extend_from_slice(&BMP_BITS_PER_PIXEL.to_le_bytes());
    output.extend_from_slice(&0_u32.to_le_bytes());
    output.extend_from_slice(&pixel_bytes.to_le_bytes());
    output.extend_from_slice(&BMP_PIXELS_PER_METER.to_le_bytes());
    output.extend_from_slice(&BMP_PIXELS_PER_METER.to_le_bytes());
    output.extend_from_slice(&0_u32.to_le_bytes());
    output.extend_from_slice(&0_u32.to_le_bytes());

    for row in image.pixels.chunks_exact(image.width()).rev() {
        for pixel in row {
            let [red, green, blue, alpha] = pixel.to_array();
            output.extend_from_slice(&[blue, green, red, alpha]);
        }
    }
    std::fs::write(path, output)
}

fn append_unique_paths(target: &mut Vec<PathBuf>, paths: Vec<PathBuf>) {
    for path in paths {
        if !target.contains(&path) {
            target.push(path);
        }
    }
}

fn open_in_explorer(path: PathBuf) {
    let argument = if path.is_file() {
        format!("/select,{}", path.display())
    } else {
        path.display().to_string()
    };
    let _ = std::process::Command::new("explorer.exe")
        .arg(argument)
        .spawn();
}
