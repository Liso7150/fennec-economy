//! Context economy for the agent loop - the Rust port of
//! `fennec_core/context.py`. Prefill dominates a turn on the Fennec engine, so
//! every character the loop re-sends each step costs real seconds. The UI keeps
//! the full transcript; only the payload sent to the model is compacted:
//! head/tail elision per result, duplicate results replaced by a reference, and
//! a total budget that shrinks the oldest tool results first.
//!
//! The rules, defaults and the exact strings are the Python module's, and the
//! golden tests in `tests/python_golden.rs` fail if this ever drifts.

use fennec_pyjson::char_len;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub const DEFAULT_BUDGET_CHARS: usize = 48_000;
pub const DEFAULT_RESULT_CHARS: usize = 6_000;
pub const MIN_RESULT_CHARS: usize = 600;
pub const MIN_BUDGET_CHARS: usize = 2_000;
const MARKER_HEAD: &str = "\n…[";
const MARKER_TAIL: &str = " chars elided; narrow the read or grep for the rest]\n";
/// head + tail + the marker without its digits: `elide` budgets 80 characters
/// of it, exactly as the Python module does.
const MARKER_COST: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub budget_chars: usize,
    pub result_chars: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub chars_before: usize,
    pub chars_after: usize,
    pub elided: usize,
    pub deduped: usize,
    pub token_savings: usize,
    pub files_summarized: usize,
    pub webpages_extracted: usize,
    pub screenshots_summarized: usize,
    pub helper_chats_summarized: usize,
    pub budget_chars: usize,
    pub result_chars: usize,
}

fn positive_int(raw: Option<String>, default: usize, floor: usize) -> usize {
    let Some(text) = raw else { return default };
    match text.trim().parse::<i64>() {
        Ok(parsed) if parsed > 0 => floor.max(parsed as usize),
        _ => default,
    }
}

pub fn limits_in(env: &dyn Fn(&str) -> Option<String>) -> Limits {
    let result = positive_int(
        env("FENNEC_AGENT_RESULT_CHARS"),
        DEFAULT_RESULT_CHARS,
        MIN_RESULT_CHARS,
    );
    let budget = positive_int(
        env("FENNEC_AGENT_CONTEXT_BUDGET"),
        DEFAULT_BUDGET_CHARS,
        MIN_BUDGET_CHARS,
    );
    Limits { budget_chars: budget.max(result), result_chars: result }
}

pub fn limits() -> Limits {
    limits_in(&|key| std::env::var(key).ok())
}

/// Keep the head and the tail of `text`, marking what was removed.
pub fn elide(text: &str, limit: usize) -> String {
    let length = text.chars().count();
    if length <= limit {
        return text.to_string();
    }
    let head = ((limit * 2) / 3).max(1);
    let tail = limit.saturating_sub(head + MARKER_COST);
    let removed = length - head - tail;
    let mut out = String::with_capacity(limit + MARKER_COST);
    out.extend(text.chars().take(head));
    out.push_str(MARKER_HEAD);
    out.push_str(&removed.to_string());
    out.push_str(MARKER_TAIL);
    if tail > 0 {
        out.extend(text.chars().skip(length - tail));
    }
    out
}

fn py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().map_or(true, |f| f != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(object) => !object.is_empty(),
    }
}

/// `len(content) + len(json.dumps(tool_calls, ensure_ascii=False))`
pub fn size(message: &Value) -> usize {
    let content = message.get("content").and_then(Value::as_str).map(str::chars).map(Iterator::count).unwrap_or(0);
    match message.get("tool_calls") {
        Some(calls) if py_truthy(calls) => content + char_len(calls),
        _ => content,
    }
}

pub fn compact(messages: &[Value], options: &Options, seen: &mut HashMap<String, String>) -> (Vec<Value>, Stats) {
    let before: usize = messages.iter().map(size).sum();
    let mut payload: Vec<Value> = Vec::with_capacity(messages.len());
    let mut elided = 0;
    let mut deduped = 0;
    let mut files_summarized = 0;
    let mut webpages_extracted = 0;
    let mut screenshots_summarized = 0;
    let mut helper_chats_summarized = 0;

    for message in messages {
        let mut copy = message.clone();
        let content = copy.get("content").and_then(Value::as_str).map(str::to_string);
        let is_tool = copy.get("role").and_then(Value::as_str) == Some("tool");
        if let (true, Some(text)) = (is_tool, content) {
            if !text.is_empty() {
                // Apply token-saving mods first
                let (optimized, mod_type) = apply_token_savings(&text, options.result_chars * 2);
                match mod_type {
                    "file" => files_summarized += 1,
                    "webpage" => webpages_extracted += 1,
                    "screenshot" => screenshots_summarized += 1,
                    "helper_chat" => helper_chats_summarized += 1,
                    _ => {}
                }
                
                let mut hasher = Sha256::new();
                // The digest is of the raw result, as in `fennec_core.context.compact`.
                // Summarising first would collapse two different tool results that
                // happen to compress to the same text, and the second would be dropped
                // as "identical to" a call it has nothing to do with.
                hasher.update(text.as_bytes());
                let digest = format!("{:x}", hasher.finalize());
                let label = copy
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .unwrap_or("an earlier call")
                    .to_string();
                if let Some(first) = seen.get(&digest) {
                    let replaced = format!(
                        "[identical to the result of {first}; {} chars not repeated]",
                        text.chars().count()
                    );
                    set_content(&mut copy, &replaced);
                    deduped += 1;
                } else {
                    seen.insert(digest, label);
                    let capped = elide(&optimized, options.result_chars);
                    if capped != optimized {
                        set_content(&mut copy, &capped);
                        elided += 1;
                    } else if optimized != text {
                        // Token savings were applied
                        set_content(&mut copy, &optimized);
                    }
                }
            }
        }
        payload.push(copy);
    }

    let mut total: usize = payload.iter().map(size).sum();
    for message in &mut payload {
        if total <= options.budget_chars {
            break;
        }
        let content = match message.get("content").and_then(Value::as_str) {
            Some(text) if text.chars().count() > options.floor_chars => text.to_string(),
            _ => continue,
        };
        if message.get("role").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        let length = content.chars().count();
        let overshoot = total.saturating_sub(options.budget_chars);
        let target = options.floor_chars.max(length.saturating_sub(overshoot));
        if target >= length {
            continue;
        }
        let shrunk = elide(&content, target);
        let old_size = size(message);
        set_content(message, &shrunk);
        total = total - old_size + size(message);
    }
    
    let token_savings = files_summarized + webpages_extracted + screenshots_summarized + helper_chats_summarized;

    (
        payload,
        Stats {
            chars_before: before,
            chars_after: total,
            elided,
            deduped,
            token_savings,
            files_summarized,
            webpages_extracted,
            screenshots_summarized,
            helper_chats_summarized,
            budget_chars: options.budget_chars,
            result_chars: options.result_chars,
        },
    )
}

fn set_content(message: &mut Value, text: &str) {
    if let Some(object) = message.as_object_mut() {
        object.insert("content".to_string(), Value::String(text.to_string()));
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub budget_chars: usize,
    pub result_chars: usize,
    pub floor_chars: usize,
}

impl Default for Options {
    fn default() -> Self {
        let limits = limits();
        Options {
            budget_chars: limits.budget_chars,
            result_chars: limits.result_chars,
            floor_chars: MIN_RESULT_CHARS,
        }
    }
}

impl Options {
    pub fn sized(budget_chars: usize, result_chars: usize) -> Self {
        Options { budget_chars, result_chars, floor_chars: MIN_RESULT_CHARS }
    }
}

// Token-saving mods: detect and compress specific content types to reduce token usage.
// These run on tool results before elision/dedup in `compact()`.

/// Detect if content looks like a file read result.
/// Matches line-numbered code (the format `read_file` returns), diff headers,
/// or multi-line code blocks with a path header — not arbitrary text with braces.
fn looks_like_file(content: &str) -> bool {
    let trimmed = content.trim_start();
    // Line-numbered file read: "   1\tfn main() {" or "1 | fn main() {"
    let first_line = trimmed.lines().next().unwrap_or("");
    if first_line.trim_start().starts_with(|c: char| c.is_ascii_digit())
        && (first_line.contains('\t') || first_line.contains(" | ") || first_line.contains(": "))
    {
        return true;
    }
    // Diff header: "--- a/path" / "+++ b/path" / "diff --git"
    if trimmed.starts_with("--- a/") || trimmed.starts_with("+++ b/") || trimmed.starts_with("diff --git") {
        return true;
    }
    // Path header followed by code: "path/to/file.rs" or "```language"
    if trimmed.starts_with("```") && trimmed.lines().count() > 3 {
        return true;
    }
    // File content with a clear path on the first line
    if let Some(first) = trimmed.lines().next() {
        let f = first.trim();
        if (f.starts_with('/') || f.starts_with("./") || f.contains('\\'))
            && f.contains('.')
            && trimmed.lines().count() > 5
        {
            return true;
        }
    }
    false
}

/// Summarize a file by keeping only the structure and key parts.
fn summarize_file(content: &str, max_chars: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    // Only summarize if there are enough lines to make it worthwhile
    if lines.len() < 20 {
        return content.to_string();
    }
    
    let mut summary = String::with_capacity(max_chars);
    
    // Keep first 30% and last 20% of lines, with a marker in between
    let keep_start = (lines.len() as f32 * 0.3) as usize;
    let keep_end = (lines.len() as f32 * 0.2) as usize;
    let removed = lines.len() - keep_start - keep_end;
    
    for line in lines.iter().take(keep_start) {
        summary.push_str(line);
        summary.push('\n');
    }
    
    summary.push_str(&format!("\n…[{} lines elided; file summarized to save tokens]\n\n", removed));
    
    for line in lines.iter().skip(lines.len() - keep_end) {
        summary.push_str(line);
        summary.push('\n');
    }
    
    // Truncate if still too long
    if summary.chars().count() > max_chars {
        elide(&summary, max_chars)
    } else {
        summary
    }
}

/// Detect if content looks like a web page (actual HTML document, not just a URL in text).
fn looks_like_webpage(content: &str) -> bool {
    let trimmed = content.trim();
    // HTML document type or root element
    trimmed.starts_with("<!DOCTYPE") || trimmed.starts_with("<!doctype") ||
    trimmed.starts_with("<html") || trimmed.starts_with("<HTML") ||
    // Contains multiple HTML structural tags (not just one URL mention)
    (trimmed.contains("<head") && trimmed.contains("<body")) ||
    (trimmed.contains("<div") && trimmed.contains("</div>") && trimmed.contains("<p")) ||
    // A full HTML page with opening and closing tags
    (trimmed.contains("<html") && trimmed.contains("</html>"))
}

/// Extract key content from a web page by removing HTML tags and keeping text.
fn extract_webpage_content(content: &str, max_chars: usize) -> String {
    // Strip HTML tags character-by-character, handling single-line and multi-line HTML
    let mut text = String::with_capacity(content.len());
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let lower = content.to_lowercase();
    
    let mut i = 0;
    while i < content.len() {
        // Check for script/style boundaries in the lowercased view
        let remaining_lower = &lower[i..];
        if remaining_lower.starts_with("<script") {
            in_script = true;
            // Skip to end of this tag
            if let Some(end) = content[i..].find('>') {
                i += end + 1;
                continue;
            }
        }
        if remaining_lower.starts_with("</script>") {
            in_script = false;
            i += 9;
            continue;
        }
        if remaining_lower.starts_with("<style") {
            in_style = true;
            if let Some(end) = content[i..].find('>') {
                i += end + 1;
                continue;
            }
        }
        if remaining_lower.starts_with("</style>") {
            in_style = false;
            i += 8;
            continue;
        }
        
        if in_script || in_style {
            i += 1;
            continue;
        }
        
        let ch = content[i..].chars().next().unwrap();
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
            text.push(' ');
        } else if !in_tag {
            text.push(ch);
        }
        i += ch.len_utf8();
    }
    
    // Clean up whitespace
    let cleaned: String = text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    
    // Collapse multiple spaces into one
    let mut result = String::with_capacity(cleaned.len());
    let mut prev_space = false;
    for ch in cleaned.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                result.push(' ');
            }
            prev_space = true;
        } else {
            result.push(ch);
            prev_space = false;
        }
    }
    
    if result.chars().count() > max_chars {
        elide(&result, max_chars)
    } else {
        result
    }
}

/// Detect if content is a screenshot or image description.
/// Matches explicit image markers, base64 image data, or structured screenshot
/// descriptions — not arbitrary text that happens to mention "shows".
fn looks_like_screenshot(content: &str) -> bool {
    let trimmed = content.trim();
    let lower = trimmed.to_lowercase();
    // Explicit image markers from tool results
    lower.starts_with("[image") || lower.starts_with("[screenshot") ||
    lower.starts_with("screenshot:") || lower.starts_with("screenshot of") ||
    lower.starts_with("image:") ||
    // Base64-encoded image data
    lower.contains("data:image/") ||
    // Structured screenshot description: "Screenshot of <thing> showing <details>"
    (lower.starts_with("screenshot") && lower.contains("showing")) ||
    // Image file reference with dimensions
    (lower.contains(".png") && lower.contains("×")) ||
    (lower.contains(".jpg") && lower.contains("×")) ||
    (lower.contains(".jpeg") && lower.contains("×"))
}

/// Summarize a screenshot by extracting the key description.
fn summarize_screenshot(content: &str, max_chars: usize) -> String {
    // Keep only the first few lines that describe the screenshot
    let lines: Vec<&str> = content.lines().collect();
    let mut summary = String::new();
    
    for line in lines.iter().take(5) {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            summary.push_str(trimmed);
            summary.push(' ');
        }
    }
    
    if summary.is_empty() {
        "[Screenshot content omitted to save tokens]".to_string()
    } else {
        let omitted = content.chars().count().saturating_sub(summary.chars().count());
        if omitted > 100 {
            summary.push_str(&format!(" [{} chars of screenshot details omitted]", omitted));
        }
        if summary.chars().count() > max_chars {
            elide(&summary, max_chars)
        } else {
            summary
        }
    }
}

/// Detect if content is a helper chat or side conversation transcript.
/// Requires structured multi-turn conversation format, not just a single mention
/// of "user:" or "assistant:" in a log line.
fn looks_like_helper_chat(content: &str) -> bool {
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() < 3 {
        return false;
    }
    // Count structured conversation markers
    let mut turn_count = 0;
    for line in &lines {
        let trimmed = line.trim().to_lowercase();
        if trimmed.starts_with("user:") || trimmed.starts_with("assistant:") ||
           trimmed.starts_with("human:") || trimmed.starts_with("ai:") ||
           trimmed.starts_with("q:") || trimmed.starts_with("question:") ||
           trimmed.starts_with("a:") || trimmed.starts_with("answer:") ||
           trimmed.starts_with("[conversation") || trimmed.starts_with("[chat") {
            turn_count += 1;
        }
    }
    // Need at least 2 turns (one question + one answer) to be a conversation
    turn_count >= 2
}

/// Summarize a helper chat by keeping only the key Q&A pairs.
fn summarize_helper_chat(content: &str, max_chars: usize) -> String {
    // Extract lines that look like questions or answers
    let lines: Vec<&str> = content.lines().collect();
    let mut key_lines = Vec::new();
    
    for line in &lines {
        let trimmed = line.trim();
        let lower = trimmed.to_lowercase();
        if lower.starts_with("q:") || 
           lower.starts_with("question:") ||
           lower.starts_with("a:") ||
           lower.starts_with("answer:") ||
           lower.starts_with("user:") ||
           lower.starts_with("assistant:") ||
           lower.contains('?') {
            key_lines.push(trimmed);
        }
    }
    
    if key_lines.is_empty() {
        // Fallback: keep first and last parts
        elide(content, max_chars)
    } else {
        let mut summary = String::from("[Helper chat summary]\n");
        for line in key_lines.iter().take(10) {
            summary.push_str(line);
            summary.push('\n');
        }
        if summary.chars().count() > max_chars {
            elide(&summary, max_chars)
        } else {
            summary
        }
    }
}

/// Apply token-saving mods to content based on detected type.
pub fn apply_token_savings(content: &str, max_chars: usize) -> (String, &'static str) {
    // Check in order of specificity
    if looks_like_screenshot(content) {
        (summarize_screenshot(content, max_chars), "screenshot")
    } else if looks_like_helper_chat(content) {
        (summarize_helper_chat(content, max_chars), "helper_chat")
    } else if looks_like_webpage(content) {
        (extract_webpage_content(content, max_chars), "webpage")
    } else if looks_like_file(content) {
        (summarize_file(content, max_chars), "file")
    } else {
        (content.to_string(), "none")
    }
}
