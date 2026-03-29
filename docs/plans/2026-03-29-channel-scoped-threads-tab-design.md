# Channel-Scoped Threads Tab Design

Date: 2026-03-29
Status: Approved design draft

## Goal

Improve Chorus thread UX for busy channels by making thread activity a first-class, channel-scoped tab instead of a weak inline affordance under parent messages.

The target behavior is:

- keep the `Chat` tab focused on top-level channel messages
- add a `Threads` tab beside `Chat` and `Tasks`
- scope the `Threads` tab to the currently selected channel only
- default the `Threads` tab to an unread-first triage view
- preserve the current unread model where channel unread includes thread replies
- make it obvious how a user can clear thread-driven unread counts

## Current State

Chorus currently uses a hybrid thread model:

- the main chat timeline shows only top-level messages
- parent messages can show a reply count button
- opening a thread uses a dedicated thread panel
- thread replies contribute to parent channel unread state

This breaks down in active channels because:

- thread activity is not discoverable enough from the channel view
- long top-level history pushes threaded conversations out of sight
- multiple active threads do not have a shared management surface
- a user can read the visible channel timeline and still have unread counts with no clear payoff path

The storage and read model already separate conversation and thread read cursors. The UI does not yet provide a matching first-class thread workflow.

## Approaches Considered

### 1. Keep inline thread previews under parent messages

Pros:

- smallest visual change
- low implementation cost

Cons:

- scales poorly in long channels
- unread thread activity remains easy to miss
- does not explain channel unread clearly

### 2. Add a right-rail active-thread surface inside `Chat`

Pros:

- keeps thread discovery visible while reading chat
- helps explain channel unread

Cons:

- increases layout density in the main chat view
- still leaves thread management as a secondary surface

### 3. Add a dedicated channel-scoped `Threads` tab

Pros:

- clean separation between top-level chat and threaded discussion
- scales better for channels with many active threads
- provides a natural place to burn down thread unread
- fits Chorus's existing room-level tab model alongside `Chat` and `Tasks`

Cons:

- requires stronger cross-tab unread signaling so thread activity is not hidden
- needs a thread index/list surface, not just the existing thread reader

## Recommendation

Choose approach 3.

The best product model is:

- `Chat` = linear room timeline
- `Threads` = threaded conversations for the selected channel
- `Tasks` = structured work for the selected channel

This keeps the main chat readable while giving users an explicit place to resolve thread-driven unread counts.

## Approved Decisions

### Channel-scoped only

The `Threads` tab is scoped to the currently selected channel only.

Out of scope for now:

- workspace-wide threads inbox
- cross-channel thread search
- global thread mentions or assignments

### Unread-first default

The default `Threads` view is `Unread first`.

This means:

- threads with unread replies are shown first
- within unread threads, newest reply sorts first
- read threads can appear in a lower-priority recent section

This makes the tab a triage surface first, not just an archive of all thread activity.

### Preserve channel unread semantics

Channel unread continues to include:

- unread top-level messages
- unread thread replies

This is a deliberate product choice. The UX must therefore explain and expose the thread portion of unread clearly instead of changing unread semantics to hide the problem.

## UX Model

### Channel tabs

For a selected channel, the primary content navigation becomes:

```text
[Chat] [Threads (N)] [Tasks]
```

Where:

- `Chat` shows only top-level messages
- `Threads (N)` shows unread thread replies for the selected channel
- `Tasks` is unchanged

The `Threads` badge reflects thread unread only, while the sidebar channel badge still reflects total channel unread.

### Chat tab behavior

`Chat` remains optimized for top-level messages only.

Thread activity should still be discoverable from `Chat`, but lightly:

- parent messages can keep a reply count / open thread affordance
- the tab bar itself shows `Threads (N)` when thread replies are unread
- optional helper copy may appear when thread unread exists, such as `9 unread replies in threads`

Reading `Chat` does not mark thread replies as read.

### Threads tab behavior

The `Threads` tab becomes the primary management surface for thread activity in a channel.

Desktop layout:

- left: unread-first thread list
- right: selected thread reader

Narrow layout:

- first screen: thread list
- selecting a thread opens a dedicated reader view
- back returns to the list

Each thread row represents one conversation anchored by its parent message, not individual reply messages.

A thread row should include:

- unread count pill
- parent message preview
- last replier
- last reply timestamp
- optional participant summary

Unread rows should be visually prominent. Read-recent rows should be quieter.

### Thread reader behavior

Selecting a thread opens a full thread reader with:

- parent message at the top
- replies below
- composer at the bottom

On open:

- if the thread has unread replies, scroll to the first unread reply
- if it has no unread replies, scroll near the latest reply

Opening the thread list alone does not mark anything as read. Reading is driven by visible replies in the thread reader.

## Read And Unread Semantics

### Parent channel

Sidebar channel unread badge:

```text
channel unread = top-level unread + thread unread
```

### `Chat` tab

Reading visible top-level messages in `Chat` advances only the top-level conversation read cursor.

It does not clear unread thread replies.

### `Threads` tab

Reading visible replies in the thread reader advances the thread read cursor for that thread.

That must decrease:

- the selected thread's unread count
- the `Threads (N)` tab badge
- the parent channel's total unread badge

### Correctness requirement

Human thread reading must affect unread counts. If the user reads replies in the thread reader and unread does not decrease, that is a product correctness bug, not a cosmetic issue.

## Data And State Requirements

The UI needs a channel-scoped thread notification model that complements the existing conversation unread state.

Per selected channel, the app needs:

- unread thread subtotal for the `Threads (N)` tab
- list of active threads with:
  - `thread_parent_id`
  - unread count
  - latest reply metadata
  - parent message preview
- per-thread read cursor state

The backend already models per-thread read state. The missing piece is a channel-level thread index that the UI can load and update efficiently.

## Component Shape

Suggested frontend structure:

- `ChannelTabs`
- `ThreadsTab`
- `ThreadList`
- `ThreadRow`
- `ThreadReader`
- optional `ThreadSummaryNotice` inside `Chat`

Suggested backend/API shape:

- channel-scoped thread notification/list endpoint or equivalent query
- thread list payload including unread count and last-reply metadata
- existing thread history endpoint/read-cursor update path reused for the selected thread reader

## Edge Cases

- No unread threads: show a calm empty state and optionally a read-recent section.
- Many unread threads: preserve scanability; unread-first sort remains stable.
- Deleted or missing parent message: keep the thread row, but degrade the preview safely.
- Channel switch: the `Threads` tab scope resets to the newly selected channel.
- Narrow screens: never force a cramped split-pane layout.
- If only `Chat` was read, thread unread should remain visible and explainable through `Threads (N)`.

## Testing And Verification

Required verification for implementation:

- store tests for thread read updates affecting parent channel unread
- handler/API tests for channel-scoped thread list payloads and ordering
- UI tests for:
  - `Threads (N)` reflecting thread unread only
  - reading `Chat` not clearing thread unread
  - reading visible replies in `Threads` decreasing both thread and parent channel unread
- browser QA for a long-history channel with multiple active threads

## Non-Goals

- redesigning the storage model to remove thread unread from parent channel unread
- building a workspace-global threads inbox in this iteration
- replacing the existing thread reply composer behavior beyond what the new tab needs

## Summary

Chorus should stop treating thread activity as a minor subcontrol under chat messages. In busy channels, threads need their own room-level surface.

The approved design is:

- add a `Threads` tab beside `Chat` and `Tasks`
- scope it to the selected channel only
- default it to `Unread first`
- keep channel unread semantics that include thread replies
- make the `Threads` tab the explicit place to resolve thread unread
