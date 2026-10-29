---
title: Antigravity quota is per model group, not per agent
summary: Argon, other Gemini models, and Claude/GPT exhaust independently; the status bar summary (most consumed) does not indicate whether a model will respond.
touches: [crates/core/src/usage.rs, crates/core/src/usage/antigravity.rs, crates/core/src/bridge.rs]
---
On 24/10, Argon had 100% of its weekly quota consumed, "Gemini Models" (Flash) had 0.4%, and "Claude and GPT models" had
95%. Antigravity's summary `AgentUsage::session/weekly` reflects the most exhausted Gemini group, so it reports "exhausted" even when
Flash is available. What determines whether a model responds is its group (`usage::group_of`).

**Why:** Claude invoked `antigravity@arquiteto` on Argon with no remaining quota; the user only had Flash 3.8 and rate-limited tiers.
Until then, `ask-agent` did not check quota.
**How to apply:** before delegating, `ask-agent --models <agent>` displays the "quota:" line and `--team` shows a notice next to the
model; the bridge rejects (409) queries sent to an exhausted model. A new server group falls into `group_of` by name:
if another appears (e.g. a new Gemini with its own quota), add the rule and test
`each_model_counts_against_its_own_group`.
