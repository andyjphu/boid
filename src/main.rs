mod api;
mod runtime;
mod server;
mod session;
mod session_store;
mod skills;
mod terminal;
mod tool_calls;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

const HELP: &str = "Usage: boid serve | boid \"task\"

boid serve (or boid with no arguments) starts the shared server and prints its web URL.
boid \"task\" submits a session to the running server and returns its ID.
The server owns execution; closing a submitting command or browser leaves it running.

Server: OPENAI_API_KEY required; BOID_MODEL defaults to gpt-6-sol with medium reasoning.
BOID_CYBER_ACCESS_PROGRAM defaults to daybreak_blue.
BOID_LISTEN defaults to 127.0.0.1:7734. Commands run with the server account permissions.
Client: BOID_URL defaults to http://127.0.0.1:7734.
BOID_CWD selects a server-side directory; omitted uses the server working directory.
BOID_TOKEN overrides the private token file under BOID_STATE_DIR (default ~/.local/share/boid).
Server shutdown stops execution; saved conversations return paused on restart. End archives a session without erasing its record.";

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("boid: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
async fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--help"] || args == ["-h"] {
        println!("{HELP}");
        return Ok(());
    }
    if args.is_empty() || args == ["serve"] {
        return server::serve().await;
    }
    let url = std::env::var("BOID_URL").unwrap_or_else(|_| "http://127.0.0.1:7734".into());
    let parsed = reqwest::Url::parse(&url).context("Invalid BOID_URL.")?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        bail!("BOID_URL must use HTTP or HTTPS.");
    }
    let token = server::read_token()?;
    let response = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()?
        .post(format!("{}/api/sessions", url.trim_end_matches('/')))
        .bearer_auth(token)
        .json(
            &json!({"message":args.join(" "),"cwd":std::env::var("BOID_CWD").unwrap_or_default()}),
        )
        .send()
        .await
        .context("Cannot reach the Boid server. Start `boid serve` first.")?;
    let status = response.status();
    let body: Value = response.json().await.context("Invalid server response.")?;
    if !status.is_success() {
        bail!(
            "Server returned {status}: {}",
            body["error"].as_str().unwrap_or("Request failed.")
        );
    }
    println!(
        "Session {}\n{}",
        body["id"].as_str().context("Missing session ID.")?,
        url
    );
    Ok(())
}
