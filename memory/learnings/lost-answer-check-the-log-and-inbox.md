---
title: Missing ask-agent --reply response - where to look
summary: The app writes data/agentdeck.log; missing responses remain in ask-agent --inbox and Integrated Chat displays a warning.
touches: [crates/core/src/bridge.rs, crates/core/src/inbox.rs, crates/app/src/deck/**, crates/app/src/logfile.rs]
---
A delayed response (received after a 202 status) only reaches the caller if the UI types it into their tab; it is lost without
an error if the tab is not open or alive, and a 200 response is likewise lost if the client already disconnected (`tiny_http` does not
detect this). Before `t27`, the app only logged to stderr, which is nonexistent for Windows GUI applications: the root cause of
Antigravity response #1 was never identified.

**Why:** "status 200 was written" does not prove the agent read it; "queued in tab buffer" does not prove it was typed.
For this reason, the inbox (`core/src/inbox.rs`) only marks items as "delivered" at the terminal step: a clean 200 write, or successful typing into the terminal.
**How to apply:** when investigating a lost response, read `data/agentdeck.log` (search for `late answer #N`) and have the
querying agent run `ask-agent --inbox --all`. Anyone modifying tab delivery must preserve one of these two outcomes
for every response: typed (`Core::answer_typed`) or warned (`Core::answer_lost`); never drop responses silently.
