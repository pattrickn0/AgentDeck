---
title: Codex app-server JSON-RPC protocol and stdio approvals
summary: codex app-server provides long-lived streaming, interactive approvals and thread resume, unlike single-turn codex exec.
touches: [crates/core/src/chat/**, crates/core/tests/data/codex-appserver-*.jsonl]
---
Recorded 2026-10-30 with `codex app-server` (`codex-cli 0.166.0`):

- **Transport:** Standard input and output exchange newline-delimited JSON-RPC messages (client requests have integer `id`, notifications omit `id`, server requests have numeric or string `id`).
- **Handshake:** Launching `codex app-server` waits for an `initialize` request (`clientInfo: { name, version }`). The server responds with userAgent/codexHome/platform. The client then sends notification `{"method":"initialized"}` before issuing thread calls.
- **Session management:** A thread is started with `thread/start` (`cwd`, optional `model`, `approvalPolicy`, `sandbox`) or resumed with `thread/resume` (`threadId`, `cwd`). Resumed threads return existing turn history with full item definitions on `thread.turns`.
- **Turns and streaming:** Client sends `turn/start` (`threadId`, `input: [{"type": "text", "text": "..."}]`). Messages sent while a turn is active are steered into the active turn unless queued by Holt. Assistant responses stream via `item/agentMessage/delta` and finalize via `item/completed` (`agentMessage`).
- **Approvals:** Codex emits server requests `item/commandExecution/requestApproval` and `item/fileChange/requestApproval` (plus `item/permissions/requestApproval`). The client must reply `{"id": <req_id>, "result": {"decision": "accept"|"acceptForSession"|"decline"|"cancel"}}`. When resolved or cancelled, Codex sends notification `serverRequest/resolved`.
- **Interrupt:** Turn execution is aborted cleanly by sending request `turn/interrupt` (`threadId`, `turnId`). Codex responds and emits `turn/completed` with `status: "interrupted"`.
- **Why not `codex exec --json`:** `codex exec` exits after every turn and requires restarting with `codex exec resume <id>` for subsequent turns. It does not provide interactive approvals over stdio (requiring flags like `--approve-for-me` or `--dangerously-bypass-approvals-and-sandbox`) and lacks bidirectional RPC control.

**Why:** Holt requires long-lived chat processes that support live streaming, interactive user approvals, and clean interrupts.
**How to apply:** Use `CodexBackend` with `app-server`, handle handshake and message queueing, map approvals to Holt permission cards, and verify against sample recordings in `crates/core/tests/data/`.
