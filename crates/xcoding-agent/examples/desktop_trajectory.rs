use std::{env, path::PathBuf, process};

use serde::Serialize;
use xcoding_agent::AgentService;
use xcoding_core::CoreService;
use xcoding_protocol::{ChatParams, ChatResult, ReplaySessionResult, SessionEvent};

#[derive(Serialize)]
struct DesktopTrajectory {
    result: ChatResult,
    events: Vec<SessionEvent>,
    replay: ReplaySessionResult,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("desktop trajectory driver: {error}");
        process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let database_path = required_path(args.next(), "database path")?;
    let workspace_root = required_path(args.next(), "workspace root")?;
    let message = args.next().ok_or("missing prompt")?;
    let model = args.next().unwrap_or_else(|| "fixture-model".to_owned());

    if let Some(parent) = database_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let core = CoreService::open(database_path)?;
    let mut events = Vec::new();
    let result = AgentService::new(&core)
        .chat(
            ChatParams {
                workspace_root: workspace_root.to_string_lossy().into_owned(),
                message,
                mode: None,
                provider: None,
                model: Some(model),
                title: None,
                session_id: None,
                images: None,
            },
            |event| events.push(event),
        )
        .await?;
    let replay = core.session_replay(result.session.id)?;
    println!(
        "{}",
        serde_json::to_string(&DesktopTrajectory {
            result,
            events,
            replay,
        })?
    );
    Ok(())
}

fn required_path(
    value: Option<String>,
    label: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    value
        .map(PathBuf::from)
        .ok_or_else(|| format!("missing {label}").into())
}
