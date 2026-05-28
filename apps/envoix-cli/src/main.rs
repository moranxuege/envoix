use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use envoix_client::{
    ClientConfig, EnvoixClient, EventSink, ReceiveFileRequest, SendFileRequest, TransferDirection,
    TransferEvent, TransportProtocol,
};

#[derive(Debug, Parser)]
#[command(name = "envoix", version, about = "Secure file transfer CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Send {
        #[arg(long)]
        peer: SocketAddr,
        #[arg(long, value_enum, default_value = "quic")]
        protocol: ProtocolArg,
        file: PathBuf,
    },
    Receive {
        #[arg(long)]
        listen: SocketAddr,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, value_enum, default_value = "quic")]
        protocol: ProtocolArg,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum ProtocolArg {
    Quic,
    Tcp,
}

impl From<ProtocolArg> for TransportProtocol {
    fn from(value: ProtocolArg) -> Self {
        match value {
            ProtocolArg::Quic => Self::Quic,
            ProtocolArg::Tcp => Self::Tcp,
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<(), envoix_client::PublicError> {
    match cli.command {
        Command::Send {
            peer,
            protocol,
            file,
        } => {
            let client = client_for_protocol(protocol);
            let summary = client
                .send_file(
                    SendFileRequest {
                        peer_addr: peer,
                        file_path: file,
                    },
                    Box::new(ConsoleEventSink),
                )
                .await?;
            eprintln!(
                "sent {} bytes from {}",
                summary.bytes_transferred, summary.file_name
            );
        }
        Command::Receive {
            listen,
            output,
            protocol,
        } => {
            let client = client_for_protocol(protocol);
            let summary = client
                .receive_file(
                    ReceiveFileRequest {
                        listen_addr: listen,
                        output_dir: output,
                    },
                    Box::new(ConsoleEventSink),
                )
                .await?;
            eprintln!(
                "received {} bytes into {}",
                summary.bytes_transferred, summary.file_name
            );
        }
    }

    Ok(())
}

fn client_for_protocol(protocol: ProtocolArg) -> EnvoixClient {
    EnvoixClient::new(ClientConfig {
        protocol: protocol.into(),
        ..ClientConfig::default()
    })
}

#[derive(Clone, Copy, Debug)]
struct ConsoleEventSink;

impl EventSink for ConsoleEventSink {
    fn on_event(&self, event: TransferEvent) {
        match event {
            TransferEvent::Started {
                direction,
                file_name,
                total_bytes,
                ..
            } => {
                let verb = match direction {
                    TransferDirection::Send => "sending",
                    TransferDirection::Receive => "receiving",
                };
                eprintln!("{verb} {file_name} ({total_bytes} bytes)");
            }
            TransferEvent::Progress {
                bytes_transferred,
                total_bytes,
                ..
            } => {
                eprintln!("progress {bytes_transferred}/{total_bytes} bytes");
            }
            TransferEvent::Completed {
                bytes_transferred, ..
            } => {
                eprintln!("completed {bytes_transferred} bytes");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_send_command() {
        let cli =
            Cli::try_parse_from(["envoix", "send", "--peer", "[::1]:9000", "hello.txt"]).unwrap();

        assert!(matches!(
            cli.command,
            Command::Send {
                peer,
                protocol: ProtocolArg::Quic,
                file
            } if peer == "[::1]:9000".parse().unwrap() && file == std::path::Path::new("hello.txt")
        ));
    }

    #[test]
    fn parses_receive_command() {
        let cli = Cli::try_parse_from([
            "envoix",
            "receive",
            "--listen",
            "[::1]:9000",
            "--output",
            "received",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Command::Receive {
                listen,
                output,
                protocol: ProtocolArg::Quic
            } if listen == "[::1]:9000".parse().unwrap() && output == std::path::Path::new("received")
        ));
    }

    #[test]
    fn parses_tcp_protocol() {
        let send = Cli::try_parse_from([
            "envoix",
            "send",
            "--peer",
            "[::1]:9000",
            "--protocol",
            "tcp",
            "hello.txt",
        ])
        .unwrap();
        let receive = Cli::try_parse_from([
            "envoix",
            "receive",
            "--listen",
            "[::1]:9000",
            "--output",
            "received",
            "--protocol",
            "tcp",
        ])
        .unwrap();

        assert!(matches!(
            send.command,
            Command::Send {
                protocol: ProtocolArg::Tcp,
                ..
            }
        ));
        assert!(matches!(
            receive.command,
            Command::Receive {
                protocol: ProtocolArg::Tcp,
                ..
            }
        ));
    }
}
