use boid::{ExecOutput, Output, Tools};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};
use std::error::Error;
use std::os::unix::process::ExitStatusExt;
use std::time::Duration;

pub fn definitions() -> Value {
    let string = json!({"type": "string"});
    json!([
        definition(
            "read_skill",
            "Read the complete guidance for a skill listed in this session's startup catalog. Returns the session's saved skill version without executing scripts.",
            &[("name", string.clone())],
        ),
        definition(
            "read_file",
            "Read up to 32 KiB from a regular file. Paths are relative to the working directory unless absolute.",
            &[("path", string.clone())],
        ),
        definition(
            "write_file",
            "Create or replace a file atomically. Its parent must exist. Final symlinks are rejected.",
            &[("path", string.clone()), ("content", string.clone())],
        ),
        definition(
            "edit_file",
            "Replace one exact, nonempty occurrence in a UTF-8 file up to 1 MiB. Final symlinks are rejected.",
            &[
                ("path", string.clone()),
                ("old", string.clone()),
                ("new", string.clone())
            ],
        ),
        definition(
            "glob",
            "List files matching a ripgrep glob, sorted by path. Explicit globs override ignore rules.",
            &[("pattern", string.clone())],
        ),
        definition(
            "search",
            "Search file contents with a ripgrep regular expression. Results include paths and line numbers. A non-null glob overrides ignore rules.",
            &[
                ("pattern", string.clone()),
                ("glob", json!({"type": ["string", "null"]}))
            ],
        ),
        definition(
            "exec_command",
            "Run Bash with the process's permissions and a small command guard. timeout_ms defaults to 30000 when null; otherwise use 1 to 300000.",
            &[
                ("command", string.clone()),
                (
                    "timeout_ms",
                    json!({"type": ["integer", "null"], "minimum": 1, "maximum": 300000})
                ),
            ],
        ),
        definition(
            "list_agents",
            "List sessions and their current status. A working status does not establish task completion.",
            &[]
        ),
        definition(
            "send_message",
            "Send an attributed message to another session. Queued delivery is not acceptance. Messages enter at the next model request; explicitly stopped agents wait for user input.",
            &[("to", string.clone()), ("text", string)]
        ),
    ])
}

fn definition(name: &str, description: &str, fields: &[(&str, Value)]) -> Value {
    let properties: Map<String, Value> = fields
        .iter()
        .map(|(name, schema)| ((*name).to_owned(), schema.clone()))
        .collect();
    let required: Vec<&str> = fields.iter().map(|(name, _)| *name).collect();
    json!({
        "type": "function",
        "name": name,
        "description": description,
        "strict": true,
        "parameters": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    path: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditArgs {
    path: String,
    old: String,
    new: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GlobArgs {
    pattern: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    pattern: String,
    #[serde(deserialize_with = "nullable")]
    glob: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecArgs {
    command: String,
    #[serde(deserialize_with = "nullable")]
    timeout_ms: Option<u64>,
}

// Strict function schemas require nullable fields to be present.
fn nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

pub async fn execute(tools: &Tools, name: &str, arguments: &str) -> Value {
    match dispatch(tools, name, arguments).await {
        Ok(result) => result,
        Err(error) => json!({"error": format!("{name}: {error}")}),
    }
}

async fn dispatch(
    tools: &Tools,
    name: &str,
    arguments: &str,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    if !arguments.trim_start().starts_with('{') {
        return Err("Arguments must be a JSON object.".into());
    }
    match name {
        "read_file" => {
            let args: ReadArgs = serde_json::from_str(arguments)?;
            Ok(output(tools.read(args.path)?))
        }
        "write_file" => {
            let args: WriteArgs = serde_json::from_str(arguments)?;
            tools.write(args.path, &args.content)?;
            Ok(json!({"ok": true}))
        }
        "edit_file" => {
            let args: EditArgs = serde_json::from_str(arguments)?;
            tools.edit(args.path, &args.old, &args.new)?;
            Ok(json!({"ok": true}))
        }
        "glob" => {
            let args: GlobArgs = serde_json::from_str(arguments)?;
            Ok(exec_output(tools.glob(&args.pattern).await?))
        }
        "search" => {
            let args: SearchArgs = serde_json::from_str(arguments)?;
            Ok(exec_output(
                tools.search(&args.pattern, args.glob.as_deref()).await?,
            ))
        }
        "exec_command" => {
            let args: ExecArgs = serde_json::from_str(arguments)?;
            let timeout_ms = args.timeout_ms.unwrap_or(30000);
            if !(1..=300000).contains(&timeout_ms) {
                return Err("timeout_ms must be between 1 and 300000, or null.".into());
            }
            Ok(exec_output(
                tools
                    .exec(&args.command, Duration::from_millis(timeout_ms))
                    .await?,
            ))
        }
        _ => Err("Unknown tool.".into()),
    }
}

fn output(result: Output) -> Value {
    json!({"text": result.text, "truncated": result.truncated})
}

fn exec_output(result: ExecOutput) -> Value {
    json!({
        "exit_code": result.status.code(),
        "signal": result.status.signal(),
        "timed_out": result.timed_out,
        "stdout": output(result.stdout),
        "stderr": output(result.stderr),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalid_arguments_return_errors_without_side_effects() {
        let directory = tempfile::tempdir().unwrap();
        let tools = Tools::new(directory.path()).unwrap();
        for (name, arguments) in [
            ("write_file", "{"),
            ("write_file", r#"["marker", "changed"]"#),
            ("write_file", r#"{"path":false,"content":"changed"}"#),
            (
                "write_file",
                r#"{"path":"marker","content":"changed","extra":true}"#,
            ),
            ("search", r#"{"pattern":"anything"}"#),
            ("exec_command", r#"{"command":"touch marker"}"#),
            (
                "exec_command",
                r#"{"command":"touch marker","timeout_ms":0}"#,
            ),
            (
                "exec_command",
                r#"{"command":"touch marker","timeout_ms":300001}"#,
            ),
            (
                "exec_command",
                r#"{"command":"touch marker","timeout_ms":-1}"#,
            ),
            (
                "exec_command",
                r#"{"command":"touch marker","timeout_ms":1.5}"#,
            ),
            ("unknown", "{}"),
        ] {
            let result = execute(&tools, name, arguments).await;
            assert!(result["error"].is_string(), "{name}: {arguments}: {result}");
        }
        assert!(!directory.path().join("marker").exists());
    }

    #[tokio::test]
    async fn command_results_keep_status_and_output_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let tools = Tools::new(directory.path()).unwrap();
        let result = execute(
            &tools,
            "exec_command",
            r#"{"command":"printf hello; printf warning >&2; exit 7","timeout_ms":null}"#,
        )
        .await;
        assert_eq!(
            result,
            json!({
                "exit_code": 7,
                "signal": null,
                "timed_out": false,
                "stdout": {"text": "hello", "truncated": false},
                "stderr": {"text": "warning", "truncated": false},
            })
        );
    }
}
