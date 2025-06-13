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
import datetime
import re
from datetime import datetime as dt
from pathlib import Path

import pydantic
from ceslib.builder import BuilderError
from ceslib.builder import log as parent_logger
from ceslib.builder.utils import get_component_version
from ceslib.utils import CommandError, async_run_cmd, git
from ceslib.utils.secrets import SecretsVaultMgr
from ceslib.versions.desc import VersionComponent
from ceslib.versions.utils import get_major_version, get_minor_version

log = parent_logger.getChild("prepare")


class BuildComponentInfo(pydantic.BaseModel):
    """Contains information about a component to be built."""

    name: str
    repo_path: Path
    repo_url: str
    base_ref: str
    sha1: str
    long_version: str


async def prepare_builder() -> None:
    def _cb(s: str) -> None:
        log.debug(s)

    try:
        rc, _, stderr = await async_run_cmd(["dnf", "update", "-y"], outcb=_cb)
        if rc != 0:
            log.error(f"error updating builder: {stderr}")
            raise BuilderError(msg="error updating 'dnf'")

        rc, _, stderr = await async_run_cmd(
            ["dnf", "install", "-y", "epel-release"],
            outcb=_cb,
        )
        if rc != 0:
            log.error(f"error installing 'epel-release': {stderr}")
            raise BuilderError(msg="error installing 'epel-release'")

        rc, _, stderr = await async_run_cmd(
            ["dnf", "config-manager", "--enable", "crb"],
            outcb=_cb,
        )
        if rc != 0:
            log.error(f"error enabling CRB: {stderr}")
            raise BuilderError(msg="error enabling CRB repository")

        rc, _, stderr = await async_run_cmd(
            ["dnf", "update", "-y"],
            outcb=_cb,
        )
        if rc != 0:
            log.error(f"error updating builder: {stderr}")
            raise BuilderError(msg="error running 'dnf update'")

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
            raise BuilderError(msg="unable to install dependencies")

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
        log.exception("unable to run 'dnf'")
        raise BuilderError(msg=f"error running 'dnf': {e}") from e


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


async def prepare_components(
    secrets: SecretsVaultMgr,
    scratch_path: Path,
    components_path: Path,
    components: list[VersionComponent],
    version: str,
) -> dict[str, BuildComponentInfo]:
    """
    Prepare all components by cloning them and applying required patches.

    This function parallelizes on a per-component basis.

    The `components_path` argument refers to the directory under which we can find the
    components supported.

    Returns a `dict` of `BuildComponentInfo` per component name, containing the
    component's git repository, version, and SHA1, alongside the `Path` on which the
    repository has been cloned.
    """

    async def _clone_repo(comp: VersionComponent) -> Path:
        """
        Clone component repository.

        Returns a `BuildComponentInfo` containing its
        original repository `URL`, the `Path` to which it has been cloned, alongside
        the `version` being cloned, and the `SHA1` of said version.
        """
        log.debug(
            f"clone repo '{comp.repo}' to '{scratch_path}', "
            + f"name: '{comp.name}', ref: '{comp.ref}'"
        )
        start = dt.now(tz=datetime.UTC)
        try:
            with secrets.git_url_for(comp.repo) as comp_url:
                cloned_path = await git.git_clone(
                    comp_url,
                    scratch_path,
                    comp.name,
                    ref=comp.ref,
                    update_if_exists=True,
                    clean_if_exists=True,
                )
        except git.GitError as e:
            log.exception(
                f"error cloning '{comp.repo}' to '{scratch_path}', ref: {comp.ref}"
            )
            raise BuilderError(msg=f"error cloning '{comp.repo}': {e}") from e
        except Exception as e:
            log.exception(f"unknown exception cloning '{comp.repo}'")
            raise BuilderError(msg=f"error cloning '{comp.repo}': {e}") from e

        delta = dt.now(tz=datetime.UTC) - start
        log.info(f"component '{comp.name}' cloned in {delta.seconds}")

        return cloned_path

    async def _apply_patches(comp: VersionComponent, repo: Path) -> None:
        """Apply required patches to component's repository."""
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
                await git.git_apply(repo, patch_path)
            except git.GitError as e:
                msg = f"unable to apply patch from '{patch_path}' to '{repo}': {e}"
                log.exception(msg)
                raise BuilderError(msg) from e

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

    async def _get_component_info(
        comp: VersionComponent, repo_path: Path
    ) -> BuildComponentInfo:
        """
        Obtain `BuildComponentInfo`.

        `BuildComponentInfo` holds all the required information to build a given
        component.
        """
        try:
            sha1 = await git.git_get_sha1(repo_path)
        except (git.GitError, Exception) as e:
            msg = f"error obtaining SHA1 for repository '{repo_path}': {e}"
            log.exception(msg)
            raise BuilderError(msg) from e

        try:
            long_version = await get_component_version(
                comp.name,
                components_path,
                repo_path,
            )
        except (BuilderError, Exception) as e:
            msg = f"error obtaining version for component '{comp.name}': {e}"
            log.exception(msg)
            raise BuilderError(msg) from e

        return BuildComponentInfo(
            name=comp.name,
            repo_path=repo_path,
            repo_url=comp.repo,
            base_ref=comp.ref,
            sha1=sha1,
            long_version=long_version,
        )

    async def _do_component(comp: VersionComponent) -> BuildComponentInfo:
        """
        Prepare a component.

        Preparing is done by cloning and then applying any required patches to the
        repository.
        """
        try:
            repo_path = await _clone_repo(comp)
        except BuilderError as e:
            log.exception(f"error cloning component '{comp.name}'")
            raise e  # noqa: TRY201

        try:
            await _apply_patches(comp, repo_path)
        except BuilderError as e:
            log.exception("error applying component patches")
            raise e  # noqa: TRY201
        except Exception as e:
            log.exception("error applying component patches")
            raise e  # noqa: TRY201

        try:
            info = await _get_component_info(comp, repo_path)
        except (BuilderError, Exception) as e:
            msg = f"error obtaining version for component '{comp.name}': {e}"
            log.exception(msg)
            raise BuilderError(msg) from e

        return info

    try:
        async with asyncio.TaskGroup() as tg:
            task_dict = {
                comp.name: tg.create_task(_do_component(comp)) for comp in components
            }
    except ExceptionGroup as e:
        log.error("error preparing components:")  # noqa: TRY400
        excs = e.subgroup(BuilderError)
        if excs is not None:
            for exc in excs.exceptions:
                log.error(f"- {exc}")  # noqa: TRY400
        raise BuilderError(msg="error preparing components") from e

    return {name: task.result() for name, task in task_dict.items()}
