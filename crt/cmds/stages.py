# Ceph Release Tool - manifest stages commands
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
from crtlib.errors.manifest import (
    EmptyActiveStageError,
    MalformedManifestError,
    MismatchStageAuthorError,
    NoSuchManifestError,
)
from crtlib.errors.stages import MalformedStageTagError
from crtlib.manifest import load_manifest, store_manifest
from crtlib.models.common import AuthorData
from crtlib.utils import get_tags

from . import Ctx, Symbols, pass_ctx, perror, pinfo, pwarn
from . import logger as parent_logger

logger = parent_logger.getChild("stages")


@click.group("stage", help="Operate on release manifest stages.")
def cmd_manifest_stage() -> None:
    pass


@cmd_manifest_stage.command("new", help="Add a new stage to a manifest.")
@click.option(
    "-m",
    "--manifest-uuid",
    required=True,
    type=uuid.UUID,
    metavar="UUID",
    help="Manifest UUID to operate on.",
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
    metavar="TYPE=N",
    multiple=True,
    help="Tag type for this stage",
)
@click.option(
    "-p",
    "--patches-repo",
    "patches_repo_path",
    type=click.Path(
        exists=True, file_okay=False, dir_okay=True, resolve_path=True, path_type=Path
    ),
    required=True,
    help="Path to patches git repository",
)
@pass_ctx
def cmd_manifest_stage_new(
    _ctx: Ctx,
    manifest_uuid: uuid.UUID,
    author_name: str,
    author_email: str,
    stage_tags: list[str],
    patches_repo_path: Path,
) -> None:
    logger.debug(
        f"add manifest '{manifest_uuid}' stage by '{author_name} <{author_email}>'"
    )

    try:
        tags = get_tags(stage_tags)
    except MalformedStageTagError as e:
        perror(f"malformed stage tag: {e}")
        sys.exit(errno.EINVAL)

    try:
        manifest = load_manifest(patches_repo_path, manifest_uuid)
    except NoSuchManifestError:
        perror(f"unable to find manifest uuid '{manifest_uuid}' in db")
        sys.exit(errno.ENOENT)
    except MalformedManifestError:
        perror(f"malformed manifest uuid '{manifest_uuid}'")
        sys.exit(errno.EINVAL)
    except Exception as e:
        perror(f"unable to obtain manifest uuid '{manifest_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        stage = manifest.new_stage(
            AuthorData(user=author_name, email=author_email), tags
        )
    except MismatchStageAuthorError as e:
        perror("already active manifest stage, author mismatch")
        perror(f"active author: {e.stage_author.user} <{e.stage_author.email}>")
        sys.exit(errno.EEXIST)

    pinfo(f"currently active stage for manifest uuid '{manifest.release_uuid}'")
    pinfo(f"{Symbols.RIGHT_ARROW} active patchsets: {len(stage.patchsets)}")

    try:
        store_manifest(patches_repo_path, manifest)
    except Exception as e:
        perror(f"unable to write manifest to disk: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    pinfo(f"wrote manifest '{manifest.release_uuid}' to disk")


@cmd_manifest_stage.command("abort", help="Abort currently active manifest stage.")
@click.option(
    "-m",
    "--manifest-uuid",
    required=True,
    type=uuid.UUID,
    metavar="UUID",
    help="Manifest UUID to operate on.",
)
@click.option(
    "-p",
    "--patches-repo",
    "patches_repo_path",
    type=click.Path(
        exists=True, file_okay=False, dir_okay=True, resolve_path=True, path_type=Path
    ),
    required=True,
    help="Path to patches git repository",
)
@pass_ctx
def cmd_manifest_stage_abort(
    _ctx: Ctx,
    manifest_uuid: uuid.UUID,
    patches_repo_path: Path,
) -> None:
    logger.debug(f"abort manifest uuid '{manifest_uuid}' active stage")

    try:
        manifest = load_manifest(patches_repo_path, manifest_uuid)
    except NoSuchManifestError:
        perror(f"unable to find manifest uuid '{manifest_uuid}' in db")
        sys.exit(errno.ENOENT)
    except MalformedManifestError:
        perror(f"malformed manifest uuid '{manifest_uuid}'")
        sys.exit(errno.EINVAL)
    except Exception as e:
        perror(f"unable to obtain manifest uuid '{manifest_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    stage = manifest.abort_active_stage()
    if not stage:
        pinfo(f"manifest uuid '{manifest_uuid}' has no active stage")
        return
    pinfo(f"aborted active stage on manifest uuid '{manifest_uuid}'")
    pinfo(f"{Symbols.RIGHT_ARROW} aborted patch sets: {len(stage.patchsets)}")

    try:
        store_manifest(patches_repo_path, manifest)
    except Exception as e:
        perror(f"unable to write manifest to disk: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    pinfo(f"wrote manifest '{manifest.release_uuid}' to disk")


@cmd_manifest_stage.command("commit", help="Commit currently active manifest stage.")
@click.option(
    "-m",
    "--manifest-uuid",
    required=True,
    type=uuid.UUID,
    metavar="UUID",
    help="Manifest UUID to operate on.",
)
@click.option(
    "-p",
    "--patches-repo",
    "patches_repo_path",
    type=click.Path(
        exists=True, file_okay=False, dir_okay=True, resolve_path=True, path_type=Path
    ),
    required=True,
    help="Path to patches git repository",
)
@pass_ctx
def cmd_manifest_stage_commit(
    _ctx: Ctx, manifest_uuid: uuid.UUID, patches_repo_path: Path
) -> None:
    logger.debug(f"commit manifest uuid '{manifest_uuid}' active stage")

    try:
        manifest = load_manifest(patches_repo_path, manifest_uuid)
    except NoSuchManifestError:
        perror(f"unable to find manifest uuid '{manifest_uuid}' in db")
        sys.exit(errno.ENOENT)
    except MalformedManifestError:
        perror(f"malformed manifest uuid '{manifest_uuid}'")
        sys.exit(errno.EINVAL)
    except Exception as e:
        perror(f"unable to obtain manifest uuid '{manifest_uuid}': {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        stage = manifest.commit_active_stage()
    except EmptyActiveStageError:
        perror(f"manifest uuid '{manifest_uuid}' active stage is empty")
        pwarn("either add patch sets to active stage, or abort active stage")
        sys.exit(errno.EAGAIN)

    if not stage:
        perror(f"manifest uuid '{manifest_uuid}' has no active stage")
        sys.exit(errno.ENOENT)

    pinfo(f"committed active stage on manifest uuid '{manifest_uuid}'")
    pinfo(f"{Symbols.RIGHT_ARROW} committed patch sets: {len(stage.patchsets)}")
    pinfo(f"{Symbols.RIGHT_ARROW} sha: {stage.computed_hash}")

    try:
        store_manifest(patches_repo_path, manifest)
    except Exception as e:
        perror(f"unable to write manifest to disk: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    pinfo(f"wrote manifest '{manifest.release_uuid}' to disk")
