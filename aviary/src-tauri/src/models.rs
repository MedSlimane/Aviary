//! Model catalogue discovered from the installed runners.
//!
//! Runner model names and reasoning levels change independently of Aviary.
//! Claude Code advertises the values it accepts in `--help`; Codex maintains a
//! local model cache. Both inputs are treated as bounded, untrusted metadata.
//! A no-flag option is always present, and a valid configured value remains
//! visible even when it is newer than the runner's advertised examples.

use serde::Serialize;
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const RUNNER_HELP_TIMEOUT: Duration = Duration::from_secs(3);
const RUNNER_HELP_DRAIN_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_HELP_BYTES: usize = 512 * 1024;
const MAX_SETTINGS_BYTES: usize = 256 * 1024;
const MAX_CODEX_CACHE_BYTES: usize = 8 * 1024 * 1024;
const MAX_MODELS: usize = 256;
const MAX_REASONING_LEVELS: usize = 32;
const MAX_MODEL_ID_CHARS: usize = 256;
const MAX_EFFORT_ID_CHARS: usize = 64;
const MAX_LABEL_CHARS: usize = 160;
const MAX_NOTE_CHARS: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReasoningLevel {
    pub effort: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelOption {
    /// Passed to `--model` / `-m`. `None` means send no flag at all.
    pub id: Option<String>,
    pub label: String,
    pub note: String,
    /// True only when the installed runner describes the value as an alias.
    pub is_alias: bool,
    /// Effort levels this model accepts, lowest first. Empty when the runner
    /// does not expose the notion.
    pub reasoning_levels: Vec<ReasoningLevel>,
    /// The configured level used when none is chosen.
    pub default_effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelCatalogue {
    pub models: Vec<ModelOption>,
    /// What the runner is configured to use when no flag is passed.
    pub configured_default: Option<String>,
    /// Where the list came from, so the UI can be honest about staleness.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AdvertisedModel {
    id: String,
    is_alias: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ClaudeHelp {
    models: Vec<AdvertisedModel>,
    efforts: Vec<String>,
}

#[derive(Debug, Default)]
struct ClaudeSettings {
    model: Option<String>,
    effort: Option<String>,
}

fn claude_catalogue() -> ModelCatalogue {
    let settings = claude_settings();
    let help = bounded_command_stdout("claude", &["--help"], MAX_HELP_BYTES)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|text| parse_claude_help(&text));
    build_claude_catalogue(help.as_ref(), settings)
}

fn build_claude_catalogue(help: Option<&ClaudeHelp>, settings: ClaudeSettings) -> ModelCatalogue {
    let configured = settings.model.and_then(|value| model_id(&value));
    let configured_effort = settings.effort.and_then(|value| effort_id(&value));
    let mut effort_ids = help.map(|value| value.efforts.clone()).unwrap_or_default();
    if let Some(value) = configured_effort.as_ref() {
        push_unique_bounded(&mut effort_ids, value.clone(), MAX_REASONING_LEVELS);
    }
    let reasoning_levels = effort_ids
        .into_iter()
        .map(|effort| ReasoningLevel {
            description: if configured_effort.as_deref() == Some(effort.as_str())
                && !help.is_some_and(|value| value.efforts.contains(&effort))
            {
                "Configured in Claude Code settings".to_string()
            } else {
                "Advertised by the installed Claude Code CLI".to_string()
            },
            effort,
        })
        .collect::<Vec<_>>();

    let mut models = vec![ModelOption {
        id: None,
        label: "Default".to_string(),
        note: configured
            .as_deref()
            .map(|model| format!("Claude Code setting: {model}"))
            .unwrap_or_else(|| "Use Claude Code's configured default".to_string()),
        is_alias: false,
        reasoning_levels: reasoning_levels.clone(),
        default_effort: configured_effort.clone(),
    }];
    let mut seen = HashSet::new();
    for advertised in help
        .into_iter()
        .flat_map(|value| value.models.iter())
        .take(MAX_MODELS.saturating_sub(1))
    {
        if !seen.insert(advertised.id.clone()) {
            continue;
        }
        models.push(ModelOption {
            id: Some(advertised.id.clone()),
            label: advertised.id.clone(),
            note: if advertised.is_alias {
                "Alias advertised by the installed Claude Code CLI".to_string()
            } else {
                "Model example advertised by the installed Claude Code CLI".to_string()
            },
            is_alias: advertised.is_alias,
            reasoning_levels: reasoning_levels.clone(),
            default_effort: configured_effort.clone(),
        });
    }
    if let Some(value) = configured.as_ref() {
        if seen.insert(value.clone()) {
            if models.len() >= MAX_MODELS {
                models.pop();
            }
            models.push(ModelOption {
                id: Some(value.clone()),
                label: value.clone(),
                note: "Currently configured in Claude Code settings".to_string(),
                is_alias: help.is_some_and(|help| {
                    help.models
                        .iter()
                        .any(|model| model.id == *value && model.is_alias)
                }),
                reasoning_levels,
                default_effort: configured_effort,
            });
        }
    }

    let source = match help {
        Some(help) => format!(
            "installed Claude Code --help ({} advertised model values)",
            help.models.len()
        ),
        None if configured.is_some() => "Claude Code settings (CLI help unavailable)".to_string(),
        None => "Claude Code CLI help unavailable".to_string(),
    };
    ModelCatalogue {
        models,
        configured_default: configured,
        source,
    }
}

fn claude_settings() -> ClaudeSettings {
    let Some(path) = crate::providers::home().map(|home| home.join(".claude/settings.json")) else {
        return ClaudeSettings::default();
    };
    let Some(bytes) = read_bounded(&path, MAX_SETTINGS_BYTES) else {
        return ClaudeSettings::default();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return ClaudeSettings::default();
    };
    ClaudeSettings {
        model: value
            .get("model")
            .and_then(serde_json::Value::as_str)
            .and_then(model_id),
        effort: value
            .get("effortLevel")
            .and_then(serde_json::Value::as_str)
            .and_then(effort_id),
    }
}

fn parse_claude_help(help: &str) -> ClaudeHelp {
    let model_description = option_description(help, "--model").unwrap_or_default();
    let effort_description = option_description(help, "--effort").unwrap_or_default();
    ClaudeHelp {
        models: parse_advertised_models(&model_description),
        efforts: parse_effort_values(&effort_description),
    }
}

/// Returns one option's wrapped help paragraph without depending on terminal
/// width. A new option line starts with `-`; indented continuation lines do not.
fn option_description(help: &str, option: &str) -> Option<String> {
    let mut found = false;
    let mut parts = Vec::new();
    for line in help.lines() {
        let trimmed = line.trim();
        if !found {
            if trimmed.starts_with('-') && option_token_present(trimmed, option) {
                found = true;
                parts.push(trimmed.to_string());
            }
            continue;
        }
        if trimmed.starts_with('-') || trimmed.ends_with(':') && !trimmed.contains(' ') {
            break;
        }
        if !trimmed.is_empty() {
            parts.push(trimmed.to_string());
        }
    }
    found.then(|| parts.join(" "))
}

fn option_token_present(line: &str, option: &str) -> bool {
    line.split_whitespace()
        .map(|token| token.trim_end_matches(','))
        .any(|token| token == option)
}

fn parse_advertised_models(description: &str) -> Vec<AdvertisedModel> {
    let lower = description.to_ascii_lowercase();
    let alias_marker = lower.find("alias");
    let full_name_marker = lower.find("full name");
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for (offset, raw) in quoted_values(description) {
        let Some(id) = model_id(raw) else {
            continue;
        };
        if !seen.insert(id.clone()) || result.len() >= MAX_MODELS.saturating_sub(1) {
            continue;
        }
        let is_alias = alias_marker.is_some_and(|marker| offset > marker)
            && full_name_marker.is_none_or(|marker| offset < marker);
        result.push(AdvertisedModel { id, is_alias });
    }
    result
}

fn parse_effort_values(description: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    for group in parenthesized_values(description) {
        let lower = group.to_ascii_lowercase();
        if !group.contains(',') && !lower.contains("choices") && !lower.contains("values") {
            continue;
        }
        for raw in group.split(',') {
            let raw = raw
                .split_once(':')
                .map(|(_, value)| value)
                .unwrap_or(raw)
                .trim()
                .trim_matches(|character| character == '\'' || character == '"');
            if let Some(value) = effort_id(raw) {
                push_unique_bounded(&mut candidates, value, MAX_REASONING_LEVELS);
            }
        }
    }
    if candidates.is_empty() {
        for (_, raw) in quoted_values(description) {
            if let Some(value) = effort_id(raw) {
                push_unique_bounded(&mut candidates, value, MAX_REASONING_LEVELS);
            }
        }
    }
    candidates
}

fn quoted_values(value: &str) -> Vec<(usize, &str)> {
    let bytes = value.as_bytes();
    let mut result = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let quote = bytes[index];
        let opening_boundary = index == 0
            || bytes[index - 1].is_ascii_whitespace()
            || matches!(bytes[index - 1], b'(' | b'[' | b'{' | b',' | b':');
        if (quote != b'\'' && quote != b'"') || !opening_boundary {
            index += 1;
            continue;
        }
        let start = index + 1;
        index = start;
        while index < bytes.len() && bytes[index] != quote {
            index += 1;
        }
        if index < bytes.len() {
            if let Some(candidate) = value.get(start..index) {
                result.push((start, candidate));
            }
            index += 1;
        }
    }
    result
}

fn parenthesized_values(value: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = None;
    for (index, character) in value.char_indices() {
        match character {
            '(' if start.is_none() => start = Some(index + 1),
            ')' => {
                if let Some(open) = start.take() {
                    if let Some(group) = value.get(open..index) {
                        result.push(group);
                    }
                }
            }
            _ => {}
        }
    }
    result
}

fn codex_catalogue() -> ModelCatalogue {
    let configured = crate::providers::home()
        .map(|home| home.join(".codex/config.toml"))
        .and_then(|path| read_bounded(&path, MAX_SETTINGS_BYTES))
        .and_then(|raw| String::from_utf8(raw).ok())
        .and_then(|raw| raw.parse::<toml::Table>().ok())
        .and_then(|table| {
            table
                .get("model")
                .and_then(toml::Value::as_str)
                .and_then(model_id)
        });

    let mut models = vec![ModelOption {
        id: None,
        label: "Default".to_string(),
        note: configured
            .as_deref()
            .map(|model| format!("Codex config setting: {model}"))
            .unwrap_or_else(|| "Use Codex's configured default".to_string()),
        is_alias: false,
        reasoning_levels: Vec::new(),
        default_effort: None,
    }];
    let cache = crate::providers::home()
        .map(|home| home.join(".codex/models_cache.json"))
        .and_then(|path| read_bounded(&path, MAX_CODEX_CACHE_BYTES))
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok());

    let mut source = "Codex model cache unavailable".to_string();
    let mut seen = HashSet::new();
    if let Some(cache) = cache {
        if let Some(list) = cache.get("models").and_then(serde_json::Value::as_array) {
            source = cache
                .get("fetched_at")
                .and_then(serde_json::Value::as_str)
                .map(|fetched| {
                    format!(
                        "~/.codex/models_cache.json · fetched {}",
                        display_text(fetched, MAX_LABEL_CHARS)
                    )
                })
                .unwrap_or_else(|| "~/.codex/models_cache.json".to_string());
            for model in list.iter().take(MAX_MODELS.saturating_sub(1)) {
                let Some(slug) = model
                    .get("slug")
                    .and_then(serde_json::Value::as_str)
                    .and_then(model_id)
                else {
                    continue;
                };
                if !seen.insert(slug.clone()) {
                    continue;
                }
                let mut levels = Vec::new();
                if let Some(values) = model
                    .get("supported_reasoning_levels")
                    .and_then(serde_json::Value::as_array)
                {
                    for value in values.iter().take(MAX_REASONING_LEVELS) {
                        let Some(effort) = value
                            .get("effort")
                            .and_then(serde_json::Value::as_str)
                            .and_then(effort_id)
                        else {
                            continue;
                        };
                        if levels
                            .iter()
                            .any(|level: &ReasoningLevel| level.effort == effort)
                        {
                            continue;
                        }
                        levels.push(ReasoningLevel {
                            effort,
                            description: value
                                .get("description")
                                .and_then(serde_json::Value::as_str)
                                .map(|text| display_text(text, MAX_NOTE_CHARS))
                                .unwrap_or_default(),
                        });
                    }
                }
                let default_effort = model
                    .get("default_reasoning_level")
                    .and_then(serde_json::Value::as_str)
                    .and_then(effort_id);
                models.push(ModelOption {
                    label: model
                        .get("display_name")
                        .and_then(serde_json::Value::as_str)
                        .map(|text| display_text(text, MAX_LABEL_CHARS))
                        .filter(|text| !text.is_empty())
                        .unwrap_or_else(|| slug.clone()),
                    note: model
                        .get("description")
                        .and_then(serde_json::Value::as_str)
                        .map(|text| display_text(text, MAX_NOTE_CHARS))
                        .unwrap_or_default(),
                    reasoning_levels: levels,
                    default_effort,
                    id: Some(slug),
                    is_alias: false,
                });
            }
        }
    }
    if let Some(value) = configured.as_ref() {
        if seen.insert(value.clone()) {
            if models.len() >= MAX_MODELS {
                models.pop();
            }
            models.push(ModelOption {
                id: Some(value.clone()),
                label: value.clone(),
                note: "Currently configured in Codex config.toml".to_string(),
                is_alias: false,
                reasoning_levels: Vec::new(),
                default_effort: None,
            });
        }
    }
    ModelCatalogue {
        models,
        configured_default: configured,
        source,
    }
}

fn model_id(value: &str) -> Option<String> {
    bounded_identifier(value, MAX_MODEL_ID_CHARS)
}

fn effort_id(value: &str) -> Option<String> {
    bounded_identifier(value, MAX_EFFORT_ID_CHARS)
}

fn bounded_identifier(value: &str, max_chars: usize) -> Option<String> {
    let value = value.trim();
    let mut count = 0;
    if value.is_empty()
        || !value.chars().all(|character| {
            count += 1;
            count <= max_chars
                && (character.is_ascii_alphanumeric()
                    || matches!(character, '-' | '_' | '.' | ':' | '/' | '@' | '+'))
        })
    {
        return None;
    }
    Some(value.to_string())
}

fn display_text(value: &str, max_chars: usize) -> String {
    let mut result = String::new();
    let mut pending_space = false;
    for character in value.chars() {
        if result.chars().count() >= max_chars {
            break;
        }
        if character.is_control() || character.is_whitespace() {
            pending_space = !result.is_empty();
            continue;
        }
        if pending_space {
            result.push(' ');
            pending_space = false;
        }
        result.push(character);
    }
    result
}

fn push_unique_bounded(values: &mut Vec<String>, value: String, max: usize) {
    if values.len() < max && !values.contains(&value) {
        values.push(value);
    }
}

fn read_bounded(path: &std::path::Path, max_bytes: usize) -> Option<Vec<u8>> {
    let file = File::open(path).ok()?;
    if file.metadata().ok()?.len() > max_bytes as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= max_bytes).then_some(bytes)
}

/// Runs discovery with a hard wall-clock and output bound. The child starts a
/// fresh process group so descendants cannot retain stdout after the CLI exits.
fn bounded_command_stdout(program: &str, args: &[&str], max_bytes: usize) -> Option<Vec<u8>> {
    bounded_command_stdout_with_timeout(program, args, max_bytes, RUNNER_HELP_TIMEOUT)
}

fn bounded_command_stdout_with_timeout(
    program: &str,
    args: &[&str],
    max_bytes: usize,
    timeout: Duration,
) -> Option<Vec<u8>> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().ok()?;
    let process_group = child.id();
    let Some(mut stdout) = child.stdout.take() else {
        terminate_discovery_process(process_group, &mut child);
        let _ = child.wait();
        return None;
    };
    let (output_tx, output_rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 8_192];
        let result = loop {
            match stdout.read(&mut buffer) {
                Ok(0) => break Some(bytes),
                Ok(read) if bytes.len().saturating_add(read) <= max_bytes => {
                    bytes.extend_from_slice(&buffer[..read]);
                }
                Ok(_) | Err(_) => break None,
            }
        };
        let _ = output_tx.send(result);
    });

    let started = Instant::now();
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if started.elapsed() < timeout => {
                thread::sleep(Duration::from_millis(20));
            }
            Ok(None) | Err(_) => {
                terminate_discovery_process(process_group, &mut child);
                let _ = child.wait();
                break false;
            }
        }
    };
    // A successful direct child may still have spawned a descendant that owns
    // the pipe. Closing its isolated group makes the reader deadline real.
    terminate_discovery_group(process_group);
    let bytes = output_rx
        .recv_timeout(RUNNER_HELP_DRAIN_TIMEOUT)
        .ok()
        .flatten()?;
    success.then_some(bytes)
}

fn terminate_discovery_process(process_group: u32, child: &mut std::process::Child) {
    terminate_discovery_group(process_group);
    let _ = child.kill();
}

fn terminate_discovery_group(process_group: u32) {
    #[cfg(unix)]
    if let Ok(process_group) = i32::try_from(process_group) {
        // SAFETY: the command was placed in a fresh process group whose id is
        // its pid. A negative pid cannot target Aviary's own process group.
        unsafe {
            libc::kill(-process_group, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = process_group;
}

pub fn catalogue(runner: crate::runner::Runner) -> ModelCatalogue {
    match runner {
        crate::runner::Runner::ClaudeCode => claude_catalogue(),
        crate::runner::Runner::Codex => codex_catalogue(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wrapped_claude_model_examples_and_efforts_from_fixture_help() {
        let help = r#"
Options:
  --effort <level>                      Effort level for the current session
                                        (low, medium, high, xhigh, max)
  --fallback-model <model>              A fallback
  --model <model>                       Model for the current session. Provide
                                        an alias for the latest model (e.g.
                                        'fable', 'opus', or 'sonnet') or a
                                        model's full name (e.g.
                                        'claude-fable-5').
  --name <name>                         A name
"#;
        let parsed = parse_claude_help(help);
        assert_eq!(
            parsed.models,
            vec![
                AdvertisedModel {
                    id: "fable".to_string(),
                    is_alias: true,
                },
                AdvertisedModel {
                    id: "opus".to_string(),
                    is_alias: true,
                },
                AdvertisedModel {
                    id: "sonnet".to_string(),
                    is_alias: true,
                },
                AdvertisedModel {
                    id: "claude-fable-5".to_string(),
                    is_alias: false,
                },
            ]
        );
        assert_eq!(parsed.efforts, ["low", "medium", "high", "xhigh", "max"]);
    }

    #[test]
    fn parser_accepts_choices_format_without_baking_in_values() {
        let help = r#"
  --effort <level>  Reasoning (choices: "tiny", "deep-next")
  --model <model>   Pick an alias ('swift-next') or a model's full name ('vendor/model@7').
  --verbose         More output
"#;
        let parsed = parse_claude_help(help);
        assert_eq!(parsed.efforts, ["tiny", "deep-next"]);
        assert_eq!(parsed.models[0].id, "swift-next");
        assert!(parsed.models[0].is_alias);
        assert_eq!(parsed.models[1].id, "vendor/model@7");
        assert!(!parsed.models[1].is_alias);
    }

    #[test]
    fn configured_values_remain_visible_without_inventing_candidates() {
        let help = ClaudeHelp {
            models: vec![AdvertisedModel {
                id: "advertised".to_string(),
                is_alias: true,
            }],
            efforts: vec!["documented".to_string()],
        };
        let catalogue = build_claude_catalogue(
            Some(&help),
            ClaudeSettings {
                model: Some("vendor/new-model-42".to_string()),
                effort: Some("configured-next".to_string()),
            },
        );
        assert_eq!(
            catalogue.configured_default.as_deref(),
            Some("vendor/new-model-42")
        );
        assert!(catalogue
            .models
            .iter()
            .any(|model| model.id.as_deref() == Some("vendor/new-model-42")));
        let efforts = &catalogue.models[0].reasoning_levels;
        assert_eq!(
            efforts
                .iter()
                .map(|level| level.effort.as_str())
                .collect::<Vec<_>>(),
            ["documented", "configured-next"]
        );
    }

    #[test]
    fn configured_model_keeps_a_slot_when_discovery_hits_its_bound() {
        let help = ClaudeHelp {
            models: (0..MAX_MODELS)
                .map(|index| AdvertisedModel {
                    id: format!("advertised-{index}"),
                    is_alias: false,
                })
                .collect(),
            efforts: Vec::new(),
        };
        let catalogue = build_claude_catalogue(
            Some(&help),
            ClaudeSettings {
                model: Some("configured-current".to_string()),
                effort: None,
            },
        );
        assert_eq!(catalogue.models.len(), MAX_MODELS);
        assert!(catalogue
            .models
            .iter()
            .any(|model| model.id.as_deref() == Some("configured-current")));
    }

    #[test]
    fn identifiers_and_display_metadata_are_strictly_bounded() {
        assert!(model_id("model\n--dangerously-skip-permissions").is_none());
        assert!(model_id(&"x".repeat(MAX_MODEL_ID_CHARS + 1)).is_none());
        assert_eq!(
            model_id(" vendor/model@7 ").as_deref(),
            Some("vendor/model@7")
        );
        assert_eq!(display_text("hello\n\tworld\0", 20), "hello world");
        assert_eq!(display_text("abcdef", 3), "abc");
    }

    #[cfg(unix)]
    #[test]
    fn help_discovery_rejects_oversized_and_timed_out_commands() {
        assert!(bounded_command_stdout_with_timeout(
            "/bin/sh",
            &["-c", "printf 123456"],
            3,
            Duration::from_secs(1),
        )
        .is_none());
        let started = Instant::now();
        assert!(bounded_command_stdout_with_timeout(
            "/bin/sh",
            &["-c", "sleep 30"],
            1024,
            Duration::from_millis(100),
        )
        .is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn live_catalogues_stay_bounded_without_assuming_installed_model_names() {
        for catalogue in [claude_catalogue(), codex_catalogue()] {
            assert!(!catalogue.models.is_empty());
            assert!(catalogue.models.len() <= MAX_MODELS);
            assert_eq!(catalogue.models[0].id, None);
            for model in &catalogue.models {
                if let Some(id) = model.id.as_deref() {
                    assert_eq!(model_id(id).as_deref(), Some(id));
                }
                assert!(model.reasoning_levels.len() <= MAX_REASONING_LEVELS);
                for level in &model.reasoning_levels {
                    assert_eq!(
                        effort_id(&level.effort).as_deref(),
                        Some(level.effort.as_str())
                    );
                }
            }
            if let Some(configured) = catalogue.configured_default.as_deref() {
                assert!(catalogue
                    .models
                    .iter()
                    .any(|model| model.id.as_deref() == Some(configured)));
            }
        }
    }
}
