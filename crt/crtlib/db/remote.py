# crt - db - remote on-disk representation
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
import uuid
from datetime import datetime as dt
from pathlib import Path
from typing import override

import pydantic
from crtlib.db.base import BaseDB
from crtlib.errors import CRTError
from crtlib.errors.manifest import NoSuchManifestError
from crtlib.models.db import DBManifestInfo
from crtlib.models.manifest import ReleaseManifest
from crtlib.models.patchset import GitHubPullRequest, PatchSet
from pydantic_core import from_json

from . import logger as parent_logger
from . import manifest_loc

logger = parent_logger.getChild("remotedb")


class RemoteDBError(CRTError):
    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("remote db error")


class _DBEntry(pydantic.BaseModel):
    obj: str
    last_updated: dt
    etag: str


class _DB(pydantic.BaseModel):
    last_updated: dt | None
    entries: dict[str, _DBEntry]
    path: Path = pydantic.Field(exclude=True)

    @classmethod
    def load(cls, path: Path) -> _DB:
        """Load the database from disk."""
        if not path.exists():
            logger.info(f"creating new remote db at '{path}'")
            _db = _DB(
                last_updated=None,
                entries={},
                path=path,
            )
            _db.store()

        try:
            db_dict = from_json(path.read_text())  # pyright: ignore[reportAny]
            db_dict["path"] = path
            return _DB.model_validate(db_dict)
        except pydantic.ValidationError:
            msg = f"malformed remote db file at '{path}"
            logger.error(msg)
            raise RemoteDBError(msg) from None

    def store(self, *, updated_on: dt | None = None) -> None:
        """Store the database to disk."""
        if not updated_on:
            updated_on = dt.now(datetime.UTC)

        try:
            _ = self.path.write_text(self.model_dump_json(indent=2))
        except Exception as e:
            msg = f"unable to write remote db file at '{self.path}': {e}"
            logger.error(msg)
            raise RemoteDBError(msg=msg) from None

    def put(
        self, obj: str, etag: str, *, flush: bool = False, updated_on: dt | None = None
    ) -> None:
        """Add an entry for an object, specifying its etag."""
        if not updated_on:
            updated_on = dt.now(datetime.UTC)

        if obj not in self.entries:
            self.entries[obj] = _DBEntry(obj=obj, last_updated=updated_on, etag=etag)

        entry = self.entries[obj]
        entry.last_updated = updated_on
        entry.etag = etag

        if flush:
            self.store(updated_on=updated_on)

    def get(self, obj: str) -> tuple[str, dt]:
        """Obtain an object's etag and last modified time."""
        if obj not in self.entries:
            raise ValueError

        entry = self.entries[obj]
        if not entry.etag:
            msg = f"missing etag for object '{obj}'"
            logger.error(msg)
            raise RemoteDBError(msg=msg)

        return (entry.etag, entry.last_updated)

    def all(self, *, prefix: str | None = None) -> dict[str, tuple[str, dt]]:
        """Obtain a `dict` of all known objects, filtered if `prefix` is given."""

        def _filter(entry: _DBEntry) -> bool:
            return entry.obj.startswith(prefix) if prefix else True

        return {
            k: (e.obj, e.last_updated) for k, e in self.entries.items() if _filter(e)
        }


class RemoteDB(BaseDB):
    _db_path: Path
    _db: _DB

    def __init__(self, base_path: Path) -> None:
        super().__init__(base_path)
        self._db_path = base_path.joinpath("remote.db")
        self._db = _DB.load(self._db_path)

    @property
    def is_init(self) -> bool:
        return self._db.last_updated is not None

    @property
    def last_updated(self) -> dt | None:
        return self._db.last_updated

    def update(
        self,
        obj: str,
        etag: str,
        value: bytes,
        *,
        flush: bool = True,
        updated_on: dt | None = None,
    ) -> None:
        """Update (or add) a given object's contents and its etag."""
        obj_path = self._base_path.joinpath(obj)
        obj_path.parent.mkdir(exist_ok=True, parents=True)
        n = obj_path.write_bytes(value)
        logger.debug(f"wrote object '{obj}' size {n}")

        self._db.put(obj, etag, flush=flush, updated_on=updated_on)

    def get(self, obj: str) -> tuple[str, bytes] | None:
        """Obtain a given object's etag and contents, if any."""
        try:
            res = self._db.get(obj)
        except ValueError:
            return None
        except RemoteDBError as e:
            logger.error(f"unable to obtain '{obj}' from remote db: {e}")
            return None

        obj_path = self._base_path.joinpath(obj)
        contents = obj_path.read_bytes()
        return (res[0], contents)

    def sync_to_disk(self, *, updated_on: dt | None = None) -> None:
        logger.info(f"persist remote db to disk, updated on: {updated_on}")
        self._db.store(updated_on=updated_on)

    def exists(self, obj: str, *, etag: str | None = None) -> bool:
        try:
            existing = self._db.get(obj)
        except ValueError:
            return False

        if etag:
            return existing[0] == etag
        return True

    def all(self, *, prefix: str | None = None) -> dict[str, tuple[str, dt]]:
        """Obtain a `dict` of all known objects, filtered if `prefix` is specified."""
        return self._db.all(prefix=prefix)

    @override
    def get_manifest(self, _uuid: uuid.UUID) -> ReleaseManifest:
        """Obtain a manifest from the remote db on-disk representation."""
        return self._read_manifest(_uuid, ReleaseManifest)

    @override
    def get_manifest_info(self, _uuid: uuid.UUID) -> DBManifestInfo:
        """Obtain a given manifest's information."""
        loc_manifest = manifest_loc("", _uuid)
        res = self._db.get(loc_manifest)
        if not res:
            raise NoSuchManifestError(_uuid)

        return DBManifestInfo(
            orig_hash=None,
            orig_etag=res[0],
            remote=True,
        )

    @override
    def store_manifest(
        self, manifest: ReleaseManifest, *, etag: str | None = None
    ) -> None:
        """
        Ignored.

        The remote db does not store manifests. Use its `update` methods instead.
        """
        raise NotImplementedError()

    @override
    def store_patchset(self, _uuid: uuid.UUID, patchset: PatchSet) -> None:
        """
        Ignored.

        The remote db does not store patch sets. Use its `update` methods instead.
        """
        raise NotImplementedError()

    @override
    def store_patchset_gh_pr(self, patchset: GitHubPullRequest) -> None:
        """
        Ignored.

        The remote db does not store patch sets. Use its `update` methods instead.
        """
        raise NotImplementedError()
