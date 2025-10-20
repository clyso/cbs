# CBS Release Tool - manifest stages commands
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

import errno
import sys
import uuid
from pathlib import Path

import click
from rich.padding import Padding

from crt.cmds._common import get_stage_rdr, get_stage_summary_rdr
from crt.crtlib.errors.manifest import (
    MalformedManifestError,
    NoStageError,
    NoSuchManifestError,
)
from crt.crtlib.errors.stages import MalformedStageTagError, StageError
from crt.crtlib.manifest import load_manifest_by_name_or_uuid, store_manifest
from crt.crtlib.models.common import AuthorData
from crt.crtlib.models.manifest import ManifestStage
from crt.crtlib.utils import get_tags

from . import console, perror, pinfo, psuccess, with_patches_repo_path
from . import logger as parent_logger

logger = parent_logger.getChild("stages")


def _show_stage_summary(stage: ManifestStage) -> None:
    rdr = get_stage_summary_rdr(stage)
    console.print(Padding(rdr, (1, 0, 1, 0)))


@click.group("stage", help="Operate on release manifest stages.")
def cmd_manifest_stage() -> None:
    pass


@cmd_manifest_stage.command("new", help="Add a new stage to a manifest.")
@click.option(
    "-m",
    "--manifest",
    "manifest_name_or_uuid",
    required=True,
    type=str,
    metavar="NAME|UUID",
    help="Manifest name or UUID to operate on.",
)
@click.option(
    "--author",
    "author_name",
    required=True,
    type=str,
    metavar="NAME",
    help="Author's name.",
)
@click.option(
    "--email",
    "author_email",
    required=True,
    type=str,
    metavar="EMAIL",
    help="Author's email.",
)
@click.option(
    "--tag",
    "-t",
    "stage_tags",
    required=False,
    type=str,
    metavar="TAG=VALUE",
    multiple=True,
    help="Tags for this stage.",
)
@click.option(
    "--desc",
    "-D",
    "stage_desc",
    required=False,
    default="",
    type=str,
    metavar="TEXT",
    help="Short description of this stage.",
)
@with_patches_repo_path
def cmd_manifest_stage_new(
    patches_repo_path: Path,
    manifest_name_or_uuid: str,
    author_name: str,
    author_email: str,
    stage_tags: list[str],
    stage_desc: str,
) -> None:
    logger.debug(
        f"add manifest '{manifest_name_or_uuid}' stage "
        + f"by '{author_name} <{author_email}>'"
    )

    try:
        tags = get_tags(stage_tags)
    except MalformedStageTagError as e:
        perror(f"malformed stage tag: {e}")
        sys.exit(errno.EINVAL)

    try:
        manifest = load_manifest_by_name_or_uuid(
            patches_repo_path, manifest_name_or_uuid
        )
    except NoSuchManifestError:
        perror(
            f"unable to find manifest uuid '{manifest_name_or_uuid}' in "
            + "patches repository"
        )
        sys.exit(errno.ENOENT)
    except MalformedManifestError:
        perror(f"malformed manifest '{manifest_name_or_uuid}'")
        sys.exit(errno.EINVAL)
    except Exception as e:
        perror(f"unable to obtain manifest '{manifest_name_or_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        stage = manifest.new_stage(
            AuthorData(user=author_name, email=author_email),
            tags,
            stage_desc,
        )
    except StageError as e:
        perror(f"unable to create new stage: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    psuccess(
        f"new stage '{stage.stage_uuid}' for manifest uuid '{manifest.release_uuid}'"
    )
    _show_stage_summary(stage)

    try:
        store_manifest(patches_repo_path, manifest)
    except Exception as e:
        perror(f"unable to write manifest to disk: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    pinfo(f"wrote manifest '{manifest.release_uuid}' to disk")


@cmd_manifest_stage.command("info", help="Show information about a stage.")
@click.option(
    "-m",
    "--manifest",
    "manifest_name_or_uuid",
    required=True,
    type=str,
    metavar="NAME|UUID",
    help="Manifest name or UUID to operate on.",
)
@click.option(
    "-s",
    "--stage",
    "stage_uuid",
    required=False,
    type=uuid.UUID,
    metavar="UUID",
    help="Stage UUID to show information on.",
)
@click.option(
    "-e",
    "--extended",
    "extended_info",
    is_flag=True,
    default=False,
    help="Show extended patch information.",
)
@with_patches_repo_path
def cmd_manifest_stage_info(
    patches_repo_path: Path,
    manifest_name_or_uuid: str,
    stage_uuid: uuid.UUID | None,
    extended_info: bool,
) -> None:
    try:
        manifest = load_manifest_by_name_or_uuid(
            patches_repo_path, manifest_name_or_uuid
        )
    except NoSuchManifestError:
        perror(f"unable to find manifest '{manifest_name_or_uuid}'")
        sys.exit(errno.ENOENT)
    except Exception as e:
        perror(f"unable to obtain manifest '{manifest_name_or_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    stage_uuid_lst = [e.stage_uuid for e in manifest.stages]
    if stage_uuid and stage_uuid not in stage_uuid_lst:
        perror(
            f"unknown stage uuid '{stage_uuid}' in manifest '{manifest_name_or_uuid}'"
        )
        sys.exit(errno.ENOENT)

    elif not stage_uuid_lst:
        pinfo(f"no stages available in manifest '{manifest_name_or_uuid}'")
        return

    for stage in manifest.stages:
        if stage_uuid and stage.stage_uuid != stage_uuid:
            continue

        console.print(
            get_stage_rdr(patches_repo_path, stage, extended_info=extended_info)
        )

    pass


@cmd_manifest_stage.command("amend", help="Amend metada for a given stage.")
@click.option(
    "-m",
    "--manifest",
    "manifest_name_or_uuid",
    required=True,
    type=str,
    metavar="NAME|UUID",
    help="Manifest name or UUID to operate on.",
)
@click.option(
    "-s",
    "--stage",
    "stage_uuid",
    required=True,
    type=uuid.UUID,
    metavar="UUID",
    help="Stage UUID to show information on.",
)
@click.option(
    "--author",
    "author_name",
    required=False,
    type=str,
    metavar="NAME",
    help="Author's name.",
)
@click.option(
    "--email",
    "author_email",
    required=False,
    type=str,
    metavar="EMAIL",
    help="Author's email.",
)
@click.option(
    "--tag",
    "-t",
    "stage_tags",
    required=False,
    type=str,
    metavar="TYPE=N",
    multiple=True,
    help="Tag type for this stage",
)
@with_patches_repo_path
def cmd_manifest_stage_amend(
    patches_repo_path: Path,
    manifest_name_or_uuid: str,
    stage_uuid: uuid.UUID,
    author_name: str | None,
    author_email: str | None,
    stage_tags: list[str],
) -> None:
    if not author_name and not author_email and not stage_tags:
        perror("no paramenters were specified to amend stage")
        sys.exit(errno.EINVAL)

    try:
        manifest = load_manifest_by_name_or_uuid(
            patches_repo_path, manifest_name_or_uuid
        )
    except NoSuchManifestError:
        perror(f"unable to find manifest '{manifest_name_or_uuid}'")
        sys.exit(errno.ENOENT)
    except Exception as e:
        perror(f"unable to obtain manifest '{manifest_name_or_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    stage: ManifestStage | None = None
    for s in manifest.stages:
        if s.stage_uuid == stage_uuid:
            stage = s
            break

    if not stage:
        perror(
            f"could not find stage uuid '{stage_uuid}' "
            + f"in manifest uuid '{manifest.release_uuid}'"
        )
        sys.exit(errno.ENOENT)

    if author_name:
        stage.author.user = author_name

    if author_email:
        stage.author.email = author_email

    if stage_tags:
        try:
            tags = get_tags(stage_tags)
        except MalformedStageTagError as e:
            perror(f"malformed stage tag: {e}")
            sys.exit(errno.EINVAL)
        stage.tags = tags

    try:
        store_manifest(patches_repo_path, manifest)
    except Exception as e:
        perror(f"unable to write manifest to disk: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    _show_stage_summary(stage)
    pinfo(f"wrote manifest '{manifest.release_uuid}' to disk")


@cmd_manifest_stage.command("remove", help="Remove a stage from a manifest.")
@click.option(
    "-m",
    "--manifest",
    "manifest_name_or_uuid",
    required=True,
    type=str,
    metavar="NAME|UUID",
    help="Manifest name or UUID to operate on.",
)
@click.option(
    "-s",
    "--stage",
    "stage_uuid",
    required=True,
    type=uuid.UUID,
    metavar="UUID",
    help="Stage UUID to show information on.",
)
@with_patches_repo_path
def cmd_manifest_stage_remove(
    patches_repo_path: Path, manifest_name_or_uuid: str, stage_uuid: uuid.UUID
) -> None:
    try:
        manifest = load_manifest_by_name_or_uuid(
            patches_repo_path, manifest_name_or_uuid
        )
    except NoSuchManifestError:
        perror(f"unable to find manifest '{manifest_name_or_uuid}'")
        sys.exit(errno.ENOENT)
    except Exception as e:
        perror(f"unable to obtain manifest '{manifest_name_or_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        manifest.remove_stage(stage_uuid)
    except NoStageError:
        perror(f"stage '{stage_uuid}' not found in manifest")
        sys.exit(errno.ENOENT)

    try:
        store_manifest(patches_repo_path, manifest)
    except Exception as e:
        perror(f"unable to write manifest to disk: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    psuccess(f"removed stage '{stage_uuid}' from manifest")
