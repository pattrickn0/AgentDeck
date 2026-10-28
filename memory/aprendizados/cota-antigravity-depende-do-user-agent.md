---
title: Antigravity quota depends on User-Agent
summary: Only with "antigravity/cli/<recent version>" does the server return Gemini groups; otherwise, only "Claude and GPT models".
touches: [crates/core/src/usage/antigravity.rs, crates/core/src/usage.rs]
---
`retrieveUserQuotaSummary` and `fetchAvailableModels` return 200 for any User-Agent, but the response body changes: with
`antigravity/cli/1.4.5 (...)` they return "Gemini Models", "Gemini 4 Argon", and "Claude and GPT models"; with `antigravity-cli`,
`antigravity/cli/1.0.0`, or no version, they return only Claude/GPT, and the model list omits Argon. Token, body, and
project have no effect.

**Why:** until 22/10, the app sent `antigravity-cli` and displayed Claude/GPT quota as if it were Gemini's (0%
when Argon was at 94% for the week).
**How to apply:** the request mimics the CLI with the version of the installed `agy` (`installed_version`). If quota appears incorrect,
compare with `agy -p "/quota"`; to see what the CLI sends, point `CLOUD_CODE_URL` to a local server that logs the request.
