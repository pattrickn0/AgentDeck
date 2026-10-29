---
title: Claude Code's structured mode (stream-json): what it really emits and expects
summary: Checked on 2.1.312 — init comes every turn, permissions arrive as control_request on stdout and are answered on stdin, no user echo, thinking text is empty, /commands work, interrupt is a control_request.
touches: [crates/core/src/chat.rs, crates/core/tests/data/**, crates/core/src/agents.rs]
---
Recorded 2026-10-29 with `claude -p --input-format stream-json --output-format stream-json --include-partial-messages --verbose
--permission-prompt-tool stdio` (samples in `crates/core/tests/data/claude-stream-*.jsonl`):

- `system/init` is emitted at the start of **every** turn (not once), with `session_id`, `model`, `slash_commands` and
  `terminal_slash_commands` (`doctor`, `color`, `focus`, `reload-plugins` there). `system/status requesting` follows.
- Text streams as `stream_event` / `content_block_delta` / `text_delta` (message id in the top-level `api_message_id`); a complete
  `assistant` message arrives per finished block and is the authority (it replaces what the deltas built). A tool call exists only
  from the complete `assistant` message (its `input_json_delta`s are ignored). `thinking` blocks come with an empty text.
- The permission prompt is a **`control_request`** line on stdout (`subtype: can_use_tool`, `tool_name`, `input`, `tool_use_id`,
  `permission_suggestions`); the answer is a `control_response` line on stdin: `{"behavior":"allow","updatedInput":<input>}` (add
  `"updatedPermissions": <the suggestions>` for "allow for this session": verified, the next edit did not ask again) or
  `{"behavior":"deny","message":…}`. Closing stdin with a request open makes the tool fail ("stream closed"). Read-only commands
  such as `ls` are allowed without a request. `--permission-prompt-tool stdio` is what turns the requests on.
- The user's own message is **not** echoed back (the chat adds it itself). Tool results come as `user` lines with `tool_result` blocks.
- Slash commands typed as the message text work (`/context` answers with a synthetic assistant message and a `result`).
- Interrupt: stdin `{"type":"control_request","request_id":…,"request":{"subtype":"interrupt"}}` → a `control_response`, a `user` line
  "[Request interrupted by user]" and a `result`; the process keeps running and the next message works.
- `--session-id <uuid>` / `--resume <uuid>` behave as in the terminal; the transcript file is written as in the terminal.
- A message sent while a turn runs is queued by the CLI (one `result` per turn): `Transcript` counts turns sent minus results.

**Why:** the chat view depends on these details and they are not documented in `--help`.
**How to apply:** after a Claude Code upgrade, record a new sample and run `cargo test -p holt-core chat::`; the real-CLI check is
`cargo test -p holt-core --test chat -- --ignored`.
