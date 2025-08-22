# crt - models - manifest
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

from __future__ import annotations

import datetime
import hashlib
import string
import uuid
from datetime import datetime as dt
from random import choices

import pydantic
from crtlib.errors.manifest import (
    ActiveManifestStageFoundError,
    EmptyActiveStageError,
    NoActiveManifestStageError,
    NoStageError,
)
from crtlib.git_utils import SHA
from crtlib.models.common import (
    AuthorData,
    ManifestPatchEntry,
)
from crtlib.models.discriminator import (
    ManifestPatchEntryWrapper,
)

from . import logger as parent_logger

logger = parent_logger.getChild("manifest")


class ManifestStage(pydantic.BaseModel):
    author: AuthorData
    creation_date: dt = pydantic.Field(default_factory=lambda: dt.now(datetime.UTC))
    tags: list[tuple[str, int]] = pydantic.Field(default=[])
    patches: list[ManifestPatchEntryWrapper] = pydantic.Field(default=[])

    stage_uuid: uuid.UUID = pydantic.Field(default_factory=lambda: uuid.uuid4())
    committed: bool = pydantic.Field(default=False)
    hash: str = pydantic.Field(default="")

    def _compute_hash(self) -> str:
        h = hashlib.sha256()
        h.update(self.author.model_dump_json().encode())
        h.update(self.creation_date.isoformat().encode())
        h.update(bytes(self.committed))

        for entry in self.patches:
            h.update(entry.contents.compute_hash_bytes())

        h.update(self.stage_uuid.bytes)

        return h.hexdigest()

    @pydantic.field_serializer("hash")
    def serialize_model_hash(self, _hash: str) -> str:
        return self.computed_hash if self.committed else ""

    @property
    def valid_hash(self) -> bool:
        return self.computed_hash == self.hash if self.committed else True

    @property
    def computed_hash(self) -> str:
        return self._compute_hash()


class ReleaseManifest(pydantic.BaseModel):
    name: str
    base_release_name: str
    base_ref_org: str
    base_ref_repo: str
    base_ref: str
    dst_repo: str

    stages: list[ManifestStage] = pydantic.Field(default=[])

    from_name: str | None = pydantic.Field(default=None)
    from_uuid: uuid.UUID | None = pydantic.Field(default=None)

    creation_date: dt = pydantic.Field(default_factory=lambda: dt.now(datetime.UTC))
    release_uuid: uuid.UUID = pydantic.Field(default_factory=lambda: uuid.uuid4())
    release_git_uid: str = pydantic.Field(
        default_factory=lambda: "".join(choices(string.ascii_letters, k=6))  # noqa: S311
    )

    hash: str = pydantic.Field(default="")

    def _compute_hash(self) -> str:
        h = hashlib.sha256()
        h.update(self.name.encode())
        h.update(self.creation_date.isoformat().encode())
        h.update(self.release_uuid.bytes)
        h.update(self.release_git_uid.encode())

        for stage in self.stages:
            h.update(stage.computed_hash.encode())

        return h.hexdigest()

    @pydantic.field_serializer("hash")
    def serialize_model_hash(self, _hash: str) -> str:
        return self.computed_hash

    @property
    def computed_hash(self) -> SHA:
        return self._compute_hash()

    @property
    def valid_hash(self) -> bool:
        return self.computed_hash == self.hash

    @property
    def patches(self) -> list[ManifestPatchEntry]:
        return [e.contents for stage in self.stages for e in stage.patches]

    @property
    def active_stage(self) -> ManifestStage | None:
        try:
            return self.get_active_stage()
        except NoActiveManifestStageError:
            return None

    @property
    def committed(self) -> bool:
        return all(s.committed for s in self.stages)

    def contains_patches(self, patches: ManifestPatchEntry) -> bool:
        """Check if the release manifest contains a given patch set."""
        return patches.entry_uuid in [e.entry_uuid for e in self.patches]

    @property
    def latest_stage(self) -> ManifestStage:
        try:
            return next(reversed(self.stages))
        except StopIteration:
            raise NoStageError(uuid=self.release_uuid) from None

    def get_active_stage(self) -> ManifestStage:
        """
        Get currently active release manifest stage.

        If none is active, raise `NoActiveManifestStageError`.
        """
        stage: ManifestStage | None = None
        try:
            stage = self.latest_stage
        except NoStageError:
            logger.debug(f"no available stages on manifest '{self.release_uuid}'")

        if not stage or stage.committed:
            raise NoActiveManifestStageError(uuid=self.release_uuid)

        return stage

    def get_stage(self, stage_uuid: uuid.UUID) -> ManifestStage:
        """Obtain a stage by its UUID."""
        for stage in self.stages:
            if stage.stage_uuid == stage_uuid:
                return stage

        msg = f"no such stage uuid '{stage_uuid}'"
        logger.error(msg)
        raise NoStageError(uuid=self.release_uuid, msg=msg)

    def new_stage(
        self, author: AuthorData, tags: list[tuple[str, int]]
    ) -> ManifestStage:
        """
        Create a new stage in the release manifest.

        An uncommitted stage is created, ready to have patch sets added to.

        If a stage is currently active, raise an error.
        """
        try:
            stage = self.get_active_stage()
        except NoActiveManifestStageError:
            stage = ManifestStage(author=author, tags=tags)
        else:
            raise ActiveManifestStageFoundError(uuid=self.release_uuid) from None

        self.stages.append(stage)
        return stage

    def abort_active_stage(self) -> ManifestStage | None:
        """Abort the currently active stage, if any."""
        try:
            _ = self.get_active_stage()
        except NoActiveManifestStageError:
            return None

        return self.stages.pop()

    def remove_stage(self, stage_uuid: uuid.UUID) -> None:
        new_stage_lst: list[ManifestStage] = [
            s for s in self.stages if s.stage_uuid != stage_uuid
        ]
        if len(new_stage_lst) == len(self.stages):
            raise NoStageError(uuid=self.release_uuid)

        self.stages = new_stage_lst

    def commit_active_stage(self) -> ManifestStage | None:
        """Commit the currently active stage."""
        try:
            stage = self.get_active_stage()
        except NoActiveManifestStageError:
            return None

        if not stage.patches:
            raise EmptyActiveStageError(uuid=self.release_uuid)

        stage.committed = True
        return stage

    def add_patches(self, patches: ManifestPatchEntry) -> bool:
        """
        Add a patch set to this release manifest.

        Returns a tuple containing:
        - `bool`, indicating whether the patch set was added or not.
        - `list[Patch]`, with the patches that were added to the release manifest.
        - `list[Patch]`, with the patches that were skipped and not added to the
                         release manifest.
        """
        if self.contains_patches(patches):
            return False

        # propagate 'NoActiveManifestStageError'
        stage = self.get_active_stage()
        stage.patches.append(ManifestPatchEntryWrapper(contents=patches))  # pyright: ignore[reportArgumentType]
        return True

    def gen_header(self) -> list[tuple[str, str]]:
        entries = [
            ("name", self.name),
            ("base release", self.base_release_name),
            ("base repository", f"{self.base_ref_org}/{self.base_ref_repo}"),
            ("base ref", self.base_ref),
            ("dest repository", self.dst_repo),
            ("creation date", str(self.creation_date)),
            ("manifest uuid", str(self.release_uuid)),
            ("stages", str(len(self.stages))),
        ]
        if self.from_name and self.from_uuid:
            entries.append(("from name", self.from_name))
            entries.append(("from uuid", str(self.from_uuid)))

        return entries
