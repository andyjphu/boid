mod api;
mod blocking;
mod browser_auth;
mod chatgpt;
mod runtime;
mod server;
mod session;
mod session_store;
mod skills;
mod startup;
mod terminal;
mod tool_calls;
mod usage;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

const HELP: &str = "Usage: boid [--no-open] | boid serve | boid stop | boid \"task\"

boid reuses the shared server or starts it in the background, then opens its web URL.
--no-open prints the URL without opening a browser. SSH sessions also print the URL only.
boid serve runs the server in the foreground. Ctrl-C stops that server.
boid stop gracefully stops the shared server and its active work.
boid passkey enroll --output PRIVATE_FILE creates a one-use browser enrollment link.
boid passkey list / boid passkey revoke ID manage browser credentials and sessions.
boid \"task\" starts or connects to the shared server and returns a new session ID.
The server owns execution; closing a submitting command or browser leaves it running.

Server: use /login in the web composer to choose API or subscription access.
OPENAI_API_KEY is optional; BOID_AUTH_ROUTE initializes the saved default.
BOID_MODEL defaults to gpt-6-sol with medium reasoning on the API-key route.
BOID_CYBER_ACCESS_PROGRAM defaults to daybreak_blue.
BOID_LISTEN defaults to 127.0.0.1:7734. Commands run with the server account permissions.
Client: BOID_URL selects an existing remote server; no local server is started for it.
Otherwise the launcher connects to BOID_LISTEN (default 127.0.0.1:7734).
BOID_CWD selects a server-side directory; local launches default to the invocation directory.
Remote connections without BOID_CWD use the server working directory.
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
    if args == ["--version"] || args == ["-V"] {
        println!("boid {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args == ["serve"] {
        return server::serve().await;
    }
    if args == ["stop"] {
        return startup::stop().await;
    }
    if args.first().is_some_and(|arg| arg == "passkey") {
        return browser_auth::command(&args[1..]).await;
    }
    let connection = startup::connect().await?;
    if args.is_empty() || args == ["--no-open"] {
        return startup::open_workspace(&connection, args == ["--no-open"]);
    }
    let response = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()?
        .post(connection.url.join("api/sessions")?)
        .bearer_auth(&connection.token)
        .json(&json!({"message":args.join(" "),"cwd":startup::invocation_cwd()?}))
        .send()
        .await
        .context("Cannot reach the Boid server.")?;
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
        connection.url
    );
    Ok(())
}
