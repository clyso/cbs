# crt - releases database
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

import pydantic
from ceslib.utils.secrets import SecretsVaultMgr
from crtlib.db.errors import (
    DBError,
    S3DBConflictingManifestError,
    S3DBError,
    S3DBExistingManifestError,
)
from crtlib.db.local import LocalDB
from crtlib.db.remote import RemoteDB
from crtlib.db.s3 import S3DB
from crtlib.errors.manifest import (
    ManifestExistsError,
    NoSuchManifestError,
)
from crtlib.errors.patchset import (
    MalformedPatchSetError,
    NoSuchPatchSetError,
    PatchSetError,
)
from crtlib.models.manifest import ReleaseManifest
from crtlib.models.patchset import (
    GitHubPullRequest,
    PatchSetBase,
)

from . import logger as parent_logger

logger = parent_logger.getChild("db")


class ManifestListResult(pydantic.BaseModel):
    manifest: ReleaseManifest
    from_s3: bool
    local: bool
    modified: bool


class ReleasesDB:
    """
    Representation of the releases database.

    For a release db root at '$root', the format is as follows:

    $root/manifests         - stores release manifests as JSON files, identified by
                              their UUIDs.
    $root/patchsets         - stores patchsets for releases, identified by their UUIDs.
    $root/patchsets/gh      - stores pull requests from GitHub, mapping them to the
                              corresponding patch set UUIDs.
    $root/patches/by_uuid   - stores patches information as JSON files, identified by
                              their UUIDs.
    $root/patches/by_sha    - stores patches' SHAs, mapping them to the corresponding
                              patch UUID.

    The release db exists both on-disk and in S3.
    """

    base_path: Path
    _remote_db: RemoteDB
    _local_db: LocalDB
    _s3db: S3DB

    def __init__(self, path: Path, secrets: SecretsVaultMgr) -> None:
        self.base_path = path
        self._remote_db = RemoteDB(path.joinpath("remote"))
        self._local_db = LocalDB(path.joinpath("local"))
        self._s3db = S3DB(self._remote_db, secrets)

        self.base_path.mkdir(exist_ok=True, parents=True)

    @property
    def s3db(self) -> S3DB:
        return self._s3db

    @property
    def is_synced(self) -> bool:
        return self._s3db.is_synced

    def list_manifests(self, *, from_remote: bool = False) -> list[ManifestListResult]:
        """List known manifests."""
        manifest_dict: dict[uuid.UUID, ManifestListResult] = {}

        for manifest, info in self._local_db.list_manifests():
            modified = manifest.computed_hash != info.orig_hash
            manifest_dict[manifest.release_uuid] = ManifestListResult(
                manifest=manifest,
                from_s3=info.orig_etag is not None,
                modified=modified,
                local=True,
            )

        if from_remote:
            for manifest, _ in self._remote_db.list_manifests():
                if manifest.release_uuid in manifest_dict:
                    continue
                manifest_dict[manifest.release_uuid] = ManifestListResult(
                    manifest=manifest,
                    from_s3=True,
                    modified=False,
                    local=False,
                )

        return sorted(manifest_dict.values(), key=lambda e: e.manifest.creation_date)

    def load_manifest(self, _uuid: uuid.UUID) -> ReleaseManifest:
        """
        Load a release maifest from disk.

        Will first attempt to load it from the local db. If non-existent, then will
        attempt to load it from the remote db.

        If the manifest exists in the remote db and not in the local db, then we will
        import it to the local db (by storing it).
        """
        try:
            return self._local_db.get_manifest(_uuid)
        except NoSuchManifestError:
            logger.debug(
                f"manifest uuid '{_uuid}' not found in local db, attempt remote db"
            )
        except Exception as e:
            msg = f"unable to load manifest uuid '{_uuid}' from local db: {e}"
            logger.error(msg)
            raise DBError(msg=msg) from None

        try:
            manifest = self._remote_db.get_manifest(_uuid)
            info = self._remote_db.get_manifest_info(_uuid)
        except NoSuchManifestError as e:
            logger.error(f"unable to find manifest uuid '{_uuid}'")
            raise e from None
        except Exception as e:
            msg = f"unable to load manifest '{_uuid}' from remote db: {e}"
            logger.error(msg)
            raise DBError(msg=msg) from None

        try:
            self._local_db.store_manifest(manifest, etag=info.orig_etag)
        except Exception as e:
            msg = f"unable to write imported manifest uuid '{_uuid}' to local db: {e}"
            logger.error(msg)
            raise DBError(msg=msg) from None

        return manifest

    def store_manifest(
        self, manifest: ReleaseManifest, *, exist_ok: bool = True
    ) -> None:
        """
        Store a release manifest to disk.

        Storing a manifest presumes it either already exists in the local db, or it's
        a newly created manifest being stored.

        We will let the local db handle how to store it.

        If `exist_ok` is `False`, we will first check if the manifest already exists
        and fail if so.

        Note: remote manifests are always first loaded, and will be stored into the
        local db on first load.
        """
        if (
            not exist_ok
            and self._local_db.manifest_path(manifest.release_uuid).exists()
        ):
            msg = (
                f"conflicting manifest uuid '{manifest.release_uuid}' "
                + "exists in local db"
            )
            logger.error(msg)
            raise ManifestExistsError(manifest.release_uuid)

        try:
            self._local_db.store_manifest(manifest)
        except DBError as e:
            msg = f"unable to store manifest uuid '{manifest.release_uuid}': {e}"
            logger.error(msg)
            raise DBError(msg=msg) from None

    def load_patchset(self, _uuid: uuid.UUID) -> tuple[PatchSetBase, bool]:
        """
        Obtain a patch set by its UUID.

        A patch set may be local, if it hasn't been published yet, or remote.

        We'll first check for a local patch set, and only check for a remote one if
        none is present.
        """
        try:
            return (self._local_db.get_patchset(_uuid), False)
        except NoSuchPatchSetError:
            logger.debug(f"patch set uuid '{_uuid}' not found in local db")
        except MalformedPatchSetError:
            msg = f"unable to load patch set uuid '{_uuid}' from local db, malformed"
            logger.error(msg)
            raise DBError(msg=msg) from None
        except Exception as e:
            msg = f"unable to load patch set uuid '{_uuid}' from local db: {e}"
            logger.error(msg)
            raise DBError(msg=msg) from None

        try:
            return (self._remote_db.get_patchset(_uuid), True)
        except NoSuchPatchSetError as e:
            logger.error(f"patch set '{_uuid}' not found")
            raise e from None
        except PatchSetError as e:
            msg = f"unable to load patch set uuid '{_uuid}' from remote db: {e}"
            logger.error(msg)
            raise DBError(msg=msg) from None
        except Exception as e:
            msg = f"unable to load patch set uuid '{uuid}': {e}"
            logger.error(msg)
            raise DBError(msg=msg) from None

    def load_gh_pr(self, org: str, repo: str, pr_id: int) -> GitHubPullRequest:
        """
        Load a patch set's information, as a GitHub pull request, from disk.

        A patch set may be local, if it hasn't been published yet, or remote.

        We'll first check for a local patch set, and only check for a remote one if
        none is present.
        """
        gh_desc = f"{org}/{repo}/{pr_id}"
        try:
            return self._local_db.get_patchset_gh_pr(org, repo, pr_id)
        except NoSuchPatchSetError:
            logger.debug(f"github patch set for '{gh_desc}' not found in local db")
        except MalformedPatchSetError:
            msg = (
                f"unable to load github patch set '{gh_desc}' from local db, malformed"
            )
            logger.error(msg)
            raise DBError(msg=msg) from None
        except Exception as e:
            msg = f"unable to load github patch set '{gh_desc}' from local db: {e}"
            logger.error(msg)
            raise DBError(msg=msg) from None

        try:
            return self._remote_db.get_patchset_gh_pr(org, repo, pr_id)
        except NoSuchPatchSetError as e:
            logger.error(f"github patch set '{gh_desc}' not found")
            raise e from None
        except PatchSetError as e:
            msg = f"unable to load github patch set '{gh_desc}' from remote db: {e}"
            logger.error(msg)
            raise DBError(msg=msg) from None
        except Exception as e:
            msg = f"unable to load github patch set '{gh_desc}': {e}"
            logger.error(msg)
            raise DBError(msg=msg) from None

    def store_gh_patchset(self, patchset: GitHubPullRequest) -> None:
        """
        Store a GitHub pull request's information as a patch set to disk.

        Storing a patch set is always done to the local db. It will only hit the remote
        db after the patch set is published and after the db is eventually sync'ed.
        """
        # propagate exceptions
        logger.debug(f"store github patch set uuid '{patchset.patchset_uuid}'")
        self._local_db.store_patchset_gh_pr(patchset)

    def publish_manifest(self, _uuid: uuid.UUID) -> None:
        """
        Publish a manifest to the S3 db.

        Will push the manifest, if modified, to S3.

        All patch sets and patches will also be pushed to S3 if they don't exist there.
        """
        logger.info(f"publish manifest '{_uuid}'")

        try:
            manifest = self._local_db.get_manifest(_uuid)
            info = self._local_db.get_manifest_info(_uuid)
        except NoSuchManifestError as e:
            logger.error(f"unable to publish non-existent manifest uuid '{_uuid}'")
            raise e from None
        except Exception as e:
            msg = f"unable to obtain local manifest uuid '{_uuid}': {e}"
            logger.error(msg)
            raise DBError(msg=msg) from None

        if info.orig_etag and info.orig_hash == manifest.computed_hash:
            logger.info(f"manifest uuid '{_uuid}' not modified and already published")
            return

        # publish all patch sets first

        for patchset_uuid in manifest.patchsets:
            try:
                patchset, is_remote = self.load_patchset(patchset_uuid)
            except (PatchSetError, Exception) as e:
                msg = f"unable to load patch set uuid '{patchset_uuid}': {e}"
                logger.error(msg)
                raise DBError(msg=msg) from None

            if is_remote:
                logger.info(f"patch set uuid '{patchset_uuid}' already published, skip")
                continue

            if isinstance(patchset, GitHubPullRequest):
                logger.info(f"publish github patch set uuid '{patchset_uuid}'")
                try:
                    self.s3db.publish_gh_patchset(patchset)
                except S3DBError as e:
                    msg = f"unable to publish patch set uuid '{patchset_uuid}': {e}"
                    logger.error(msg)
                    raise DBError(msg=msg) from None

                # drop patch set from local db
                self._local_db.remove_gh_patchset(patchset)

            else:
                logger.info(f"publish vanilla patch set uuid '{patchset_uuid}'")

        # finally publish the manifest
        try:
            self.s3db.publish_manifest(manifest, orig_etag=info.orig_etag)
        except S3DBConflictingManifestError as e:
            logger.error(
                f"found conflicting manifest in s3db, uuid '{_uuid}', "
                + f"etag '{info.orig_etag}'"
            )
            logger.error(e)
            raise ManifestExistsError(_uuid) from None
        except S3DBExistingManifestError as e:
            logger.error(f"found existing manifest in s3db, uuid '{_uuid}'")
            logger.error(e)
            raise ManifestExistsError(_uuid) from None
        except Exception as e:
            msg = f"unable to publish manifest uuid '{_uuid}' to S3: {e}"
            logger.error(msg)
            raise DBError(msg=msg) from None

        # clean up
        logger.info(f"published manifest '{manifest.release_uuid}'")
        logger.debug("clean up local artefacts")
        self._local_db.remove_manifest(manifest.release_uuid)
