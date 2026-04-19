# CLAUDE.md — cbsd-rs/docs

All Markdown files under this directory must observe a hard line-wrap at
79 characters (80 chars including the newline). This applies to prose,
list items, table cells, and code block captions — but not to code
fences, URLs, or table separator rows, which must not be broken.

When creating or editing any `.md` file here, enforce formatting with
`prettier` using the repository's config at `.prettierrc.json` (repo
root):

```bash
prettier --write path/to/file.md
```

If `prettier` is not available on PATH, **DO NOT perform manual line
wrapping**. Instead, stop and alert the user with a clearly visible
message:

> **WARNING: prettier not found. Markdown formatting was NOT applied to
> [file]. Please install prettier and run `prettier --write [file]`
> before committing.**

Do not attempt manual line-wrapping as a fallback.
