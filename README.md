# fennec-economy

Agent context economy for the Fennec suite. Compacts the transcript sent to the model so prefill stays cheap.

## What it does

- **Head/tail elision** — long tool results keep their beginning and end, with a marker for the removed middle
- **Duplicate detection** — identical results become a short reference to the first occurrence
- **Character budget** — shrinks the oldest tool results first, never the system prompt or user message
- **Token-saving mods** — detects and compresses file reads, web pages, screenshots, and helper-chat transcripts before elision

## Token-saving mods

| Type | Detection | Compression |
|------|-----------|-------------|
| File | Line-numbered code, diff headers, path+code blocks | First 30% + last 20% of lines |
| Web page | `<!DOCTYPE`, `<html>`, structural HTML tags | Strip tags, remove script/style, keep text |
| Screenshot | `[image`, `[screenshot`, base64 image data | Keep first 5 description lines |
| Helper chat | Multi-turn `user:`/`assistant:` format | Extract Q&A pairs |

## Dependency

Depends on `fennec-pyjson` (path dependency to `../fennec-core/src/fennec-pyjson`) for Python-compatible JSON character counting.

## Tests

Golden tests in `tests/python_golden.rs` verify byte-for-byte agreement with the Python fallback in `fennec-core/context.py`. The goldens live in `goldens/` (copied from `fennec-core/src/goldens/`).

```
cargo test
```

## Used by

- `fennec-cli` — the `fennec-rs serve` bridge delegates `compact` here
- `fennec-desktop` — context budgeting in the Rust shell
- `shell-bench` — chromium-probe and tauri-probe measure economy overhead
