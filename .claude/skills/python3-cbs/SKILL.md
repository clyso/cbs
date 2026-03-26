---
name: python3-cbs
description: >
  Use this skill whenever working on a Python 3.13+ monorepo that uses uv workspaces, ruff for
  formatting/linting, and basedpyright for type checking. Trigger this skill when: writing new
  Python code, editing existing Python files, fixing lint or type errors, running ruff or
  basedpyright, interpreting tool output, adding dependencies, writing tests, writing docstrings,
  handling exceptions, or any time code quality tools are mentioned. Also trigger when the user
  says things like "check types", "fix lint", "run ruff", "type errors", "pyright errors",
  "make this pass CI", "add a dependency", "write a test", "async or sync?", or similar.
  This skill covers config resolution across root and package levels, correct tool invocation
  via uv, strict typing rules, async awareness, exception conventions, logging, imports,
  dependency management, and docstring style.
---

# Python3 CBS Skill

## Related Skills

This skill works in conjunction with **`git-commit-messages`**, which must also be consulted whenever working toward a commit. That skill governs logical change boundaries, atomic commit discipline, and commit message format. Read it before staging or committing anything.

---

## Monorepo Structure

This is a **uv workspace** monorepo:

- `uv.lock` and root `pyproject.toml` live at the repository root
- Individual packages live in subdirectories, each with their own `pyproject.toml` (optional)
- Workspace-wide dev tools (ruff, basedpyright, etc.) are managed at the root
- Per-package prod deps and package-specific dev deps (e.g. pytest) are managed per-package
- Lefthook pre-commit hooks are configured in `.lefthook.yaml` at the root
- Python version: **3.13+** — use modern syntax freely

---

## Config Resolution

Configuration is **layered**: root is the baseline, package-level files override specific settings.

### ruff
1. Check for `[tool.ruff]` in the **package's** `pyproject.toml` first
2. Fall back to root `pyproject.toml` for anything not overridden
3. Always defer to configured values — never assume rule sets or line lengths

### basedpyright
1. Check for `pyrightconfig.json` in the **package directory** first
2. Fall back to root `pyrightconfig.json`
3. **Always defer to the config** — never assume or impose a strictness mode
4. If no config exists anywhere, tell the user rather than guessing

```bash
# Quickly find all config files (run from repo root)
find . -name "pyrightconfig.json" -o -name "pyproject.toml" | grep -v ".venv" | sort
```

---

## Running the Tools

Prefer `uv run`; fall back to direct CLI invocation if the tool isn't found.

```bash
# Lint
uv run ruff check path/to/file.py

# Format
uv run ruff format path/to/file.py

# Lint + auto-fix
uv run ruff check --fix path/to/file.py

# Type check
uv run basedpyright path/to/file.py
```

**Scope**: always target **only the files being actively edited**. Pass specific file paths, not directories, unless the user explicitly asks for broader scope.

---

## Type Safety Rules

All code must be **fully typed**. This is non-negotiable.

- Every function parameter, return type, and class attribute must have a type annotation
- Use modern union syntax: `str | None`, not `Optional[str]`
- Prefer `type` statement (3.12+), `typing.TypeAlias`, `typing.TypeVar`, `typing.ParamSpec` as appropriate
- `Any` must be justified — if used, add a comment explaining why

### Suppressing type errors

Type errors may only be suppressed using **basedpyright's ignore syntax**:

```python
x = some_untyped_call()  # pyright: ignore[reportUnknownVariableType]
```

**Rules around ignores — strictly enforced:**
- `# type: ignore` (mypy style) is **not allowed** — use `# pyright: ignore[ruleCode]` only
- Every ignore must include the specific rule code — bare `# pyright: ignore` is not acceptable
- **Every new ignore must be explicitly reported to the user with a written justification**
- No code containing a new `# pyright: ignore` will be committed without **prior user approval**
- When proposing an ignore, always first exhaust alternatives: proper typing, `cast()`, overloads, `TypeGuard`, `TypeNarrow`, `@overload`, protocol types, etc.

---

## Fix vs. Report Behavior

| Situation | Action |
|---|---|
| Error is in a file **being actively worked on** | Fix it, explain what changed and why |
| Error is in a **different file** (found incidentally) | Report it, ask permission before touching it |
| Auto-fixable ruff issue | Apply `--fix`, summarize changes |
| Type error requiring logic changes | Explain, propose fix, apply after confirmation |
| Fix requires adding a `# pyright: ignore` | **Always ask for approval first**, justify the ignore |

---

## Async Awareness

Before writing any non-trivial function, check whether the package or file uses asyncio:

```bash
# Check for async usage in a package
grep -r "async def\|asyncio\|await" packages/mypkg/src/ --include="*.py" -l
```

**Rules:**
- If a package or file uses asyncio, prefer `async def` for new I/O-bound or complex functions
- Do not add a purely synchronous implementation when an async one is clearly more appropriate
- For CPU-bound work, sync is fine — use `asyncio.to_thread()` if it needs to be called from async context
- Never mix blocking I/O (`open`, `requests`, `time.sleep`) into async functions — use async equivalents (`aiofiles`, `httpx`, `asyncio.sleep`, etc.)
- When unsure, ask the user before committing to sync vs async

```python
# Concurrent tasks
results = await asyncio.gather(fetch_a(), fetch_b())

# Blocking work from async context
result = await asyncio.to_thread(cpu_bound_fn, arg)

# Background task
task = asyncio.create_task(some_coroutine())
```

---

## Exception Handling

### When to use stdlib vs custom exceptions
- Use **stdlib exceptions** (`ValueError`, `TypeError`, `KeyError`, `FileNotFoundError`, etc.) when they are semantically accurate and sufficient
- Use **custom exceptions** for everything else — especially domain-specific errors, module-aware context, or when callers need to catch your errors specifically

### Finding the right base exception
Before defining a new exception class:
1. Search the **current package** for an existing base exception (e.g. `class MyPackageError(Exception)`)
2. Search **shared packages** for a base or derived exception that fits the context
3. Only define a new base if none exists — and if so, create a package-level base first

### Custom exception conventions
```python
# Package base exception
class MyPackageError(Exception):
    """Base exception for mypackage."""

# Derived, context-specific exceptions
class ConfigValidationError(MyPackageError):
    """Raised when configuration fails validation."""

    def __init__(self, field: str, reason: str) -> None:
        self.field = field
        self.reason = reason
        super().__init__(f"Config field '{field}' invalid: {reason}")
```

### Exception chaining
Always use `raise X from Y` when re-raising to preserve the original cause:

```python
try:
    raw = json.loads(data)
except json.JSONDecodeError as exc:
    raise ConfigValidationError("config", "invalid JSON") from exc
```

- Never silently swallow exceptions — at minimum log them before discarding
- Avoid bare `except:` or `except Exception:` without re-raising or logging

---

## Logging

Use **stdlib `logging`** only. No third-party logging libraries.

Loggers follow a strict **hierarchical inheritance pattern** — never use `logging.getLogger(__name__)` in module files.

### Package `__init__.py` — root of the logger tree
Each package defines its top-level logger in `__init__.py`:

```python
# packages/mypkg/src/mypkg/__init__.py
import logging

logger = logging.getLogger("mypkg")  # Named explicitly, not __name__
```

### Sub-module `__init__.py` — inherit and branch
A sub-module's `__init__.py` imports the parent logger and creates a named child:

```python
# packages/mypkg/src/mypkg/users/__init__.py
from mypkg import logger as parent_logger

logger = parent_logger.getChild("users")
# Effective logger name: "mypkg.users"
```

### Module files — same pattern, one level deeper
Every `.py` file imports its parent module's logger and calls `getChild` with a descriptive name (not necessarily `__name__`):

```python
# packages/mypkg/src/mypkg/users/builder.py
from mypkg.users import logger as parent_logger

logger = parent_logger.getChild("builder")
# Effective logger name: "mypkg.users.builder"
```

The child name should describe the file's role or responsibility — `"builder"`, `"parser"`, `"client"`, `"handler"` — not mechanically mirror the filename if a clearer name exists.

### Using the logger
```python
logger.debug("Trace detail: %s", detail)
logger.info("User %s logged in", user_id)
logger.warning("Deprecated path used: %s", path)
logger.error("Failed to connect: %s", err)
logger.exception("Unexpected error")  # Inside except blocks — captures traceback automatically
```

**Rules:**
- Never `logging.getLogger(__name__)` in module files — always inherit via `getChild`
- Never call `logging.info(...)` / `logging.error(...)` directly — that writes to the root logger
- Use `%s`-style formatting in log calls, not f-strings — avoids formatting cost when the level is disabled
- Use `logger.exception(...)` inside `except` blocks — it includes the full traceback automatically
- Never use `print()` for diagnostic output in library/package code
- When in doubt about which parent logger to import, trace the package's `__init__.py` chain upward

---

## Imports

- **Absolute imports everywhere** — no relative imports (`from .module import X` is not used)
- Group order: stdlib → third-party → internal, separated by blank lines (ruff `I` rules enforce this)
- No wildcard imports (`from x import *`)
- No `__all__` declarations

```python
import asyncio
import logging
from pathlib import Path

import httpx

from mypackage.exceptions import UserNotFoundError
from mypackage.models import User
```

---

## Dependency Management

```bash
# Add a production dependency to a specific package
uv add httpx --package mypkg

# Add a package-level dev dependency (e.g. testing tools)
uv add pytest --dev --package mypkg

# Add a workspace-wide dev tool (run from repo root, no --package flag)
uv add basedpyright ruff --dev
```

**Rules:**
- Typechecking and linting tools (ruff, basedpyright) are **always** workspace-wide root deps — never per-package
- Always use `--package <name>` when adding per-package deps from the repo root
- Check the existing `pyproject.toml` before adding — the dep may already be declared

---

## Docstrings

Use **Google style**. Since all code is fully typed, do not repeat types in docstrings — the signature is the source of truth.

```python
def fetch_user(user_id: int, active_only: bool = True) -> User:
    """Fetch a user by ID from the database.

    Args:
        user_id: The unique identifier of the user.
        active_only: If True, raises an error for inactive users.

    Returns:
        The matching User object.

    Raises:
        UserNotFoundError: If no user exists with the given ID.
    """
```

- Module-level: brief one-liner describing the module's purpose
- Class docstrings: describe the class; document `__init__` args in the class docstring if non-trivial
- Omit trivial docstrings on obvious one-liners — they add noise, not value

---

## Testing

- Framework: **pytest**, tests in `tests/` within each package
- No async test convention established yet — before writing async tests, check the package's existing test files and `[tool.pytest.ini_options]` in its `pyproject.toml`; ask the user if unclear

```bash
# Run a package's tests
uv run pytest packages/mypkg/tests/

# Run a specific file
uv run pytest packages/mypkg/tests/test_users.py -v
```

---

## Modern Python 3.13+ Patterns

Write idiomatic modern Python from the start:

```python
# Union types — never Optional[x]
def process(value: str | None) -> int | None: ...

# Type aliases (3.12+)
type Vector = list[float]
type Callback[T] = Callable[[T], Awaitable[None]]

# Generic classes (3.12+)
class Repository[T]:
    async def get(self, id: int) -> T: ...

# Pattern matching
match event:
    case {"type": "login", "user_id": int(uid)}:
        await handle_login(uid)
    case {"type": "logout"}:
        await handle_logout()

# tomllib is stdlib (3.11+)
import tomllib
with open("pyproject.toml", "rb") as f:
    config = tomllib.load(f)

# Exception groups (3.11+)
try:
    async with asyncio.TaskGroup() as tg:
        tg.create_task(fetch_a())
        tg.create_task(fetch_b())
except* ValueError as eg:
    for exc in eg.exceptions:
        logger.error("Value error: %s", exc)
```

---

## Pre-Commit Checks

> **Before preparing any commit, read the `git-commit-messages` skill.** It defines how to identify logical change boundaries, how to split work into atomic commits, and the required commit message format. The checks below are the Python-specific gate that must pass before those commits are made.

Before committing any change, **all three checks must pass** on the modified files, in this order:

### 1. Format (ruff format)
Always run formatting first — it may change code that linting then checks.

```bash
uv run ruff format path/to/file.py
```

Formatting is governed by `pyproject.toml` — never assume defaults are in effect. Always let the config drive formatting behaviour, even if it appears to match ruff's defaults.

### 2. Lint (ruff check)
```bash
uv run ruff check path/to/file.py
# Auto-fix where possible:
uv run ruff check --fix path/to/file.py
```

### 3. Type check (basedpyright)
```bash
uv run basedpyright path/to/file.py
```

### All three, in one go
```bash
FILE=path/to/file.py
uv run ruff format "$FILE" && uv run ruff check --fix "$FILE" && uv run basedpyright "$FILE"
```

**Rules:**
- All three must pass with zero errors before a change is considered commit-ready
- If any check fails, fix it before proceeding — do not commit with known failures
- Run checks on **all files touched** by the change, not just the primary file
- If `# pyright: ignore` was added anywhere in the change, **obtain user approval before committing** (see Type Safety Rules)

---

## Quick Reference

```bash
# Lint / format / fix / type-check
uv run ruff check path/to/file.py
uv run ruff format path/to/file.py
uv run ruff check --fix path/to/file.py
uv run basedpyright path/to/file.py

# Dependencies
uv add httpx --package mypkg                  # prod dep
uv add pytest --dev --package mypkg           # package dev dep
uv add ruff basedpyright --dev                # workspace-wide dev tool (from root)

# Tests
uv run pytest packages/mypkg/tests/

# Check async usage in a package
grep -r "async def\|asyncio\|await" packages/mypkg/src/ --include="*.py" -l

# Find all config files
find . -name "pyrightconfig.json" -o -name "pyproject.toml" | grep -v ".venv" | sort
```
