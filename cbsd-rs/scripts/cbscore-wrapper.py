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

"""Bridge between the cbsd-rs worker subprocess protocol and cbscore."""

import asyncio
import json
import logging
import os
import re
import sys
import tempfile
from pathlib import Path
from typing import NoReturn, cast

# Redirect stderr to stdout BEFORE any imports that may log.
# Prevents pipe deadlock (Rust side sets stderr to Stdio::null, but
# os.dup2 ensures any pre-null writes also go to stdout) and captures
# all cbscore diagnostic logging in the build log.
_ = os.dup2(1, 2)

# Set up logging so cbscore output appears in the build log.
# cbscore emits most of its useful output at DEBUG level.
logging.basicConfig(
    level=logging.DEBUG,
    format="%(levelname)s:%(name)s:%(message)s",
    stream=sys.stdout,
)
logging.getLogger("cbscore").setLevel(logging.DEBUG)


def _emit_result(
    exit_code: int,
    error: str | None,
    build_report: dict[str, object] | None = None,
) -> NoReturn:
    """Print the structured result line and exit."""
    result: dict[str, object] = {
        "type": "result",
        "exit_code": exit_code,
        "error": error,
        "build_report": build_report,
    }
    print(json.dumps(result, separators=(",", ":")), flush=True)
    sys.exit(exit_code)


def _parse_el_version(os_version: str) -> int:
    """Extract integer from 'elN' string (e.g., 'el9' -> 9)."""
    m = re.match(r"^el(\d+)$", os_version)
    if not m:
        _emit_result(2, f"[infra] invalid os_version: '{os_version}'")
    return int(m.group(1))


type JsonDict = dict[str, object]


def _as_dict(d: object) -> JsonDict:
    """Narrow an object to a JSON dict, or exit."""
    if not isinstance(d, dict):
        _emit_result(2, f"[infra] expected dict, got {type(d).__name__}")
    return cast(JsonDict, d)


def _get_str(d: JsonDict, key: str) -> str:
    """Extract a string value from a parsed JSON dict."""
    val = d.get(key)
    if val is None:
        _emit_result(2, f"[infra] missing key: {key}")
    return str(val)


def _get_dict(d: JsonDict, key: str) -> JsonDict:
    """Extract a nested dict from a parsed JSON dict."""
    val = d.get(key)
    if not isinstance(val, dict):
        _emit_result(2, f"[infra] expected dict for key '{key}'")
    return cast(JsonDict, val)


def _get_list(d: JsonDict, key: str) -> list[object]:
    """Extract a list from a parsed JSON dict."""
    val = d.get(key)
    if not isinstance(val, list):
        _emit_result(2, f"[infra] expected list for key '{key}'")
    return cast(list[object], val)


def main() -> None:
    """Read build descriptor from stdin and execute the build."""
    # CBS_TRACE_ID is already set by the Rust executor via .env().
    trace_id = os.environ.get("CBS_TRACE_ID", "unknown")

    # --- Read stdin ---
    try:
        raw = cast(object, json.load(sys.stdin))
    except json.JSONDecodeError as e:
        _emit_result(2, f"[infra] failed to parse stdin: {e}")

    input_data = _as_dict(raw)
    descriptor = _get_dict(input_data, "descriptor")
    component_path = _get_str(input_data, "component_path")
    trace_id_val = input_data.get("trace_id")
    if isinstance(trace_id_val, str):
        trace_id = trace_id_val

    version = _get_str(descriptor, "version")
    print(
        f"cbscore-wrapper: starting build {version} trace_id={trace_id}",
        flush=True,
    )

    # --- Validate dst_image.tag ---
    dst_image = _get_dict(descriptor, "dst_image")
    dst_tag = _get_str(dst_image, "tag")
    if not dst_tag:
        _emit_result(2, "[infra] descriptor dst_image.tag is empty")

    # --- Load cbscore config ---
    config_path_str = os.environ.get("CBSCORE_CONFIG")
    if not config_path_str:
        # Fallback for manual testing only.
        config_path_str = "cbs-build.config.yaml"

    try:
        from cbscore.config import Config, ConfigError
    except ImportError as e:
        _emit_result(2, f"[infra] cbscore not installed: {e}")

    try:
        config = Config.load(Path(config_path_str))
    except ConfigError as e:
        _emit_result(2, f"[infra] config error: {e}")
    except Exception as e:
        _emit_result(2, f"[infra] failed to load config: {e}")

    if not config.storage or not config.storage.registry:
        _emit_result(2, "[infra] registry not configured in cbscore config")

    # --- Override components path ---
    config.paths.components = [Path(component_path)]

    # --- Parse os_version ---
    build_section = _get_dict(descriptor, "build")
    el_version = _parse_el_version(_get_str(build_section, "os_version"))

    # --- Create VersionDescriptor ---
    components_list = _get_list(descriptor, "components")
    signed_off = _get_dict(descriptor, "signed_off_by")

    try:
        from cbscore.versions.create import version_create_helper
    except ImportError as e:
        _emit_result(2, f"[infra] cbscore not installed: {e}")

    version_desc = version_create_helper(
        version=_get_str(descriptor, "version"),
        version_type_name=_get_str(descriptor, "version_type"),
        component_refs={
            _get_str(_as_dict(c), "name"): _get_str(_as_dict(c), "ref")
            for c in components_list
        },
        components_paths=config.paths.components,
        component_uri_overrides={
            _get_str(_as_dict(c), "name"): _get_str(_as_dict(c), "repo")
            for c in components_list
            if _as_dict(c).get("repo") is not None
        },
        distro=_get_str(build_section, "distro"),
        el_version=el_version,
        registry=config.storage.registry.url,
        image_name=_get_str(dst_image, "name"),
        image_tag=_get_str(dst_image, "tag"),
        user_name=_get_str(signed_off, "user"),
        user_email=_get_str(signed_off, "email"),
    )

    # --- Write descriptor to temp file ---
    fd, temp_path_str = tempfile.mkstemp(prefix="cbsd-wrapper-", suffix=".json")
    temp_path = Path(temp_path_str)
    try:
        with os.fdopen(fd, "w") as f:
            _ = f.write(version_desc.model_dump_json())

        # --- Resolve cbscore_path (package root with pyproject.toml) ---
        cbscore_path_str = os.environ.get("CBSCORE_PATH")
        if cbscore_path_str:
            cbscore_path = Path(cbscore_path_str)
        else:
            import cbscore as _cbscore_pkg

            candidate = Path(_cbscore_pkg.__file__).parent
            found: Path | None = None
            for _ in range(6):
                if (candidate / "pyproject.toml").exists():
                    found = candidate
                    break
                candidate = candidate.parent
            if found is None:
                _emit_result(
                    2,
                    "[infra] cannot locate cbscore package root"
                    + " (no pyproject.toml found in parent dirs)",
                )
            cbscore_path = found

        # --- Verify entrypoint (inner package dir, not cbscore_path) ---
        # runner.py derives the entrypoint from Path(__file__).parent,
        # independent of cbscore_path. Mirror that here.
        import cbscore as _cbscore_ep

        entrypoint = (
            Path(_cbscore_ep.__file__).parent / "_tools" / "cbscore-entrypoint.sh"
        )
        if not entrypoint.exists():
            _emit_result(
                2,
                f"[infra] entrypoint not found: {entrypoint}",
            )

        # --- Run the build ---
        timeout = int(os.environ.get("CBS_BUILD_TIMEOUT", "7200"))
        run_name = f"cbs-{trace_id.replace('-', '')[:12]}"

        async def log_cb(msg: str) -> None:
            print(msg, end="", flush=True)

        from cbscore.errors import MalformedVersionError
        from cbscore.runner import RunnerError, runner
        from cbscore.versions.errors import VersionError

        try:
            report = asyncio.run(
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
            report_dict: dict[str, object] | None = (
                report.model_dump(mode="json") if report else None
            )
            _emit_result(0, None, build_report=report_dict)
        except (RunnerError, VersionError, MalformedVersionError) as e:
            _emit_result(1, str(e))
        except Exception as e:
            _emit_result(2, f"[infra] {e}")
    finally:
        temp_path.unlink(missing_ok=True)


if __name__ == "__main__":
    main()
