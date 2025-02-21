# CES library - CES builder
# Copyright (C) 2025  Clyso GmbH
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.


import asyncio
import stat
from datetime import datetime as dt
from pathlib import Path

from ceslib.builder import BuilderError
from ceslib.builder import log as parent_logger
from ceslib.utils import CommandError, async_run_cmd

log = parent_logger.getChild("rpmbuild")


def _setup_rpm_topdir(rpms_path: Path, component_name: str) -> Path:
    comp_rpm_path = rpms_path.joinpath(component_name).resolve()
    comp_rpm_path.mkdir(exist_ok=True)

    for d in ["BUILD", "SOURCES", "RPMS", "SRPMS", "SPECS"]:
        comp_rpm_path.joinpath(d).mkdir(exist_ok=True)

    return comp_rpm_path


# build a given component, by running the script provided by 'script_path' in the
# repository 'repo_path'.
# returns the number of seconds the script took to execute.
async def _build_component(
    rpms_path: Path, el_version: int, comp_name: str, script_path: Path, repo_path: Path
) -> int:
    log.info(f"build component {comp_name} in '{repo_path}' using '{script_path}'")

    def _outcb(s: str) -> None:
        log.debug(s)

    comp_rpms_path = _setup_rpm_topdir(rpms_path, comp_name)

    dist_version = f".el{el_version}.clyso"
    cmd = [
        script_path.resolve().as_posix(),
        repo_path.resolve().as_posix(),
        dist_version,
        comp_rpms_path.resolve().as_posix(),
    ]

    start = dt.now()
    try:
        rc, _, _ = await async_run_cmd(
            cmd, outcb=_outcb, cwd=repo_path, reset_python_env=True
        )
    except CommandError as e:
        msg = (
            f"error running build script for '{comp_name}' "
            + f"with '{script_path}': {e}"
        )
        log.error(msg)
        raise BuilderError(msg)
    except Exception as e:
        msg = (
            f"unknown error running build script for '{comp_name}' "
            + f"with '{script_path}': {e}"
        )
        log.error(msg)
        raise BuilderError(msg)
    delta = (dt.now() - start).seconds

    if rc != 0:
        log.error(f"error running build script for '{comp_name}'")
        raise BuilderError(f"error running build script for '{comp_name}'")

    return delta


# build RPMs for the various components provided in 'components'.
# relies on a 'build_rpms.sh' script that must be found in the
# 'components_path' directory, for each specific component.
async def build_rpms(
    rpms_path: Path, el_version: int, components_path: Path, components: dict[str, Path]
) -> None:
    if not components_path.exists():
        raise BuilderError(f"components path at '{components_path}' not found")

    to_build: dict[str, Path] = {}
    for comp_name in components.keys():
        comp_path = components_path.joinpath(comp_name)
        if not comp_path.exists():
            log.warning(
                f"component path for '{comp_name}' "
                + f"not found in '{components_path}'"
            )
            continue

        comp_scripts_path = comp_path.joinpath("scripts")
        if not comp_scripts_path.exists():
            log.warning(
                f"component scripts path for '{comp_name}' "
                + f"not found in '{comp_path}'"
            )
            continue

        candidates = list(comp_scripts_path.glob("build_rpms.*"))
        if len(candidates) != 1:
            log.error(
                f"found '{len(candidates)}' candidate build scripts in '{comp_scripts_path}', needs 1"
            )
            continue
        build_script_path = candidates[0]

        if (
            not build_script_path.is_file()
            or not build_script_path.stat().st_mode & stat.S_IXUSR
        ):
            log.error(
                f"build script for component '{comp_name}' "
                + f"at '{build_script_path}' is not a file or not executable"
            )
            continue

        to_build[comp_name] = build_script_path

    try:
        async with asyncio.TaskGroup() as tg:
            tasks = {
                name: tg.create_task(
                    _build_component(
                        rpms_path, el_version, name, to_build[name], components[name]
                    )
                )
                for name in to_build.keys()
            }
    except ExceptionGroup as e:
        excs = e.subgroup(BuilderError)
        if excs is not None:
            log.error("error building component RPMs:")
            for exc in excs.exceptions:
                log.error(f"- {exc}")
        else:
            log.error(f"unexpected error building component RPMs: {e}")

        raise BuilderError("error building component RPMs")

    for name, task in tasks.items():
        log.info(f"built component '{name}' in {task.result()} seconds")
