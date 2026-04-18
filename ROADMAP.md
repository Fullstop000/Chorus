# Chorus Roadmap

_Updated: 2026-03-29_

Priority: `P0` = now, `P1` = next, `P2` = later

## Milestone 0. Shipped Baseline

- [x] Runtime Support
  - [x] Claude driver
  - [x] Codex driver
  - [x] Kimi driver

- [x] Core Communication
  - [x] Channel messaging
  - [x] DM messaging
  - [x] Thread messaging
  - [x] Attachments and history persistence

- [x] Work Management
  - [x] Task board
  - [x] Message-to-task creation

- [x] Agent Operations
  - [x] Activity log
  - [x] Workspace browser and file preview
  - [x] Agent edit, restart, reset, and delete controls

- [x] Collaboration
  - [x] Shared memory flow
  - [x] Team channels
  - [x] `leader_operators` collaboration model
  - [x] `swarm` collaboration model
  - [x] Channel edit, archive, and permanent delete flows

## Engineering

- [ ] QA Alignment
  - [ ] [P0] Update channel QA cases for edit, archive, and delete
  - [ ] [P0] Update agent QA cases for edit, restart, reset, and delete
  - [ ] [P1] Remove stale blocked-case assumptions
  - [ ] [P1] Keep Playwright specs aligned with the case catalog

- [ ] Runtime Reliability
  - [ ] [P0] Harden restart behavior across Claude, Codex, and Kimi
  - [ ] [P0] Harden resume behavior across Claude, Codex, and Kimi
  - [ ] [P0] Harden sleep and wake behavior across Claude, Codex, and Kimi
  - [ ] [P0] Reduce status drift between sidebar, profile, activity, and process state

- [ ] Driver Platform
  - [ ] [P0] Define a driver spec for local runtimes
  - [ ] [P0] Define a driver spec for cloud runtimes
  - [ ] [P0] Unify the integration surface for local and cloud drivers
  - [ ] [P1] Reduce per-driver integration work

- [ ] Chorus Runtime
  - [ ] [P0] Design a dedicated agent runtime for Chorus
  - [ ] [P0] Define the process, tool, and session contract for the Chorus runtime
  - [ ] [P1] Add verification coverage for the Chorus runtime

- [ ] Authentication and Identity
  - [ ] [P0] Define the identity model for local and multi-user Chorus
  - [ ] [P0] Add authentication for non-local access
  - [ ] [P0] Add session and credential handling
  - [ ] [P1] Add role-aware permission checks

- [ ] Error Handling
  - [ ] [P0] Define structured API error types
  - [ ] [P0] Define retry and failure policies for agent workflows
  - [ ] [P1] Add clearer client-visible error codes
  - [ ] [P1] Add recovery paths for unprocessable work

- [ ] Data Integrity
  - [ ] [P0] Add restore integrity checks
  - [ ] [P0] Add corruption detection for critical data paths
  - [ ] [P1] Add crash-recovery validation
  - [ ] [P1] Add safer schema migration tooling

- [ ] Visibility
  - [ ] [P1] Turn activity events into clearer collaboration traces
  - [ ] [P1] Add clearer agent health summaries
  - [ ] [P1] Add clearer team health summaries

- [ ] Runtime Operations
  - [ ] [P1] Improve runtime install diagnostics
  - [ ] [P1] Improve runtime auth diagnostics
  - [ ] [P1] Improve runtime configuration diagnostics
  - [ ] [P1] Strengthen environment isolation

- [ ] Admin Safety
  - [ ] [P0] Add stronger destructive-action guardrails
  - [ ] [P1] Clarify archive semantics
  - [ ] [P1] Clarify delete semantics
  - [ ] [P1] Improve auditability for destructive actions

- [ ] Deployment
  - [ ] [P1] Add a cleaner production deploy path
  - [ ] [P0] Add backup flow for the data directory
  - [ ] [P0] Add restore flow for the data directory

- [ ] Data and Recovery
  - [ ] [P1] Improve data-dir portability
  - [ ] [P0] Improve backup reliability
  - [ ] [P0] Improve restore reliability

## Features

- [ ] Instant Teams
  - [ ] [P1] Turn a user goal into a ready-to-work team so people can start real work without hand-building prompts, skills, and runtime setup.
  - [ ] [P1] Add team templates
  - [ ] [P1] Store preset team metadata
  - [ ] [P1] Store template agent prompt presets
  - [ ] [P1] Store template agent skill presets
  - [ ] [P1] Store template agent runtime presets
  - [ ] [P1] Add auto-team suggestions
  - [ ] [P1] Offer curated team templates to users

- [ ] Teams of Teams
  - [ ] [P2] Let one team lead specialist subteams so complex work can scale without collapsing into one crowded room.
  - [ ] [P2] Support team-extending-team
  - [ ] [P2] Let one team lead another team
  - [ ] [P1] Show team roles in the UI
  - [ ] [P1] Show forwarded-task provenance in the UI
  - [ ] [P1] Show readiness and consensus state in the UI
  - [ ] [P1] Let teams own tasks directly
  - [ ] [P2] Improve team workspace flow
  - [ ] [P2] Improve team memory flow

- [ ] Quick Landing
  - [ ] [P0] Help a new user get their first useful result in minutes instead of learning the whole system before seeing value.
  - [ ] [P0] Add quick landing for users
  - [ ] [P0] Improve first-run entry flow
  - [ ] [P1] Improve first-use guidance for channels, teams, and agents
  - [ ] [P1] Improve onboarding for humans joining an active workspace

- [ ] Mission Control
  - [ ] [P1] Show the full task graph, execution pipeline, owners, blockers, and live progress in one place.
  - [ ] [P1] Add task graph view
  - [ ] [P1] Add executing pipeline view
  - [ ] [P1] Show whole-plan progress
  - [ ] [P1] Show in-flight work status

- [ ] Human Help Lane
  - [ ] [P1] Let agents pause at the right moment, ask a human for help, and resume without losing context.
  - [ ] [P1] Let agents ask for human help inside task flow

- [ ] Cloud Workforce
  - [ ] [P1] Let users run always-on agents beyond the local machine so work can continue even when a laptop is offline.
  - [ ] [P1] Add cloud agent driver support

- [ ] Chorus Native Runtime
  - [ ] [P1] Build a runtime designed for multi-agent collaboration instead of forcing Chorus to adapt general-purpose agent shells forever.

- [ ] Open Agent Ecosystem
  - [ ] [P2] Make Chorus a better host for outside agent ecosystems so teams can adopt the best runtime for each job.
  - [ ] [P2] Add OpenClaw driver
  - [ ] [P2] Add new runtimes only when they materially improve collaboration

- [ ] Shared Context
  - [ ] [P1] Make memory and workspace feel like one shared brain instead of separate tools people have to manage manually.
  - [ ] [P2] Add better memory browse tools
  - [ ] [P2] Add better memory filter tools
  - [ ] [P2] Add better memory recall tools
  - [ ] [P1] Tighten handoff flow between chat and memory
  - [ ] [P1] Add workspace search
  - [ ] [P1] Improve large-file preview behavior
  - [ ] [P2] Improve workspace navigation speed
  - [ ] [P1] Tighten handoff flow between chat and workspace

- [ ] Chorus Everywhere
  - [ ] [P1] Bring outside communication into Chorus so teams can coordinate where work already happens instead of forcing everyone into a new inbox.
  - [ ] [P1] Add a common IM bridge layer
  - [ ] [P2] Support Discord
  - [ ] [P2] Support Telegram
  - [ ] [P2] Support Lark
  - [ ] [P2] Connect external task systems
  - [ ] [P2] Connect external ticket systems
  - [ ] [P2] Connect external notification systems
  - [ ] [P1] Add automation workflows that stay visible in chat

- [ ] Core UX
  - [ ] [P1] Make the product feel reliable and obvious in daily use so users trust it with real work, not just demos.
  - [ ] [P1] Tighten selection recovery after channel archive and delete
  - [ ] [P1] Tighten selection recovery after agent delete
  - [ ] [P0] Improve composer error recovery
  - [ ] [P0] Improve attachment error recovery
  - [ ] [P0] Improve task action error recovery
  - [ ] [P1] Normalize channel, team, and membership controls
