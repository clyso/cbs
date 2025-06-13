# crt - db - local state db
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


import uuid
from pathlib import Path
from typing import override

from crtlib.db.base import BaseDB
from crtlib.db.errors import DBError
from crtlib.errors.manifest import NoSuchManifestError
from crtlib.errors.patchset import PatchSetExistsError
from crtlib.models.db import DBLocalManifestWrapper, DBManifestInfo
from crtlib.models.manifest import ReleaseManifest
from crtlib.models.patchset import GitHubPullRequest, PatchSet

from . import logger as parent_logger

logger = parent_logger.getChild("localdb")


class LocalDB(BaseDB):
    def __init__(self, path: Path) -> None:
        super().__init__(path)

    @override
    def get_manifest(self, _uuid: uuid.UUID) -> ReleaseManifest:
        """Obtain a manifest by `uuid`."""
        wrapper = self._read_manifest(_uuid, DBLocalManifestWrapper)
        return wrapper.manifest

    @override
    def get_manifest_info(self, _uuid: uuid.UUID) -> DBManifestInfo:
        """Obtain a given manifest's information, by `uuid`."""
        wrapper = self._read_manifest(_uuid, DBLocalManifestWrapper)
        return DBManifestInfo(
            orig_hash=wrapper.orig_hash,
            orig_etag=wrapper.orig_etag,
            remote=False,
        )

    @override
    def store_manifest(
        self, manifest: ReleaseManifest, *, etag: str | None = None
    ) -> None:
        """Store to disk the provided manifest."""
        try:
            wrapper = self._read_manifest(manifest.release_uuid, DBLocalManifestWrapper)
            wrapper.manifest = manifest
        except NoSuchManifestError:
            wrapper = DBLocalManifestWrapper(
                orig_etag=etag, orig_hash=manifest.computed_hash, manifest=manifest
            )
        self._write_manifest(manifest.release_uuid, wrapper)

    @override
    def store_patchset_gh_pr(self, patchset: GitHubPullRequest) -> None:
        """Store a GitHub pull request patch set to disk."""
        pr_base_path = self.gh_prs_path.joinpath(patchset.org_name).joinpath(
            patchset.repo_name
        )
        pr_base_path.mkdir(exist_ok=True, parents=True)
        pr_path = pr_base_path.joinpath(str(patchset.pull_request_id))

        if pr_path.exists():
            gh_desc = pr_path.relative_to(self.gh_prs_path)
            msg = f"github patch set '{gh_desc}' already exists"
            logger.warning(msg)
            raise PatchSetExistsError(msg=msg)

        try:
            _ = self.store_patchset(patchset.patchset_uuid, PatchSet(info=patchset))
            _ = pr_path.write_text(str(patchset.patchset_uuid))
        except Exception as e:
            msg = (
                f"unable to write github patch set uuid '{patchset.patchset_uuid}' "
                + f"to local db: {e}"
            )
            logger.error(msg)
            raise DBError(msg=msg) from None

    def remove_manifest(self, _uuid: uuid.UUID) -> None:
        """
        Remove a manifest from the local db.

        This will often happen because the manifest has been published and is no longer
        needed locally.
        """
        manifest_path = self.manifest_path(_uuid)
        manifest_path.unlink(missing_ok=True)

    def remove_gh_patchset(self, patchset: GitHubPullRequest) -> None:
        """
        Remove a GitHub pull request patch set from the local db.

        This will often happen because the patch set has been published and is no
        longer needed locally.
        """
        pr_desc = f"{patchset.org_name}/{patchset.repo_name}/{patchset.pull_request_id}"
        pr_path = self.gh_prs_path.joinpath(pr_desc)
        pr_path.unlink(missing_ok=True)

        patchset_path = self.patchsets_path.joinpath(f"{patchset.patchset_uuid}.json")
        patchset_path.unlink(missing_ok=True)

        # attempt to remove gh directories if empty
        for p in pr_path.parents:
            if not p.is_relative_to(self.patchsets_path):
                break
            try:
                p.rmdir()
            except OSError as e:
                logger.debug(f"unable to remove dir at '{p}': {e}")
                break
