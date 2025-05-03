# CES library - CES container images, repositories
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

# pyright: reportUnsafeMultipleInheritance=false

import abc
import contextlib
import os
import re
import tempfile
from collections.abc import AsyncGenerator
from pathlib import Path
from typing import Any, cast, override

import aiohttp
import pydantic
from ceslib.containers import ContainerError, find_path_relative_to
from ceslib.containers import log as parent_logger
from ceslib.utils.buildah import BuildahContainer, BuildahError

log = parent_logger.getChild("repos")


def repo_discriminator(v: dict[str, Any]) -> str:  # pyright: ignore[reportExplicitAny]
    source: str = cast(str, v["source"])
    if not source:
        raise pydantic.ValidationError()

    if source.startswith("file://"):
        return "file"
    elif source.startswith("http://") or source.startswith("https://"):
        return "url"
    elif source.startswith("copr://"):
        return "copr"
    else:
        raise pydantic.ValidationError()


class ContainerRepository(pydantic.BaseModel, abc.ABC):
    name: str
    source: str

    @abc.abstractmethod
    async def install(
        self, container: BuildahContainer, hint: Path, root: Path
    ) -> None:
        pass

    @abc.abstractmethod
    @contextlib.asynccontextmanager
    async def get_source_path(
        self, hint: Path, root: Path
    ) -> AsyncGenerator[Path | None]:
        yield

    async def _install_path(
        self, container: BuildahContainer, hint: Path, root: Path, dst: str
    ) -> None:
        async with self.get_source_path(hint, root) as src:
            if not src:
                msg = f"unexpected missing source path for '{self.source}'"
                log.error(msg)
                raise ContainerError(msg)

            try:
                await container.copy(src, dst)
            except (BuildahError, Exception) as e:
                msg = f"unable to copy repository from '{src}' to '{dst}': {e}"
                log.exception(msg)
                raise ContainerError(msg) from e


class ContainerFileRepository(ContainerRepository):
    dest: str

    @override
    async def install(
        self, container: BuildahContainer, hint: Path, root: Path
    ) -> None:
        await self._install_path(container, hint, root, self.dest)

    @override
    @contextlib.asynccontextmanager
    async def get_source_path(
        self, hint: Path, root: Path
    ) -> AsyncGenerator[Path | None]:
        m = re.match(r"^file://(.+)", self.source)
        if not m:
            msg = f"empty 'file://' or wrong source type for '{self.source}'"
            log.error(msg)
            raise ContainerError(msg)

        try:
            _ = hint.relative_to(root)
        except ValueError:
            msg = f"hint path '{hint}' not relative to root '{root}'"
            log.exception(msg)
            raise ContainerError(msg) from None

        name: str = m.group(1)
        p = find_path_relative_to(name, hint, root)
        if not p:
            msg = f"error finding '{name}' between '{root}' and '{hint}'"
            log.error(msg)
            raise ContainerError(msg)

        yield p


class ContainerURLRepository(ContainerRepository):
    dest: str

    @override
    async def install(
        self, container: BuildahContainer, hint: Path, root: Path
    ) -> None:
        await self._install_path(container, hint, root, self.dest)

    @override
    @contextlib.asynccontextmanager
    async def get_source_path(
        self, hint: Path, root: Path
    ) -> AsyncGenerator[Path | None]:
        if not (re.match(r"^https?://.+", self.source)):
            msg = f"wrong source for URL: '{self.source}'"
            log.error(msg)
            raise ContainerError(msg)

        tmp_fd, tmp_source = tempfile.mkstemp()
        tmp_source_path = Path(tmp_source)
        try:
            async with (
                aiohttp.ClientSession() as session,
                session.get(self.source) as response,
            ):
                data = await response.read()
                with tmp_source_path.open("wb") as f:
                    _ = f.write(data)

            yield tmp_source_path

        finally:
            os.close(tmp_fd)
            os.unlink(tmp_source)


class ContainerCOPRRepository(ContainerRepository):
    @override
    async def install(
        self, container: BuildahContainer, hint: Path, root: Path
    ) -> None:
        m = re.match(r"^copr://(.+)", self.source)
        if not m:
            msg = f"empty 'copr://' or wrong source type for '{self.source}'"
            log.error(msg)
            raise ContainerError(msg)

        copr_source: str = m.group(1)
        cmd = ["dnf", "copr", "enable", "-y", copr_source]

        try:
            await container.run(cmd)
        except (BuildahError, Exception) as e:
            msg = f"error enabling COPR repository '{copr_source}': {e}"
            log.exception(msg)
            raise ContainerError(msg) from e

    @override
    @contextlib.asynccontextmanager
    async def get_source_path(
        self, hint: Path, root: Path
    ) -> AsyncGenerator[Path | None]:
        yield None
