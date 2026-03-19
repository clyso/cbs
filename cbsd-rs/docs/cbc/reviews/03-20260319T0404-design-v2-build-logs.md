# Design Review: 03 — Build Logs (v2)

**Verdict: Approved.**

Both v1 blockers resolved: SSE event name is `output`, retry
counting is manual with consecutive-error counter and `Open`
reset. Additional fixes: tail response shows all 4 fields,
`done` event follow-up GET documented, reconnection limitation
noted, default `-n 30` matches server.

No blockers. No major concerns.

## Minor Issues

- **`Event::Open` reset semantics.** A connection that flaps
  open-then-error-then-open will never reach the 3-error
  threshold. This favors liveness — call it out as
  intentional.

- **Server "no logs yet" vs design "no log available".**
  The server returns `"no logs yet"` with 404. Align the
  client message wording.

## Strengths

- Most carefully specified document in the set.
- SSE event name `output` is correct.
- Retry counter with `Open` reset is well-designed.
- Follow-up GET for final build state after `done` is the
  right approach.
- Tail response uses all 4 fields with count context.
- Reconnection limitation honestly documented.
