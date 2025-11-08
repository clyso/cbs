# CES library - version descriptor creation
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

import re
from pathlib import Path

from cbscore.core.component import CoreComponentLoc, load_components
from cbscore.errors import MalformedVersionError
from cbscore.versions import logger as parent_logger
from cbscore.versions.desc import (
    VersionComponent,
    VersionDescriptor,
    VersionImage,
    VersionSignedOffBy,
)
from cbscore.versions.errors import VersionError
from cbscore.versions.utils import (
    VersionType,
    get_version_type,
    get_version_type_desc,
    parse_version,
)

logger = parent_logger.getChild("create")


def _validate_version(v: str) -> bool:
    try:
        _, _, minor, patch, _ = parse_version(v)
    except MalformedVersionError:
        return False
    return minor is not None and patch is not None


def _parse_component_refs(components: list[str]) -> dict[str, str]:
    comps: dict[str, str] = {}

    for c in components:
        m = re.match(r"^([\w_-]+)@([\d\w_./-]+)$", c)
        if not m:
            msg = f"malformed component name/version pair '{c}'"
            logger.error(msg)
            raise VersionError(msg)
        comps[m.group(1)] = m.group(2)

    return comps


def _do_version_title(version: str, version_type: VersionType) -> str:
    try:
        prefix, major, minor, patch, suffix = parse_version(version)
    except MalformedVersionError:
        msg = f"malformed version '{version}'"
        logger.error(msg)
        raise VersionError(msg) from None

    if not minor or not patch:
        msg = f"malformed version '{version}'"
        logger.error(msg)
        raise VersionError(msg)

    version_title = f"{prefix.upper()} " if prefix else ""
    version_title += f"version {major}.{minor}.{patch}"

    if suffix:
        parts = suffix.split("-")
        parts_str = ""
        for p in parts:
            p_entries = p.split(".", maxsplit=1)
            if parts_str:
                parts_str += ", "
            parts_str += (
                f"{p_entries[0].upper()} {p_entries[1]}"
                if len(p_entries) > 1
                else f"{p_entries[0].upper()}"
            )
        if parts_str:
            version_title += f" ({parts_str})"

    return f"Release {get_version_type_desc(version_type)} {version_title}"


def create(
    version: str,
    version_type: VersionType,
    component_refs: list[str],
    components: dict[str, CoreComponentLoc],
    component_uri_overrides: dict[str, str],
    distro: str,
    el_version: int,
    registry: str | None,
    image_name: str,
    image_tag: str | None,
    user_name: str,
    user_email: str,
) -> VersionDescriptor:
    if not _validate_version(version):
        msg = f"malformed version '{version}'"
        logger.error(msg)
        raise VersionError(msg)

    component_refs_map = _parse_component_refs(component_refs)
    if len(component_refs_map) == 0:
        msg = "missing valid components"
        logger.error(msg)
        raise VersionError(msg)

    components_uris: dict[str, str] = {}

    for c in component_refs_map:
        if c not in components:
            msg = f"unknown component '{c}' specified"
            logger.error(msg)
            raise VersionError(msg)

        components_uris[c] = components[c].comp.repo
        if c in component_uri_overrides:
            components_uris[c] = component_uri_overrides[c]

    version_title = _do_version_title(version, version_type)
    logger.debug(f"version title: {version_title}")

    component_res: list[VersionComponent] = []
    for comp_name, comp_ref in component_refs_map.items():
        assert comp_name in components_uris
        component_res.append(
            VersionComponent(
                name=comp_name, repo=components_uris[comp_name], ref=comp_ref
            )
        )

    image_tag_str = image_tag if image_tag else version

    return VersionDescriptor(
        version=version,
        title=version_title,
        signed_off_by=VersionSignedOffBy(
            user=user_name,
            email=user_email,
        ),
        image=VersionImage(
            registry=registry,
            name=image_name,
            tag=image_tag_str,
        ),
        components=component_res,
        distro=distro,
        el_version=el_version,
    )


def version_create_helper(
    version: str,
    version_type_name: str,
    component_refs: list[str],
    components_paths: list[Path],
    component_uri_overrides: list[str],
    distro: str,
    el_version: int,
    registry: str | None,
    image_name: str,
    image_tag: str | None,
    user_name: str,
    user_email: str,
) -> VersionDescriptor:
    if not components_paths:
        our_comps_path = Path.cwd() / "components"
        if not our_comps_path.exists():
            msg = "no components paths provided, nor could any be found!"
            logger.error(msg)
            raise VersionError(msg)
        components_paths.append(our_comps_path)

    comps: dict[str, CoreComponentLoc] = load_components(components_paths)
    if not comps:
        msg = "no components provided, nor could they be found!"
        logger.error(msg)
        raise VersionError(msg)

    uri_overrides: dict[str, str] = {}
    for override in component_uri_overrides:
        entries = override.split("=", 1)
        if len(entries) != 2:
            msg = f"malformed URI override '{override}'"
            logger.error(msg)
            raise VersionError(msg)
        comp, uri = entries
        uri_overrides[comp] = uri
        logger.debug(f"overriding component '{comp}' URI with '{uri}'")

    try:
        version_type = get_version_type(version_type_name)
    except VersionError as e:
        msg = f"error validating version type: {e}"
        logger.error(msg)
        raise VersionError(msg) from e

    try:
        return create(
            version,
            version_type,
            component_refs,
            comps,
            uri_overrides,
            distro,
            el_version,
            registry,
            image_name,
            image_tag,
            user_name,
            user_email,
        )
    except VersionError as e:
        msg = f"error creating version descriptor: {e}"
        logger.error(msg)
        raise VersionError(msg) from e
