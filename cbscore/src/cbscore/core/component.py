# CBS library - core - component
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

from pathlib import Path

import pydantic
import yaml

from cbscore.core import logger as parent_logger
from cbscore.errors import CESError

logger = parent_logger.getChild("component")


class CoreComponentContainersSection(pydantic.BaseModel):
    path: Path


class CoreComponentBuildRPMSection(pydantic.BaseModel):
    build: str
    release_rpm: str = pydantic.Field(alias="release-rpm")


class CoreComponentBuildSection(pydantic.BaseModel):
    rpm: CoreComponentBuildRPMSection | None
    get_version: str = pydantic.Field(alias="get-version")
    deps: str


class CoreComponent(pydantic.BaseModel):
    name: str
    repo: str

    build: CoreComponentBuildSection
    containers: CoreComponentContainersSection

    @classmethod
    def load(cls, path: Path) -> CoreComponent:
        if not path.exists() or not path.is_file():
            raise FileNotFoundError(f"Component file '{path}' not found")

        try:
            yaml_dict = yaml.safe_load(path.read_text())  # pyright: ignore[reportAny]
            return CoreComponent.model_validate(yaml_dict)
        except (yaml.YAMLError, pydantic.ValidationError) as e:
            msg = f"error loading core component at '{path}': {e}"
            logger.exception(msg)
            raise CESError(msg) from e
        except Exception as e:
            msg = f"unexpected error loading core component at '{path}': {e}"
            logger.exception(msg)
            raise CESError(msg) from e


class CoreComponentLoc(pydantic.BaseModel):
    path: Path
    comp: CoreComponent


def load_components(paths: list[Path]) -> dict[str, CoreComponentLoc]:
    components: dict[str, CoreComponentLoc] = {}

    def _do_path(path: Path) -> None:
        for entry in path.iterdir():
            if not entry.is_dir():
                continue

            comp_file = entry / "cbs.component.yaml"
            if not comp_file.exists() or not comp_file.is_file():
                logger.warning(f"skipping '{entry}': no cbs.component.yaml found")
                continue

            try:
                comp = CoreComponent.load(comp_file)
                logger.debug(f"loaded component '{comp.name}' from '{comp_file}'")
                components[comp.name] = CoreComponentLoc(path=entry, comp=comp)
            except CESError as e:
                logger.error(f"skipping '{entry}': {e}")
                continue

    for path in paths:
        _do_path(path)

    return components
