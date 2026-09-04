#[cfg(windows)]
use envoix_client::model::TransferDirection;
use envoix_client::model::TransferState;
use envoix_client::product::AgentPathKind;

pub(crate) fn transfer_state_text(state: TransferState) -> &'static str {
    match state {
        TransferState::Offered => "等待确认",
        TransferState::Queued => "已排队",
        TransferState::Connecting => "正在连接",
        TransferState::Transferring => "传输中",
        TransferState::Paused => "已暂停",
        TransferState::AwaitingDeliveryProof => "等待接收确认",
        TransferState::Delivered => "已送达",
        TransferState::Rejected => "已拒绝",
        TransferState::Failed => "失败",
        TransferState::Canceled => "已取消",
    }
}

#[cfg(windows)]
pub(crate) fn direction_text(direction: TransferDirection) -> &'static str {
    match direction {
        TransferDirection::Send => "发送",
        TransferDirection::Receive => "接收",
    }
}

pub(crate) fn transfer_path_text(path: AgentPathKind) -> &'static str {
    match path {
        AgentPathKind::Lan => "局域网",
        AgentPathKind::Direct => "直接连接",
        AgentPathKind::Relay => "中继",
        AgentPathKind::WifiAware => "Wi-Fi Aware",
        AgentPathKind::Other => "网络连接",
    }
}

pub(crate) fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_delivery_is_not_presented_as_merely_sent() {
        assert_eq!(transfer_state_text(TransferState::Delivered), "已送达");
        assert_eq!(
            transfer_state_text(TransferState::AwaitingDeliveryProof),
            "等待接收确认"
        );
    }

    #[test]
    fn byte_sizes_use_defined_binary_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1024), "1.0 KB");
        assert_eq!(human_bytes(1_572_864), "1.5 MB");
    }

    #[test]
    fn connection_paths_use_product_language() {
        assert_eq!(transfer_path_text(AgentPathKind::Lan), "局域网");
        assert_eq!(transfer_path_text(AgentPathKind::Direct), "直接连接");
        assert_eq!(transfer_path_text(AgentPathKind::Relay), "中继");
        assert_eq!(transfer_path_text(AgentPathKind::WifiAware), "Wi-Fi Aware");
        assert_eq!(transfer_path_text(AgentPathKind::Other), "网络连接");
    }
}
