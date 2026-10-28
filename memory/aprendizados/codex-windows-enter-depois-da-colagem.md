---
title: On Windows Codex swallows Enter that arrives before it finishes reading pasted text
summary: Codex reads pasted content key by key; an early Enter turns into a newline. Enter delay increases by 1 ms per character (delivery::enter_delay).
touches: [crates/core/src/delivery.rs, crates/app/src/deck/**]
---
The app pastes messages using bracketed paste and sends Enter 250 ms later. On Windows, Codex 0.164 turns on bracketed
paste mode (`?2004h`), but through ConPTY it receives text as individual keystrokes and processes them gradually: an Enter arriving before
the end becomes a newline inside "[Pasted Content N chars]" and the message stays sitting in the input field until someone presses
Enter. Measured on 2026-10-25 with a PTY probe: 1,735 characters lose Enter at 600 ms and receive it at 1,200 ms; 3,593
lose it at 1,200 ms and receive it at 2,400 ms. Short text is picked up in 250 ms.

**Why:** the user had to press Enter manually during Codex tasks (task prompts and `ask-agent` messages are long).
**How to apply:** the delay for the first Enter comes from `delivery::enter_delay` (Codex on Windows only, 1 ms per character, capped at
10 s). Do not "solve" this with a second fixed Enter: in a Codex that just displayed an approval prompt, the extra Enter
approves the command. For a new CLI that leaves messages stranded in the input field: measure it the same way (paste N characters and vary the delay).
