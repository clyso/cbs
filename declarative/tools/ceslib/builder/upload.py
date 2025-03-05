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

import aioboto3
from ceslib.builder import BuilderError
from ceslib.builder import log as parent_logger
from ceslib.builder.rpmbuild import ComponentBuild
from ceslib.utils import CommandError, async_run_cmd
from ceslib.utils.secrets import SecretsVaultError, SecretsVaultMgr
from types_aiobotocore_s3.service_resource import S3ServiceResource

log = parent_logger.getChild("upload")


class _TargetLocation:
    src: Path
    dst: str
    name: str

    def __init__(self, src: Path, dst: str, name: str) -> None:
        self.src = src
        self.dst = dst
        self.name = name


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
) -> list[_TargetLocation]:
    rpms: list[_TargetLocation] = []
    for parent, _, files in base_path.walk():
        for f in files:
            if not f.endswith(".rpm"):
                continue

            rpm_file_path = Path(parent, f)
            relative_path = rpm_file_path.relative_to(relative_to)

            dst_loc = f"{base_dst_loc}/{relative_path.as_posix()}"
            rpms.append(_TargetLocation(rpm_file_path, dst_loc, f))

    return rpms


async def _upload_rpm(
    s3: S3ServiceResource,
    tgt_loc: _TargetLocation,
) -> None:
    bucket = await s3.Bucket("ces-packages")

    log.debug(f"uploading rpm '{tgt_loc.name}' to '{tgt_loc.dst}'")
    try:
        await bucket.upload_file(
            tgt_loc.src.as_posix(), Key=tgt_loc.dst, ExtraArgs={"ACL": "public-read"}
        )
    except Exception as e:
        msg = (
            f"error uploading '{tgt_loc.name}' from '{tgt_loc.src}' "
            + f"to '{tgt_loc.dst}': {e}"
        )
        log.error(msg)
        raise BuilderError(msg)


async def _get_repo(
    target_path: Path, s3_base_dst: str, relative_to: Path
) -> list[_TargetLocation]:
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
            log.error(msg)
            raise BuilderError(msg)
        except Exception as e:
            msg = f"unknown error creating repodata at '{repodata_path}': {e}"
            log.error(msg)
            raise BuilderError(msg)

        if not repodata_path.exists() or not repodata_path.is_dir():
            msg = f"unexpected missing repodata dir at '{repodata_path}'"
            log.error(msg)
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

    tgt_locs: list[_TargetLocation] = []
    for path in repo_paths:
        for src_path in path.iterdir():
            assert not src_path.is_dir(), "unexpected directory in repository"
            dst_path = src_path.relative_to(relative_to)
            dst_loc = f"{s3_base_dst}/{dst_path}"
            tgt_locs.append(_TargetLocation(src_path, dst_loc, dst_path.name))

    return tgt_locs


async def _upload_component_rpms(
    secrets: SecretsVaultMgr, name: str, version: str, el_version: int, rpms_path: Path
) -> str:
    s3_base_dst = f"{name}/rpm-{version}/el{el_version}.clyso"

    path_to_rpms = rpms_path.joinpath("RPMS")
    path_to_srpms = rpms_path.joinpath("SRPMS")

    # NOTE: this bit could be more efficient, but would be uglier after formatting.
    to_upload: list[_TargetLocation] = []
    to_upload.extend(_get_rpms(path_to_rpms, s3_base_dst, path_to_rpms))
    to_upload.extend(_get_rpms(path_to_srpms, s3_base_dst, path_to_srpms.parent))
    to_upload.extend(await _get_repo(path_to_rpms, s3_base_dst, path_to_rpms))
    to_upload.extend(await _get_repo(path_to_srpms, s3_base_dst, path_to_srpms.parent))

    for rpm in to_upload:
        log.debug(f"{rpm.name}\n-> SRC: {rpm.src}\n=> DST: {rpm.dst}")

    try:
        hostname, access_id, secret_id = secrets.s3_creds()
    except SecretsVaultError as e:
        msg = f"error obtaining S3 credentials: {e}"
        log.error(msg)
        raise BuilderError(msg)

    log.debug(f"S3: hostname = {hostname}, access_id = {access_id}")

    s3_session = aioboto3.Session(
        aws_access_key_id=access_id,
        aws_secret_access_key=secret_id,
    )

    if not hostname.startswith("http"):
        hostname = f"https://{hostname}"

    async with s3_session.resource("s3", None, None, True, True, hostname) as s3:
        for f in to_upload:
            try:
                await _upload_rpm(s3, f)
            except BuilderError as e:
                msg = f"error uploading rpm: {e}"
                log.error(msg)
                raise BuilderError(msg)
            except Exception as e:
                msg = f"unknown error uploading rpm: {e}"
                log.error(msg)
                raise BuilderError(msg)

    return s3_base_dst


async def s3_upload_rpms(
    secrets: SecretsVaultMgr,
    components: dict[str, ComponentBuild],
    el_version: int,
) -> dict[str, S3ComponentLocation]:
    try:
        async with asyncio.TaskGroup() as tg:
            tasks = {
                name: tg.create_task(
                    _upload_component_rpms(
                        secrets, name, e.version, el_version, e.rpms_path
                    )
                )
                for name, e in components.items()
            }
    except ExceptionGroup as e:
        excs = e.subgroup(BuilderError)
        if excs is not None:
            log.error("error uploading components RPMs:")
            for exc in excs.exceptions:
                log.error(f"- {exc}")
        else:
            log.error(f"unexpected error uploading RPMs: {e}")

        raise BuilderError(f"error uploading component RPMs: {e}")

    except Exception as e:
        msg = f"unexpected error uploading RPMs: {e}"
        log.error(msg)
        raise BuilderError(msg)

    s3_comp_loc: dict[str, S3ComponentLocation] = {}
    for comp_name, task in tasks.items():
        s3_loc = task.result()
        s3_comp_loc[comp_name] = S3ComponentLocation(
            comp_name, components[comp_name].version, s3_loc
        )
        log.info(f"uploaded '{comp_name}' to '{s3_loc}'")
    return s3_comp_loc


async def s3_upload_json(
    secrets: SecretsVaultMgr, location: str, contents: str
) -> None:
    try:
        hostname, access_id, secret_id = secrets.s3_creds()
    except SecretsVaultError as e:
        msg = f"error obtaining S3 credentials: {e}"
        log.error(msg)
        raise BuilderError(msg)

    log.debug(f"S3: hostname = {hostname}, access_id = {access_id}")

    s3_session = aioboto3.Session(
        aws_access_key_id=access_id,
        aws_secret_access_key=secret_id,
    )

    if not hostname.startswith("http"):
        hostname = f"https://{hostname}"

    async with s3_session.resource("s3", None, None, True, True, hostname) as s3:
        bucket = await s3.Bucket("ces-packages")
        try:
            _ = await bucket.put_object(
                Key=location,
                Body=contents,
            )
        except Exception as e:
            msg = f"error uploading json to '{location}': {e}"
            log.error(msg)
            raise BuilderError(msg)
