//! Detect selected literal POSIX command patterns. The container supplies isolation.
// Adapted from OpenAI Codex's shell-command/src/bash.rs and
// shell-command/src/command_safety/is_dangerous_command.rs at a90c808.
// Copyright 2025 OpenAI. Apache-2.0; see NOTICE and licenses/Apache-2.0.txt.

use tree_sitter::{Node, Parser};

const MAX_WRAPPER_DEPTH: usize = 8;

/// Check Bash command text before execution. Success does not establish safety.
pub fn check(command: &str) -> Result<(), String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .map_err(|error| format!("Cannot load the command grammar: {error}"))?;
    check_script(command, 0, &mut parser)
}

fn check_script(script: &str, depth: usize, parser: &mut Parser) -> Result<(), String> {
    let tree = parser
        .parse(script, None)
        .ok_or_else(|| "Cannot parse the command.".to_string())?;
    let root = tree.root_node();
    // Match Codex's heuristic: an invalid tree supplies no literal-command evidence.
    if root.has_error() {
        return Ok(());
    }
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "command"
            && let Some(words) = literal_command(node, script)
        {
            check_words(&words, depth + 1, parser)?;
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    Ok(())
}

fn check_words(words: &[String], depth: usize, parser: &mut Parser) -> Result<(), String> {
    if depth > MAX_WRAPPER_DEPTH {
        return Err("Command blocked: inspection exceeded eight wrapper levels.".into());
    }
    let Some((program, args)) = words.split_first() else {
        return Ok(());
    };
    match program.rsplit('/').next().unwrap_or(program) {
        "rm" => {
            let forced = args
                .iter()
                .take_while(|arg| arg.as_str() != "--")
                .any(|arg| {
                    arg == "--force"
                        || arg
                            .strip_prefix('-')
                            .is_some_and(|flags| !flags.starts_with('-') && flags.contains('f'))
                });
            if forced {
                return Err("Command blocked: rm uses a force option.".into());
            }
        }
        "sudo" => check_words(args, depth + 1, parser)?,
        "env" => {
            let mut index = 0;
            while let Some(arg) = args.get(index) {
                if arg == "--" {
                    index += 1;
                    break;
                }
                if matches!(arg.as_str(), "-i" | "--ignore-environment")
                    || arg
                        .split_once('=')
                        .is_some_and(|(name, _)| !name.is_empty() && !name.starts_with('-'))
                {
                    index += 1;
                } else {
                    break;
                }
            }
            check_words(&args[index..], depth + 1, parser)?;
        }
        "trap" => {
            let index = usize::from(args.first().is_some_and(|arg| arg == "--"));
            if let Some(action) = args.get(index).filter(|arg| !arg.starts_with('-')) {
                check_script(action, depth + 1, parser)?;
            }
        }
        "sh" | "bash" | "zsh" => {
            if let [flag, script] = args
                && matches!(flag.as_str(), "-c" | "-lc")
            {
                check_script(script, depth, parser)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn literal_command(node: Node<'_>, source: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut found_name = false;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "command_name" {
            words.push(literal_word(child.named_child(0)?, source)?);
            found_name = true;
        } else if found_name && let Some(word) = literal_word(child, source) {
            words.push(word);
        }
    }
    found_name.then_some(words)
}

fn literal_word(node: Node<'_>, source: &str) -> Option<String> {
    let text = node.utf8_text(source.as_bytes()).ok()?;
    match node.kind() {
        "word" | "number" => {
            let mut cursor = node.walk();
            let literal = node.named_children(&mut cursor).next().is_none()
                && !text.starts_with('=')
                && !text.contains(['{', '}', '*', '?', '[', ']', '\\', '~', '^', '#', '$', '`']);
            literal.then(|| text.to_owned())
        }
        "raw_string" => text
            .strip_prefix('\'')?
            .strip_suffix('\'')
            .map(str::to_owned),
        "string" => {
            let mut cursor = node.walk();
            if node
                .named_children(&mut cursor)
                .any(|part| part.kind() != "string_content")
            {
                return None;
            }
            let contents = text.strip_prefix('"')?.strip_suffix('"')?;
            if contents.as_bytes().windows(2).any(|pair| {
                pair[0] == b'\\' && matches!(pair[1], b'$' | b'`' | b'"' | b'\\' | b'\n')
            }) {
                return None;
            }
            Some(contents.to_owned())
        }
        "concatenation" => {
            let mut joined = String::new();
            let mut cursor = node.walk();
            for part in node.named_children(&mut cursor) {
                joined.push_str(&literal_word(part, source)?);
            }
            (!joined.is_empty()).then_some(joined)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::check;

    #[test]
    fn rejects_force_options_through_literal_shell_forms() {
        for command in [
            "rm -f example",
            "/bin/rm -fr example",
            "rm -r -f example",
            "rm --force example",
            "sudo rm -rf example",
            "env -i TARGET=example rm -rf example",
            "if test -d example; then rm -rf example; fi",
            "printf x | rm -rf example",
            "echo \"$(rm -rf example)\"",
            "bash -c 'rm -rf example'",
            "trap 'rm -rf example' EXIT",
            "'r'm '-f' example",
        ] {
            assert!(check(command).is_err(), "{command}");
        }
    }

    #[test]
    fn distinguishes_command_syntax_from_quoted_text() {
        for command in [
            "printf '%s' 'rm -rf example'",
            "rm -r example",
            "rm -- -f",
            "env TARGET=example printf '%s' ok",
            "trap 'echo rm -rf example' EXIT",
        ] {
            assert_eq!(check(command), Ok(()), "{command}");
        }
    }

    #[test]
    fn bounds_wrapper_recursion() {
        assert!(check(&format!("{}true", "env ".repeat(7))).is_ok());
        assert!(check(&format!("{}true", "env ".repeat(8))).is_err());
    }
}
