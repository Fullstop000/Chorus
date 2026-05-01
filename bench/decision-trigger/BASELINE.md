# Decision-trigger benchmark — A/B baseline (OLD vs NEW prompt)

Head-to-head between the **OLD** prompt (input-pattern enumeration, on `main` before PR #133) and the **NEW** prompt (four-property structural test, on PR #133 branch). Same 15 hard cases, same 4 models, same parallel runner. Captured 2026-05-02.

## Headline scores (cases-hard.tsv)

| Model | Tier | OLD | NEW | Δ |
|---|---|---|---|---|
| claude/opus | best | **15/15** | 9/15 | **−6** |
| claude/sonnet | efficiency | 14/15 | **15/15** | +1 |
| codex/gpt-5.5 | best | 14/15 | 14/15 | 0 |
| codex/gpt-5.4-mini | efficiency | 12/15 | **13/15** | +1 |
| **average** | | **13.75/15** | **12.75/15** | **−1.0** |

NEW prompt **regresses on Opus by 6 points**, gains 1 on Sonnet, gains 1 on gpt-5.4-mini, washes on gpt-5.5. Net negative on average.

## Aggregate behavior delta

| | OLD prompt | NEW prompt |
|---|---|---|
| Decision-cases caught (max 32 = 8 cases × 4 models) | 30/32 (94%) | 23/32 (72%) |
| Chat-cases held back (max 28 = 7 cases × 4 models) | 25/28 (89%) | **28/28 (100%)** |

OLD is better at firing decisions. NEW is better at restraint. Different tradeoff, not a strict win.

## Per-case breakdown

```
case  predicted   OLD-opus  NEW-opus    OLD-sonnet  NEW-sonnet  OLD-gpt5.5  NEW-gpt5.5  OLD-mini   NEW-mini
 1    decision    decision  chat        decision    decision    decision    decision    chat       unknown
 2    decision    decision  chat        decision    decision    decision    decision    decision   decision
 3    decision    decision  chat        decision    decision    decision    decision    decision   decision
 4    decision    decision  chat        decision    decision    decision    decision    decision   decision
 5    decision    decision  chat        decision    decision    decision    chat        decision   decision
 6    decision    decision  decision    decision    decision    decision    decision    decision   decision
 7    decision    decision  chat        decision    decision    decision    decision    decision   decision
 8    decision    decision  decision    decision    decision    decision    decision    chat       chat
 9    chat        chat      chat        chat        chat        chat        chat        chat       chat
10    chat        chat      chat        decision    chat        decision    chat        decision   chat
11    chat        chat      chat        chat        chat        chat        chat        chat       chat
12    chat        chat      chat        chat        chat        chat        chat        chat       chat
13    chat        chat      chat        chat        chat        chat        chat        chat       chat
14    chat        chat      chat        chat        chat        chat        chat        chat       chat
15    chat        chat      chat        chat        chat        chat        chat        chat       chat
```

## Where each prompt wins

### NEW wins on case 10 (retrospective trap), 3 cells

Case 10 prompt: *"I shipped the auth fix yesterday. In hindsight, given what we now know about the migration timing, was that the right call?"*

| Model | OLD prompt | NEW prompt |
|---|---|---|
| sonnet | decision (over-fires) | chat ✓ |
| gpt-5.5 | decision (over-fires) | chat ✓ |
| gpt-5.4-mini | decision (over-fires) | chat ✓ |
| opus | chat ✓ | chat ✓ |

The structural rule's properties #2 (Blocking) and #3 (Material consequence) explicitly fail for retrospectives — the PR already shipped, nothing is gated on the agent's verdict. The OLD prompt's input-pattern matching can't distinguish *"was that the right call?"* from a current verdict, so 3/4 models fire incorrectly. **NEW is genuinely better at restraint.**

### OLD wins on Opus, 6 cells (cases 1, 2, 3, 4, 5, 7 — all implicit-delegation decisions)

Each of these cases presents mutually exclusive options + a deadline + situational context, but lacks an explicit *"you pick"* clause.

- **OLD prompt** enumerates *"presents two or more concrete alternatives and asks you to pick"*. Opus interprets "presents alternatives + deadline" as the trigger and fires.
- **NEW prompt** requires all four structural properties including #4 (Delegated). Opus reads *"we need a responder lined up before the call"* as the team's own action item, not a delegation to the agent, and refuses to fire.

Why is this Opus-specific? Sonnet, gpt-5.5, and gpt-5.4-mini all infer delegation from situational context regardless of which prompt is loaded. Opus is the only model that strictly waits for an explicit *"you pick"* under the NEW rule. **The NEW rule's strict interpretation of property #4 is exactly what trips Opus.**

### Stable across both prompts

- **Case 6** (*"I need to give my VP an answer by 4pm"* — explicit time-anchored ask): every model fires decision under both prompts.
- **Case 8** (sprint-end time-box with options laid out): every model except gpt-5.4-mini fires decision under both prompts. gpt-5.4-mini misses under both — model capability ceiling, not a prompt issue.
- **All chat cases except 10**: clean restraint across all 4 models, both prompts.

## Failure modes worth noting

1. **gpt-5.4-mini case 1 (NEW = `unknown`)** — the model didn't call any tool at all under the NEW prompt. It saw the prompt, the run completed `reason=Natural`, but no `dispatch_decision` and no `send_message`. Same case, OLD prompt → it correctly chose `chat` (which is wrong vs prediction but a real choice). The NEW prompt seems to have caused gpt-5.4-mini to freeze on this prompt — worth investigating.

2. **gpt-5.5 case 5 NEW = `chat`** — only OLD/NEW divergence on gpt-5.5. The hiring case under NEW landed in chat. Looking at the agent's actual reply would tell us why.

## Conclusion

**The structural rewrite is a tradeoff, not a strict win.** Average pass rate drops 1 point (13.75 → 12.75 / 15) across 4 models, but the loss is concentrated on a single model (Opus) and the gain is real signal (case 10 restraint).

What it actually achieves:
- ✅ **Clean restraint on retrospectives.** The OLD prompt's input-pattern matching has a known false-positive on retrospective phrasing; the NEW rule closes it.
- ❌ **Loses on implicit-delegation decisions for Opus.** The strict reading of property #4 (Delegated) excludes the *"we need X by Y"* framings that real teams use all the time. Opus is the only model that takes this strictness literally.
- 〇 **Wash on Sonnet, gpt-5.5, gpt-5.4-mini.** Those models infer delegation from situational context regardless of which prompt is loaded.

## Implications for the next iteration

Three options for the prompt-rule tuning:

1. **Soften property #4.** Add a clause like *"a request that lays out mutually exclusive alternatives plus a deadline counts as implicit delegation, even without an explicit 'you pick'."* Recovers Opus without losing Sonnet/gpt-5.5/gpt-5.4-mini.
2. **Accept the Opus regression.** Ship the NEW rule as-is — the chat-restraint gain is principled, and Opus users can be coached toward explicit phrasing. Trade decision-firing for false-positive avoidance.
3. **Split the prompt by tier.** Opus gets a more permissive trigger, Sonnet gets the strict one. Maintenance cost.

This baseline lets us measure each iteration against real signal instead of guessing. Re-run after any prompt change that affects routing.

## Reproducing this report

```bash
# OLD prompt baseline (main, port 3002 + bridge 4322 to coexist with a running NEW server):
git worktree add /tmp/chorus-main main
cd /tmp/chorus-main && cargo build --bin chorus
/tmp/chorus-main/target/debug/chorus serve --port 3002 --bridge-port 4322 \
  > /tmp/chorus-old.log 2>&1 &
CHORUS_LOG=/tmp/chorus-old.log \
  CASES=$PWD/bench/decision-trigger/cases-hard.tsv \
  ./bench/decision-trigger/run-matrix.sh http://localhost:3002

# NEW prompt baseline (PR #133 branch, port 3001):
cargo build --bin chorus
./target/debug/chorus serve --port 3001 > /tmp/chorus-new.log 2>&1 &
CHORUS_LOG=/tmp/chorus-new.log \
  CASES=$PWD/bench/decision-trigger/cases-hard.tsv \
  ./bench/decision-trigger/run-matrix.sh http://localhost:3001
```

Each matrix takes ~45-50 min (4 models, parallel-per-model). Raw results live under `bench/decision-trigger/results/matrix-<unix_ts>/`.

Captured runs in this report:
- OLD: `matrix-1777658557/`
- NEW: `matrix-1777647089/`
