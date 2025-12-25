# CES library - CES builder, upload rpms
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
import shutil
from pathlib import Path

from cbscore.builder import BuilderError
from cbscore.builder import logger as parent_logger
from cbscore.builder.rpmbuild import ComponentBuild
from cbscore.utils import CommandError, async_run_cmd
from cbscore.utils.s3 import S3Error, S3FileLocator, s3_upload_files
from cbscore.utils.secrets.mgr import SecretsMgr

logger = parent_logger.getChild("upload")


class S3ComponentLocation:
    name: str
    version: str
    location: str

    def __init__(self, name: str, version: str, location: str) -> None:
        self.name = name
        self.version = version
        self.location = location


def _get_rpms(
    base_path: Path, base_dst_loc: str, relative_to: Path
) -> list[S3FileLocator]:
    rpms: list[S3FileLocator] = []
    for parent, _, files in base_path.walk():
        for f in files:
            if not f.endswith(".rpm"):
                continue

            rpm_file_path = Path(parent, f)
            relative_path = rpm_file_path.relative_to(relative_to)

            dst_loc = f"{base_dst_loc}/{relative_path.as_posix()}"
            rpms.append(S3FileLocator(rpm_file_path, dst_loc, f))

    return rpms


async def _get_repo(
    target_path: Path, s3_base_dst: str, relative_to: Path
) -> list[S3FileLocator]:
    # create a repository at 'p', and return the corresponding
    # 'repodata' directory path.
    async def _create_repo(p: Path) -> Path:
        repodata_path = p.joinpath("repodata")
        if repodata_path.exists():
            shutil.rmtree(repodata_path)

        try:
            _ = await async_run_cmd(["createrepo", p.resolve().as_posix()])
        except CommandError as e:
            msg = f"error creating repodata at '{repodata_path}': {e}"
            logger.exception(msg)
            raise BuilderError(msg) from e
        except Exception as e:
            msg = f"unknown error creating repodata at '{repodata_path}': {e}"
            logger.exception(msg)
            raise BuilderError(msg) from e

        if not repodata_path.exists() or not repodata_path.is_dir():
            msg = f"unexpected missing repodata dir at '{repodata_path}'"
            logger.error(msg)
            raise BuilderError(msg)

        return repodata_path

    # get all the 'repodata' directories for this path. A repository will
    # be created under 'p' if at least one RPM exists. Will still descend
    # into all child directories, doing the same.
    async def _get_repo_r(p: Path) -> list[Path]:
        repo_paths: list[Path] = []

        has_repo = False
        for entry in p.iterdir():
            if entry.is_dir():
                repo_paths.extend(await _get_repo_r(entry))
                continue

            if entry.suffix == ".rpm" and not has_repo:
                repo_paths.append(await _create_repo(entry.parent))
                continue

        return repo_paths

    repo_paths = await _get_repo_r(target_path)

    tgt_locs: list[S3FileLocator] = []
    for path in repo_paths:
        for src_path in path.iterdir():
            assert not src_path.is_dir(), "unexpected directory in repository"
            dst_path = src_path.relative_to(relative_to)
            dst_loc = f"{s3_base_dst}/{dst_path}"
            tgt_locs.append(S3FileLocator(src_path, dst_loc, dst_path.name))

    return tgt_locs


async def _upload_component_rpms(
    secrets: SecretsMgr,
    upload_to_url: str,
    bucket: str,
    bucket_loc: str,
    name: str,
    version: str,
    el_version: int,
    rpms_path: Path,
) -> str:
    s3_base_dst = f"{bucket_loc}/{name}/rpm-{version}/el{el_version}.clyso"

    path_to_rpms = rpms_path.joinpath("RPMS")
    path_to_srpms = rpms_path.joinpath("SRPMS")

    # NOTE: this bit could be more efficient, but would be uglier after formatting.
    to_upload: list[S3FileLocator] = []
    to_upload.extend(_get_rpms(path_to_rpms, s3_base_dst, path_to_rpms))
    to_upload.extend(_get_rpms(path_to_srpms, s3_base_dst, path_to_srpms.parent))
    to_upload.extend(await _get_repo(path_to_rpms, s3_base_dst, path_to_rpms))
    to_upload.extend(await _get_repo(path_to_srpms, s3_base_dst, path_to_srpms.parent))

    for rpm in to_upload:
        logger.debug(f"{rpm.name}\n-> SRC: {rpm.src}\n=> DST: {rpm.dst}")

    try:
        await s3_upload_files(secrets, upload_to_url, bucket, to_upload, public=True)
    except S3Error as e:
        msg = f"error uploading rpms: {e}"
        logger.exception(msg)
        raise BuilderError(msg) from e

    return s3_base_dst


async def s3_upload_rpms(
    secrets: SecretsMgr,
    upload_to_url: str,
    bucket: str,
    bucket_loc: str,
    components: dict[str, ComponentBuild],
    el_version: int,
) -> dict[str, S3ComponentLocation]:
    try:
        async with asyncio.TaskGroup() as tg:
            tasks = {
                name: tg.create_task(
                    _upload_component_rpms(
                        secrets,
                        upload_to_url,
                        bucket,
                        bucket_loc,
                        name,
                        e.version,
                        el_version,
                        e.rpms_path,
                    )
                )
                for name, e in components.items()
            }
    except ExceptionGroup as e:
        excs = e.subgroup(BuilderError)
        if excs is not None:
            logger.error("error uploading components RPMs:")
            for exc in excs.exceptions:
                logger.error(f"- {exc}")
        else:
            logger.error(f"unexpected error uploading RPMs: {e}")

        raise BuilderError(msg=f"error uploading component RPMs: {e}") from e

    except Exception as e:
        msg = f"unexpected error uploading RPMs: {e}"
        logger.exception(msg)
        raise BuilderError(msg) from e

    s3_comp_loc: dict[str, S3ComponentLocation] = {}
    for comp_name, task in tasks.items():
        s3_loc = task.result()
        s3_comp_loc[comp_name] = S3ComponentLocation(
            comp_name, components[comp_name].version, s3_loc
        )
        logger.info(f"uploaded '{comp_name}' to '{s3_loc}'")
    return s3_comp_loc
