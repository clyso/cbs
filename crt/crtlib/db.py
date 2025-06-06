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


import contextlib
import uuid
from pathlib import Path

import pydantic
from crtlib.logger import logger as parent_logger
from crtlib.manifest import MalformedManifestError, NoSuchManifestError, ReleaseManifest
from crtlib.patch import MalformedPatchError, NoSuchPatchError, Patch, PatchExistsError
from crtlib.patchset import (
    GitHubPullRequest,
    MalformedPatchSetError,
    NoSuchPatchSetError,
    PatchSet,
    PatchSetBase,
    PatchSetError,
    PatchSetMismatchError,
)

logger = parent_logger.getChild("db")


class ReleasesDB:
    """
    On-disk representation of the releases database.

    For a release db root at '$root', the on-disk format is as follows:

    $root/manifests         - stores release manifests as JSON files, identified by
                              their UUIDs.
    $root/patchsets         - stores patchsets for releases, identified by their UUIDs.
    $root/patchsets/gh      - stores pull requests from GitHub, mapping them to the
                              corresponding patch set UUIDs.
    $root/patches/by_uuid   - stores patches information as JSON files, identified by
                              their UUIDs.
    $root/patches/by_sha    - stores patches' SHAs, mapping them to the corresponding
                              patch UUID.
    """

    db_path: Path

    def __init__(self, path: Path) -> None:
        self.db_path = path
        self._init_tree()

    def _init_tree(self) -> None:
        self.manifests_path.mkdir(exist_ok=True, parents=True)
        self.gh_prs_path.mkdir(exist_ok=True, parents=True)
        self.patches_by_uuid_path.mkdir(exist_ok=True, parents=True)
        self.patches_by_sha_path.mkdir(exist_ok=True, parents=True)

    @property
    def manifests_path(self) -> Path:
        return self.db_path.joinpath("manifests")

    @property
    def patchsets_path(self) -> Path:
        return self.db_path.joinpath("patchsets")

    @property
    def gh_prs_path(self) -> Path:
        return self.patchsets_path.joinpath("gh")

    @property
    def patches_path(self) -> Path:
        return self.db_path.joinpath("patches")

    @property
    def patches_by_uuid_path(self) -> Path:
        return self.patches_path.joinpath("by_uuid")

    @property
    def patches_by_sha_path(self) -> Path:
        return self.patches_path.joinpath("by_sha")

    def list_manifests_uuids(self) -> list[uuid.UUID]:
        """Obtain the UUIDs for all known release manifests."""
        uuids_lst: list[uuid.UUID] = []
        for entry in self.manifests_path.glob("*.json"):
            try:
                entry_uuid = uuid.UUID(entry.stem)
            except Exception:  # noqa: S112
                # malformed UUID, ignore.
                continue
            uuids_lst.append(entry_uuid)

        return uuids_lst

    def load_manifest(self, uuid: uuid.UUID) -> ReleaseManifest:
        """Load a release manifest from disk."""
        manifest_path = self.manifests_path.joinpath(f"{uuid}.json")
        if not manifest_path.exists():
            raise NoSuchManifestError(uuid)

        try:
            with manifest_path.open("r") as fd:
                manifest = ReleaseManifest.model_validate_json(fd.read())
        except pydantic.ValidationError:
            raise MalformedManifestError(uuid) from None
        # propagate further exceptions
        return manifest

    def store_manifest(self, manifest: ReleaseManifest) -> None:
        """Store a release manifest to disk."""
        manifest_path = self.manifests_path.joinpath(f"{manifest.release_uuid}.json")
        _ = manifest_path.write_text(manifest.model_dump_json(indent=2))

    def get_patchset_path(self, uuid: uuid.UUID) -> Path:
        return self.patchsets_path.joinpath(f"{uuid}.json")

    def load_patchset(self, uuid: uuid.UUID) -> PatchSetBase:
        """Obtain a patch set by its UUID."""
        patchset_path = self.patchsets_path.joinpath(f"{uuid}.json")
        if not patchset_path.exists():
            raise NoSuchPatchSetError(msg=f"uuid '{uuid}'")

        try:
            patchset_ctr = PatchSet.model_validate_json(patchset_path.read_text())
        except pydantic.ValidationError:
            raise MalformedPatchSetError(msg=f"uuid '{uuid}'") from None

        return patchset_ctr.info

    def load_gh_pr(self, org: str, repo: str, pr_id: int) -> GitHubPullRequest:
        """Load a patch set's information, as a GitHub pull request, from disk."""
        pr_path = self.gh_prs_path.joinpath(f"{org}/{repo}/{pr_id}")
        logger.debug(f"pr path: {pr_path}")
        if not pr_path.exists():
            raise NoSuchPatchSetError(f"gh/{org}/{repo}/{pr_id}")

        try:
            patchset_uuid = uuid.UUID(pr_path.read_text())
        except Exception as e:
            raise PatchSetError(
                msg=f"missing uuid for 'gh/{org}/{repo}/{pr_id}: {e}"
            ) from None

        patchset_path = self.patchsets_path.joinpath(f"{patchset_uuid}.json")
        if not patchset_path.exists():
            raise NoSuchPatchSetError(msg=f"uuid '{patchset_uuid}'")

        try:
            patchset = PatchSet.model_validate_json(patchset_path.read_text())
        except pydantic.ValidationError:
            raise MalformedPatchSetError(msg=f"uuid '{patchset_uuid}'") from None
        # propagate further exceptions

        if not isinstance(patchset.info, GitHubPullRequest):
            raise PatchSetMismatchError(msg=f"uuid '{patchset_uuid}' expected github")
        return patchset.info

    def store_gh_patchset(self, patchset: GitHubPullRequest) -> None:
        """Store a GitHub pull request's information as a patch set to disk."""
        pr_base_path = self.gh_prs_path.joinpath(patchset.org_name).joinpath(
            patchset.repo_name
        )
        pr_base_path.mkdir(exist_ok=True, parents=True)
        pr_path = pr_base_path.joinpath(f"{patchset.pull_request_id}")
        patchset_path = self.patchsets_path.joinpath(f"{patchset.patchset_uuid}.json")

        patchset_ctr = PatchSet(info=patchset)
        # propagate exceptions
        _ = patchset_path.write_text(patchset_ctr.model_dump_json(indent=2))
        _ = pr_path.write_text(str(patchset.patchset_uuid))

        for patch in patchset.patches:
            with contextlib.suppress(PatchExistsError):
                self.store_patch(patch)

    def load_patch(self, patch_uuid: uuid.UUID) -> Patch:
        """Load a patch's information from disk, by its UUID."""
        patch_path = self.patches_by_uuid_path.joinpath(f"{patch_uuid}.json")
        if not patch_path.exists():
            raise NoSuchPatchError(patch_uuid)

        try:
            patch = Patch.model_validate_json(patch_path.read_text())
        except pydantic.ValidationError:
            raise MalformedPatchError(patch_uuid) from None

        return patch

    def store_patch(self, patch: Patch) -> None:
        """Store a patch's information to disk."""
        sha_path = self.patches_by_sha_path.joinpath(patch.sha)
        uuid_path = self.patches_by_uuid_path.joinpath(f"{patch.patch_uuid}.json")

        if sha_path.exists() or uuid_path.exists():
            raise PatchExistsError(patch.sha, patch.patch_uuid)

        # propagate exceptions
        _ = sha_path.write_text(str(patch.patch_uuid))
        _ = uuid_path.write_text(patch.model_dump_json(indent=2))
