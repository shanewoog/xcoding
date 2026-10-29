//! Deterministic, recoverable compression for successful tool output.
//!
//! The stored tool message always remains the full, redacted output.  This
//! module only builds the smaller representation that is sent to the model.

use std::collections::BTreeSet;

use serde_json::Value;

/// Prefix used by the model-visible recovery header.
pub const RECOVERY_HEADER_PREFIX: &str = "[XCoding compressed tool output;";

const RUN_COMMAND_HEAD_LINES: usize = 16;
const RUN_COMMAND_TAIL_LINES: usize = 16;
const RUN_COMMAND_DIAGNOSTIC_LINES: usize = 16;
const RUN_COMMAND_LINE_CHARS: usize = 1_200;
const DIAGNOSTIC_KEYWORDS: [&str; 5] = ["error", "warning", "panic", "failed", "exception"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompressionOptions {
    pub enabled: bool,
    pub min_chars: usize,
    pub min_savings_percent: u32,
}

impl Default for CompressionOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            min_chars: 4_000,
            min_savings_percent: 20,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompressionResult {
    pub content: String,
    pub compressed: bool,
    pub strategy: Option<&'static str>,
}

impl CompressionResult {
    fn unchanged(content: &str) -> Self {
        Self {
            content: content.to_owned(),
            compressed: false,
            strategy: None,
        }
    }
}

/// Compress a freshly executed tool output for the model-facing request.
///
/// `content` is expected to be the already redacted, complete tool result.
/// The caller must persist that complete value before using this result.
pub fn compress_tool_output(
    tool_name: &str,
    content: &str,
    original_message_id: &str,
    options: &CompressionOptions,
) -> CompressionResult {
    if !options.enabled
        || original_message_id.trim().is_empty()
        || content.trim_start().starts_with(RECOVERY_HEADER_PREFIX)
    {
        return CompressionResult::unchanged(content);
    }

    let original_chars = content.chars().count();
    if original_chars < options.min_chars {
        return CompressionResult::unchanged(content);
    }

    let strategy = match tool_name {
        "run_command" => "run_command",
        "search_code" => "search_code",
        _ => return CompressionResult::unchanged(content),
    };

    let value = match serde_json::from_str::<Value>(content) {
        Ok(value) => value,
        Err(_) => return CompressionResult::unchanged(content),
    };
    let compacted = match strategy {
        "run_command" => compact_run_command(&value),
        "search_code" => compact_search_code(&value),
        _ => None,
    };
    let Some(compacted) = compacted.and_then(|value| serde_json::to_string(&value).ok()) else {
        return CompressionResult::unchanged(content);
    };

    finish(
        content,
        compacted,
        strategy,
        original_message_id,
        original_chars,
        options,
    )
}

/// Rebuild the model-facing form of a stored tool message.
///
/// The strategy is inferred from the JSON shape and success conditions are
/// checked again, so old failed or timed-out messages are never compressed.
pub fn compress_stored_tool_output(
    content: &str,
    message_id: &str,
    options: &CompressionOptions,
) -> CompressionResult {
    if !options.enabled
        || message_id.trim().is_empty()
        || content.trim_start().starts_with(RECOVERY_HEADER_PREFIX)
    {
        return CompressionResult::unchanged(content);
    }

    let original_chars = content.chars().count();
    if original_chars < options.min_chars {
        return CompressionResult::unchanged(content);
    }

    let value = match serde_json::from_str::<Value>(content) {
        Ok(value) => value,
        Err(_) => return CompressionResult::unchanged(content),
    };

    let (strategy, compacted) = if let Some(compacted) = compact_run_command(&value) {
        ("run_command", Some(compacted))
    } else if let Some(compacted) = compact_search_code(&value) {
        ("search_code", Some(compacted))
    } else {
        return CompressionResult::unchanged(content);
    };
    let Some(compacted) = compacted.and_then(|value| serde_json::to_string(&value).ok()) else {
        return CompressionResult::unchanged(content);
    };

    finish(
        content,
        compacted,
        strategy,
        message_id,
        original_chars,
        options,
    )
}

fn finish(
    original: &str,
    compacted: String,
    strategy: &'static str,
    message_id: &str,
    original_chars: usize,
    options: &CompressionOptions,
) -> CompressionResult {
    if compacted.chars().count() >= original_chars {
        return CompressionResult::unchanged(original);
    }

    let header = format!(
        "{RECOVERY_HEADER_PREFIX} message_id={message_id}; strategy={strategy}; original_chars={original_chars}; use history_read with message_id={message_id} to recover.]"
    );
    let content = format!("{header}\n{compacted}");
    let final_chars = content.chars().count();
    if final_chars >= original_chars {
        return CompressionResult::unchanged(original);
    }

    let saved = original_chars - final_chars;
    let min_savings = options.min_savings_percent.min(100) as usize;
    if saved * 100 < original_chars * min_savings {
        return CompressionResult::unchanged(original);
    }

    CompressionResult {
        content,
        compressed: true,
        strategy: Some(strategy),
    }
}

fn compact_run_command(value: &Value) -> Option<Value> {
    let object = value.as_object()?;
    if object.get("success").and_then(Value::as_bool) != Some(true)
        || object.get("exit_code").and_then(Value::as_i64) != Some(0)
        || object.get("timed_out").and_then(Value::as_bool) != Some(false)
    {
        return None;
    }

    let mut compacted = object.clone();
    let mut changed = false;
    for key in ["stdout", "stderr"] {
        if let Some(text) = object.get(key).and_then(Value::as_str) {
            let next = compact_stream(text);
            if next != text {
                changed = true;
                compacted.insert(key.to_owned(), Value::String(next));
            }
        }
    }
    changed.then_some(Value::Object(compacted))
}

fn compact_search_code(value: &Value) -> Option<Value> {
    let object = value.as_object()?;
    if object
        .get("success")
        .is_some_and(|value| value.as_bool() != Some(true))
        || object
            .get("timed_out")
            .is_some_and(|value| value.as_bool() != Some(false))
    {
        return None;
    }

    let results = object.get("results")?.as_array()?;
    if object.get("truncated")?.as_bool().is_none() {
        return None;
    }
    let stats = object.get("stats")?.as_object()?;
    for key in [
        "directories_scanned",
        "files_scanned",
        "bytes_scanned",
        "elapsed_ms",
    ] {
        stats.get(key)?.as_u64()?;
    }

    let mut compacted_results = Vec::with_capacity(results.len());
    for result in results {
        let result = result.as_object()?;
        result.get("path")?.as_str()?;
        result.get("line")?.as_u64()?;
        result.get("text")?.as_str()?;
        for key in ["before", "after"] {
            if let Some(value) = result.get(key) {
                value.as_array()?;
            }
        }

        let mut compacted = serde_json::Map::new();
        for key in ["path", "line", "text"] {
            compacted.insert(key.to_owned(), result.get(key)?.clone());
        }
        compacted_results.push(Value::Object(compacted));
    }

    let mut compacted = object.clone();
    compacted.insert("results".to_owned(), Value::Array(compacted_results));
    Some(Value::Object(compacted))
}

fn compact_stream(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return text.to_owned();
    }

    let has_long_line = lines
        .iter()
        .any(|line| line.chars().count() > RUN_COMMAND_LINE_CHARS);
    if lines.len() <= RUN_COMMAND_HEAD_LINES + RUN_COMMAND_TAIL_LINES && !has_long_line {
        return text.to_owned();
    }

    let mut selected = BTreeSet::new();
    for index in 0..lines.len().min(RUN_COMMAND_HEAD_LINES) {
        selected.insert(index);
    }
    let tail_start = lines.len().saturating_sub(RUN_COMMAND_TAIL_LINES);
    for index in tail_start..lines.len() {
        selected.insert(index);
    }

    let mut diagnostic_count = 0;
    let mut diagnostic_lines = BTreeSet::new();
    for (index, line) in lines.iter().enumerate() {
        if diagnostic_count >= RUN_COMMAND_DIAGNOSTIC_LINES {
            break;
        }
        if !is_diagnostic_line(line) || !diagnostic_lines.insert(line.to_owned()) {
            continue;
        }
        diagnostic_count += 1;
        selected.insert(index);
    }

    let mut output_lines = Vec::new();
    let mut previous = None;
    for index in selected {
        if let Some(previous) = previous {
            if index > previous + 1 {
                output_lines.push(format!("[... {} lines omitted ...]", index - previous - 1));
            }
        }
        output_lines.push(truncate_line(lines[index], RUN_COMMAND_LINE_CHARS));
        previous = Some(index);
    }
    if let Some(previous) = previous {
        if previous + 1 < lines.len() {
            output_lines.push(format!(
                "[... {} lines omitted ...]",
                lines.len() - previous - 1
            ));
        }
    }
    output_lines.join("\n")
}

fn is_diagnostic_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    DIAGNOSTIC_KEYWORDS
        .iter()
        .any(|keyword| lower.contains(keyword))
}

fn truncate_line(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        return line.to_owned();
    }
    let mut truncated = line.chars().take(max_chars).collect::<String>();
    truncated.push_str("... [line truncated]");
    truncated
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn options(min_chars: usize, min_savings_percent: u32) -> CompressionOptions {
        CompressionOptions {
            enabled: true,
            min_chars,
            min_savings_percent,
        }
    }

    fn long_run_output() -> String {
        let stdout = (0..200)
            .map(|index| format!("line {index}: ordinary output"))
            .collect::<Vec<_>>()
            .join("\n");
        serde_json::to_string(&json!({
            "executable": "cargo",
            "args": ["test"],
            "cwd": "D:/work",
            "success": true,
            "exit_code": 0,
            "stdout": stdout,
            "stderr": "warning: one\nordinary",
            "timed_out": false
        }))
        .expect("json")
    }

    fn long_search_output() -> String {
        let results = (0..50)
            .map(|index| {
                json!({
                    "path": format!("src/file{index}.rs"),
                    "line": index,
                    "text": "match",
                    "before": ["context ".repeat(50)],
                    "after": ["context ".repeat(50)]
                })
            })
            .collect::<Vec<_>>();
        serde_json::to_string(&json!({
            "results": results,
            "truncated": false,
            "stats": {
                "directories_scanned": 4,
                "files_scanned": 10,
                "bytes_scanned": 4096,
                "elapsed_ms": 1
            }
        }))
        .expect("json")
    }

    #[test]
    fn compresses_successful_run_command() {
        let content = long_run_output();
        let result = compress_tool_output(
            "run_command",
            &content,
            "message-1",
            &options(10, 1),
        );
        assert!(result.compressed);
        assert_eq!(result.strategy, Some("run_command"));
        assert!(result
            .content
            .starts_with("[XCoding compressed tool output;"));
        assert!(result.content.contains("strategy=run_command"));
        assert!(!result.content.contains("compressed_chars"));
        let compacted = result.content.split_once('\n').expect("header").1;
        let value: Value = serde_json::from_str(compacted).expect("json");
        assert_eq!(value["executable"], "cargo");
        assert_eq!(value["stdout"].as_str().unwrap().lines().count() > 0, true);
        assert!(value["stdout"].as_str().unwrap().contains("line 0"));
        assert!(value["stdout"].as_str().unwrap().contains("line 199"));
    }

    #[test]
    fn does_not_compress_failed_or_timed_out_run_command() {
        let stdout = "x\n".repeat(10_000);
        let failed = json!({
            "success": false,
            "exit_code": 1,
            "timed_out": false,
            "stdout": stdout,
        });
        let timed_out = json!({
            "success": true,
            "exit_code": 0,
            "timed_out": true,
            "stdout": stdout,
        });
        let contradictory_success = json!({
            "success": false,
            "exit_code": 0,
            "timed_out": false,
            "stdout": stdout,
        });
        for value in [failed, timed_out, contradictory_success] {
            let content = serde_json::to_string(&value).expect("json");
            let result = compress_tool_output(
                "run_command",
                &content,
                "message-1",
                &options(10, 1),
            );
            assert!(!result.compressed);
            assert_eq!(result.content, content);
        }
    }

    #[test]
    fn compresses_search_code_and_removes_context_arrays() {
        let content = long_search_output();
        let result = compress_tool_output(
            "search_code",
            &content,
            "message-2",
            &options(10, 1),
        );
        assert!(result.compressed);
        assert!(result.content.contains("strategy=search_code"));
        let compacted: Value =
            serde_json::from_str(result.content.split_once('\n').unwrap().1).expect("json");
        assert_eq!(compacted["truncated"], false);
        assert_eq!(compacted["stats"]["files_scanned"], 10);
        assert!(compacted["results"][0].get("before").is_none());
        assert!(compacted["results"][0].get("after").is_none());
    }

    #[test]
    fn compresses_stored_search_code_output() {
        let content = long_search_output();
        let result =
            compress_stored_tool_output(&content, "message-2", &options(10, 1));
        assert!(result.compressed);
        assert_eq!(result.strategy, Some("search_code"));
        assert!(result.content.contains("message_id=message-2"));
    }

    #[test]
    fn does_not_compress_failed_or_timed_out_search_code() {
        let base: Value = serde_json::from_str(&long_search_output()).expect("json");
        let mut failed = base.clone();
        failed["success"] = json!(false);
        let mut timed_out = base;
        timed_out["timed_out"] = json!(true);

        for value in [failed, timed_out] {
            let content = serde_json::to_string(&value).expect("json");
            let result = compress_tool_output(
                "search_code",
                &content,
                "message-2",
                &options(10, 1),
            );
            assert!(!result.compressed);
            assert_eq!(result.content, content);
        }
    }

    #[test]
    fn does_not_compress_non_success_search_code_shapes() {
        let base: Value = serde_json::from_str(&long_search_output()).expect("json");
        let mut missing_stats_field = base.clone();
        missing_stats_field["stats"]
            .as_object_mut()
            .unwrap()
            .remove("bytes_scanned");
        let mut missing_result_field = base.clone();
        missing_result_field["results"][0]
            .as_object_mut()
            .unwrap()
            .remove("text");
        let mut wrong_truncated_type = base.clone();
        wrong_truncated_type["truncated"] = json!("false");
        let mut wrong_context_type = base;
        wrong_context_type["results"][0]["before"] = json!({});

        for value in [
            missing_stats_field,
            missing_result_field,
            wrong_truncated_type,
            wrong_context_type,
        ] {
            let content = serde_json::to_string(&value).expect("json");
            let result = compress_tool_output(
                "search_code",
                &content,
                "message-2",
                &options(10, 1),
            );
            assert!(!result.compressed);
            assert_eq!(result.content, content);
        }
    }

    #[test]
    fn stored_message_rebuild_is_deterministic() {
        let content = long_search_output();
        let opts = options(10, 1);
        let first = compress_stored_tool_output(&content, "message-2", &opts);
        let second = compress_stored_tool_output(&content, "message-2", &opts);
        assert!(first.compressed);
        assert_eq!(first, second);
    }

    #[test]
    fn threshold_and_savings_fall_back_to_original() {
        let content = long_run_output();
        let below_min = compress_tool_output(
            "run_command",
            &content,
            "message-1",
            &options(content.chars().count() + 1, 1),
        );
        assert!(!below_min.compressed);

        let high_savings = compress_tool_output(
            "run_command",
            &content,
            "message-1",
            &options(10, 99),
        );
        assert!(!high_savings.compressed);
        assert_eq!(high_savings.content, content);
    }

    #[test]
    fn same_input_is_deterministic_and_not_double_compressed() {
        let content = long_run_output();
        let opts = options(10, 1);
        let first = compress_tool_output("run_command", &content, "message-1", &opts);
        let second = compress_tool_output("run_command", &content, "message-1", &opts);
        assert_eq!(first, second);
        let again = compress_tool_output("run_command", &first.content, "message-1", &opts);
        assert!(!again.compressed);
        assert_eq!(again.content, first.content);
    }

    #[test]
    fn malformed_or_unknown_output_is_unchanged() {
        let content = "not json ".repeat(1_000);
        let result = compress_tool_output(
            "run_command",
            &content,
            "message-1",
            &options(10, 1),
        );
        assert!(!result.compressed);
        assert_eq!(result.content, content);
        let unknown = compress_tool_output(
            "read_file",
            &content,
            "message-1",
            &options(10, 1),
        );
        assert!(!unknown.compressed);
    }

    #[test]
    fn original_chars_counts_unicode_characters() {
        let content = serde_json::to_string(&json!({
            "success": true,
            "exit_code": 0,
            "timed_out": false,
            "stdout": "汉字\n".repeat(2_000),
        }))
        .expect("json");
        let result = compress_tool_output(
            "run_command",
            &content,
            "message-1",
            &options(10, 1),
        );
        assert!(result.compressed);
        let expected = content.chars().count();
        assert!(result
            .content
            .contains(&format!("original_chars={expected};")));
    }
}
