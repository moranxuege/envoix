use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use envoix_client::agent_control::AgentControlClient;
use envoix_client::model::Transfer;
use envoix_client::product::{
    AgentDiagnostics, AgentOfferDecision, AgentPairingInput, AgentPendingOffer, AgentRequest,
    AgentResponse, AgentStatus, AgentTransferPath, DeviceSummary, InboxItem,
};

const WINDOWS_CLI_NAMES: [&str; 2] = ["envoix.exe", "envoix-cli-windows-x86_64.exe"];
const WINDOWS_AGENT_NAMES: [&str; 2] = ["envoix-agent.exe", "envoix-agent-windows-x86_64.exe"];
const WINDOWS_NO_CONSOLE: u32 = 0x0800_0000;
const DEFAULT_WINDOWS_DEVICE_NAME: &str = "Envoix Windows";

#[derive(Clone, Debug)]
pub(crate) struct Dashboard {
    pub status: AgentStatus,
    pub diagnostics: AgentDiagnostics,
    pub devices: Vec<DeviceSummary>,
    pub transfers: Vec<Transfer>,
    pub active_paths: Vec<AgentTransferPath>,
    pub pending_offers: Vec<AgentPendingOffer>,
    pub inbox: Vec<InboxItem>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Operation {
    Pair,
    JoinPairing,
    CreateTransfer,
    ApproveOffer,
    RejectOffer,
    RevokeDevice,
    InstallAgent,
    StartAgent,
    RestartAgent,
}

pub(crate) enum ControllerCommand {
    Refresh,
    Agent {
        operation: Operation,
        request: AgentRequest,
    },
    Lifecycle(Operation),
}

pub(crate) enum ControllerEvent {
    Dashboard(Result<Dashboard, String>),
    Operation {
        operation: Operation,
        result: Result<Option<AgentResponse>, String>,
    },
}

pub(crate) struct AgentController {
    commands: Sender<ControllerCommand>,
    events: Receiver<ControllerEvent>,
}

impl AgentController {
    pub fn start(context: egui::Context) -> Self {
        let (command_sender, command_receiver) = channel();
        let (event_sender, event_receiver) = channel();
        thread::Builder::new()
            .name("envoix-windows-agent-control".to_owned())
            .spawn(move || run_worker(command_receiver, event_sender, context))
            .expect("start Envoix Agent control worker");
        Self {
            commands: command_sender,
            events: event_receiver,
        }
    }

    pub fn send(&self, command: ControllerCommand) -> Result<(), String> {
        self.commands
            .send(command)
            .map_err(|_| "Agent 控制线程已经停止".to_owned())
    }

    pub fn try_event(&self) -> Option<ControllerEvent> {
        self.events.try_recv().ok()
    }
}

fn run_worker(
    commands: Receiver<ControllerCommand>,
    events: Sender<ControllerEvent>,
    context: egui::Context,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = events.send(ControllerEvent::Dashboard(Err(format!(
                "无法启动 Agent 控制运行时：{error}"
            ))));
            context.request_repaint();
            return;
        }
    };

    while let Ok(command) = commands.recv() {
        let event = match command {
            ControllerCommand::Refresh => {
                ControllerEvent::Dashboard(runtime.block_on(load_dashboard()))
            }
            ControllerCommand::Agent { operation, request } => ControllerEvent::Operation {
                operation,
                result: runtime.block_on(call_agent(request)).map(Some),
            },
            ControllerCommand::Lifecycle(operation) => ControllerEvent::Operation {
                operation,
                result: run_lifecycle(operation).map(|_| None),
            },
        };
        if events.send(event).is_err() {
            return;
        }
        context.request_repaint();
    }
}

async fn call_agent(request: AgentRequest) -> Result<AgentResponse, String> {
    let client = AgentControlClient::for_current_user()
        .map_err(|error| format!("无法确定 Agent 本机接口：{error}"))?;
    client
        .call(request)
        .await
        .map_err(|error| format!("Agent 请求失败：{error}"))
}

async fn load_dashboard() -> Result<Dashboard, String> {
    let snapshot = match call_agent(AgentRequest::Snapshot { inbox_limit: 50 }).await? {
        AgentResponse::Snapshot { snapshot } => snapshot,
        response => return Err(unexpected_response("snapshot", &response)),
    };
    let devices = match call_agent(AgentRequest::ListDevices).await? {
        AgentResponse::Devices { devices } => devices,
        response => return Err(unexpected_response("devices", &response)),
    };
    let transfers = match call_agent(AgentRequest::ListTransfers).await? {
        AgentResponse::Transfers { transfers } => transfers,
        response => return Err(unexpected_response("transfers", &response)),
    };
    let diagnostics = match call_agent(AgentRequest::Diagnostics).await? {
        AgentResponse::Diagnostics { diagnostics } => diagnostics,
        response => return Err(unexpected_response("diagnostics", &response)),
    };
    Ok(Dashboard {
        status: snapshot.status,
        diagnostics,
        devices,
        transfers,
        active_paths: snapshot.active_paths,
        pending_offers: snapshot.pending_offers,
        inbox: snapshot.inbox,
    })
}

fn unexpected_response(operation: &str, response: &AgentResponse) -> String {
    match response {
        AgentResponse::Error { code, message } => {
            format!("Agent {operation} 失败（{code}）：{message}")
        }
        _ => format!("Agent {operation} 返回了不兼容的响应"),
    }
}

fn run_lifecycle(operation: Operation) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    let executable =
        std::env::current_exe().map_err(|error| format!("无法确定应用目录：{error}"))?;
    let directory = executable
        .parent()
        .ok_or_else(|| "应用路径没有父目录".to_owned())?;
    let cli = find_sibling(directory, &WINDOWS_CLI_NAMES)
        .ok_or_else(|| "应用目录中缺少 envoix.exe".to_owned())?;

    let mut process = ProcessCommand::new(cli);
    process.creation_flags(WINDOWS_NO_CONSOLE);
    process.arg("--json").arg("agent");
    match operation {
        Operation::InstallAgent => {
            let agent = find_sibling(directory, &WINDOWS_AGENT_NAMES)
                .ok_or_else(|| "应用目录中缺少 envoix-agent.exe".to_owned())?;
            process
                .arg("install")
                .arg("--agent-binary")
                .arg(agent)
                .arg("--device-name")
                .arg(DEFAULT_WINDOWS_DEVICE_NAME);
        }
        Operation::StartAgent => {
            process.arg("start");
        }
        Operation::RestartAgent => {
            process.arg("restart");
        }
        _ => return Err("不支持的 Agent 生命周期操作".to_owned()),
    }
    let output = process
        .output()
        .map_err(|error| format!("无法运行 Envoix CLI：{error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    if detail.is_empty() {
        Err(format!("Agent 生命周期命令失败：{}", output.status))
    } else {
        Err(format!("Agent 生命周期命令失败：{detail}"))
    }
}

fn find_sibling(directory: &Path, names: &[&str]) -> Option<PathBuf> {
    names
        .iter()
        .map(|name| directory.join(name))
        .find(|path| path.is_file())
}

pub(crate) fn join_request(
    label: String,
    invitation: String,
    verification_code: String,
) -> AgentRequest {
    AgentRequest::JoinPairing {
        pairing: AgentPairingInput {
            label,
            invitation,
            verification_code,
        },
    }
}

pub(crate) fn offer_decision_request(
    offer_id: String,
    decision: AgentOfferDecision,
) -> AgentRequest {
    AgentRequest::DecidePendingOffer { offer_id, decision }
}
