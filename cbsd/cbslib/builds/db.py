# CBS service library - builds - db
# Copyright (C) 2025  Clyso GmbH
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU Affero General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU Affero General Public License for more details.

from __future__ import annotations

import asyncio
import datetime
import dbm
from datetime import datetime as dt
from pathlib import Path

import pydantic
from cbscore.errors import CESError
from cbsdcore.builds.types import BuildEntry, BuildID, EntryState

from cbslib.builds import logger as parent_logger

logger = parent_logger.getChild("db")


class BuildsDBError(CESError):
    pass


class NoSuchBuildError(BuildsDBError):
    """Build ID not found in database."""

    def __init__(self, build_id: int) -> None:
        super().__init__(f"no such build ID {build_id} in database")


class MalformedBuildEntryError(BuildsDBError):
    """Build entry in database is malformed."""

    def __init__(self, build_id: int) -> None:
        super().__init__(f"malformed build entry for build ID {build_id} in database")


class MalformedDBRootError(BuildsDBError):
    """DB Root entry in database is malformed."""

    def __init__(self) -> None:
        super().__init__("malformed builds DB root entry in database")


class _DBRoot(pydantic.BaseModel):
    """Builds database root entry."""

    last_build_id: BuildID

    @property
    def next_build_id(self) -> int:
        """Increment and return the next build ID."""
        self.last_build_id += 1
        return self.last_build_id

    def save(self, path: Path) -> None:
        """Store the builds DB root back to disk."""
        try:
            with dbm.open(path, flag="c") as db:
                db["builds_root"] = self.model_dump_json()
        except Exception as e:
            msg = f"failed to save builds db root: {e}"
            logger.error(msg)
            raise BuildsDBError(msg) from e

    @classmethod
    def load(cls, path: Path) -> _DBRoot:
        """Load the builds DB root from disk."""
        try:
            with dbm.open(path, flag="c") as db:
                if "builds_root" in db:
                    data = db["builds_root"]
                    return _DBRoot.model_validate_json(data)
                else:
                    logger.info("builds db root not found, creating new")
                    return cls(last_build_id=0)

        except pydantic.ValidationError as e:
            logger.error(f"malformed builds db root in db:\n{e}")
            raise MalformedDBRootError() from e

        except Exception as e:
            msg = f"failed to load builds db root: {e}"
            logger.error(msg)
            raise BuildsDBError(msg) from e


class DBBuildEntry(pydantic.BaseModel):
    """Individual build entry in the builds database."""

    build_id: BuildID
    entry: BuildEntry

    def save(self, path: Path) -> None:
        """Store the build entry back to disk."""
        try:
            with dbm.open(path, flag="c") as db:
                db[f"build_{self.build_id}"] = self.model_dump_json()
        except Exception as e:
            msg = f"failed to save build entry {self.build_id}: {e}"
            logger.error(msg)
            raise BuildsDBError(msg) from e


class BuildsDB:
    """Interface to the builds database."""

    _db_path: Path
    _root: _DBRoot
    _lock: asyncio.Lock

    def __init__(self, db_path: Path) -> None:
        self._db_path = db_path
        # propagate exceptions
        self._root = _DBRoot.load(db_path)
        self._lock = asyncio.Lock()

    async def new(self, entry: BuildEntry) -> BuildID:
        """Create a new build entry in the database."""
        async with self._lock:
            build_id = self._root.next_build_id
            db_entry = DBBuildEntry(build_id=build_id, entry=entry)
            db_entry.save(self._db_path)
            self._root.save(self._db_path)
            return build_id

    async def update(self, build_id: BuildID, entry: BuildEntry) -> None:
        """Update an existing build entry in the database."""
        async with self._lock:
            if build_id < 1 or build_id > self._root.last_build_id:
                msg = f"no such build ID {build_id} in database"
                logger.error(msg)
                raise BuildsDBError(msg)

            db_key = f"build_{build_id}"

            try:
                with dbm.open(self._db_path, flag="c") as db:
                    db_entry = DBBuildEntry.model_validate_json(db[db_key])
                    db_entry.entry = entry
                    db_entry.save(self._db_path)

            except pydantic.ValidationError as e:
                logger.error(f"malformed build entry {build_id} in db:\n{e}")
                raise MalformedBuildEntryError(build_id) from None

            except ValueError:
                logger.error(f"build entry {build_id} missing from db")
                raise NoSuchBuildError(build_id) from None

            except Exception as e:
                msg = f"failed to update build entry {build_id}: {e}"
                logger.error(msg)
                raise BuildsDBError(msg) from e

    async def gc(self) -> None:
        """Garbage collect old build entries from the database, marking them failed."""
        logger.info("starting builds db garbage collection")
        start = dt.now(datetime.UTC)

        _ = await self._lock.acquire()
        try:
            with dbm.open(self._db_path, flag="c") as db:
                for id in range(1, self._root.last_build_id + 1):
                    key = f"build_{id}"
                    if key not in db:
                        logger.warning(f"build entry {id} missing from db, skipping")
                        continue

                    try:
                        db_entry = DBBuildEntry.model_validate_json(db[key])
                    except pydantic.ValidationError as e:
                        logger.warning(
                            f"malformed build entry {id} in db, skipping:\n{e}"
                        )
                        continue

                    if db_entry.entry.state in {
                        "SUCCESS",
                        "FAILURE",
                        "REVOKED",
                        "REJECTED",
                    }:
                        continue

                    logger.info(f"garbage collecting build {id}, marking as FAILURE")
                    db_entry.entry.state = EntryState.failure
                    db[key] = db_entry.model_dump_json()

        except Exception as e:
            msg = f"failed to garbage collect builds db: {e}"
            logger.error(msg)
            raise BuildsDBError(msg) from e
        finally:
            self._lock.release()

        delta = dt.now(datetime.UTC) - start
        logger.info(
            f"completed builds db garbage collection in {delta.total_seconds()} seconds"
        )

    async def ls(
        self,
        *,
        start_id: int = 1,
        max_entries: int | None = None,
    ) -> list[DBBuildEntry]:
        """
        List build entries in the database.

        If 'max_entries' is None, list all entries from 'start_id' to the latest.
        """
        async with self._lock:
            end_id = (
                start_id + max_entries if max_entries else self._root.last_build_id + 1
            )
            if end_id > self._root.last_build_id + 1:
                end_id = self._root.last_build_id + 1

        entries: list[DBBuildEntry] = []
        try:
            with dbm.open(self._db_path, flag="c") as db:
                for id in range(start_id, end_id):
                    key = f"build_{id}"
                    if key not in db:
                        logger.warning(f"build entry {id} missing from db, skipping")
                        continue

                    try:
                        db_entry = DBBuildEntry.model_validate_json(db[key])
                    except pydantic.ValidationError as e:
                        logger.warning(
                            f"malformed build entry {id} in db, skipping:\n{e}"
                        )
                        continue

                    entries.append(db_entry)

        except Exception as e:
            msg = f"failed to list builds from db: {e}"
            logger.error(msg)
            raise BuildsDBError(msg) from e

        return entries

    async def get(self, build_id: BuildID) -> DBBuildEntry:
        """Obtain a specific build entry from the database."""
        async with self._lock:
            if build_id < 1 or build_id > self._root.last_build_id:
                raise NoSuchBuildError(build_id)

            db_entry_raw: bytes | None = None
            try:
                with dbm.open(self._db_path, flag="c") as db:
                    key = f"build_{build_id}"
                    db_entry_raw = db.get(key)
            except Exception as e:
                msg = f"failed to get build entry {build_id} from db: {e}"
                logger.error(msg)
                raise BuildsDBError(msg) from e

        if not db_entry_raw:
            raise NoSuchBuildError(build_id)

        try:
            db_entry = DBBuildEntry.model_validate_json(db_entry_raw)
        except pydantic.ValidationError as e:
            logger.error(f"malformed build entry {build_id} in db:\n{e}")
            raise MalformedBuildEntryError(build_id) from e

        return db_entry
