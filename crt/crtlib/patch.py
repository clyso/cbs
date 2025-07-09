# crt - patch utilities
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
from datetime import datetime as dt
from pathlib import Path
from typing import cast

import pydantic
from crtlib.errors import CRTError
from crtlib.git_utils import SHA, GitError, git_format_patch, git_patch_id
from crtlib.logger import logger as parent_logger
from crtlib.models.common import AuthorData

logger = parent_logger.getChild("patch")


class PatchError(CRTError):
    pass


class MalformedPatchBodyError(PatchError):
    pass


class PatchExistsError(PatchError):
    pass


class PatchInfo(pydantic.BaseModel):
    author: AuthorData
    date: dt
    title: str
    desc: str
    signed_off_by: list[AuthorData]
    cherry_picked_from: list[str]
    fixes: list[str]

    @property
    def canonical_title(self) -> str:
        r1 = re.compile(r"[\s:/\]\[\(\)]")
        r2 = re.compile(r"['\",.+\<>~^$@!?%&=;`]")
        return r2.sub(r"", r1.sub(r"-", self.title.lower()))


class PatchMeta(pydantic.BaseModel):
    sha: SHA
    patch_id: SHA
    src_version: str | None
    info: PatchInfo


def _split_version_into_paths(version: str) -> list[Path]:
    if not version:
        raise PatchError(msg="missing version")

    def _parse_version_hierarchy() -> list[str]:
        v_re = re.compile(
            r"""
            ^(?P<prefix>.*?)(?:-)?  # optional prefix plus dash
            v
            (?P<major>\d{2})        # major version (required)
            (?:\.(?P<minor>\d{2}))? # minor version (optional)
            (?:\.(?P<patch>\d+))?   # patch version (optional)
            (?:-(?P<suffix>.+))?    # dash with suffix (optional)
            $
            """,
            re.VERBOSE,
        )
        m = v_re.match(version)
        if not m:
            raise ValueError(f"invalid version '{version}'")  # noqa: TRY003

        prefix = cast(str | None, m.group("prefix"))
        major = cast(str, m.group("major"))
        minor = cast(str | None, m.group("minor"))
        patch = cast(str | None, m.group("patch"))
        suffix = cast(str | None, m.group("suffix"))

        levels: list[str] = []
        base = f"v{major}"
        levels.append(base)
        if minor:
            base = f"{base}.{minor}"
            levels.append(base)
        if patch:
            base = f"{base}.{patch}"
            levels.append(base)

        if prefix:
            levels = [f"{prefix}-{lvl}" for lvl in levels]
        if suffix:
            last = next(reversed(levels))
            levels.append(f"{last}-{suffix}")

        return levels

    paths_lst: list[Path] = []
    for p in _parse_version_hierarchy():
        last_elem_path = next(reversed(paths_lst)) if paths_lst else Path()
        part_path = last_elem_path.joinpath(p)
        paths_lst.append(part_path)

    return paths_lst


def parse_formatted_patch_info(patch: str) -> PatchInfo:
    """
    Parse a from a formatted patch from the Ceph repository.

    Assumes a certain format is in place for the commit's message body, according to
    the Ceph upstream's commit guidelines.
    """
    idx = patch.find("---")
    patch = patch if idx < 0 else patch[:idx]

    desc_lst: list[str] = []
    signed_offs_lst: list[AuthorData] = []
    cherry_picks_lst: list[str] = []
    fixes_lst: list[str] = []

    sign_off_re = re.compile(r"[sS]igned-[oO]ff-[bB]y:\s+(.*)\s+<(.*)>")
    cherry_picked_re = re.compile(r"\(cherry picked from commit ([\w\d]+)\)")
    fixes_re = re.compile(r"[fF]ixes: (.*)|[rR]esolves: (.*)")

    lines = iter(patch.splitlines())

    # first line is the 'From' bit, ignore.
    _ = next(lines)

    # second line is the author
    author_m = re.match(r"^From:\s+(.*)\s+<(.*)>$", next(lines))
    if not author_m:
        raise MalformedPatchBodyError(msg="malformed author")
    patch_author = AuthorData(
        user=cast(str, author_m.group(1)), email=cast(str, author_m.group(2))
    )
    logger.debug(f"patch_author = {patch_author}")

    # third line is the patch's date
    date_m = re.match(r"^Date:\s+(.*)$", next(lines))
    if not date_m:
        raise MalformedPatchBodyError(msg="malformed date")

    try:
        patch_date = dt.strptime(cast(str, date_m.group(1)), "%a, %d %b %Y %H:%M:%S %z")
    except Exception as e:
        raise MalformedPatchBodyError(msg=f"malformed date: {e}") from None
    logger.debug(f"patch date: {patch_date}")

    # forth line, and up until a blank line, is the subject.
    subject_lines: list[str] = []
    while True:
        subj_line = next(lines)
        if not subj_line:
            break
        elif m := re.match(r"^Subject:\s+\[PATCH\]\s+(.*)$", subj_line):
            subject_lines.append(m.group(1))
        else:
            subject_lines.append(subj_line)

    subject = "".join(subject_lines)
    logger.debug(f"subject: {subject}")

    end_of_desc = False
    for line in lines:
        if m := re.match(sign_off_re, line):
            signed_offs_lst.append(AuthorData(user=m.group(1), email=m.group(2)))
            end_of_desc = True
        elif m := re.match(cherry_picked_re, line):
            cherry_picks_lst.append(m.group(1))
            end_of_desc = True
        elif m := re.match(fixes_re, line):
            fixes_lst.append(m.group(1))
            end_of_desc = True
        elif not end_of_desc:
            desc_lst.append(line)

    # remove any leading empty lines from the description list
    while len(desc_lst) > 0:
        if not len(desc_lst[0]):
            _ = desc_lst.pop(0)
        else:
            break

    # remove newlines from end of description
    desc = "".join(desc_lst).strip()

    return PatchInfo(
        author=patch_author,
        date=patch_date,
        title=subject,
        desc=desc,
        signed_off_by=signed_offs_lst,
        cherry_picked_from=cherry_picks_lst,
        fixes=fixes_lst,
    )


def patch_import(
    patches_path: Path,
    repo_path: Path,
    sha: SHA,
    *,
    src_version: str | None = None,
    target_version: str | None = None,
) -> None:
    try:
        patch_id = git_patch_id(repo_path, sha)
    except GitError as e:
        msg = f"unable to obtain patch id for sha '{sha}': {e}"
        logger.error(msg)
        raise PatchError(msg=msg) from None

    patch_meta_path = patches_path.joinpath("meta").joinpath(f"{sha}.json")
    if patch_meta_path.exists():
        msg = f"patch sha '{sha}' already imported"
        logger.warning(msg)
        raise PatchExistsError(msg=msg)

    try:
        formatted_patch = git_format_patch(repo_path, sha)
    except GitError as e:
        msg = f"unable to obtain formatted patch for sha '{sha}': {e}"
        logger.error(msg)
        raise PatchError(msg=msg) from None

    try:
        patch_info = parse_formatted_patch_info(formatted_patch)
    except PatchError as e:
        logger.error(f"unable to parse formatted patch info: {e}")
        raise e from None

    patch_path = patches_path.joinpath("patches").joinpath(f"{sha}.patch")
    patch_meta_path.parent.mkdir(parents=True, exist_ok=True)
    patch_path.parent.mkdir(parents=True, exist_ok=True)

    patch_meta = PatchMeta(
        sha=sha,
        patch_id=patch_id,
        src_version=src_version,
        info=patch_info,
    )

    try:
        _ = patch_path.write_text(formatted_patch)
        _ = patch_meta_path.write_text(patch_meta.model_dump_json(indent=2))
    except Exception as e:
        msg = f"unable to write imported patch: {e}"
        logger.error(msg)
        raise PatchError(msg=msg) from None

    if target_version:
        target_paths = _split_version_into_paths(target_version)
        if not target_paths:
            msg = f"unable to get destination path for '{target_version}'"
            logger.error(msg)
            raise PatchError(msg=msg)

        target_path = patches_path.joinpath("ces").joinpath(
            next(reversed(target_paths))
        )
        target_path.mkdir(parents=True, exist_ok=True)

        existing_patches_it = target_path.glob("*.patch")
        patch_n = 0
        existing_patch_re = re.compile(r"^(\d+)-.*\.patch$")
        for p in existing_patches_it:
            if m := re.match(existing_patch_re, p.name):
                p_n = int(cast(str, m.group(1)))
                if p_n > patch_n:
                    patch_n = p_n

        next_patch_n = patch_n + 1
        target_patch_name = f"{next_patch_n:04d}-{patch_info.canonical_title}.patch"
        target_patch_lnk = target_path.joinpath(target_patch_name)
        relative_to_root_path = patches_path.relative_to(target_path, walk_up=True)
        logger.debug(f"relative_to_root_path: {relative_to_root_path}")

        patch_path_relative_to_root = patch_path.relative_to(patches_path)
        logger.debug(f"patch path relative to root: {patch_path_relative_to_root}")
        relative_patch_path = relative_to_root_path.joinpath(
            patch_path_relative_to_root
        )

        target_patch_lnk.symlink_to(relative_patch_path)
        logger.info(
            f"linked patch '{sha}' to version '{target_version}' "
            + f"patch '{target_patch_name}'"
        )
