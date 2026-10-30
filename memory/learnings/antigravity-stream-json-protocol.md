---
title: Antigravity's structured mode (stream-json): what it really emits and expects
summary: Checked on agy 1.4.10 — stream input requires {"event":"user","message":{"content":...}}, stdout uses {"event":...}, permission mode defaults to always-proceed, turns cut via SIGINT to process group.
touches: [crates/core/src/chat.rs, crates/core/src/agents.rs, crates/core/src/process.rs, crates/core/tests/data/antigravity-stream-*.jsonl]
---
Recorded 2026-10-29 with `agy --input-format stream-json --output-format stream-json --mode auto` (samples in `crates/core/tests/data/antigravity-stream-*.jsonl`):

- **Input protocol:** Every line sent on stdin must be `{"event":"user","message":{"content":"<prompt>"}}`. If the `"event"` field is missing, `agy` aborts with `stream input message is missing the "event" field`. If `message` is a plain string instead of an object, it errors with `cannot unmarshal string into StreamInputUserMessage`.
- **Output protocol:** Lines on stdout have a top-level `"event"` field (distinct from Claude's top-level `"type"`):
  - `{"event":"init","conversation_id":"<uuid>","init":{"cwd":"...","tools":[...],"permission_mode":"always-proceed"}}`: Emitted once at launch. The session ID is available immediately from this event.
  - `{"event":"step_update","step_update":{"conversation_id":"...","step_index":N,"state":"ACTIVE"|"DONE"|"ERROR","step_type":"agent_response"|"tool"|"user_input",...}}`: Text arrives via `text_delta` fields on `agent_response`. Tool calls arrive with `tool_name` and `tool_info.parameters` (which only contains `Path` for file operations). Tool errors come as `state: "ERROR"` with `tool_info.error`.
  - `{"event":"result","result":{"conversation_id":"...","status":"SUCCESS"|"ERROR","duration_seconds":...,"usage":{...}}}`: Marks the end of a turn.
- **Permissions:** In headless `stream-json` mode, `agy` runs with `permission_mode: "always-proceed"` and does not issue interactive stdio permission requests.
- **Interrupt:** `agy` does not accept a JSON interrupt message on stdin; turns are interrupted by sending `SIGINT` to the child's process group (`libc::kill(-pid, SIGINT)`). `agy` stays alive and cleanly emits a `result` event with `status: "ERROR"`, `error: "interrupted"`.
- **History transcripts:** Transcripts are written to `<home>/.gemini/antigravity-cli/brain/<session_id>/.system_generated/logs/transcript.jsonl`. Tool calls appear under `PLANNER_RESPONSE` with JSON-quoted arguments (`{"Path":"\"...\"","Target":"\"...\"","Replacement":"\"...\""}`), followed by `GENERIC` tool execution outputs.
- **Resuming:** Previous sessions are resumed with `agy --conversation <session_id>`. In chat mode, `--log-file` is omitted because the conversation id is provided directly in `init`.

**Why:** `agy` headless stream mode has undocumented JSON envelope requirements and signal semantics that differ fundamentally from Claude Code.
**How to apply:** Keep `parse_line` checking `v.get("type").is_none() && v.get("event").is_some()`; ensure interrupts target the process group; check parser against sample recordings with `cargo test -p holt-core chat::tests`.
