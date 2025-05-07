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
import datetime
from datetime import datetime as dt
from pathlib import Path

from ceslib.builder import BuilderError
from ceslib.builder import log as parent_logger
from ceslib.builder.prepare import BuildComponentInfo
from ceslib.utils import CmdArgs, CommandError, async_run_cmd
from ceslib.utils.paths import get_component_scripts_path, get_script_path

log = parent_logger.getChild("rpmbuild")


class ComponentBuild:
    version: str
    rpms_path: Path

    def __init__(self, version: str, rpms_path: Path) -> None:
        self.version = version
        self.rpms_path = rpms_path


def _get_component_build_script(
    component_name: str, component_scripts_path: Path
) -> Path | None:
    build_script_path = get_script_path(component_scripts_path, "build_rpms.*")
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


async def _build_component(
    rpms_path: Path,
    el_version: int,
    comp_name: str,
    script_path: Path,
    repo_path: Path,
    version: str,
    *,
    ccache_path: Path | None = None,
    skip_build: bool = False,
) -> tuple[int, Path]:
    """
    Build a given component.

    Build is performed by running the script provided by `script_path` in the
    repository's `repo_path`.

    Returns the number of seconds the script took to execute, and a `Path` to
    `rpmbuild`'s topdir, where the RPMs will be located.
    """
    mlog = log.getChild(f"comp[{comp_name}]")
    mlog.info(f"build component {comp_name} in '{repo_path}' using '{script_path}'")

    def _outcb(s: str) -> None:
        mlog.debug(s)

    comp_rpms_path = _setup_rpm_topdir(rpms_path, comp_name, version)

    if skip_build:
        return 1, comp_rpms_path

    cmd: CmdArgs = [
        script_path.resolve().as_posix(),
        repo_path.resolve().as_posix(),
        str(el_version),
        comp_rpms_path.resolve().as_posix(),
    ]

    if version:
        cmd.append(version)

    extra_env: dict[str, str] | None = None
    if ccache_path is not None:
        extra_env = {"CES_CCACHE_PATH": ccache_path.resolve().as_posix()}

    start = dt.now(tz=datetime.UTC)
    try:
        rc, _, _ = await async_run_cmd(
            cmd, outcb=_outcb, cwd=repo_path, reset_python_env=True, extra_env=extra_env
        )
    except CommandError as e:
        msg = (
            f"error running build script for '{comp_name}' "
            + f"with '{script_path}': {e}"
        )
        mlog.exception(msg)
        raise BuilderError(msg) from e
    except Exception as e:
        msg = (
            f"unknown error running build script for '{comp_name}' "
            + f"with '{script_path}': {e}"
        )
        mlog.exception(msg)
        raise BuilderError(msg) from e
    delta = (dt.now(datetime.UTC) - start).seconds

    if rc != 0:
        mlog.error(f"error running build script for '{comp_name}'")
        raise BuilderError(msg=f"error running build script for '{comp_name}'")

    return delta, comp_rpms_path


async def _install_deps(
    components_path: Path,
    components: dict[str, BuildComponentInfo],
) -> None:
    """Install dependencies for all components, sequentially."""
    for name, comp in components.items():
        comp_scripts_path = get_component_scripts_path(components_path, comp.name)
        if not comp_scripts_path:
            log.warning(f"scripts for component '{comp.name}' not found, continue")
            continue

        install_deps_script = get_script_path(comp_scripts_path, "install_deps.*")
        if not install_deps_script:
            log.info(f"no dependencies to install for component '{comp.name}'")
            continue

        clog = log.getChild(f"comp[{name}]")

        def _comp_out(s: str) -> None:
            clog.debug(s)  # noqa: B023

        repo_path = comp.repo_path.resolve()
        cmd: CmdArgs = [
            install_deps_script.resolve().as_posix(),
            repo_path.as_posix(),
        ]

        try:
            rc, _, stderr = await async_run_cmd(
                cmd,
                outcb=_comp_out,
                cwd=repo_path,
                reset_python_env=True,
            )
        except CommandError as e:
            msg = f"error installing dependencies for '{comp.name}': {e}"
            clog.exception(msg)
            raise BuilderError(msg) from e
        except Exception as e:
            msg = f"unknown error installing dependencies for '{comp.name}': {e}"
            clog.exception(msg)
            raise BuilderError(msg) from e

        if rc != 0:
            msg = f"error installing dependencies for '{comp.name}': {stderr}"
            clog.error(msg)
            raise BuilderError(msg)


async def build_rpms(
    rpms_path: Path,
    el_version: int,
    components_path: Path,
    components: dict[str, BuildComponentInfo],
    *,
    ccache_path: Path | None = None,
    skip_build: bool = False,
) -> dict[str, ComponentBuild]:
    """
    Build RPMs for the various components provided in `components`.

    Relies on a `build_rpms.sh` script that must be found in the `components_path`
    directory, for each specific component.
    Returns a `ComponentBuild`, containing the component's built version and a
    `Path` to where its RPMs can be found.
    """
    if not components_path.exists():
        raise BuilderError(msg=f"components path at '{components_path}' not found")

    try:
        await _install_deps(components_path, components)
    except BuilderError as e:
        msg = f"error installing components' dependencies: {e}"
        log.exception(msg)
        raise BuilderError(msg) from e

    class _ToBuildComponent:
        build_script: Path
        version: str

        def __init__(self, build_script: Path, version: str) -> None:
            self.build_script = build_script
            self.version = version

    to_build: dict[str, _ToBuildComponent] = {}
    for comp_name, comp_info in components.items():
        comp_path = components_path.joinpath(comp_name)
        if not comp_path.exists():
            log.warning(
                f"component path for '{comp_name}' "
                + f"not found in '{components_path}'"
            )
            continue

        comp_scripts_path = get_component_scripts_path(components_path, comp_name)
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

        to_build[comp_name] = _ToBuildComponent(
            build_script_path, comp_info.long_version
        )

    try:
        async with asyncio.TaskGroup() as tg:
            tasks = {
                name: tg.create_task(
                    _build_component(
                        rpms_path,
                        el_version,
                        name,
                        to_build[name].build_script,
                        components[name].repo_path,
                        to_build[name].version,
                        ccache_path=ccache_path,
                        skip_build=skip_build,
                    )
                )
                for name in to_build
            }
    except ExceptionGroup as e:
        excs = e.subgroup(BuilderError)
        if excs is not None:
            log.error("error building component RPMs:")  # noqa: TRY400
            for exc in excs.exceptions:
                log.error(f"- {exc}")  # noqa: TRY400
        else:
            log.error(f"unexpected error building component RPMs: {e}")  # noqa: TRY400
            for exc in e.exceptions:
                log.error(f"- {exc}")  # noqa: TRY400

        raise BuilderError(msg="error building component RPMs") from e

    comps_rpms_paths: dict[str, ComponentBuild] = {}
    for name, task in tasks.items():
        time_spent, comp_rpms_path = task.result()
        log.info(f"built component '{name}' in {time_spent} seconds")
        comps_rpms_paths[name] = ComponentBuild(to_build[name].version, comp_rpms_path)

    return comps_rpms_paths
