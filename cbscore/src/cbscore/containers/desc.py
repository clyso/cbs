# CES library - CES container images, descriptor
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
#
# NOTE: pydantic makes basedpyright complain about 'Any' when using Field
# defaults. Disable temporarily.
#
# pyright: reportAny=false, reportUnknownArgumentType=false

from __future__ import annotations

from pathlib import Path
from typing import Annotated, Any

import pydantic
import yaml

from cbscore.containers import ContainerError, repos
from cbscore.containers import logger as parent_logger

logger = parent_logger.getChild("descriptor")


class ContainerScript(pydantic.BaseModel):
    name: str
    run: str


class ContainerPre(pydantic.BaseModel):
    keys: list[str] = pydantic.Field(default=[])
    packages: list[str] = pydantic.Field(default=[])
    repos: Annotated[
        list[
            Annotated[
                Annotated[repos.ContainerFileRepository, pydantic.Tag("file")]
                | Annotated[repos.ContainerURLRepository, pydantic.Tag("url")]
                | Annotated[repos.ContainerCOPRRepository, pydantic.Tag("copr")],
                pydantic.Discriminator(repos.repo_discriminator),
            ]
        ]
        | None,
        pydantic.Field(default=[]),
        pydantic.Field(default=[]),
    ]
    scripts: list[ContainerScript] = pydantic.Field(default=[])


class ContainerPackagesEntry(pydantic.BaseModel):
    section: str
    packages: list[str]
    cond: str | None = pydantic.Field(default=None)


class ContainerPackages(pydantic.BaseModel):
    required: list[ContainerPackagesEntry] = pydantic.Field(default=[])
    optional: list[ContainerPackagesEntry] = pydantic.Field(default=[])


class ContainerConfig(pydantic.BaseModel):
    env: dict[str, str] = pydantic.Field(default={})
    labels: dict[str, str] = pydantic.Field(default={})
    annotations: dict[str, str] = pydantic.Field(default={})


class ContainerDescriptor(pydantic.BaseModel):
    config: ContainerConfig | None = pydantic.Field(default=None)
    pre: ContainerPre
    packages: ContainerPackages
    post: list[ContainerScript] = pydantic.Field(default=[])

    @classmethod
    def load(
        cls,
        path: Path,
        *,
        vars: dict[str, Any] | None = None,  ## pyright: ignore[reportExplicitAny]
    ) -> ContainerDescriptor:
        try:
            with path.open("r") as f:
                to_load_str = f.read()
                to_load = to_load_str.format(**vars) if vars else to_load_str
                yaml_dict = yaml.safe_load(to_load)

            return ContainerDescriptor.model_validate(yaml_dict)

        except (yaml.YAMLError, pydantic.ValidationError) as e:
            msg = f"error loading container descriptor at '{path}': {e}"
            logger.exception(msg)
            raise ContainerError(msg) from None
        except Exception as e:
            msg = f"unknown error loading descriptor at '{path}': {e}"
            logger.exception(msg)
            raise ContainerError(msg) from e
