//! Tests for the token-saving mods (file/webpage/screenshot/helper-chat compression).

use fennec_economy::{apply_token_savings, compact, Options};
use serde_json::json;
use std::collections::HashMap;

#[test]
fn file_detection_matches_line_numbered_code() {
    let content = "   1\tfn main() {\n   2\t    println!(\"Hello\");\n   3\t}\n";
    let (_, mod_type) = apply_token_savings(content, 1000);
    assert_eq!(mod_type, "file", "line-numbered code should be detected as file");
}

#[test]
fn file_detection_matches_diff_headers() {
    let content = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,5 +1,6 @@\n use std::io;\n+use std::fs;\n";
    let (_, mod_type) = apply_token_savings(content, 1000);
    assert_eq!(mod_type, "file", "diff headers should be detected as file");
}

#[test]
fn file_detection_rejects_plain_text() {
    let content = "This is just a regular text response with some {braces} and function mentions.";
    let (_, mod_type) = apply_token_savings(content, 1000);
    assert_ne!(mod_type, "file", "plain text should not be detected as file");
}

#[test]
fn file_detection_rejects_json_without_structure() {
    let content = r#"{"key": "value", "number": 42}"#;
    let (_, mod_type) = apply_token_savings(content, 1000);
    assert_ne!(mod_type, "file", "single-line JSON should not be detected as file");
}

#[test]
fn webpage_detection_matches_html_document() {
    let content = "<!DOCTYPE html>\n<html>\n<head><title>Test</title></head>\n<body><p>Content</p></body>\n</html>";
    let (_, mod_type) = apply_token_savings(content, 1000);
    assert_eq!(mod_type, "webpage", "HTML document should be detected as webpage");
}

#[test]
fn webpage_detection_matches_structured_html() {
    let content = "<html>\n<head><title>Test</title></head>\n<body>\n<div><p>Content</p></div>\n</body>\n</html>";
    let (_, mod_type) = apply_token_savings(content, 1000);
    assert_eq!(mod_type, "webpage", "structured HTML should be detected as webpage");
}

#[test]
fn webpage_detection_rejects_url_in_text() {
    let content = "Check out https://example.com for more info.";
    let (_, mod_type) = apply_token_savings(content, 1000);
    assert_ne!(mod_type, "webpage", "URL in text should not be detected as webpage");
}

#[test]
fn webpage_detection_rejects_partial_html() {
    let content = "Use <div> elements for layout.";
    let (_, mod_type) = apply_token_savings(content, 1000);
    assert_ne!(mod_type, "webpage", "HTML tag mention should not be detected as webpage");
}

#[test]
fn screenshot_detection_matches_explicit_marker() {
    let content = "[Screenshot: Desktop app showing the main window]";
    let (_, mod_type) = apply_token_savings(content, 1000);
    assert_eq!(mod_type, "screenshot", "explicit screenshot marker should be detected");
}

#[test]
fn screenshot_detection_matches_screenshot_prefix() {
    let content = "Screenshot of the login page showing the form fields and submit button.";
    let (_, mod_type) = apply_token_savings(content, 1000);
    assert_eq!(mod_type, "screenshot", "screenshot description should be detected");
}

#[test]
fn screenshot_detection_matches_base64_image() {
    let content = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    let (_, mod_type) = apply_token_savings(content, 1000);
    assert_eq!(mod_type, "screenshot", "base64 image data should be detected");
}

#[test]
fn screenshot_detection_rejects_casual_mention() {
    let content = "The app shows a login form with username and password fields.";
    let (_, mod_type) = apply_token_savings(content, 1000);
    assert_ne!(mod_type, "screenshot", "casual 'shows a' should not be detected as screenshot");
}

#[test]
fn helper_chat_detection_matches_conversation_format() {
    let content = "User: How do I fix this bug?\nAssistant: Try restarting the service.\nUser: That worked, thanks!";
    let (_, mod_type) = apply_token_savings(content, 1000);
    assert_eq!(mod_type, "helper_chat", "multi-turn conversation should be detected");
}

#[test]
fn helper_chat_detection_matches_qa_format() {
    let content = "Q: What is the return type?\nA: It returns a Result<String, Error>.\nQ: Does it handle timeouts?\nA: Yes, with a 30s default.";
    let (_, mod_type) = apply_token_savings(content, 1000);
    assert_eq!(mod_type, "helper_chat", "Q&A format should be detected");
}

#[test]
fn helper_chat_detection_rejects_single_mention() {
    let content = "The user: assistant ratio is 1:1 in this test.";
    let (_, mod_type) = apply_token_savings(content, 1000);
    assert_ne!(mod_type, "helper_chat", "single mention should not be detected as conversation");
}

#[test]
fn helper_chat_detection_rejects_short_text() {
    let content = "User: hello\nAssistant: hi";
    let (_, mod_type) = apply_token_savings(content, 1000);
    assert_ne!(mod_type, "helper_chat", "two lines is too short to be a conversation");
}

#[test]
fn file_summarization_reduces_large_content() {
    let mut content = String::new();
    for i in 1..=100 {
        content.push_str(&format!("   {}\tfn function_{}() {{\n", i, i));
        content.push_str(&format!("   {}\t    // Implementation line 1\n", i + 100));
        content.push_str(&format!("   {}\t    // Implementation line 2\n", i + 200));
        content.push_str(&format!("   {}\t}}\n", i + 300));
    }
    let (summarized, mod_type) = apply_token_savings(&content, 1000);
    assert_eq!(mod_type, "file");
    assert!(summarized.len() < content.len(), "summarized file should be smaller");
    assert!(summarized.contains("elided"), "summary should contain elision marker");
}

#[test]
fn webpage_extraction_removes_html_tags() {
    let content = "<html><head><title>Test</title><style>body { color: red; }</style></head><body><p>Hello world</p><script>alert('hi');</script></body></html>";
    let (extracted, mod_type) = apply_token_savings(content, 1000);
    assert_eq!(mod_type, "webpage");
    assert!(!extracted.contains("<p>"), "HTML tags should be removed");
    assert!(!extracted.contains("<script>"), "script tags should be removed");
    assert!(!extracted.contains("<style>"), "style tags should be removed");
    assert!(extracted.contains("Hello world"), "text content should be preserved");
}

#[test]
fn screenshot_summarization_keeps_description() {
    let mut content = String::from("Screenshot: Desktop app showing the main window with three panels.\n");
    // Add enough detail lines that summarization actually shrinks it
    for i in 0..20 {
        content.push_str(&format!("Detail line {}: The left panel shows the file tree with {} entries.\n", i, i * 3));
    }
    let (summarized, mod_type) = apply_token_savings(&content, 200);
    assert_eq!(mod_type, "screenshot");
    assert!(summarized.contains("Screenshot:"), "description should be preserved");
    assert!(summarized.len() < content.len(), "summarized should be shorter than input");
}

#[test]
fn helper_chat_summarization_extracts_key_pairs() {
    let mut content = String::new();
    // Create a conversation long enough that summarization shrinks it
    for i in 0..15 {
        content.push_str(&format!("User: Question number {} about the codebase?\n", i));
        content.push_str(&format!("Assistant: Here is the answer to question {} with some details about the implementation.\n", i));
    }
    let (summarized, mod_type) = apply_token_savings(&content, 500);
    assert_eq!(mod_type, "helper_chat");
    assert!(summarized.contains("[Helper chat summary]"), "should have summary marker");
    assert!(summarized.len() < content.len(), "summarized should be shorter than input");
}

#[test]
fn compact_applies_token_savings_to_tool_results() {
    let messages = vec![
        json!({"role": "user", "content": "Read the file"}),
        json!({"role": "tool", "tool_call_id": "call_1", "content": "   1\tfn main() {\n   2\t    println!(\"Hello\");\n   3\t}\n".repeat(50)}),
    ];
    let options = Options::sized(10000, 5000);
    let (payload, stats) = compact(&messages, &options, &mut HashMap::new());
    assert_eq!(stats.files_summarized, 1, "file content should be summarized");
    let tool_content = payload[1]["content"].as_str().unwrap();
    assert!(tool_content.len() < messages[1]["content"].as_str().unwrap().len(),
        "tool result should be compressed");
}

#[test]
fn compact_tracks_all_mod_types_in_stats() {
    let messages = vec![
        json!({"role": "tool", "tool_call_id": "1", "content": "<!DOCTYPE html><html><head><title>T</title></head><body><p>Content</p></body></html>"}),
        json!({"role": "tool", "tool_call_id": "2", "content": "[Screenshot: Main window]"}),
        json!({"role": "tool", "tool_call_id": "3", "content": "User: Q1\nAssistant: A1\nUser: Q2\nAssistant: A2"}),
    ];
    let options = Options::sized(10000, 5000);
    let (_, stats) = compact(&messages, &options, &mut HashMap::new());
    assert_eq!(stats.webpages_extracted, 1);
    assert_eq!(stats.screenshots_summarized, 1);
    assert_eq!(stats.helper_chats_summarized, 1);
    assert_eq!(stats.token_savings, 3);
}

#[test]
fn no_false_positives_on_normal_tool_results() {
    let messages = vec![
        json!({"role": "tool", "tool_call_id": "1", "content": "File written successfully: 42 bytes"}),
        json!({"role": "tool", "tool_call_id": "2", "content": "Found 3 matches in src/lib.rs"}),
        json!({"role": "tool", "tool_call_id": "3", "content": "Command exited with code 0"}),
    ];
    let options = Options::sized(10000, 5000);
    let (_, stats) = compact(&messages, &options, &mut HashMap::new());
    assert_eq!(stats.token_savings, 0, "normal tool results should not trigger mods");
}

#[test]
fn results_that_summarise_alike_stay_distinct() {
    // The file mod keeps the head and tail lines and drops the middle, so these two
    // reads compress to the same text while being different results. Deduplicating on
    // the summarised text would report the second as "identical to" a file it is not.
    let shared_head: Vec<String> = (1..=6).map(|n| format!("{n:4}\tshared line {n}")).collect();
    let shared_tail: Vec<String> = (17..=20).map(|n| format!("{n:4}\tshared tail {n}")).collect();
    let build = |body: &str| -> String {
        let mut lines = shared_head.clone();
        lines.extend((7..=16).map(|n| format!("{n:4}\t{body} {n}")));
        lines.extend(shared_tail.clone());
        lines.join("\n")
    };
    let (alpha, beta) = (build("alpha"), build("beta"));
    assert_ne!(alpha, beta, "the two results differ in the elided middle");
    assert_eq!(apply_token_savings(&alpha, 5000).0, apply_token_savings(&beta, 5000).0,
        "and the premise of the test is that they summarise to one text");
    let messages = vec![
        json!({"role": "tool", "tool_call_id": "call_alpha", "content": alpha}),
        json!({"role": "tool", "tool_call_id": "call_beta", "content": beta}),
    ];
    let (payload, stats) = compact(&messages, &Options::sized(10000, 5000), &mut HashMap::new());
    assert_eq!(stats.deduped, 0, "different results are not repeats of each other");
    assert_eq!(stats.files_summarized, 2);
    for index in 1..=2 {
        let text = payload[index - 1]["content"].as_str().unwrap();
        assert!(!text.starts_with("[identical to"), "{text}");
    }
}

#[test]
fn a_repeated_result_is_still_recognised_after_the_mod() {
    let content = (1..=40).map(|n| format!("{n:4}\tfn body() {{ {n} }}")).collect::<Vec<_>>().join("\n");
    let messages = vec![
        json!({"role": "tool", "tool_call_id": "call_1", "content": content.clone()}),
        json!({"role": "tool", "tool_call_id": "call_2", "content": content.clone()}),
    ];
    let (payload, stats) = compact(&messages, &Options::sized(10000, 5000), &mut HashMap::new());
    assert_eq!(stats.deduped, 1, "the second identical read is a repeat");
    let label = payload[1]["content"].as_str().unwrap();
    assert!(label.starts_with("[identical to the result of call_1; "), "{label}");
    // The count is of the raw result, as in context.py; summarising first would quote
    // the length of the summary rather than the bytes the caller stopped resending.
    assert!(label.contains(&format!("{} chars", content.chars().count())), "{label}");
}
