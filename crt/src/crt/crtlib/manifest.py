# crt - release manifests
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


import datetime
import re
import uuid
from datetime import datetime as dt
from pathlib import Path
from typing import cast

import pydantic
from cbscore.versions.utils import parse_version

from crt.crtlib.apply import ApplyError, apply_manifest
from crt.crtlib.errors import CRTError
from crt.crtlib.errors.manifest import (
    MalformedManifestError,
    ManifestError,
    ManifestExistsError,
    NoSuchManifestError,
)
from crt.crtlib.errors.stages import MissingStagePatchError
from crt.crtlib.git_utils import (
    GitError,
    GitFetchError,
    GitFetchHeadNotFoundError,
    GitIsTagError,
    GitPushError,
    git_checkout_ref,
    git_cleanup_repo,
    git_fetch_ref,
    git_prepare_remote,
    git_push,
    git_status,
)
from crt.crtlib.logger import logger as parent_logger
from crt.crtlib.models.common import ManifestPatchEntry
from crt.crtlib.models.manifest import ReleaseManifest
from crt.crtlib.models.patch import Patch, PatchMeta
from crt.crtlib.models.patchset import GitHubPullRequest
from crt.crtlib.utils import split_version_into_paths

logger = parent_logger.getChild("manifest")


class ManifestExecuteResult(pydantic.BaseModel):
    target_branch: str
    applied: bool
    added: list[Patch]
    skipped: list[Patch]


class ManifestPublishResult(pydantic.BaseModel):
    remote_updated: bool
    heads_updated: list[str]
    heads_rejected: list[str]


def _prepare_repo(
    repo_path: Path,
    manifest_uuid: uuid.UUID,
    base_ref: str,
    target_branch: str,
    base_remote_name: str,
    push_remote_name: str,
    token: str,
) -> None:
    try:
        git_cleanup_repo(repo_path)
    except GitError as e:
        msg = f"unable to clean up repository: {e}"
        logger.error(msg)
        raise ManifestError(uuid=manifest_uuid, msg=msg) from None

    try:
        base_remote_uri = f"github.com/{base_remote_name}"
        _ = git_prepare_remote(repo_path, base_remote_uri, base_remote_name, token)
        push_remote_uri = f"github.com/{push_remote_name}"
        _ = git_prepare_remote(repo_path, push_remote_uri, push_remote_name, token)
    except GitError as e:
        raise ManifestError(uuid=manifest_uuid, msg=str(e)) from None

    # fetch from base repository, if it exists.
    try:
        _ = git_fetch_ref(repo_path, target_branch, target_branch, push_remote_name)
    except GitIsTagError as e:
        msg = f"unexpected tag for branch '{target_branch}': {e}"
        logger.error(msg)
        raise ManifestError(uuid=manifest_uuid, msg=msg) from None
    except GitFetchHeadNotFoundError:
        # does not exist in the provided remote.
        logger.debug(
            f"branch '{target_branch}' does not exist in remote '{push_remote_name}'"
        )
    except GitFetchError as e:
        msg = f"unable to fetch '{target_branch}' from '{push_remote_name}': {e}"
        logger.error(msg)
        raise ManifestError(uuid=manifest_uuid, msg=msg) from None
    except GitError as e:
        msg = (
            f"unexpected error fetching branch '{target_branch}' "
            + f"from '{push_remote_name}': {e}"
        )
        logger.error(msg)
        raise ManifestError(uuid=manifest_uuid, msg=msg) from None

    # we either fetched and thus we have an up-to-date local branch, or we didn't find
    # a corresponding reference in the remote and we need to either:
    #  1. checkout a new copy of the base ref to the target branch
    #  2. use an existing local target branch
    try:
        _ = git_checkout_ref(
            repo_path,
            base_ref,
            to_branch=target_branch,
            remote_name=base_remote_name,
            update_from_remote=False,
            fetch_if_not_exists=True,
        )
        git_cleanup_repo(repo_path)

        logger.debug(f"git status:\n{git_status(repo_path)}")
    except GitError as e:
        msg = f"unable to checkout ref '{base_ref}' to '{target_branch}': {e}"
        logger.error(msg)
        raise ManifestError(uuid=manifest_uuid, msg=msg) from None

    logger.debug(f"checked out '{target_branch}'")

    pass


def manifest_execute(
    manifest: ReleaseManifest,
    ceph_repo_path: Path,
    patches_repo_path: Path,
    token: str,
    *,
    no_cleanup: bool = True,
) -> ManifestExecuteResult:
    """
    Execute a manifest against its base ref.

    If the target branch for this manifest exists locally, attempt to fetch changes
    from the base repository (if it exists). Then execute the manifest against the
    target branch.

    If the target branch for this manifest exists in the manifest's base repository,
    checkout said branch and execute the manifest against it.

    If the target branch doesn't exist at all, checkout the branch from the manifest's
    base ref and execute the manifest against it.

    Patches will be applied from the patches repository, `patches_repo_path`.
    """
    base_remote_name = f"{manifest.base_ref_org}/{manifest.base_ref_repo}"
    logger.info(
        f"execute manifest '{manifest.release_uuid}' for repo '{base_remote_name}'"
    )

    ts = dt.now(datetime.UTC).strftime("%Y%m%dT%H%M%S")
    seq = f"exec-{ts}"
    target_branch = f"{manifest.name}-{seq}"
    logger.debug(f"execute manifest on branch '{target_branch}'")

    try:
        _prepare_repo(
            ceph_repo_path,
            manifest.release_uuid,
            manifest.base_ref,
            target_branch,
            base_remote_name,
            manifest.dst_repo,
            token,
        )
    except ManifestError as e:
        logger.error(f"unable to prepare repository to execute manifest: {e}")
        raise e from None

    # apply manifest to currently checked out branch
    try:
        res, added, skipped = apply_manifest(
            manifest,
            ceph_repo_path,
            patches_repo_path,
            target_branch,
            token,
            no_cleanup=no_cleanup,
        )
        pass
    except ApplyError as e:
        msg = f"unable to apply manifest to '{target_branch}': {e}"
        logger.error(msg)
        raise ManifestError(uuid=manifest.release_uuid, msg=msg) from None

    logger.debug(
        f"applied manifest: {res}, added '{len(added)}' "
        + f"skipped '{len(skipped)}' patches"
    )
    return ManifestExecuteResult(
        applied=res,
        target_branch=target_branch,
        added=[],
        skipped=[],
    )


def manifest_publish_stages(
    patches_repo_path: Path,
    manifest: ReleaseManifest,
) -> int:
    """
    Publish all patch sets for each stage in the provided manifest.

    Creates symlinks for the corresponding version, each pointing to the original
    patch set.
    """
    version_paths = split_version_into_paths(manifest.name)
    if not version_paths:
        raise ManifestError(
            uuid=manifest.release_uuid, name=manifest.name, msg="invalid manifest name"
        )
    end_path = next(iter(reversed(version_paths)))

    try:
        version_prefix, _, _, _, _ = parse_version(manifest.name)
    except ValueError:
        msg = f"invalid version in manifest name '{manifest.name}'"
        logger.error(msg)
        raise ManifestError(
            uuid=manifest.release_uuid, name=manifest.name, msg=msg
        ) from None

    if not version_prefix:
        version_prefix = "vanilla"

    version_path = patches_repo_path / version_prefix / end_path

    if version_path.exists():
        if not version_path.is_dir():
            msg = f"patches path '{version_path}' exists and is not a directory"
            logger.error(msg)
            raise ManifestExistsError(
                uuid=manifest.release_uuid, name=manifest.name, msg=msg
            )

        if list(version_path.glob("*.patch")):
            msg = f"patches exist at '{version_path}' for release '{manifest.name}'"
            logger.error(msg)
            raise ManifestError(uuid=manifest.release_uuid, name=manifest.name, msg=msg)

    version_path.mkdir(parents=True, exist_ok=True)

    patch_n = 0
    for stage in manifest.stages:
        for p in stage.patches:
            patch = p.contents

            patch_path = (
                patches_repo_path.joinpath("ceph")
                .joinpath("patches")
                .joinpath(f"{patch.entry_uuid}.patch")
            )
            if not patch_path.exists():
                msg = f"missing patch for uuid '{patch.entry_uuid}'"
                logger.error(msg)
                raise MissingStagePatchError(msg=msg)

            patch_n = patch_n + 1
            target_patch_name = f"{patch_n:04d}-{patch.canonical_title}.patch"
            target_patch_lnk = version_path.joinpath(target_patch_name)

            relative_to_root_path = patches_repo_path.relative_to(
                version_path, walk_up=True
            )
            patch_path_relative_to_root = patch_path.relative_to(patches_repo_path)
            relative_patch_path = relative_to_root_path.joinpath(
                patch_path_relative_to_root
            )

            logger.debug(f"symlink '{target_patch_lnk}' to '{relative_patch_path}'")
            target_patch_lnk.symlink_to(relative_patch_path)

        stage.is_published = True

    store_manifest(patches_repo_path, manifest)

    return patch_n


def manifest_publish_branch(
    manifest: ReleaseManifest,
    repo_path: Path,
    our_branch: str,
    dst_branch: str,
) -> ManifestPublishResult:
    """
    Publish a manifest's local branch to its remote repository.

    The local branch to be published / pushed to the remote repository is provided by
    `our_branch`, while the destination branch is automatically crafted from the
    manifest's name and its `release_git_uid`.

    Will return `ManifestPublishResult`, containing information on whether the remote
    repository was updated, and which heads were updated or rejected.
    """
    dst_repo = manifest.dst_repo
    logger.info(
        f"publish manifest branch '{our_branch}' to "
        + f"repo '{dst_repo}' branch '{dst_branch}"
    )

    heads_updated: list[str] = []
    heads_rejected: list[str] = []
    logger.info(f"push '{our_branch}' to '{dst_repo}'")
    try:
        push_res, heads_updated, heads_rejected = git_push(
            repo_path,
            our_branch,
            dst_repo,
            branch_to=dst_branch,
        )
    except GitPushError as e:
        msg = f"unable to push '{our_branch}': {e}"
        logger.error(msg)
        raise ManifestError(uuid=manifest.release_uuid, msg=msg) from None
    except GitError as e:
        msg = f"unexpected error pushing '{our_branch}': {e}"
        logger.error(msg)
        raise ManifestError(uuid=manifest.release_uuid, msg=msg) from None

    if not push_res:
        logger.info(f"branch '{dst_branch}' not updated on remote '{dst_repo}'")

    logger.debug(f"updated heads: {heads_updated}")
    logger.debug(f"rejected heads: {heads_rejected}")

    return ManifestPublishResult(
        remote_updated=push_res,
        heads_updated=heads_updated,
        heads_rejected=heads_rejected,
    )


def manifest_exists(
    patches_repo_path: Path,
    *,
    manifest_uuid: uuid.UUID | None = None,
    manifest_name: str | None = None,
) -> bool:
    if not manifest_uuid and not manifest_name:
        raise CRTError("either uuid or name must be provided")

    base_path = patches_repo_path.joinpath("ceph").joinpath("manifests")
    if manifest_uuid:
        return base_path.joinpath(f"{manifest_uuid}.json").exists()
    else:
        return base_path.joinpath(f"{manifest_name}.json").exists()


def remove_manifest(
    patches_repo_path: Path,
    *,
    manifest_uuid: uuid.UUID | None = None,
    manifest_name: str | None = None,
) -> tuple[uuid.UUID, str]:
    if not manifest_uuid and not manifest_name:
        raise CRTError("either uuid or name must be provided")

    base_path = patches_repo_path.joinpath("ceph").joinpath("manifests")
    manifest_uuid_path: Path | None = None
    manifest_name_path: Path | None = None

    if manifest_name:
        try:
            manifest = load_manifest_by_name(patches_repo_path, manifest_name)
        except Exception as e:
            raise e from None
    else:
        assert manifest_uuid
        try:
            manifest = load_manifest(patches_repo_path, manifest_uuid)
        except Exception as e:
            raise e from None

    manifest_name_path = base_path / "by_name" / f"{manifest.name}.json"
    if manifest_name_path.exists():
        manifest_name_path.unlink()

    manifest_uuid_path = base_path.joinpath(f"{manifest.release_uuid}.json")
    manifest_uuid_path.unlink()

    return (manifest.release_uuid, manifest.name)


def load_manifest(patches_repo_path: Path, manifest_uuid: uuid.UUID) -> ReleaseManifest:
    logger.debug(f"load manifest uuid '{manifest_uuid}'")
    manifest_path = (
        patches_repo_path.joinpath("ceph")
        .joinpath("manifests")
        .joinpath(f"{manifest_uuid}.json")
    )
    if not manifest_path.exists():
        logger.error(f"manifest uuid '{manifest_uuid}' does not exist")
        raise NoSuchManifestError(uuid=manifest_uuid)

    try:
        return ReleaseManifest.model_validate_json(manifest_path.read_text())
    except pydantic.ValidationError as e:
        logger.error(f"malformed manifest uuid '{manifest_uuid}'")
        logger.debug(e)
        raise MalformedManifestError(uuid=manifest_uuid) from None


def load_manifest_by_name(patches_repo_path: Path, name: str) -> ReleaseManifest:
    logger.debug(f"load manifest by name '{name}'")
    manifest_path = (
        patches_repo_path.joinpath("ceph")
        .joinpath("manifests")
        .joinpath("by_name")
        .joinpath(f"{name}.json")
    )
    if not manifest_path.exists():
        logger.error(f"manifest name '{name}' does not exist")
        raise NoSuchManifestError(name=name)

    try:
        return ReleaseManifest.model_validate_json(manifest_path.read_text())
    except pydantic.ValidationError:
        logger.error(f"malformed manifest name '{name}'")
        raise MalformedManifestError(name=name) from None


def load_manifest_by_name_or_uuid(
    patches_repo_path: Path, what: str
) -> ReleaseManifest:
    logger.debug(f"load manifest by name or uuid '{what}'")
    manifest_uuid: uuid.UUID | None = None
    manifest_name: str | None = None

    try:
        manifest_uuid = uuid.UUID(what)
    except Exception as e:
        logger.debug(str(e))
        manifest_name = what

    if manifest_uuid:
        return load_manifest(patches_repo_path, manifest_uuid)
    elif manifest_name:
        return load_manifest_by_name(patches_repo_path, manifest_name)
    else:
        raise CRTError("either uuid or name must be provided")


def store_manifest(patches_repo_path: Path, manifest: ReleaseManifest) -> None:
    logger.debug(f"store manifest uuid '{manifest.release_uuid}'")
    base_path = patches_repo_path.joinpath("ceph").joinpath("manifests")
    manifest_uuid_path = base_path.joinpath(f"{manifest.release_uuid}.json")
    manifest_name_path = base_path.joinpath("by_name").joinpath(f"{manifest.name}.json")
    base_path.mkdir(parents=True, exist_ok=True)

    if manifest_name_path.exists():
        if not manifest_name_path.is_symlink():
            msg = f"manifest name '{manifest.name}' exists and is not a symlink"
            logger.error(msg)
            raise CRTError(msg)
        elif manifest_name_path.resolve() != manifest_uuid_path:
            msg = f"manifest name '{manifest.name}' already in use by another manifest"
            logger.error(msg)
            raise ManifestExistsError(name=manifest.name, msg=msg)

    try:
        _ = manifest_uuid_path.write_text(manifest.model_dump_json(indent=2))
    except Exception as e:
        msg = (
            f"error writing manifest uuid '{manifest.release_uuid}' "
            + f"to '{manifest_uuid_path}': {e}"
        )
        logger.error(msg)
        raise ManifestError(uuid=manifest.release_uuid, msg=msg) from None

    if not manifest_name_path.exists():
        try:
            manifest_name_path.symlink_to(Path("..").joinpath(manifest_uuid_path.name))
        except Exception as e:
            msg = (
                f"unable to symlink manifest name '{manifest.name}' "
                + f"to uuid '{manifest.release_uuid}': {e}"
            )
            logger.error(msg)
            raise ManifestError(
                uuid=manifest.release_uuid, name=manifest.name, msg=msg
            ) from None


def list_manifests(patches_repo_path: Path) -> list[ReleaseManifest]:
    manifests_path = patches_repo_path.joinpath("ceph").joinpath("manifests")
    if not manifests_path.exists():
        return []

    manifests: list[ReleaseManifest] = []
    for entry in manifests_path.glob("*.json"):
        try:
            entry_uuid = uuid.UUID(entry.stem)
        except Exception:
            logger.warning(f"malformed manifest uuid '{entry.stem}', ignore")
            continue

        try:
            manifests.append(load_manifest(patches_repo_path, entry_uuid))
        except ManifestError as e:
            logger.error(f"error loading manifest uuid '{entry_uuid}', skip")
            logger.error(f"error: {e}")
            continue

    return sorted(manifests, key=lambda e: e.creation_date)


def manifest_release_notes(
    manifest: ReleaseManifest,
    *,
    image_loc: str | None = None,
    cephadm_loc: str | None = None,
) -> str:
    doc_lines: list[str] = []
    doc_links: list[tuple[str, str]] = []

    def _header(title: str, h: int) -> None:
        if doc_lines and doc_lines[-1].strip() != "":
            doc_lines.append("")

        doc_lines.append("#" * h + f" {title.strip()}")
        doc_lines.append("")

    def _paragraph(value: str) -> None:
        tokens = value.strip().split(" ")

        def _get_line(tkns: list[str]) -> str:
            acc = 0
            acc_str = ""
            it = iter(tkns)
            for t in it:
                if acc + len(t) + 1 > 79:
                    it = iter([t, *list(it)])
                    break
                acc += len(t) + 1
                acc_str += f" {t}"

            acc_str = acc_str.strip()

            if not acc_str:
                return ""
            return f"{acc_str}\n{_get_line(list(it))}"

        if len(tokens) > 0:
            if doc_lines and doc_lines[-1].strip() != "":
                doc_lines.append("")

            doc_lines.append(_get_line(tokens))

    def _block(contents: str, block_type: str = "text") -> None:
        content_lines = [ln.strip() for ln in contents.strip().split("\n")]
        doc_lines.append(f"```{block_type}")
        doc_lines.extend([ln for ln in content_lines if ln])
        doc_lines.append("```")

    def _add_link(ref: str, link: str) -> None:
        doc_links.append((ref, link))

    def _render_links() -> None:
        if not doc_links:
            return

        if doc_lines and doc_lines[-1].strip() != "":
            doc_lines.append("")
        for ref, link in doc_links:
            doc_lines.append(f"[{ref}]: {link}")
        doc_lines.append("")

    def _get_human_version(v: str) -> str | None:
        rstr = r"""
            (?P<channel>ces-)?
            v?
            (?P<version>\d+\.\d+\.\d+)
            (?P<suffixes>(?:-(?:[a-zA-Z]+\.\d+))*)
            """

        m = re.match(rstr, v, re.VERBOSE)
        if not m:
            return None

        prefix = "CES" if cast(str, m.group("channel")) else "Ceph"
        version = cast(str, m.group("version"))
        suffix_str = ""

        if m.group("suffixes"):
            suffixes = [
                s[1:].split(".")
                for s in cast(
                    list[str],
                    re.findall(r"(-[a-zA-Z]+\.\d+)", cast(str, m.group("suffixes"))),
                )
            ]
            suffix_str = " ".join([f"{t.upper()}-{n}" for t, n in suffixes])

        suffix_str = f" ({suffix_str})" if suffix_str else ""
        return f"{prefix} version {version}{suffix_str}"

    version = _get_human_version(manifest.name)

    _header("Release Notes", 1)

    _paragraph(
        "At Clyso, we are thrilled to announce the release of a new version of our "
        + "Enterprise Storage solution, built on the robust and reliable Ceph "
        + "platform. This release brings a host of fixes and enhancements over the "
        + "upstream release that we believe will significantly improve your storage "
        + "experience."
    )

    _header("About this Release", 2)
    _paragraph(
        f"The new {version} is based on Ceph {manifest.base_ref} "
        + "and has been developed and tested to ensure compatibility and performance."
    )

    downstream_patches: list[ManifestPatchEntry] = []
    upstream_patches: list[GitHubPullRequest] = []

    for p in manifest.patches:
        if isinstance(p, PatchMeta):
            downstream_patches.append(p)
            continue

        assert isinstance(p, GitHubPullRequest)
        if p.org_name == "ceph":
            upstream_patches.append(p)
        else:
            downstream_patches.append(p)

    downstream_patches_items: list[str] = []
    for p in downstream_patches:
        if isinstance(p, PatchMeta):
            downstream_patches_items.append(f"- {p.info.title.strip('.')}")
        else:
            assert isinstance(p, GitHubPullRequest)
            downstream_patches_items.append(f"- {p.title.rstrip('.')}")

    if downstream_patches_items:
        _header("Downstream patches", 2)
        doc_lines.extend(downstream_patches_items)
        doc_lines.append("")

    upstream_patches_items: list[str] = []
    for p in upstream_patches:
        upstream_patches_items.append(
            f"- {p.title.rstrip('.')} ("
            + f"[{p.pull_request_id}][_pr_{p.pull_request_id}]"
            + ")"
        )
        _add_link(f"_pr_{p.pull_request_id}", f"{p.repo_url}/pull/{p.pull_request_id}")

    if upstream_patches_items:
        _header("Upstream patches", 2)
        doc_lines.extend(upstream_patches_items)
        doc_lines.append("")

    _header("Usage", 2)
    _paragraph(
        "This release brings a container image equivalent to the upstream Ceph's "
        + "image. At Clyso, we value your right to be free from vendor lock-in, "
        + "and thus we make sure the our images are compatible with the upstream's "
        + "Ceph releases. This means you will be able to upgrade to our image from "
        + "an upstream release, and downgrade it back should you want to."
    )

    cephadm_loc = cephadm_loc or "UPDATE_CEPHADM_LINK_HERE"
    image_loc = image_loc or "UPDATE_IMAGE_LINK_HERE"
    _header("Installing `cephadm`", 3)
    _paragraph(
        "The `cephadm` binary can be found at [our repositories][_cephadm_loc]."
        + "To install `cephadm`, the recommended way is to use the following command:"
    )
    _block(
        f"""
           # curl --silent --remote-name --location {cephadm_loc}
           """,
        block_type="shell",
    )
    _add_link("_cephadm_loc", cephadm_loc)

    _header("Container image", 3)
    _paragraph(
        f"The container image for {version} can be found in "
        + f"our container registry at `{image_loc}`"
    )

    _header("Installing or Upgrading", 3)
    _paragraph(
        "To install or upgrade to this release, please follow the instructions found "
        + "in [our documentation][_docs_loc]. If you find any issues, please reach out "
        + "to our support team."
    )
    _add_link(
        "_docs_loc",
        "https://docs.clyso.com/docs/products/clyso-enterprise-storage/install/",
    )

    _render_links()
    return "\n".join(doc_lines)
