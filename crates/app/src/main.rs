use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use flow_agent_bridge::{BridgeClient, BridgeListener, default_socket_path};
use flow_agent_core::{
    BridgeRequest, BridgeResponse, Decision, MAX_HOOK_PAYLOAD_BYTES, Provider, permission_directive,
};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::str::FromStr;
use std::thread;
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(
    name = "flow-agent",
    version,
    about = "Local-first agent attention runtime"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the local M0 bridge runtime.
    Serve {
        #[arg(long, value_enum, default_value_t = ApprovalMode::Prompt)]
        approval: ApprovalMode,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
    /// Receive one provider hook payload from stdin and forward it to the runtime.
    Hook {
        #[arg(long)]
        provider: String,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ApprovalMode {
    Prompt,
    Allow,
    Deny,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Serve { approval, socket } => {
            serve(socket.unwrap_or_else(default_socket_path), approval)
        }
        Command::Hook { provider, socket } => {
            // Hook failures must be silent and fail open. Parsing CLI arguments still
            // reports errors because malformed installation is an operator error.
            let provider = Provider::from_str(&provider)?;
            let _ = run_hook(provider, socket.unwrap_or_else(default_socket_path));
            Ok(())
        }
    }
}

fn serve(socket_path: PathBuf, approval: ApprovalMode) -> Result<()> {
    let listener = BridgeListener::bind(&socket_path)
        .with_context(|| format!("failed to bind {}", socket_path.display()))?;
    println!(
        "flow-agent M0 bridge listening on {}",
        socket_path.display()
    );

    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        thread::spawn(move || {
            let Ok(request) = BridgeListener::read_request(&mut stream) else {
                return;
            };
            println!(
                "provider={} event={} session={}",
                request.provider,
                request.event_name().unwrap_or("unknown"),
                request.session_id().unwrap_or("unknown")
            );

            if request.needs_reply {
                let decision = choose_decision(approval);
                let _ = BridgeListener::write_response(
                    &mut stream,
                    &BridgeResponse::decided(request.id, decision),
                );
            }
        });
    }
    Ok(())
}

fn choose_decision(mode: ApprovalMode) -> Decision {
    match mode {
        ApprovalMode::Allow => Decision::Allow,
        ApprovalMode::Deny => Decision::Deny,
        ApprovalMode::Prompt => loop {
            eprint!("Approve this request? [y/N] ");
            let _ = io::stderr().flush();
            let mut answer = String::new();
            if io::stdin().read_line(&mut answer).is_err() {
                return Decision::Deny;
            }
            match answer.trim().to_ascii_lowercase().as_str() {
                "y" | "yes" => return Decision::Allow,
                "" | "n" | "no" => return Decision::Deny,
                _ => continue,
            }
        },
    }
}

fn run_hook(provider: Provider, socket_path: PathBuf) -> Result<()> {
    let mut input = Vec::new();
    io::stdin()
        .take((MAX_HOOK_PAYLOAD_BYTES + 1) as u64)
        .read_to_end(&mut input)?;
    if input.len() > MAX_HOOK_PAYLOAD_BYTES {
        anyhow::bail!("hook payload exceeds {} bytes", MAX_HOOK_PAYLOAD_BYTES);
    }
    let raw = serde_json::from_slice(&input)?;
    let request = BridgeRequest::from_hook(provider, raw);
    let timeout = if request.needs_reply {
        reply_timeout(provider)
    } else {
        Duration::from_millis(200)
    };

    let Some(response) = BridgeClient::new(socket_path).send(&request, timeout)? else {
        return Ok(());
    };
    let Some(decision) = response.decision else {
        return Ok(());
    };
    if let Some(directive) = permission_directive(provider, decision) {
        serde_json::to_writer(io::stdout(), &directive)?;
        println!();
    }
    Ok(())
}

fn reply_timeout(provider: Provider) -> Duration {
    if let Ok(value) = std::env::var("FLOW_AGENT_HOOK_REPLY_TIMEOUT_MS")
        && let Ok(milliseconds) = value.parse::<u64>()
    {
        return Duration::from_millis(milliseconds);
    }
    match provider {
        Provider::Claude => Duration::from_secs(24 * 60 * 60),
        Provider::Codex => Duration::from_secs(60 * 60),
        Provider::Gemini => Duration::from_millis(200),
    }
}
