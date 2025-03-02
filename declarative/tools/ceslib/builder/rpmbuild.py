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


def _get_component_scripts_path(
    components_path: Path, component_name: str
) -> Path | None:
    comp_path = components_path.joinpath(component_name)
    if not comp_path.exists():
        log.warning(
            f"component path for '{component_name}' "
            + f"not found in '{components_path}'"
        )
        return None

    comp_scripts_path = comp_path.joinpath("scripts")
    if not comp_scripts_path.exists():
        log.warning(
            f"component scripts path for '{component_name}' "
            + f"not found in '{comp_path}'"
        )
        return None

    return comp_scripts_path


def _get_script_path(scripts_path: Path, glob: str) -> Path | None:
    candidates = list(scripts_path.glob(glob))
    if len(candidates) != 1:
        log.error(
            f"found '{len(candidates)}' candidate build scripts in "
            + f"'{scripts_path}' for glob '{glob}', needs 1"
        )
        return None

    script_path = candidates[0]
    if not script_path.is_file() or not script_path.stat().st_mode & stat.S_IXUSR:
        log.error(f"script at '{script_path}' either not a file or not executable")
        return None
    return script_path


async def _get_component_version(
    component_name: str, component_scripts_path: Path, repo_path: Path
) -> str | None:
    version_script_path = _get_script_path(component_scripts_path, "get_version.*")
    if not version_script_path:
        log.error(
            f"unable to find 'get_version' script for component '{component_name}'"
        )
        return None

    cmd = [
        version_script_path.resolve().as_posix(),
    ]

    try:
        rc, stdout, stderr = await async_run_cmd(cmd, cwd=repo_path)
    except CommandError as e:
        msg = f"error running version script for '{component_name}': {e}"
        log.error(msg)
        raise BuilderError(msg)
    except Exception as e:
        msg = f"unknown exception running version script for '{component_name}: {e}"
        log.error(msg)
        raise BuilderError(msg)

    if rc != 0:
        msg = f"error running version script for '{component_name}': {stderr}"
        log.error(msg)
        raise BuilderError(msg)

    return stdout.strip()


def _get_component_build_script(
    component_name: str, component_scripts_path: Path
) -> Path | None:
    build_script_path = _get_script_path(component_scripts_path, "build_rpms.*")
    if not build_script_path:
        log.error(f"unable to find build script for component '{component_name}'")
        return None

    return build_script_path


def _setup_rpm_topdir(
    rpms_path: Path, component_name: str, version: str | None
) -> Path:
    comp_rpm_path = rpms_path.joinpath(component_name).resolve()
    if version:
        comp_rpm_path = comp_rpm_path.joinpath(version).resolve()

    comp_rpm_path.mkdir(exist_ok=True, parents=True)

    for d in ["BUILD", "SOURCES", "RPMS", "SRPMS", "SPECS"]:
        comp_rpm_path.joinpath(d).mkdir(exist_ok=True)

    return comp_rpm_path


# build a given component, by running the script provided by 'script_path' in the
# repository 'repo_path'.
# returns the number of seconds the script took to execute.
async def _build_component(
    rpms_path: Path,
    el_version: int,
    comp_name: str,
    script_path: Path,
    repo_path: Path,
    version: str | None,
    *,
    ccache_path: Path | None = None,
    skip_build: bool = False,
) -> tuple[int, Path]:
    log.info(f"build component {comp_name} in '{repo_path}' using '{script_path}'")

    def _outcb(s: str) -> None:
        log.debug(s)

    comp_rpms_path = _setup_rpm_topdir(rpms_path, comp_name, version)

    if skip_build:
        return 1, comp_rpms_path

    dist_version = f".el{el_version}.clyso"
    cmd = [
        script_path.resolve().as_posix(),
        repo_path.resolve().as_posix(),
        dist_version,
        comp_rpms_path.resolve().as_posix(),
    ]

    if version:
        cmd.append(version)

    extra_env: dict[str, str] | None = None
    if ccache_path is not None:
        extra_env = {"CES_CCACHE_PATH": ccache_path.resolve().as_posix()}

    start = dt.now()
    try:
        rc, _, _ = await async_run_cmd(
            cmd, outcb=_outcb, cwd=repo_path, reset_python_env=True, extra_env=extra_env
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

    return delta, comp_rpms_path


# build RPMs for the various components provided in 'components'.
# relies on a 'build_rpms.sh' script that must be found in the
# 'components_path' directory, for each specific component.
async def build_rpms(
    rpms_path: Path,
    el_version: int,
    components_path: Path,
    components: dict[str, Path],
    *,
    ccache_path: Path | None = None,
    skip_build: bool = False,
) -> dict[str, Path]:
    if not components_path.exists():
        raise BuilderError(f"components path at '{components_path}' not found")

    class _ComponentBuild:
        build_script: Path
        version: str | None

        def __init__(self, build_script: Path, version: str | None) -> None:
            self.build_script = build_script
            self.version = version

    to_build: dict[str, _ComponentBuild] = {}
    for comp_name, comp_repo in components.items():
        comp_path = components_path.joinpath(comp_name)
        if not comp_path.exists():
            log.warning(
                f"component path for '{comp_name}' "
                + f"not found in '{components_path}'"
            )
            continue

        comp_scripts_path = _get_component_scripts_path(components_path, comp_name)
        if not comp_scripts_path:
            log.warning(
                f"component scripts path for '{comp_name}' "
                + f"not found in '{comp_path}'"
            )
            continue

        build_script_path = _get_component_build_script(comp_name, comp_scripts_path)
        if not build_script_path:
            log.warning(f"build script not found for '{comp_name}'")
            continue

        try:
            comp_version = await _get_component_version(
                comp_name, comp_scripts_path, comp_repo
            )
        except BuilderError as e:
            msg = f"error building RPMs for '{comp_name}', unable to find version: {e}"
            log.error(msg)
            raise BuilderError(msg)

        to_build[comp_name] = _ComponentBuild(build_script_path, comp_version)

    try:
        async with asyncio.TaskGroup() as tg:
            tasks = {
                name: tg.create_task(
                    _build_component(
                        rpms_path,
                        el_version,
                        name,
                        to_build[name].build_script,
                        components[name],
                        to_build[name].version,
                        ccache_path=ccache_path,
                        skip_build=skip_build,
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
            for exc in e.exceptions:
                log.error(f"- {exc}")

        raise BuilderError("error building component RPMs")

    comps_rpms_paths: dict[str, Path] = {}
    for name, task in tasks.items():
        time_spent, comp_rpms_path = task.result()
        log.info(f"built component '{name}' in {time_spent} seconds")
        comps_rpms_paths[name] = comp_rpms_path

    return comps_rpms_paths
