# CES library - prepare builder
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
import re
from datetime import datetime as dt
from pathlib import Path

from ceslib.builder import BuilderError, get_component_scripts_path, get_script_path
from ceslib.builder import log as parent_logger
from ceslib.utils import CommandError, async_run_cmd, git
from ceslib.utils.secrets import SecretsVaultMgr
from ceslib.versions.desc import VersionComponent, VersionDescriptor
from ceslib.versions.utils import get_major_version, get_minor_version

log = parent_logger.getChild("prepare")


async def prepare_builder() -> None:
    def _cb(s: str) -> None:
        log.debug(s)

    try:
        rc, _, stderr = await async_run_cmd(["dnf", "update", "-y"], outcb=_cb)
        if rc != 0:
            log.error(f"error updating builder: {stderr}")
            raise BuilderError("error updating 'dnf'")

        rc, _, stderr = await async_run_cmd(
            ["dnf", "install", "-y", "epel-release"],
            outcb=_cb,
        )
        if rc != 0:
            log.error(f"error installing 'epel-release': {stderr}")
            raise BuilderError("error installing 'epel-release'")

        rc, _, stderr = await async_run_cmd(
            ["dnf", "config-manager", "--enable", "crb"],
            outcb=_cb,
        )
        if rc != 0:
            log.error(f"error enabling CRB: {stderr}")
            raise BuilderError("error enabling CRB repository")

        rc, _, stderr = await async_run_cmd(
            ["dnf", "update", "-y"],
            outcb=_cb,
        )
        if rc != 0:
            log.error(f"error updating builder: {stderr}")
            raise BuilderError("error running 'dnf update'")

        rc, _, stderr = await async_run_cmd(
            [
                "dnf",
                "install",
                "-y",
                "git",
                "wget",
                "rpm-build",
                "rpmdevtools",
                "gcc-c++",
                "createrepo",
                "rpm-sign",
                "pinentry",
                "s3cmd",
                "jq",
                "ccache",
                "buildah",
            ],
            outcb=_cb,
        )
        if rc != 0:
            log.error(f"error installing builder packages: {stderr}")
            raise BuilderError("unable to install dependencies")

        # install cosign rpm
        rc, _, stderr = await async_run_cmd(
            [
                "rpm",
                "-Uvh",
                "https://github.com/sigstore/cosign/releases/download/v2.4.3/"
                + "cosign-2.4.3-1.x86_64.rpm",
            ],
            outcb=_cb,
        )
        if rc != 0:
            msg = f"error installing cosign package: {stderr}"
            log.error(msg)
            raise BuilderError(msg)
    except CommandError as e:
        log.error(f"unable to run 'dnf': {e}")
        raise BuilderError(f"error running 'dnf': {e}")


def _get_patch_list(patches_path: Path, version: str) -> list[Path]:
    patches_pattern = re.compile(r"(\d+)-.*\.patch")
    patches_dict: dict[int, list[tuple[int, Path]]] = {}

    log.debug(f"get patch list for version '{version}'")

    def _get_patches_by_prio(path: Path, cur_prio: int, filter_version: str) -> None:
        if cur_prio not in patches_dict:
            patches_dict[cur_prio] = []

        if cur_prio > 0:
            if path.name == filter_version:
                log.debug(f"{filter_version} matches path at {path.name}")
                pass
            elif path.name == get_minor_version(filter_version):
                log.debug(f"{filter_version} matches minor for path at {path.name}")
                pass
            elif path.name == get_major_version(filter_version):
                log.debug(f"{filter_version} matches major for path at {path.name}")
                pass
            else:
                log.debug(f"{path.name} does not match {filter_version}")
                return

        for entry in path.iterdir():
            if entry.suffix == ".patch":
                m = re.match(patches_pattern, entry.name)
                if m is None:
                    log.warning(f"patch name '{entry.name}' malformed, continue.")
                    continue
                patches_dict[cur_prio].append((int(m.group(1)), entry))

            else:
                _get_patches_by_prio(entry, cur_prio + 1, filter_version)

        patches_dict[cur_prio] = sorted(
            patches_dict[cur_prio], key=lambda item: item[0]
        )

    _get_patches_by_prio(patches_path, 0, version)

    patch_list_order: list[Path] = []
    for _, lst in reversed(patches_dict.items()):
        for _, entry in lst:
            patch_list_order.append(entry)

    return patch_list_order


# Prepares all components by cloning them and applying required patches, parallelizing
# on a per-component basis.
# Returns a dict of Paths per component name, where said path refers to the location
# of the component's repository that was cloned.
#
async def prepare_components(
    secrets: SecretsVaultMgr,
    scratch_path: Path,
    components_path: Path,
    components: list[VersionComponent],
    version: str,
) -> dict[str, Path]:
    # clone component repository, returns its Path upon successful completion.
    #
    async def _clone_repo(comp: VersionComponent) -> Path:
        log.debug(
            f"clone repo '{comp.repo}' to '{scratch_path}', "
            + f"name: '{comp.name}', version: '{comp.version}'"
        )
        start = dt.now()
        try:
            with secrets.git_url_for(comp.repo) as comp_url:
                res = git.git_clone(
                    comp_url,
                    scratch_path,
                    comp.name,
                    ref=comp.version,
                    update_if_exists=True,
                    clean_if_exists=True,
                )
        except git.GitError as e:
            log.error(
                f"error cloning '{comp.repo}' to '{scratch_path}', version: {comp.version}: {e}"
            )
            raise BuilderError(f"error cloning '{comp.repo}': {e}")
        except Exception as e:
            log.error(f"unknown exception cloning '{comp.repo}': {e}")
            raise BuilderError(f"error cloning '{comp.repo}': {e}")
        delta = dt.now() - start
        log.info(f"component '{comp.name}' cloned in {delta.seconds}")
        return res

    # apply required patches to component's repository.
    #
    async def _apply_patches(comp: VersionComponent, repo: Path) -> None:
        log.info(f"apply patches to '{comp.name}' at '{repo}'")

        comp_path = components_path.joinpath(comp.name)
        if not comp_path.exists():
            log.warning(f"component '{comp.name}' not found in {components_path}")
            return

        comp_patches_path = comp_path.joinpath("patches")
        if not comp_patches_path.exists():
            log.info(f"no patches to apply to '{comp.name}'")
            return

        patches_to_apply = _get_patch_list(comp_patches_path, version)
        for patch_path in patches_to_apply:
            log.info(f"applying patch from '{patch_path}'")
            try:
                git.git_apply(repo, patch_path)
            except git.GitError as e:
                msg = f"unable to apply patch from '{patch_path}' to '{repo}': {e}"
                log.error(msg)
                raise BuilderError(msg)

        patches_lst: list[tuple[int, Path]] = []
        patch_pattern = re.compile(r"(\d+)-.*")
        for path, _, patch_files in comp_patches_path.walk():
            for patch_file in patch_files:
                patch_path = path.joinpath(patch_file)
                m = re.match(patch_pattern, patch_file)
                if m is None:
                    log.warning(f"file at '{patch_path}' does not match patch format")
                    continue
                patches_lst.append((int(m.group(1)), patch_path))

        patches_lst = sorted(patches_lst, key=lambda item: item[0])

    # prepares a component, by cloning and then applying any required patches to the
    # repository.
    #
    async def _do_component(comp: VersionComponent) -> Path:
        try:
            repo_path = await _clone_repo(comp)
        except BuilderError as e:
            log.error(f"error cloning component '{comp.name}': {e}")
            raise e

        try:
            await _apply_patches(comp, repo_path)
        except BuilderError as e:
            log.error(f"error applying component patches: {e}")
            raise e
        except Exception as e:
            log.error(f"error applying component patches: {e}")
            raise e

        return repo_path

    # install dependencies for all components, sequentially.
    async def _install_deps(repo_paths: dict[str, Path]) -> None:
        for comp in components:
            comp_scripts_path = get_component_scripts_path(components_path, comp.name)
            if not comp_scripts_path:
                log.warning(f"scripts for component '{comp.name}' not found, continue")
                continue

            install_deps_script = get_script_path(comp_scripts_path, "install_deps.*")
            if not install_deps_script:
                log.info(f"no dependencies to install for component '{comp.name}'")
                continue

            if comp.name not in repo_paths:
                log.error(
                    f"unable to find repository for component '{comp.name}', continue"
                )
                continue

            clog = log.getChild(f"comp[{comp.name}]")

            def _comp_out(s: str) -> None:
                clog.debug(s)

            cmd = [
                install_deps_script.resolve().as_posix(),
                repo_paths[comp.name].resolve().as_posix(),
            ]

            try:
                rc, _, stderr = await async_run_cmd(
                    cmd,
                    outcb=_comp_out,
                    cwd=repo_paths[comp.name],
                    reset_python_env=True,
                )
            except CommandError as e:
                msg = f"error installing dependencies for '{comp.name}': {e}"
                clog.error(msg)
                raise BuilderError(msg)
            except Exception as e:
                msg = f"unknown error installing dependencies for '{comp.name}': {e}"
                clog.error(msg)
                raise BuilderError(msg)

            if rc != 0:
                msg = f"error installing dependencies for '{comp.name}': {stderr}"
                clog.error(msg)
                raise BuilderError(msg)

    try:
        async with asyncio.TaskGroup() as tg:
            task_dict = {
                comp.name: tg.create_task(_do_component(comp)) for comp in components
            }
    except ExceptionGroup as e:
        log.error("error preparing components:")
        excs = e.subgroup(BuilderError)
        if excs is not None:
            for exc in excs.exceptions:
                log.error(f"- {exc}")
        raise BuilderError("error preparing components")

    repo_paths = {name: task.result() for name, task in task_dict.items()}

    try:
        await _install_deps(repo_paths)
    except BuilderError as e:
        msg = f"error installing component dependencies: {e}"
        log.error(msg)
        raise BuilderError(msg)

    return repo_paths


async def prepare(
    desc: VersionDescriptor,
    secrets: SecretsVaultMgr,
    scratch_path: Path,
    components_path: Path,
) -> dict[str, Path]:
    try:
        await prepare_builder()
    except BuilderError as e:
        log.error(f"unable to prepare builder: {e}")
        raise BuilderError(f"error preparing builder: {e}")

    try:
        comp_paths = await prepare_components(
            secrets,
            scratch_path,
            components_path,
            desc.components,
            desc.version,
        )
    except BuilderError as e:
        log.error(f"unable to prepare components: {e}")
        raise BuilderError(f"error preparing components: {e}")

    return comp_paths
