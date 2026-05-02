# Decision-trigger benchmark

Evaluates whether the prompt in `src/agent/drivers/prompt.rs` causes agents to correctly route work between the **decision channel** (`dispatch_decision`) and the **chat channel** (`send_message`).

The current rule is structural — a request is a decision when ALL FOUR hold:

1. **Mutually exclusive** options
2. **Blocking** — the asker can't move until a pick lands
3. **Material consequence** — the pick commits resources or forecloses paths
4. **Delegated** — the asker is asking the agent to pick

Cases that hit all four should produce `dispatch_decision`. Anything else should produce `send_message`.

## What's measured

| | Description |
|---|---|
| **Input** | 15 hand-curated prompts spanning 8 work domains (PR review, vendor pick, architecture, status, triage, hiring, doc edit, compliance, time-box, naming). |
| **Setup** | One isolated Chorus agent per case (claude/sonnet), so there's no session-context bleed between cases. All agents run in parallel. |
| **Signal** | Per-agent log scrape: did the agent call `dispatch_decision` or `send_message` in its response turn? |
| **Score** | Match rate vs. the `predicted` column in `cases.tsv`. |

## Why one-agent-per-case in parallel

Running cases sequentially through a single agent corrupts the test in two ways:
1. **Context bleed** — case N inherits memory of cases 1..N-1, so the agent's choice on case N is biased.
2. **Stale-session timeouts** — codex/opencode `--resume` silently fails after a few minutes idle (see TODOS.md). Sequential runs hit this gap; one agent per case dodges it entirely.

Total wall time is `max(per_agent_turn) ≈ 2 min`, not `sum`.

## Prerequisites

- `chorus` binary built: `cargo build --bin chorus`
- Chorus server running with stdout/stderr captured to a log file
- Claude runtime authed (`chorus setup` confirms)
- `CHORUS_LOG` env var pointing to the server log (defaults to `/tmp/chorus-qa-server.log`)

## Cases

Two case files at different difficulty:

| File | Style | What it measures |
|---|---|---|
| `cases.tsv` | Easy / smoke. Decision-shaped requests use verdict-flavored phrasing (*"merge or hold?"*, *"what do you recommend?"*, *"your call"*). | Sanity check: prompt teaches the rule at all. Both input-pattern and structural-rule prompts hit 15/15 on this. |
| `cases-hard.tsv` | Realistic narrative scenarios. Decision-shaped requests use **neutral phrasing** (no "recommend", no "verdict", no "X or Y"). Trap cases include rhetorical frustration, retrospectives, exploration, status updates, and facilitation asks. | Differentiates prompts that pattern-match input phrasing from prompts that test the structural shape of the agent's intended reply. |

To use the harder set:
```bash
CASES=$PWD/bench/decision-trigger/cases-hard.tsv ./bench/decision-trigger/run.sh
```

## Running

Single run against the default model (`claude/sonnet`):
```bash
./bench/decision-trigger/run.sh
```

Pick a different runtime/model:
```bash
RUNTIME=codex MODEL=gpt-5.5 ./bench/decision-trigger/run.sh
```

Sweep all models in `models.tsv` and produce a side-by-side matrix:
```bash
./bench/decision-trigger/run-matrix.sh
CASES=$PWD/bench/decision-trigger/cases-hard.tsv ./bench/decision-trigger/run-matrix.sh
```

Common options:
```bash
./bench/decision-trigger/run.sh http://localhost:3001    # explicit server URL
KEEP_AGENTS=1 ./bench/decision-trigger/run.sh            # don't auto-delete agents on exit (forensics)
CHORUS_LOG=/var/log/chorus.log ./bench/decision-trigger/run.sh
```

## Models matrix

`models.tsv` lists the (runtime, model, tier) combinations the matrix runner sweeps. Default ships with the two-per-family pattern: best + efficiency for Anthropic and OpenAI.

| runtime | model | tier | resolves to |
|---|---|---|---|
| claude | opus | best | Claude Opus 4.7 |
| claude | sonnet | efficiency | Claude Sonnet 4.6 |
| codex | gpt-5.5 | best | GPT-5.5 |
| codex | gpt-5.4-mini | efficiency | GPT-5.4-mini |

Add other rows (kimi, gemini, opencode) as Chorus drivers stabilize. Each row produces one column in the matrix output.

## A/B testing prompt variants

The whole system prompt is injectable via `CHORUS_SYSTEM_PROMPT_OVERRIDE_FILE`. To compare a candidate prompt against the built-in:

```bash
# 1. Save the current built-in prompt (e.g. by capturing what build_system_prompt
#    produces from a unit test or a one-shot CLI helper) to baseline.md.
# 2. Write your candidate prompt to candidate.md.
# 3. For each variant, restart the chorus server with the env var pointing at it:

CHORUS_SYSTEM_PROMPT_OVERRIDE_FILE=$PWD/baseline.md  chorus serve --port 3001 &
./bench/decision-trigger/run.sh   # records run as bench/.../results/<ts>/results.tsv
kill %1

CHORUS_SYSTEM_PROMPT_OVERRIDE_FILE=$PWD/candidate.md chorus serve --port 3001 &
./bench/decision-trigger/run.sh
kill %1
```

The override is a verbatim substitution — the file content becomes the system prompt. No template substitution, no merging. Tool names must already be resolved (use `mcp__chat__send_message` for the claude runtime, bare `send_message` for codex/kimi/gemini/opencode).

## Output

Each run writes to `bench/decision-trigger/results/<unix_ts>/`:

- `results.tsv` — per-case `id, agent, predicted, actual, match, prompt`
- `log-slice.txt` — the relevant slice of the server log for forensics

Exit code is `0` if all cases match, `1` otherwise.

## Cases (`cases.tsv`)

Each row is `id <tab> predicted <tab> prompt`. To add a case:

1. Append a new row.
2. Set `predicted` to `decision` or `chat` based on the structural test above.
3. Make the prompt **current-tense and unambiguous** about who is blocked. Retrospective phrasing ("should we have shipped X?") fails property #2 and is correctly classified as `chat`, so don't predict `decision` for it.

## Interpreting results

A `match: 15/15` confirms the prompt rule is well-formed for general work. Anything below that needs investigation:

- **`predicted=decision actual=chat`** — the agent missed a verdict-shaped request. Either the prompt is too restrictive, or the case wording is too soft. Check whether all four properties actually hold; if so, the rule needs a stronger trigger for that workflow class.
- **`predicted=chat actual=decision`** — the agent over-fired. The structural rule has a false positive. Tighten the trigger or improve the canonical example.
- **`actual=unknown`** — the agent didn't call either tool, or the log scrape missed the call. Check `log-slice.txt`.

## Known limitations

- Single-runtime test (claude/sonnet). Codex/opencode have known stale-session bugs and aren't included until those drivers ship the analogous `--resume` guard.
- Log-scrape classification is brittle to log format changes. If the `tool call agent=...` log line moves or renames, update the grep in `run.sh`.
- Per-agent agent boot time (~10-30s) dominates wall time for short tests.
- Cases must be **side-effect-free**. An agent given "edit X" or "fix typos in Y" will mutate the repo, leaving uncommitted changes. Frame action cases as "report what you'd change" or use a sandbox path the runner pre-stages and cleans up.

## Provenance

This benchmark was added in the PR that rewrote the prompt's decision trigger from input-pattern enumeration to a structural four-property test. See git history for context.
