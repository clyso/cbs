-- Build artifact report: stores the structured JSON report produced by
-- cbscore after a successful build.  NULL for builds that predate this
-- column, for failed/revoked builds, and for workers that have not been
-- upgraded.
ALTER TABLE builds ADD COLUMN build_report TEXT;
