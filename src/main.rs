mod api;
mod session;
mod tool_calls;

use anyhow::Result;
use boid::Tools;
use std::io::{self, BufRead, IsTerminal, Write};
use tokio::sync::mpsc;

const HELP: &str = "Usage: boid [task]

Run in the task container, from the working directory.
Set OPENAI_API_KEY; BOID_MODEL defaults to gpt-6.1-sol.
Type a new message to interrupt and steer ongoing work.
/stop or Ctrl-C stops a turn. /quit exits. Ctrl-C while idle exits.
Piped input runs to completion and exits. Conversation state lasts this process.";

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("\nboid: {error:#}");
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
    let client = api::Client::new()?;
    let tools = Tools::new(std::env::current_dir()?)?;
    let interactive = io::stdin().is_terminal();
    let (tx, mut rx) = mpsc::unbounded_channel();
    // A detached reader lets the runtime exit while the terminal waits for input.
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut session = session::Session::default();
    let task = args.join(" ");
    let mut active = !task.trim().is_empty();
    if active {
        session.user(task);
    }
    loop {
        let event = if active {
            tokio::select! {
                biased;
                signal = tokio::signal::ctrl_c() => { signal?; Event::Stop }
                line = next_steering(&mut rx) => Event::Input(Some(line)),
                result = session.run(&client, &tools) => Event::Finished(result),
            }
        } else {
            if interactive {
                print!("\nboid> ");
                io::stdout().flush()?;
            }
            tokio::select! {
                signal = tokio::signal::ctrl_c() => { signal?; break; }
                line = rx.recv() => Event::Input(line),
            }
        };
        match event {
            Event::Stop => {
                session.interrupt();
                session.user("Stop the current task.".into());
                eprintln!("\nStopped.");
                active = false;
                if !interactive {
                    anyhow::bail!("Interrupted.");
                }
            }
            Event::Input(Some(line)) => {
                let line = line?;
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if active {
                    session.interrupt();
                }
                match line {
                    "/quit" => break,
                    "/stop" | "stop" => {
                        session.user("Stop the current task.".into());
                        eprintln!("\nStopped.");
                        active = false;
                    }
                    _ => {
                        if active {
                            eprintln!("\nInterrupted the current turn. Applying new input.");
                        }
                        session.user(line.into());
                        active = true;
                    }
                }
            }
            Event::Input(None) => break,
            Event::Finished(result) => {
                active = false;
                if let Err(error) = result {
                    session.interrupt();
                    if !interactive {
                        return Err(error);
                    }
                    eprintln!("\nboid: {error:#}");
                }
            }
        }
    }
    Ok(())
}

enum Event {
    Input(Option<io::Result<String>>),
    Stop,
    Finished(Result<()>),
}

async fn next_steering(rx: &mut mpsc::UnboundedReceiver<io::Result<String>>) -> io::Result<String> {
    loop {
        match rx.recv().await {
            Some(Ok(line)) if line.trim().is_empty() => continue,
            Some(line) => return line,
            // EOF is not a request to cancel the current turn.
            None => return std::future::pending().await,
        }
    }
}

pub(crate) fn show(text: &str) {
    let visible: String = text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    let mut out = io::stdout().lock();
    let _ = out.write_all(visible.as_bytes());
    let _ = out.flush();
}
