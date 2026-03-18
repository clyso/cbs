#!/usr/bin/env python3
# Copyright (C) 2026  Clyso
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU Affero General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU Affero General Public License for more details.

"""CBS core build wrapper for cbsd-rs worker.

Bridge between the Rust worker subprocess interface (stdin JSON, stdout
log lines, structured result on last line) and cbscore's async runner.

See: _docs/cbsd-rs/design/2026-03-18-cbscore-wrapper.md
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import sys
import tempfile
from pathlib import Path

# Redirect stderr to stdout BEFORE any imports that may log.
# Prevents pipe deadlock (Rust side sets stderr to Stdio::null, but
# os.dup2 ensures any pre-null writes also go to stdout) and captures
# all cbscore diagnostic logging in the build log.
os.dup2(1, 2)


def _emit_result(exit_code: int, error: str | None) -> None:
    """Print the structured result line and exit."""
    result = {"type": "result", "exit_code": exit_code, "error": error}
    print(json.dumps(result, separators=(",", ":")), flush=True)
    sys.exit(exit_code)


def _parse_el_version(os_version: str) -> int:
    """Extract integer from 'elN' string (e.g., 'el9' → 9)."""
    m = re.match(r"^el(\d+)$", os_version)
    if not m:
        _emit_result(2, f"[infra] invalid os_version: '{os_version}'")
    return int(m.group(1))  # type: ignore[union-attr]


def main() -> None:
    """Read build descriptor from stdin and execute the build."""
    # CBS_TRACE_ID is already set by the Rust executor via .env().
    trace_id = os.environ.get("CBS_TRACE_ID", "unknown")

    # --- Read stdin ---
    try:
        input_data = json.load(sys.stdin)
        descriptor = input_data["descriptor"]
        component_path = input_data["component_path"]
        trace_id = input_data.get("trace_id", trace_id)
    except (json.JSONDecodeError, KeyError) as e:
        _emit_result(2, f"[infra] failed to parse stdin: {e}")

    version = descriptor.get("version", "unknown")
    print(
        f"cbscore-wrapper: starting build {version} trace_id={trace_id}",
        flush=True,
    )

    # --- Validate dst_image.tag ---
    dst_tag = descriptor.get("dst_image", {}).get("tag", "")
    if not dst_tag:
        _emit_result(2, "[infra] descriptor dst_image.tag is empty")

    # --- Load cbscore config ---
    config_path_str = os.environ.get("CBSCORE_CONFIG")
    if not config_path_str:
        # Fallback for manual testing only.
        config_path_str = "cbs-build.config.yaml"

    try:
        from cbscore.config import Config, ConfigError

        config = Config.load(Path(config_path_str))
    except ImportError as e:
        _emit_result(2, f"[infra] cbscore not installed: {e}")
    except ConfigError as e:
        _emit_result(2, f"[infra] config error: {e}")
    except Exception as e:
        _emit_result(2, f"[infra] failed to load config: {e}")

    if not config.storage or not config.storage.registry:
        _emit_result(2, "[infra] registry not configured in cbscore config")

    # --- Override components path ---
    config.paths.components = [Path(component_path)]

    # --- Parse os_version ---
    el_version = _parse_el_version(descriptor["build"]["os_version"])

    # --- Create VersionDescriptor ---
    try:
        from cbscore.versions.create import version_create_helper

        version_desc = version_create_helper(
            version=descriptor["version"],
            version_type_name=descriptor["version_type"],
            component_refs={
                c["name"]: c["ref"]  # JSON key is "ref" (Rust serde rename)
                for c in descriptor["components"]
            },
            components_paths=config.paths.components,
            component_uri_overrides={
                c["name"]: c["repo"]
                for c in descriptor["components"]
                if c.get("repo") is not None
            },
            distro=descriptor["build"]["distro"],
            el_version=el_version,
            registry=config.storage.registry.url,
            image_name=descriptor["dst_image"]["name"],
            image_tag=descriptor["dst_image"]["tag"],
            user_name=descriptor["signed_off_by"]["user"],
            user_email=descriptor["signed_off_by"]["email"],
        )
    except ImportError as e:
        _emit_result(2, f"[infra] cbscore not installed: {e}")

    # --- Write descriptor to temp file ---
    fd, temp_path_str = tempfile.mkstemp(prefix="cbsd-wrapper-", suffix=".json")
    temp_path = Path(temp_path_str)
    try:
        with os.fdopen(fd, "w") as f:
            f.write(version_desc.model_dump_json())

        # --- Resolve cbscore_path ---
        cbscore_path_str = os.environ.get("CBSCORE_PATH")
        if cbscore_path_str:
            cbscore_path = Path(cbscore_path_str)
        else:
            import cbscore

            cbscore_path = Path(cbscore.__file__).parent

        entrypoint = cbscore_path / "_tools" / "cbscore-entrypoint.sh"
        if not entrypoint.exists():
            _emit_result(
                2,
                f"[infra] entrypoint not found: {entrypoint}",
            )

        # --- Run the build ---
        timeout = int(os.environ.get("CBS_BUILD_TIMEOUT", "7200"))
        run_name = f"cbs-{trace_id.replace('-', '')[:12]}"

        async def log_cb(msg: str) -> None:
            # runner() normalizes lines with trailing \n — use end=""
            # to avoid double newlines.
            print(msg, end="", flush=True)

        from cbscore.errors import MalformedVersionError
        from cbscore.runner import RunnerError, runner
        from cbscore.versions.errors import VersionError

        try:
            asyncio.run(
                runner(
                    desc_file_path=temp_path,
                    cbscore_path=cbscore_path,
                    config=config,
                    run_name=run_name,
                    replace_run=True,
                    timeout=timeout,
                    log_out_cb=log_cb,
                )
            )
            _emit_result(0, None)
        except (RunnerError, VersionError, MalformedVersionError) as e:
            _emit_result(1, str(e))
        except Exception as e:
            _emit_result(2, f"[infra] {e}")
    finally:
        temp_path.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
