# Design Review: 05 — Worker Administration

**Document:** `docs/cbc/design/05-20260318T1805-worker-admin.md`

---

## Summary

The design is tight and accurate. No blockers. The worker token
UX is correctly handled. Two concerns need addressing: ID prefix
matching requires `workers:view` which manage-only users may
lack, and the `worker list` output omits `current_build_id`.

**Verdict: Approve with conditions.**

---

## Blockers

None.

---

## Major Concerns

### M1 — ID prefix matching requires `workers:view`

The prefix resolution calls `GET /api/workers` which requires
`workers:view`. A user with `workers:manage` but not
`workers:view` cannot use prefix matching — the list request
returns 403 before the actual operation.

**Fix:** Document: "Prefix matching requires `workers:view`.
Users with `workers:manage` only must use full UUIDs." Or
fall back to treating the argument as a full UUID when the
list request returns 403.

### M2 — `worker list` omits `current_build_id`

The server's `WorkerInfoResponse` includes
`current_build_id: Option<i64>`. When a worker has status
`building`, the build ID is the most operationally useful
datum. The design's table omits it.

**Fix:** Add a `BUILD` column showing the build ID when
status is `building` (or `—` when idle).

### M3 — `regenerate-token` interrupts in-flight builds

`regenerate-token` calls `force_disconnect_worker` which
re-queues any active build. The design says "the worker must
be restarted" but doesn't warn that this interrupts an active
build. Operators may not expect a token rotation to abort a
running build.

**Fix:** Add a warning in the help text: "If the worker is
currently building, the build will be re-queued."

---

## Minor Issues

- **Missing error handling section.** Unlike docs 04 and 06,
  doc 05 has no explicit error handling section. Add coverage
  for: 404 (worker not found), 409 (name already exists),
  403 (missing capability).

- **`deregister` doesn't report re-queue status.** The server
  response has no field indicating whether a build was
  re-queued. Document this limitation.

- **`worker register` response has `name` and `arch` fields.**
  The design doesn't mention displaying these from the server
  response. Confirm them against the locally echoed CLI args.

- **Client-side arch validation.** The server accepts only
  `x86_64` or `aarch64`. Validate client-side before the
  round-trip for a better error message.

---

## Strengths

- "Save this — it cannot be recovered" UX for worker tokens is
  exactly right for one-time secrets.
- ID prefix resolution is fully specified with zero/multiple
  match error paths.
- Status vocabulary matches the server's `WorkerState` exactly.
- Worker name validation rules match the server's
  `is_valid_worker_name`.
