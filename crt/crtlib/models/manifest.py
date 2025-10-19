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
import string
import uuid
from datetime import datetime as dt
from random import choices

import pydantic

from crtlib.errors.manifest import (
    NoStageError,
)
from crtlib.errors.stages import StageError
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
    desc: str = pydantic.Field(default="")
    tags: list[tuple[str, str]] = pydantic.Field(default=[])
    patches: list[ManifestPatchEntryWrapper] = pydantic.Field(default=[])

    stage_uuid: uuid.UUID = pydantic.Field(default_factory=lambda: uuid.uuid4())
    is_published: bool = pydantic.Field(
        default=False,
        validation_alias=pydantic.AliasChoices("committed", "is_published"),
    )


class ReleaseManifest(pydantic.BaseModel):
    name: str
    base_release_name: str
    base_ref_org: str
    base_ref_repo: str
    base_ref: str
    dst_repo: str
    dst_branch: str | None = pydantic.Field(default=None)

    stages: list[ManifestStage] = pydantic.Field(default=[])

    from_name: str | None = pydantic.Field(default=None)
    from_uuid: uuid.UUID | None = pydantic.Field(default=None)

    creation_date: dt = pydantic.Field(default_factory=lambda: dt.now(datetime.UTC))
    release_uuid: uuid.UUID = pydantic.Field(default_factory=lambda: uuid.uuid4())
    release_git_uid: str = pydantic.Field(
        default_factory=lambda: "".join(choices(string.ascii_letters, k=6))  # noqa: S311
    )

    @property
    def patches(self) -> list[ManifestPatchEntry]:
        return [e.contents for stage in self.stages for e in stage.patches]

    def contains_patchset(self, patchset: ManifestPatchEntry) -> bool:
        """Check if the release manifest contains a given patch set."""
        return patchset.entry_uuid in [e.entry_uuid for e in self.patches]

    @property
    def is_published(self) -> bool:
        return len(self.stages) > 0 and all(s.is_published for s in self.stages)

    @property
    def latest_stage(self) -> ManifestStage:
        try:
            return next(reversed(self.stages))
        except StopIteration:
            raise NoStageError(uuid=self.release_uuid) from None

    def get_stage(self, stage_uuid: uuid.UUID) -> ManifestStage:
        """Obtain a stage by its UUID."""
        for stage in self.stages:
            if stage.stage_uuid == stage_uuid:
                return stage

        msg = f"no such stage uuid '{stage_uuid}'"
        logger.error(msg)
        raise NoStageError(uuid=self.release_uuid, msg=msg)

    @property
    def active_stage(self) -> ManifestStage | None:
        try:
            stage = self.latest_stage
        except NoStageError:
            return None
        return stage if not stage.is_published else None

    def new_stage(
        self,
        author: AuthorData,
        tags: list[tuple[str, str]],
        desc: str,
    ) -> ManifestStage:
        """
        Create a new stage in the release manifest.

        An uncommitted stage is created, ready to have patch sets added to.

        If a stage is currently active, raise an error.
        """
        active_stage = self.active_stage
        if active_stage and not active_stage.patches:
            raise StageError(msg="latest stage has no patches, cannot create new stage")

        stage = ManifestStage(author=author, tags=tags, desc=desc)
        self.stages.append(stage)
        return stage

    def remove_stage(self, stage_uuid: uuid.UUID) -> None:
        new_stage_lst: list[ManifestStage] = [
            s for s in self.stages if s.stage_uuid != stage_uuid
        ]
        if len(new_stage_lst) == len(self.stages):
            raise NoStageError(uuid=self.release_uuid)

        self.stages = new_stage_lst

    def add_patches(self, patchset: ManifestPatchEntry) -> bool:
        """
        Add a patch set to this release manifest.

        Returns a tuple containing:
        - `bool`, indicating whether the patch set was added or not.
        - `list[Patch]`, with the patches that were added to the release manifest.
        - `list[Patch]`, with the patches that were skipped and not added to the
                         release manifest.
        """
        if self.contains_patchset(patchset):
            return False

        # propagate 'NoActiveManifestStageError'
        stage = self.latest_stage
        stage.patches.append(ManifestPatchEntryWrapper(contents=patchset))  # pyright: ignore[reportArgumentType]
        return True

    def gen_header(self) -> list[tuple[str, str]]:
        entries = [
            ("name", self.name),
            ("base release", self.base_release_name),
            ("base repository", f"{self.base_ref_org}/{self.base_ref_repo}"),
            ("base ref", self.base_ref),
            ("dest repository", self.dst_repo),
            ("dest branch", self.dst_branch or "n/a"),
            ("creation date", str(self.creation_date)),
            ("manifest uuid", str(self.release_uuid)),
            ("stages", str(len(self.stages))),
            ("published", "yes" if self.is_published else "no"),
        ]
        if self.from_name and self.from_uuid:
            entries.append(("from name", self.from_name))
            entries.append(("from uuid", str(self.from_uuid)))

        return entries
