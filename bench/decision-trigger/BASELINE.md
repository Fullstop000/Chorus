# Decision-trigger benchmark — baseline results

Recorded baseline scores for the structural-rule prompt (PR #133). Run with:

```bash
CASES=$PWD/bench/decision-trigger/cases-hard.tsv ./bench/decision-trigger/run-matrix.sh
```

## Hard-cases matrix (cases-hard.tsv, 15 cases: 8 decision / 7 chat)

| Model | Tier | Score | Notes |
|---|---|---|---|
| claude/opus | best | **9/15** | All 7 chat cases correct. 6 of 8 decision cases read as chat — Opus interprets property #4 (Delegated) strictly: implicit "we need" doesn't qualify. |
| claude/sonnet | efficiency | **15/15** | Infers delegation from situational context. Counterintuitively beats Opus. |
| codex/gpt-5.5 | best | **14/15** | Misses only the hiring case (case 5). |
| codex/gpt-5.4-mini | efficiency | **13/15** | 1 mis-fire (case 8 time-box → chat). 1 silent (case 1: no tool called at all). |

All 4 models score 7/7 on chat cases (correct restraint).
The 8 decision cases are where the rule's "implicit delegation" reading differs by model.

## Easy-cases (cases.tsv, 15 cases with verdict-flavored phrasing)

Both the OLD prompt (input-pattern enumeration) and the NEW prompt (structural rule) score **15/15** on the easy cases with claude/sonnet — they don't differentiate at this difficulty.

## What the matrix tells us

1. **The structural rule's behavior depends heavily on the model.** Same prompt, same cases, scores from 9/15 to 15/15.
2. **Larger ≠ better on this benchmark.** Opus 4.7 (the "best" Anthropic model) is more conservative than Sonnet 4.6 — it refuses to infer delegation from context and only fires on explicit asks.
3. **Chat cases are easy across the board.** All 4 models nailed all 7. Restraint is not the problem.
4. **Implicit-delegation is the hard part.** Cases like *"We need a responder lined up before the call"* (Acme P0) require the model to infer that the asker is delegating the pick. Sonnet and gpt-5.5 mostly do; Opus doesn't.

## Implications for prompt design

If we want consistent behavior across models, the rule needs either:
- A stronger nudge that "implicit delegation in workplace context = delegation" (cost: more chat false-positives on smaller models), OR
- Explicit per-tier prompt variants (cost: maintenance), OR
- Acceptance that this is a model-capability ceiling and pick the model accordingly (cost: model lock-in).

This baseline lets us measure the next prompt iteration against a real signal instead of guessing.

## How to update this baseline

After any prompt change that affects routing, re-run the matrix and replace this file. Keep the previous version in git history so we can diff baselines over time.
