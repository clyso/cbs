# Design Review: 05 — Worker Administration (v2)

**Verdict: Approved.**

All v1 concerns resolved: `current_build_id` column added
to `worker list`, prefix matching `workers:view` requirement
documented with 403 fallback, `regenerate-token` warning about
in-flight build re-queuing, error handling section added with
all cases (404, 409, 403).

No blockers. No major concerns.

## Minor Issues

- **Client-side arch validation error message.** The design
  says "client-side arch validation before the round-trip"
  but doesn't specify the message text. Define it to avoid
  drift from the server message.

- **Prefix matching fallback prose.** "Falls back to treating
  the argument as a full UUID" could be cleaner: "If the
  list request returns 403, proceed with the argument as-is
  (assumed full UUID)."

## Strengths

- "Save this — it cannot be recovered" UX for worker tokens.
- `BUILD` column in list output shows `current_build_id`.
- Prefix matching fully specified with fallback.
- Error handling section covers all cases.
- `regenerate-token` warning about in-flight builds.
