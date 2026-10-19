# Scrawl

## Routing

**Capture**:
One recording, compiled into a `.scrawl` bundle.

**Destination**:
Where a capture is delivered: an OpenCode session, on this Mac or the agent host. The clipboard always gets the capture as well, so "no destination" means clipboard only.
_Avoid_: target (that's what was recorded)

**Address**:
A destination someone stated outright, either the user's **choice** or the recording's **recorded destination**. Addresses are obeyed, never weighed.

**Choice**:
A session the user picked for this capture (`scrawl send --session`, a chip on the pill).

**Recorded destination**:
The session a recording carries: a ScrawlKit timeline's top-level `session`, or the destination an earlier routing wrote into `scrawl.json`. The session that was merely on screen (`context.session`, OpenCode Desktop's viewed session) is a clue, not a recorded destination.

**Clue**:
An observed fact about a capture's surroundings that points at sessions: a page's registration, the folder of the process serving the tab, a terminal's folder, the session OpenCode Desktop showed, the captured app's name, words in the window title or in what was said.
_Avoid_: signal, hint

**Source**:
Where a clue came from (page, port, terminal, document, viewed, app, title, speech). Each source counts once per candidate.

**Candidate**:
An open root session that a clue could point at.

**Evidence**:
The address, clues and candidates for one capture, with the time it ended. Everything a decision depends on.

**Decision**:
Send (one candidate is strong and clearly ahead), offer (one or two are plausible), or keep (clipboard only).

**Ladder**:
The routing that runs with smart routing off: the page's registration, else the loopback port's folder, and the newest session there.

**Undo window**:
The few seconds after a send is decided and before it leaves the Mac, when ✕ or Paste cancels it.
