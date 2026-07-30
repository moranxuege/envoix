//! `envoix`: a newline-delimited generated-contract frontend.

#![forbid(unsafe_code)]

use std::env;
use std::io::{self, BufRead, Read, Write};
use std::process::ExitCode;

use envoix_bindings::capability::{
    CapabilityBody, CapabilityExchangeView, CapabilityFrame, PickSourceExchangeView,
    PickSourceStepView, ScanInviteExchangeView, ScanInviteStepView, SourceAcquisitionKeyView,
    encode_capability_frame,
};
use envoix_bindings::command::LocalDirectionView;
use envoix_bindings::read::CommandKindView;
use envoix_cli::{Frontend, create_join_frame, create_mint_frame, render};
use envoix_platform_local::answer_capability;

const USAGE: &str = "\
Usage:
  envoix observe
  envoix mint REQUEST_ID send|receive
  envoix join REQUEST_ID
  envoix command CARD COMMAND_ID pause|cancel|resume|remove|re-pick-source
  envoix capability scan-invite
  envoix capability pick-source CARD GENERATION REQUEST

observe and command read newline-delimited generated lane frames from stdin.
join reads opaque invite text from stdin unchanged. Generated frames are
written to stdout; the authority transport stays outside this frontend.";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("envoix: {error}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args().skip(1);
    match arguments.next().as_deref() {
        Some("observe") if arguments.next().is_none() => observe(),
        // Minting carries no document. Which side this endpoint is on is the
        // only thing a room's creator states; a source is acquired afterwards,
        // under an identity the authority mints.
        Some("mint") => {
            let request_id = required(&mut arguments, "REQUEST_ID")?;
            let direction = match required(&mut arguments, "send|receive")?.as_str() {
                "send" => LocalDirectionView::Send,
                "receive" => LocalDirectionView::Receive,
                other => return Err(format!("unknown direction {other}")),
            };
            no_more(arguments)?;
            write_frame(
                &create_mint_frame(request_id, direction).map_err(|error| error.to_string())?,
            )
        }
        Some("join") => {
            let request_id = required(&mut arguments, "REQUEST_ID")?;
            no_more(arguments)?;
            let mut invite = String::new();
            io::stdin()
                .read_to_string(&mut invite)
                .map_err(|error| format!("could not read invite text: {error}"))?;
            write_frame(&create_join_frame(request_id, invite).map_err(|error| error.to_string())?)
        }
        Some("command") => {
            let card = required(&mut arguments, "CARD")?;
            let command_id = required(&mut arguments, "COMMAND_ID")?;
            let command = parse_command(&required(&mut arguments, "COMMAND")?)?;
            no_more(arguments)?;
            let mut frontend = Frontend::default();
            read_frames(|bytes| {
                frontend
                    .ingest(bytes)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })?;
            write_frame(
                &frontend
                    .command_frame(&card, command_id, command)
                    .map_err(|error| error.to_string())?,
            )
        }
        Some("capability") => {
            let exchange = match arguments.next().as_deref() {
                Some("scan-invite") => CapabilityExchangeView::ScanInvite(ScanInviteExchangeView {
                    step: ScanInviteStepView::Requested,
                }),
                // The acquisition a real frontend would take from the card's
                // published `pick_source` action. Spelled on the command line
                // here because this CLI holds no card of its own.
                Some("pick-source") => {
                    let acquisition = SourceAcquisitionKeyView {
                        card: arguments.next().ok_or_else(|| USAGE.to_owned())?,
                        generation: arguments
                            .next()
                            .ok_or_else(|| USAGE.to_owned())?
                            .parse()
                            .map_err(|_| USAGE.to_owned())?,
                        request: arguments.next().ok_or_else(|| USAGE.to_owned())?,
                    };
                    CapabilityExchangeView::PickSource(PickSourceExchangeView {
                        acquisition,
                        step: PickSourceStepView::Requested,
                    })
                }
                _ => return Err(USAGE.to_owned()),
            };
            no_more(arguments)?;
            let request = encode_capability_frame(&CapabilityFrame {
                body: CapabilityBody::Exchange(exchange),
            })
            .map_err(|error| format!("capability contract error: {error:?}"))?;
            write_frame(&answer_capability(&request).map_err(|error| error.to_string())?)
        }
        Some("help" | "--help" | "-h") if arguments.next().is_none() => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn observe() -> Result<(), String> {
    let mut frontend = Frontend::default();
    read_frames(|bytes| {
        let event = frontend.ingest(bytes).map_err(|error| error.to_string())?;
        println!("{}", render(&event));
        Ok(())
    })
}

fn read_frames(mut consume: impl FnMut(&[u8]) -> Result<(), String>) -> Result<(), String> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        let read = input
            .read_until(b'\n', &mut bytes)
            .map_err(|error| format!("could not read a lane frame: {error}"))?;
        if read == 0 {
            return Ok(());
        }
        while matches!(bytes.last(), Some(b'\n' | b'\r')) {
            bytes.pop();
        }
        if !bytes.is_empty() {
            consume(&bytes)?;
        }
    }
}

fn write_frame(bytes: &[u8]) -> Result<(), String> {
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(bytes)
        .and_then(|()| stdout.write_all(b"\n"))
        .map_err(|error| format!("could not write a contract frame: {error}"))
}

fn required(arguments: &mut impl Iterator<Item = String>, name: &str) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("missing {name}\n{USAGE}"))
}

fn no_more(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    if arguments.next().is_some() {
        Err(USAGE.to_owned())
    } else {
        Ok(())
    }
}

fn parse_command(command: &str) -> Result<CommandKindView, String> {
    match command {
        "pause" => Ok(CommandKindView::Pause),
        "cancel" => Ok(CommandKindView::Cancel),
        "resume" => Ok(CommandKindView::Resume),
        "remove" => Ok(CommandKindView::Remove),
        "re-pick-source" => Ok(CommandKindView::RePickSource),
        _ => Err(format!("unknown command {command:?}\n{USAGE}")),
    }
}
