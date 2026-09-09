//! Which model a role will actually run.
//!
//! rigg picks the binary and never the model: `kind = "claude"` and the model
//! claude runs are two different settings, and only the first is rigg's. So
//! reporting one means reading the places the binary itself reads, in its own
//! order - a flag on the role first, then the environment, then the checkout's
//! config, then the user's.

use std::path::{Path, PathBuf};

use crate::config::AgentCfg;

/// What to print when nothing names a model and the agent picks for itself.
pub const UNSET: &str = "(its own default)";

/// The model `cfg` would run in a checkout at `root`.
pub fn model_for(cfg: &AgentCfg, root: &Path) -> String {
    let flagged = cfg
        .command
        .as_deref()
        .and_then(flag)
        .or_else(|| flag(&cfg.args));
    if let Some(model) = flagged {
        return model;
    }
    // A `command` role runs a binary rigg knows nothing else about, so with no
    // flag on it there is nowhere left to look.
    if cfg.command.is_some() {
        return UNSET.to_string();
    }
    let (env, files) = match cfg.kind.as_str() {
        "claude" => ("ANTHROPIC_MODEL", claude_files(root)),
        "opencode" => ("OPENCODE_MODEL", opencode_files(root)),
        _ => return UNSET.to_string(),
    };
    std::env::var(env)
        .ok()
        .filter(|m| !m.trim().is_empty())
        .or_else(|| files.iter().find_map(|p| top_level_string(p, "model")))
        .unwrap_or_else(|| UNSET.to_string())
}

/// The value after `--model` / `-m`, which both kinds take.
fn flag(argv: &[String]) -> Option<String> {
    let at = argv.iter().position(|a| a == "--model" || a == "-m")?;
    argv.get(at + 1).cloned()
}

/// claude's own order: the checkout's private settings, then the ones it
/// shares, then the user's.
fn claude_files(root: &Path) -> Vec<PathBuf> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".claude")));
    let mut files = vec![
        root.join(".claude").join("settings.local.json"),
        root.join(".claude").join("settings.json"),
    ];
    files.extend(dir.map(|d| d.join("settings.json")));
    files
}

fn opencode_files(root: &Path) -> Vec<PathBuf> {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".config")))
        .map(|d| d.join("opencode"));
    let mut files = vec![root.join("opencode.jsonc"), root.join("opencode.json")];
    files.extend(dir.into_iter().flat_map(|d| {
        [d.join("opencode.jsonc"), d.join("opencode.json")]
    }));
    files
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// A top-level string field out of a config file that may be JSONC.
///
/// Top-level only: opencode also takes a `model` inside an agent block, and
/// reporting one of those as the model the run uses would be wrong.
fn top_level_string(path: &Path, key: &str) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&plain_json(&raw)).ok()?;
    json.get(key)?.as_str().map(str::to_string)
}

/// JSON with the comments and trailing commas taken out, because opencode's
/// config is allowed both and serde_json will read neither.
fn plain_json(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
                // Kept, so a `//` comment cannot join two lines into one.
                out.push('\n');
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for c in chars.by_ref() {
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
            }
            _ => out.push(c),
        }
    }
    drop_trailing_commas(&out)
}

fn drop_trailing_commas(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut in_string = false;
    let mut escaped = false;
    for (i, &c) in chars.iter().enumerate() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '"' {
            in_string = true;
        }
        let trailing = c == ','
            && matches!(
                chars[i + 1..].iter().copied().find(|c| !c.is_whitespace()),
                Some('}') | Some(']')
            );
        if !trailing {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(kind: &str, args: &[&str]) -> AgentCfg {
        AgentCfg {
            kind: kind.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            command: None,
        }
    }

    #[test]
    fn a_flag_on_the_role_wins() {
        let cfg = agent("opencode", &["-m", "vllm/qwen3"]);
        assert_eq!(model_for(&cfg, Path::new("/nowhere")), "vllm/qwen3");
    }

    #[test]
    fn jsonc_is_read_the_way_opencode_writes_it() {
        let src = r#"{
            // the one it runs
            "model": "vllm/qwen3-coder-next",
            "provider": { "vllm": { "models": { "other": {} } } },
        }"#;
        let json: serde_json::Value = serde_json::from_str(&plain_json(src)).unwrap();
        assert_eq!(json["model"], "vllm/qwen3-coder-next");
    }

    #[test]
    fn a_slash_inside_a_string_is_not_a_comment() {
        let src = r#"{"model": "vllm/qwen3", "note": "a // b"}"#;
        let json: serde_json::Value = serde_json::from_str(&plain_json(src)).unwrap();
        assert_eq!(json["model"], "vllm/qwen3");
        assert_eq!(json["note"], "a // b");
    }

    #[test]
    fn an_unreadable_config_is_not_a_guess() {
        let cfg = agent("something-else", &[]);
        assert_eq!(model_for(&cfg, Path::new("/nowhere")), UNSET);
    }
}
