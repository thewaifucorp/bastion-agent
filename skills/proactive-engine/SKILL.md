---
name: bastion/proactive-engine
version: 1.0.0
description: >
  Proactive intelligence engine — detects inactivity, stale memories, CVEs,
  and temporal patterns, then generates context-aware suggestions via LLM.
  Replaces the deprecated bastion/proactive skill.
triggers:
  - scheduled via HEARTBEAT every 2h (run-cycle)
  - scheduled via HEARTBEAT every 24h (run-cve-check)
  - scheduled via HEARTBEAT weekly (run-weekly)
---

# Skill: bastion/proactive-engine

## Objective

Monitor the user's interaction patterns across personas and generate proactive
suggestions without requiring the user to ask.

---

## Architecture

```
Layer 0 (Detectors)           Layer 1 (Generators)
─────────────────────         ──────────────────────
InactivityDetector     ──┐
MemoryStalenessDetector──┤→ EventBus → SuggestionGenerator → memupalace
TemporalPatternDetector──┤                                  → LLM call
CVEDetector            ──┘            WeeklySynthesizer
IntentTracker
```

- **EventBus**: deduplicates events (6h default, 24h for CVE/staleness)
- **SuggestionGenerator**: single LLM call per cycle via OpenRouter
- **Degraded mode**: if memupalace is unavailable, engine continues without persistence

---

## CLI Commands

> Invoke via `exec python3 skills/proactive-engine/main.py <command>`

### run-cycle (every 2h)
```bash
python3 skills/proactive-engine/main.py run-cycle \
  --personas '["carreira","estudos","projetos-pessoais"]' \
  --skills '["bastion/life-log","bastion/guardrails"]'
```

### run-cve-check (every 24h)
```bash
python3 skills/proactive-engine/main.py run-cve-check \
  --skills '["bastion/life-log","bastion/guardrails"]'
```

### run-weekly
```bash
python3 skills/proactive-engine/main.py run-weekly \
  --personas '["carreira","estudos"]'
```

---

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PROACTIVE_ENABLED` | `true` | Set to `false` to disable entirely |
| `PROACTIVE_LLM_MODEL` | `google/gemini-flash-1.5` | OpenRouter model |
| `PROACTIVE_INACTIVITY_DAYS` | `3` | Days before marking persona inactive |
| `PROACTIVE_STALENESS_DAYS` | `14` | Days before marking memory stale |
| `PROACTIVE_LIFELOG_WINDOW` | `50` | Records to include in LLM context |
| `PROACTIVE_DEDUP_WINDOW_HOURS` | `6` | Event dedup window (hours) |
| `CLAWHUB_URL` | _(empty)_ | ClawHub base URL — CVE checks disabled if unset |
| `CLAWHUB_API_KEY` | _(empty)_ | ClawHub API key |
| `OPENROUTER_API_KEY` | _(required)_ | OpenRouter API key for LLM calls |

---

## Persistence

| File | Purpose |
|------|---------|
| `db/proactive-engine/pending-events.json` | Unprocessed detection events |
| `db/proactive-engine/intent-queue.json` | Intent queue (when memupalace offline) |
| `db/proactive-engine/heartbeat-state.json` | Last run timestamps per task |

---

## Memupalace Wings

| Wing | Content |
|------|---------|
| `proactive/suggestions` | Generated suggestions |
| `proactive/intent` | Tracked user intents |
| `proactive/weekly` | Weekly summaries |
