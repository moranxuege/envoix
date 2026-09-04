use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, RichText};
use envoix_client::model::TransferState;
use envoix_client::product::{AgentOfferDecision, AgentRequest, AgentResponse, PairingInvitation};

use crate::controller::{
    AgentController, ControllerCommand, ControllerEvent, Dashboard, Operation, join_request,
    offer_decision_request,
};
use crate::presentation::{direction_text, human_bytes, transfer_state_text};

const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const TOAST_DURATION: Duration = Duration::from_secs(4);
const SIDEBAR_WIDTH: f32 = 172.0;
const CARD_RADIUS: u8 = 14;
const CARD_PADDING: i8 = 16;

const BACKGROUND: Color32 = Color32::from_rgb(246, 248, 252);
const SURFACE: Color32 = Color32::from_rgb(255, 255, 255);
const TEXT: Color32 = Color32::from_rgb(17, 24, 39);
const MUTED: Color32 = Color32::from_rgb(91, 103, 125);
const BORDER: Color32 = Color32::from_rgb(225, 231, 240);
const ACCENT: Color32 = Color32::from_rgb(24, 119, 242);
const SUCCESS: Color32 = Color32::from_rgb(18, 137, 83);
const WARNING: Color32 = Color32::from_rgb(190, 116, 17);
const DANGER: Color32 = Color32::from_rgb(211, 55, 55);

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
}

impl EnvoixWindowsApp {
    pub(crate) fn new(context: &egui::Context) -> Self {
        configure_style(context);
        let controller = AgentController::start(context.clone());
        let mut app = Self {
            controller,
            page: Page::Devices,
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
        };
        app.request_refresh();
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
            Some(AgentResponse::DeviceRevoked { device })
                if operation == Operation::RevokeDevice =>
            {
                self.selected_device = None;
                self.revoke_device = None;
                self.show_toast(&format!("已忘记设备 {}", device.label));
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
                    Operation::InstallAgent => "Agent 已安装并启动",
                    Operation::StartAgent => "Agent 已启动",
                    Operation::RestartAgent => "Agent 已重新启动",
                    _ => unreachable!(),
                });
            }
            _ => self.error = Some("Agent 返回了与当前操作不匹配的响应".to_owned()),
        }
    }

    fn show_toast(&mut self, message: &str) {
        self.error = None;
        self.toast = Some((message.to_owned(), Instant::now()));
    }

    fn busy(&self) -> bool {
        self.active_operation.is_some()
    }

    fn render_header(&mut self, root: &mut egui::Ui) {
        egui::Panel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(SURFACE)
                    .inner_margin(egui::Margin::symmetric(22, 14))
                    .stroke(egui::Stroke::new(1.0, BORDER)),
            )
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Envoix").size(24.0).strong().color(TEXT));
                    ui.add_space(12.0);
                    match &self.dashboard {
                        Some(dashboard) => {
                            ui.colored_label(SUCCESS, "● Agent 在线");
                            ui.label(
                                RichText::new(format!(
                                    "{} · {} 个已配对设备",
                                    dashboard.status.device_name,
                                    dashboard.devices.len()
                                ))
                                .color(MUTED),
                            );
                        }
                        None => {
                            ui.colored_label(DANGER, "● Agent 未连接");
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !self.refresh_pending && !self.busy(),
                                egui::Button::new("刷新"),
                            )
                            .clicked()
                        {
                            self.request_refresh();
                        }
                        if self.busy() {
                            ui.spinner();
                            ui.label(RichText::new("正在处理…").color(MUTED));
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
                    .fill(Color32::from_rgb(238, 243, 250))
                    .inner_margin(14.0),
            )
            .show(root, |ui| {
                ui.add_space(8.0);
                nav_button(ui, &mut self.page, Page::Devices, "已配对设备");
                nav_button(ui, &mut self.page, Page::Activity, "传输活动");
                nav_button(ui, &mut self.page, Page::Inbox, "收件箱");
                nav_button(ui, &mut self.page, Page::Settings, "设置与诊断");
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.label(RichText::new("v0.3 · protocol 12").small().color(MUTED));
                });
            });
    }

    fn render_unavailable(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.heading("后台 Agent 尚未连接");
            ui.label(
                "图形界面不直接持有设备凭据。安装并启动当前用户的 Agent 后，设备、房间和传输会显示在这里。",
            );
            if let Some(error) = &self.error {
                ui.colored_label(DANGER, error);
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.busy(), egui::Button::new("安装并启动 Agent"))
                    .clicked()
                {
                    self.start_lifecycle(Operation::InstallAgent);
                }
                if ui
                    .add_enabled(!self.busy(), egui::Button::new("启动已有 Agent"))
                    .clicked()
                {
                    self.start_lifecycle(Operation::StartAgent);
                }
            });
        });
    }

    fn render_devices(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("已配对设备");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("加入配对").clicked() {
                    self.show_join = true;
                }
                if ui.button("添加设备").clicked() {
                    self.show_pair = true;
                    self.pairing = None;
                }
            });
        });
        ui.label(
            RichText::new("选择一台设备进入房间并发送文件。Agent 会在窗口关闭后继续处理队列。")
                .color(MUTED),
        );
        ui.add_space(12.0);

        let devices = self
            .dashboard
            .as_ref()
            .map(|dashboard| dashboard.devices.clone())
            .unwrap_or_default();
        if devices.is_empty() {
            card(ui, |ui| {
                ui.strong("还没有已验证的设备");
                ui.label("点击“添加设备”创建房间，或使用另一台设备给出的房间码加入配对。");
            });
            return;
        }

        ui.horizontal_wrapped(|ui| {
            for device in &devices {
                let selected = self.selected_device.as_deref() == Some(device.id.as_str());
                let button = egui::Button::new(
                    RichText::new(format!(
                        "{}\n已验证 · 第 {} 代",
                        device.label, device.generation
                    ))
                    .color(if selected { Color32::WHITE } else { TEXT }),
                )
                .fill(if selected { ACCENT } else { SURFACE })
                .stroke(egui::Stroke::new(
                    1.0,
                    if selected { ACCENT } else { BORDER },
                ))
                .corner_radius(CARD_RADIUS)
                .min_size(egui::vec2(210.0, 62.0));
                if ui.add(button).clicked() {
                    self.selected_device = Some(device.id.clone());
                }
            }
        });
        ui.add_space(14.0);

        let Some(device) = devices
            .iter()
            .find(|device| Some(device.id.as_str()) == self.selected_device.as_deref())
            .cloned()
        else {
            return;
        };
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.heading(&device.label);
                    ui.colored_label(SUCCESS, "● 房间就绪 · 对方上线后文件会保留在队列中");
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("忘记设备").clicked() {
                        self.revoke_device = Some(device.id.clone());
                    }
                });
            });
            ui.separator();
            ui.strong("发送文件");
            ui.horizontal(|ui| {
                if ui.button("选择文件").clicked()
                    && let Some(paths) = rfd::FileDialog::new().pick_files()
                {
                    append_unique_paths(&mut self.selected_paths, paths);
                }
                if ui.button("选择文件夹").clicked()
                    && let Some(path) = rfd::FileDialog::new().pick_folder()
                {
                    append_unique_paths(&mut self.selected_paths, vec![path]);
                }
                if !self.selected_paths.is_empty() && ui.button("清空").clicked() {
                    self.selected_paths.clear();
                }
            });
            if self.selected_paths.is_empty() {
                ui.label(RichText::new("尚未选择内容").color(MUTED));
            } else {
                egui::ScrollArea::vertical()
                    .max_height(110.0)
                    .show(ui, |ui| {
                        for path in &self.selected_paths {
                            ui.label(path.display().to_string());
                        }
                    });
            }
            let can_send = !self.selected_paths.is_empty() && !self.busy();
            if ui
                .add_enabled(can_send, egui::Button::new("发送到此设备").fill(ACCENT))
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
        ui.add_space(16.0);
        ui.heading("等待你确认的接收");
        for offer in offers {
            card(ui, |ui| {
                ui.strong(format!("来自 {}", offer.from_device_label));
                ui.label(format!(
                    "{} 个项目 · {} 个文件夹 · {}",
                    offer.item_count,
                    offer.directory_count,
                    human_bytes(offer.total_bytes)
                ));
                ui.label(offer.root_names.join("、"));
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.busy(), egui::Button::new("允许接收").fill(ACCENT))
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
                        .add_enabled(!self.busy(), egui::Button::new("拒绝"))
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
        }
    }

    fn render_activity(&mut self, ui: &mut egui::Ui) {
        ui.heading("传输活动");
        ui.label(
            RichText::new("“已送达”表示接收方已经保存并返回确认，不只是本机发送完成。")
                .color(MUTED),
        );
        ui.add_space(12.0);
        let Some(dashboard) = &self.dashboard else {
            return;
        };
        if dashboard.transfers.is_empty() {
            card(ui, |ui| {
                ui.label("暂无传输记录");
            });
            return;
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
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(format!(
                        "{} · {}",
                        direction_text(transfer.direction),
                        device
                    ));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.colored_label(state_color, transfer_state_text(transfer.state));
                    });
                });
                let progress = if transfer.total_bytes == 0 {
                    0.0
                } else {
                    transfer.transferred_bytes as f32 / transfer.total_bytes as f32
                };
                ui.add(
                    egui::ProgressBar::new(progress.clamp(0.0, 1.0)).text(format!(
                        "{} / {}",
                        human_bytes(transfer.transferred_bytes),
                        human_bytes(transfer.total_bytes)
                    )),
                );
                if let Some(path) = dashboard
                    .active_paths
                    .iter()
                    .find(|path| path.transfer_id == transfer.id.to_string())
                {
                    ui.label(RichText::new(format!("当前路径：{:?}", path.path)).color(MUTED));
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
                ui.label(RichText::new(transfer.id.to_string()).small().color(MUTED));
            });
        }
    }

    fn render_inbox(&mut self, ui: &mut egui::Ui) {
        ui.heading("收件箱");
        ui.label(RichText::new("这里只显示已经完整校验并保存到磁盘的内容。").color(MUTED));
        ui.add_space(12.0);
        let items = self
            .dashboard
            .as_ref()
            .map(|dashboard| dashboard.inbox.clone())
            .unwrap_or_default();
        if items.is_empty() {
            card(ui, |ui| {
                ui.label("尚未收到文件");
            });
            return;
        }
        for item in items {
            card(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(format!("来自 {}", item.from_device_label));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.colored_label(SUCCESS, "已保存");
                    });
                });
                ui.label(format!(
                    "{} 个文件 · {} 个文件夹 · {}",
                    item.file_count,
                    item.directory_count,
                    human_bytes(item.total_bytes)
                ));
                for root in &item.roots {
                    ui.horizontal(|ui| {
                        ui.label(&root.name);
                        if ui.button("在资源管理器中显示").clicked() {
                            open_in_explorer(PathBuf::from(&root.path));
                        }
                    });
                }
            });
        }
    }

    fn render_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("设置与诊断");
        ui.label(
            RichText::new("图形界面只使用本机控制接口，不读取 Agent 的凭据内容。").color(MUTED),
        );
        ui.add_space(12.0);
        let Some(dashboard) = &self.dashboard else {
            self.render_unavailable(ui);
            return;
        };
        let status = dashboard.status.clone();
        let diagnostics = dashboard.diagnostics.clone();
        card(ui, |ui| {
            ui.heading("Agent");
            key_value(ui, "设备名称", &status.device_name);
            key_value(ui, "协议", &status.protocol_version.to_string());
            key_value(
                ui,
                "控制通道",
                &format!("{:?}", diagnostics.control_transport),
            );
            key_value(
                ui,
                "凭据保护",
                &format!("{:?}", diagnostics.credential_protection),
            );
            key_value(
                ui,
                "Engine schema",
                &diagnostics.engine_schema_version.to_string(),
            );
            key_value(ui, "Broker", &status.broker);
            key_value(ui, "Relay", status.relay.as_deref().unwrap_or("未启用"));
            key_value(ui, "Inbox", &status.inbox_directory);
            ui.add_space(8.0);
            if ui
                .add_enabled(!self.busy(), egui::Button::new("重新启动 Agent"))
                .clicked()
            {
                self.start_lifecycle(Operation::RestartAgent);
            }
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
                        .add_enabled(valid, egui::Button::new("创建配对房间").fill(ACCENT))
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
                    .add_enabled(valid, egui::Button::new("验证并加入").fill(ACCENT))
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
                ui.label("这会撤销双方关系，并删除此设备对应的本地凭据。已接收文件不会被删除。");
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.busy(), egui::Button::new("确认忘记").fill(DANGER))
                        .clicked()
                    {
                        self.start_agent_operation(
                            Operation::RevokeDevice,
                            AgentRequest::RevokeDevice {
                                device: device_id.clone(),
                            },
                        );
                    }
                    if ui.button("取消").clicked() {
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
        self.drain_events();
        if self.last_refresh.elapsed() >= REFRESH_INTERVAL {
            self.request_refresh();
        }
        context.request_repaint_after(Duration::from_millis(250));
    }

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = root.ctx().clone();
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
    }
}

fn configure_style(context: &egui::Context) {
    configure_system_font(context);
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        let mut style = (*context.style_of(theme)).clone();
        style.spacing.item_spacing = egui::vec2(10.0, 10.0);
        style.visuals.panel_fill = BACKGROUND;
        style.visuals.window_fill = SURFACE;
        style.visuals.override_text_color = Some(TEXT);
        style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(9);
        style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(9);
        style.visuals.widgets.active.corner_radius = egui::CornerRadius::same(9);
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

fn nav_button(ui: &mut egui::Ui, page: &mut Page, target: Page, label: &str) {
    let selected = *page == target;
    let response = ui.add_sized(
        [ui.available_width(), 40.0],
        egui::Button::new(RichText::new(label).color(if selected { Color32::WHITE } else { TEXT }))
            .fill(if selected {
                ACCENT
            } else {
                Color32::TRANSPARENT
            })
            .corner_radius(10),
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
        .show(ui, content);
}

fn key_value(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(key).color(MUTED).strong());
        ui.label(value);
    });
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
