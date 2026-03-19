# Design Review: 03 — Build Logs

**Document:** `docs/cbc/design/03-20260318T1803-build-logs.md`

---

## Summary

Two blockers: the SSE event name is wrong (`log` vs server's
`output`), and the 3-retry limit is not automatic in
`reqwest-eventsource`. The tail response shape is richer than
documented, and the `done` event lacks build state information.

**Verdict: Revise and re-review.**

---

## Blockers

### B1 — SSE event name mismatch

The design says `event: log`. The server emits `event: output`
(`sse.rs` line 215). If the client filters on `"log"`, all
build output lines are silently discarded.

**Fix:** Change `event: log` to `event: output` throughout.

### B2 — 3-retry limit is not automatic

The design says: "exit after 3 retries." `reqwest-eventsource`
retries indefinitely per SSE spec. Limiting retries requires
a wrapper that counts reconnection errors and calls `es.close()`
after 3.

**Fix:** Document the retry-counting approach explicitly. The
crate exposes reconnection errors that can be counted.

---

## Major Concerns

### M1 — `tail` response has 4 fields, design documents 1

The server returns:

```json
{
  "build_id": 42,
  "lines": [...],
  "total_lines": 1542,
  "returned": 20
}
```

The design says only `lines: Vec<String>`. The `total_lines`
and `returned` fields are useful for display:
`"(showing 20 of 1,542 lines)"`.

**Fix:** Document all 4 fields. Display the count context.

### M2 — `done` event data lacks build state

The server sends `data: "build complete"` for the `done` event
— always the literal string, not `"success"` or `"failure"`.
The design shows `"--- build finished: success ---"` which
cannot be derived from the SSE event alone.

**Fix:** After receiving `done`, call `GET /api/builds/{id}`
to get the final state. Document this two-step pattern.

### M3 — SSE reconnection is best-effort for finished builds

`reqwest-eventsource` resends `Last-Event-ID` on reconnect.
But for finished builds, the server's seq-offset map may have
been cleared by GC. Reconnecting after GC may replay the
entire log from sequence 0.

**Fix:** Document: "Resumption is reliable while the build is
active. For finished builds, reconnection may replay from the
start."

---

## Minor Issues

- **Default tail lines mismatch.** Design says `-n 50`, server
  defaults to 30 (`default_tail_n()`). Either match the server
  default or always send `?n=` explicitly.

- **`logs/follow` and `logs` (full) endpoints have no
  `AuthUser` extractor.** These endpoints are unauthenticated
  on the server — likely a server oversight. Note it so it can
  be addressed server-side.

- **`reqwest-eventsource` crate.** Verify this crate exists
  and is maintained. The SSE client ecosystem in Rust is
  fragmented. Alternative: `eventsource-client`.

---

## Strengths

- Streaming download via `get_stream()` avoids buffering
  multi-MB logs.
- Writing in 8KB chunks is the right approach.
- Documenting the "already finished" case (full log then
  `done`) is important.
- Reconnection via `Last-Event-ID` is the correct SSE pattern.
