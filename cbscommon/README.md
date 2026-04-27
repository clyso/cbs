# cbscommon

[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](../COPYING-GPL3)

`cbscommon` is a shared Python library providing foundational utilities and unified modules for the Clyso Enterprise Storage Build Service (CBS) project.

## Overview

This package centralizes common logic used across various CBS components (e.g., `crt`, `cbscore`, `cbsd`), ensuring consistency in process execution, git operations, and error handling. It is designed for asynchronous environments and prioritizes security and isolation.

## Key Features

- **Asynchronous by Design**: Built on top of `asyncio` for non-blocking I/O operations.
- **Secure Process Execution**: Subprocess runners include automatic log sanitization to prevent leaking sensitive data (passwords, tokens).
- **Environment Isolation**: Support for scrubbing Python environment variables to prevent host/venv leakage into subprocesses.
- **Unified Git Interface**: A comprehensive, async-first wrapper around the `git` CLI, eliminating the need for heavyweight libraries like `GitPython`.

## Modules

### `cbscommon.git`
Provides a high-level asynchronous API for git operations.
- **Commands**: Support for `clone`, `checkout`, `fetch`, `push`, `status`, `am`, `worktree`, and more.
- **Parsing**: Replicates logic for parsing git porcelain output (e.g., push status).
- **Exceptions**: A dedicated hierarchy of git-related errors (`GitError`, `GitPushError`, etc.).

### `cbscommon.process`
A robust wrapper for `asyncio.create_subprocess_exec`.
- **Log Sanitization**: Automatically redacts `--passphrase`, `--pass`, and custom `SecureArg` values from logs.
- **Python Env Reset**: Capability to reset the `PATH` and other variables to ensure subprocesses use the intended system Python instead of the calling virtual environment.
- **Streaming**: Supports real-time log streaming via callbacks.

### `cbscommon.exceptions`
Defines the base `CBSCommonError` and other shared exception types used throughout the library to provide consistent error reporting.

## Installation

This package is intended for internal use within the CBS workspace. It can be added as a dependency in other CBS components via `uv` or standard Python packaging tools.

```toml
[tool.uv.sources]
cbscommon = { workspace = true }
```

## Development

- **Python Version**: 3.13+
- **Type Checking**: Type hints are provided and verified with `basedpyright` (via `py.typed`).
- **Coding Style**: Follows the workspace-wide `ruff` configuration.
