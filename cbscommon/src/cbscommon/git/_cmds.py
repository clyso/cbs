# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 Clyso GmbH


import logging
import re
import tempfile
from collections.abc import Callable, Generator
from pathlib import Path
from typing import cast

import git

from cbscommon.git._exceptions import (
    GitAMApplyError,
    GitCherryPickConflictError,
    GitCherryPickError,
    GitCreateHeadExistsError,
    GitEmptyPatchDiffError,
    GitError,
    GitFetchError,
    GitFetchHeadNotFoundError,
    GitHeadNotFoundError,
    GitIsTagError,
    GitMissingBranchError,
    GitMissingRemoteError,
    GitPatchDiffError,
    GitPushError,
)
from cbscommon.git._types import SHA

logger = logging.getLogger(__name__)


def git_check_patches_diff(
    ceph_git_path: Path,
    upstream_ref: str | SHA,
    head_ref: str | SHA,
    *,
    limit: str | SHA | None = None,
) -> tuple[list[str], list[str]]:
    logger.debug(
        f"check ref '{head_ref}' against upstream '{upstream_ref}', limit '{limit}'"
    )
    repo = git.Repo(ceph_git_path)

    cmd = ["git", "cherry", upstream_ref, head_ref]
    if limit:
        cmd.append(limit)

    try:
        res = repo.git.execute(
            cmd,
            with_extended_output=False,
            as_process=False,
            stdout_as_string=True,
        )
    except Exception as e:
        msg = (
            f"unable to check patch diff between '{upstream_ref}' and '{head_ref}': {e}"
        )
        logger.error(msg)
        raise GitPatchDiffError(msg=msg) from None

    if not res:
        logger.warning(f"empty diff between '{upstream_ref}' and '{head_ref}")
        raise GitEmptyPatchDiffError()

    patches_res = res.splitlines()
    patches_add: list[str] = []
    patches_drop: list[str] = []

    entry_re = re.compile(r"^([-+])\s+(.*)$")
    for entry in patches_res:
        m = re.match(entry_re, entry)
        if not m:
            logger.error(f"unexpected entry format: {entry}")
            continue

        action = cast(str, m.group(1))
        sha = cast(str, m.group(2))

        match action:
            case "+":
                patches_add.append(sha)
            case "-":
                patches_drop.append(sha)
            case _:
                logger.error(f"unexpected patch action '{action}' for sha '{sha}'")

    logger.debug(f"ref '{head_ref}' add {patches_add}")
    logger.debug(f"ref '{head_ref}' drop {patches_drop}")

    return (patches_add, patches_drop)


def git_patches_in_interval(
    repo_path: Path, from_ref: SHA, to_ref: SHA
) -> list[tuple[SHA, str]]:
    logger.debug(f"get patch interval from '{from_ref}' to '{to_ref}'")
    repo = git.Repo(repo_path)

    cmd = [
        "git",
        "rev-list",
        "--ancestry-path",
        "--pretty=oneline",
        f"{from_ref}~1..{to_ref}",
    ]
    try:
        res = repo.git.execute(
            cmd,
            with_extended_output=False,
            as_process=False,
            stdout_as_string=True,
        )
    except Exception as e:
        msg = f"unable to obtain patch interval: {e}"
        logger.error(msg)
        raise GitError(msg=msg) from None

    def _split(ln: str) -> tuple[str, str]:
        sha, title = ln.split(maxsplit=1)
        return (sha, title)

    return list(
        map(_split, [line.strip() for line in res.splitlines() if line.strip()])
    )


def git_get_patch_sha_title(repo_path: Path, sha: SHA) -> tuple[str, str]:
    logger.debug(f"get patch sha and title for '{sha}'")
    repo = git.Repo(repo_path)

    cmd = ["git", "show", "--format=%H %s", "--no-patch", sha]
    try:
        res = repo.git.execute(
            cmd, with_extended_output=False, as_process=False, stdout_as_string=True
        )
    except Exception as e:
        msg = f"unable to obtain patch sha and title for '{sha}': {e}"
        logger.error(msg)
        raise GitError(msg=msg) from None

    logger.debug(res)
    lst = [line.strip() for line in res.splitlines() if line.strip()]
    if len(lst) > 1:
        raise GitError(msg=f"unexpected multiple lines for patch '{sha}'")
    logger.debug(lst)
    patch_sha, patch_title = next(iter(lst)).split(maxsplit=1)
    return (patch_sha, patch_title)


def git_status(repo_path: Path) -> list[tuple[str, str]]:
    repo = git.Repo(repo_path)

    try:
        res = cast(str, repo.git.status(["--porcelain"]))  # pyright: ignore[reportAny]
    except git.CommandError as e:
        msg = f"unable to run git status on '{repo_path}'"
        logger.error(msg)
        logger.error(e.stderr)
        raise GitError(msg=msg) from None

    status_lst: list[tuple[str, str]] = []
    for entry in res.splitlines():
        status, file = entry.split()
        status_lst.append((status, file))

    return status_lst


def _git_cherry_pick(repo_path: Path, sha: SHA) -> None:  # pyright: ignore[reportUnusedFunction]
    repo = git.Repo(repo_path)

    try:
        repo.git.cherry_pick(["-x", "-s", sha])  # pyright: ignore[reportAny]
    except git.CommandError as e:
        msg = f"unable to cherry-pick patch sha '{sha}'"
        logger.error(msg)

        status_files = git_status(repo_path)
        conflicts: list[str] = [f for s, f in status_files if s == "UU"]

        if conflicts:
            raise GitCherryPickConflictError(sha, conflicts) from None

        logger.error(e.stderr)
        raise GitCherryPickError(msg=msg) from None


def _git_abort_cherry_pick(repo_path: Path) -> None:  # pyright: ignore[reportUnusedFunction]
    repo = git.Repo(repo_path)

    try:
        _ = repo.git.cherry_pick("--abort")  # pyright: ignore[reportAny]
    except git.CommandError as e:
        logger.error(f"found error aborting cherry-pick: {e.stderr}")


def git_am_apply(repo_path: Path, patch_path: Path) -> None:
    repo = git.Repo(repo_path)

    try:
        _ = repo.git.am(str(patch_path))  # pyright: ignore[reportAny]
    except git.CommandError as e:
        msg = f"unable to apply patch '{patch_path}'"
        logger.error(msg)
        logger.error(e.stderr)
        raise GitAMApplyError(msg=msg) from None


def git_am_abort(repo_path: Path) -> None:
    repo = git.Repo(repo_path)

    try:
        _ = repo.git.am(["--abort"])  # pyright: ignore[reportAny]
    except git.CommandError as e:
        logger.error(f"found error aborting git-am:\n{e.stderr}")


def git_cleanup_repo(repo_path: Path) -> None:
    repo = git.Repo(repo_path)
    try:
        repo.git.submodule(  # pyright: ignore[reportAny]
            [
                "deinit",
                "--all",
                "-f",
            ]
        )
        repo.git.clean(["-ffdx"])  # pyright: ignore[reportAny]
        repo.git.reset(["--hard"])  # pyright: ignore[reportAny]
    except git.CommandError as e:
        msg = f"unable to clean up repository '{repo_path}': {e}"
        logger.error(msg)
        logger.error(e.stderr)
        raise GitError(msg=msg) from None


def git_prepare_remote(
    repo_path: Path, remote_uri: str, remote_name: str, token: str
) -> None:
    logger.info(f"prepare remote '{remote_name}' uri '{remote_uri}'")

    repo = git.Repo(repo_path)
    try:
        remote = repo.remote(remote_name)
    except ValueError:
        remote_url = f"https://crt:{token}@{remote_uri}"
        remote = repo.create_remote(remote_name, remote_url)
        logger.debug(f"created remote '{remote_name}' url '{remote_url}'")

    logger.info(f"update remote '{remote_name}'")
    try:
        _ = remote.update()
    except git.CommandError as e:
        logger.error(f"unable to update remote '{remote_name}'")
        logger.error(e.stderr)
        raise GitError(msg=f"unable to update remote '{remote_name}'") from None


def git_remote_exists(repo_path: Path, remote_name: str) -> bool:
    logger.info(f"does remote '{remote_name}' exist.")

    repo = git.Repo(repo_path)
    return remote_name in repo.remotes


def _get_remote_ref_name(
    remote_name: str, remote_ref: str, *, ref_name: str | None = None
) -> tuple[str, str] | None:
    ref_re = re.compile(rf"^{remote_name}/(.*)$")
    if m := re.match(ref_re, remote_ref):
        name = cast(str, m.group(1))
        if ref_name and ref_name != name:
            return None

        return (remote_name, m.group(1))
    return None


def git_remote_ref_exists(repo_path: Path, ref_name: str, remote_name: str) -> bool:
    repo = git.Repo(repo_path)

    try:
        remote = repo.remote(remote_name)
    except ValueError:
        logger.error(f"remote '{remote_name}' not found")
        raise GitMissingRemoteError(remote_name) from None

    for ref in remote.refs:
        if _get_remote_ref_name(remote_name, ref.name, ref_name=ref_name):
            return True

    return False


def _git_pull_ref(
    repo_path: Path, from_ref: str, to_ref: str, remote_name: str
) -> bool:
    repo = git.Repo(repo_path)
    if repo.active_branch.name != to_ref:
        return False

    if not git_remote_ref_exists(repo_path, from_ref, remote_name):
        logger.warning(f"ref '{from_ref}' not found in remote '{remote_name}'")
        return False

    try:
        _ = repo.git.pull([remote_name, f"{from_ref}:{to_ref}"])  # pyright: ignore[reportAny]
    except git.CommandError as e:
        logger.error(
            f"unable to pull from '{remote_name}' ref '{from_ref}' to '{to_ref}'"
        )
        logger.error(e.stderr)
        raise GitFetchError(remote_name, from_ref, to_ref) from None

    return True


def _get_tag(repo_path: Path, tag_name: str) -> git.TagReference | None:
    repo = git.Repo(repo_path)
    for tag in repo.tags:
        if tag.name == tag_name:
            return tag
    return None


def _git_get_local_head(repo_path: Path, name: str) -> git.Head | None:
    repo = git.Repo(repo_path)
    return repo.heads[name] if git_local_head_exists(repo_path, name) else None


def git_reset_head(repo_path: Path, new_head: str) -> None:
    """Reset current checked out head to `new_head`."""
    repo = git.Repo(repo_path)

    head = _git_get_local_head(repo_path, new_head)
    if not head:
        msg = f"unexpected missing local head '{new_head}'"
        logger.error(msg)
        raise GitError(msg)

    repo.head.reference = head
    _ = repo.head.reset(index=True, working_tree=True)


def git_branch_from(repo_path: Path, src_ref: str, dst_branch: str) -> None:
    """Create a new branch `dst_branch` from `src_ref`."""
    logger.debug(f"create branch '{dst_branch}' from '{src_ref}'")

    repo = git.Repo(repo_path)
    logger.debug(f"repo active branch: {repo.active_branch}")

    if git_local_head_exists(repo_path, dst_branch):
        msg = f"unable to create branch '{dst_branch}', already exists"
        logger.error(msg)
        raise GitCreateHeadExistsError(dst_branch)

    if _get_tag(repo_path, src_ref):
        logger.debug(f"source ref '{src_ref}' is a tag")
        src_ref = f"refs/tags/{src_ref}"

    try:
        _ = repo.git.branch([dst_branch, src_ref])  # pyright: ignore[reportAny]
    except git.CommandError as e:
        msg = f"unable to create branch '{dst_branch}' from '{src_ref}': {e}"
        logger.error(msg)
        logger.error(e.stderr)
        raise GitError(msg) from None


def git_fetch_ref(
    repo_path: Path, from_ref: str, to_ref: str, remote_name: str
) -> bool:
    """
    Fetch a reference from a remote into a given branch.

    If the target branch is already checked out, perform a `git pull` instead.
    If the source ref is a tag, do not fetch.

    Will raise if `from_ref` is a tag, or if it doesn't exist in the specified remote.
    Might raise in other `git fetch` error conditions.
    """
    logger.debug(f"fetch from '{remote_name}' ref '{from_ref}' to '{to_ref}'")

    repo = git.Repo(repo_path)
    logger.debug(f"repo active branch: {repo.active_branch}")

    if repo.active_branch.name == to_ref:
        logger.warning(f"checked out branch is '{to_ref}', pull instead.")
        return _git_pull_ref(repo_path, from_ref, to_ref, remote_name)

    # check whether 'from_ref' is a tag
    if _get_tag(repo_path, from_ref):
        logger.warning(f"can't fetch tag '{from_ref}' from remote '{remote_name}'")
        raise GitIsTagError(from_ref)

    # check whether 'from_ref' is a remote head
    if not git_remote_ref_exists(repo_path, from_ref, remote_name):
        logger.warning(f"unable to find ref '{from_ref}' in remote '{remote_name}'")
        raise GitFetchHeadNotFoundError(remote_name, from_ref)

    try:
        remote = repo.remote(remote_name)
    except ValueError:
        msg = f"unexpected error obtaining remote '{remote_name}'"
        logger.error(msg)
        raise GitError(msg) from None

    try:
        _ = remote.fetch(f"{from_ref}:{to_ref}")
    except git.CommandError as e:
        logger.error(
            f"unable to fetch from remote '{remote_name}' "
            + f"ref '{from_ref}' to '{to_ref}'"
        )
        logger.error(e.stderr)
        raise GitFetchError(remote_name, from_ref, to_ref) from None

    return True


def git_checkout_ref(
    repo_path: Path,
    ref: str,
    *,
    to_branch: str | None = None,
    remote_name: str | None = None,
    update_from_remote: bool = False,
    fetch_if_not_exists: bool = False,
) -> None:
    """
    Check out a reference, possibly to a new branch.

    If `ref` exists in the repository, checks out said head. Otherwise, either raise
    `GitMissingBranchError`, or attempt to fetch the branch from `remote_name` if
    `remote_name` is `True` and `fetch_if_not_exists` is defined.

    If `to_branch` is defined, either checks out the provided `ref` to the specified
    branch, or attempts to fetch it from remote `remote_name` (if defined).

    If `update_from_remote` is `True`, always attempt to fetch the latest updates in
    the remote branch to the target branch. The target branch can be `ref` or
    `to_branch` depending on whether the latter is defined. If `remote_name` is not
    specified, `update_from_remote` has no effect.
    """
    repo = git.Repo(repo_path)

    def _update_from_remote(head: git.Head, remote: str) -> None:
        logger.debug(f"update '{head}' from remote if it exists")
        try:
            res = git_fetch_ref(repo_path, head.name, head.name, remote)
        except Exception as e:
            logger.error(f"unable to update '{head.name}' from remote '{remote}: {e}")
            return

        if not res:
            logger.info(f"whatever to update for '{head.name}' from remote '{remote}'")
        pass

    def _checkout_head(head: git.Head, *, target_branch: str | None = None) -> None:
        """
        Checkout a given head.

        If `target_branch` is specified, checkout the provided head to a new branch.
        """
        logger.debug(f"checkout head '{head}' to '{target_branch}'")

        if target_branch and head.name != target_branch:
            # checkout 'head' to a new branch
            if git_local_head_exists(repo_path, target_branch):
                raise GitCreateHeadExistsError(target_branch)
            head = repo.create_head(target_branch, head)
        # should we update from remote first?
        if update_from_remote and remote_name:
            _update_from_remote(head, remote_name)

        repo.head.reference = head
        _ = repo.head.reset(index=True, working_tree=True)
        pass

    target_branch = to_branch if to_branch else ref

    # check if 'ref' exists as a branch locally
    if head := _git_get_local_head(repo_path, target_branch):
        _checkout_head(head, target_branch=target_branch)
        return

    if not fetch_if_not_exists:
        logger.debug(f"not fetching '{ref}' as specified")
        raise GitMissingBranchError(ref)

    if not remote_name:
        msg = f"unable to fetch ref '{ref}', no remote given"
        logger.error(msg)
        raise GitError(msg) from None

    # local head does not exist, fetch it.
    is_tag = False
    try:
        _ = git_fetch_ref(repo_path, ref, target_branch, remote_name)
    except GitIsTagError:
        logger.debug(f"ref '{ref}' is a tag, must checkout instead.")
        is_tag = True
    except GitFetchHeadNotFoundError as e:
        logger.error(f"ref '{ref}' not found in remote.")
        raise e from None
    except GitError as e:
        logger.error(f"error occurred fetching ref '{ref}': {e}")
        raise e from None

    if is_tag:
        try:
            _ = repo.git.checkout(  # pyright: ignore[reportAny]
                [ref, "-b", target_branch]
            )
        except git.CommandError as e:
            msg = f"unable to checkout ref '{ref}' to '{target_branch}': {e}"
            logger.error(msg)
            logger.error(e.stderr)
            raise GitError(msg) from None
        return

    # propagate exceptions
    git_reset_head(repo_path, target_branch)


def git_branch_delete(repo_path: Path, branch: str) -> None:
    """Delete a local branch."""
    repo = git.Repo(repo_path)
    if repo.active_branch.name == branch:
        git_cleanup_repo(repo_path)
        repo.head.reference = repo.heads["main"]

    repo.git.branch(["-D", branch])  # pyright: ignore[reportAny]


def git_push(
    repo_path: Path,
    ref: str,
    remote_name: str,
    *,
    ref_to: str | None = None,
) -> tuple[bool, list[str], list[str]]:
    """Pushes either a local head of branch or a local tag to the remote."""
    dst_ref = ref_to if ref_to else ref

    if _get_tag(repo_path, ref):
        ref = f"refs/tags/{ref}"
        dst_ref = f"refs/tags/{dst_ref}"
    elif not git_local_head_exists(repo_path, ref):
        # ref is neither a local branch nor tag
        logger.error(f"unable to find ref '{ref}' to push")
        raise GitHeadNotFoundError(ref)

    repo = git.Repo(repo_path)
    try:
        remote = repo.remote(remote_name)
    except ValueError:
        logger.error(f"unable to find remote '{remote_name}'")
        raise GitMissingRemoteError(remote_name) from None

    try:
        info = remote.push(f"{ref}:{dst_ref}")
    except git.CommandError as e:
        msg = f"unable to push '{ref}' to '{dst_ref}': {e}"
        logger.error(msg)
        logger.error(e.stderr)
        raise GitPushError(ref, dst_ref, remote_name) from None

    updated: list[str] = []
    rejected: list[str] = []
    failed = len(info) == 0

    for entry in info:
        entry_names = _get_remote_ref_name(remote_name, entry.remote_ref.name)
        if not entry_names:
            logger.warning(f"mismatched remote ref on push: '{entry.remote_ref.name}'")
            continue

        remote_ref_name = entry_names[1]
        logger.debug(f"entry '{remote_ref_name}' flags '{entry.flags}'")
        if entry.flags & entry.ERROR:
            logger.debug(f"rejected head: {remote_ref_name}")
            rejected.append(remote_ref_name)
        elif entry.flags & (entry.NEW_HEAD | entry.FAST_FORWARD):
            logger.debug(f"updated head: {remote_ref_name}")
            updated.append(remote_ref_name)

    return (failed, updated, rejected)


def git_tag(
    repo_path: Path,
    tag_name: str,
    ref: str,
    *,
    msg: str | None = None,
    push_to: str | None = None,
) -> None:
    repo = git.Repo(repo_path)

    logger.debug(f"create tag '{tag_name}' at ref '{ref}'")
    try:
        _ = repo.create_tag(tag_name, ref, msg)
    except Exception as e:
        msg = f"unable to create tag '{tag_name}' at ref '{ref}': {e}"
        logger.error(msg)
        raise GitError(msg=msg) from None

    if push_to:
        logger.debug(f"push tag '{tag_name}' to remote '{push_to}'")
        try:
            repo.git.push([push_to, "tag", tag_name])  # pyright: ignore[reportAny]
        except Exception as e:
            msg = f"unable to push tag '{tag_name}' to remote '{push_to}': {e}"
            logger.error(msg)
            raise GitError(msg=msg) from None


def git_patch_id(repo_path: Path, sha: SHA) -> str:
    repo = git.Repo(repo_path)

    with tempfile.TemporaryFile() as tmp:
        try:
            repo.git.show(sha, output_stream=tmp)  # pyright: ignore[reportAny]
        except git.CommandError:
            msg = f"unable to find patch sha '{sha}'"
            logger.error(msg)
            raise GitError(msg=msg) from None

        _ = tmp.seek(0)
        res = cast(str, repo.git.patch_id(["--stable"], istream=tmp))  # pyright: ignore[reportAny]

    if not res:
        raise GitError(msg="unable to obtain git patch id")
    return res.split()[0]


def git_revparse(repo_path: Path, commitish: SHA | str) -> str:
    repo = git.Repo(repo_path)

    try:
        res = repo.rev_parse(commitish)
    except git.BadObject:
        msg = f"rev '{commitish}' not found"
        logger.error(msg)
        raise GitError(msg=msg) from None
    except ValueError:
        msg = f"malformed rev '{commitish}'"
        logger.error(msg)
        raise GitError(msg=msg) from None
    except Exception as e:
        msg = f"unable to obtain revision for '{commitish}': {e}"
        logger.error(msg)
        raise GitError(msg=msg) from None

    return res.hexsha


def git_format_patch(repo_path: Path, rev: SHA, *, base_rev: SHA | None = None) -> str:
    repo = git.Repo(repo_path)

    args = ["--stdout"]
    if not base_rev:
        args.append("-1")

    rev_str = f"{base_rev}..{rev}" if base_rev else rev
    args.append(rev_str)

    try:
        res = cast(str, repo.git.format_patch(args))  # pyright: ignore[reportAny]
    except git.CommandError as e:
        msg = f"unable to obtain format patch for '{rev_str}': {e}"
        logger.error(msg)
        raise GitError(msg=msg) from None

    return res


def git_tag_exists_in_remote(repo_path: Path, remote_name: str, tag_name: str) -> bool:
    try:
        repo = git.Repo(repo_path)
        ls_remote = cast(Callable[..., str], repo.git.ls_remote)
        raw_tag = ls_remote("--tags", remote_name, f"refs/tags/{tag_name}")
        return bool(raw_tag.strip())
    except git.CommandError as e:
        msg = f"unable to execute git ls-remote --tags {remote_name} refs/tags/{tag_name}: {e}"
        logger.error(msg)
        raise GitError(msg) from None


def git_remote_ref_names(repo_path: Path, remote_name: str) -> Generator[str]:
    try:
        repo = git.Repo(repo_path)
        remote = repo.remotes[remote_name]
        for ref in remote.refs:
            yield ref.name
    except git.NoSuchPathError:
        msg = f"path '{repo_path}' doesn't exist"
        logger.error(msg)
        raise GitError(msg) from None
    except git.InvalidGitRepositoryError:
        msg = f"path '{repo_path}' isn't a valid git repository"
        logger.error(msg)
        raise GitError(msg) from None
    except IndexError:
        msg = f"repository '{repo_path}' has no remote '{remote_name}'"
        logger.error(msg)
        raise GitMissingRemoteError(msg) from None


def git_checkout_from_local_ref(
    repo_path: Path, from_ref: str, branch_name: str
) -> None:
    logger.debug(f"checkout ref '{from_ref}' to '{branch_name}'")
    repo = git.Repo(repo_path)
    if head := _git_get_local_head(repo_path, branch_name):
        logger.debug(f"branch '{branch_name}' already exists, simply checkout")
        repo.head.reference = head
        _ = repo.head.reset(index=True, working_tree=True)
        return

    assert branch_name not in repo.heads
    try:
        new_head = repo.create_head(branch_name, from_ref)
    except Exception:
        msg = f"unable to create new head '{branch_name}' " + f"from '{from_ref}'"
        logger.exception(msg)
        raise GitError(msg=msg) from None

    repo.head.reference = new_head
    _ = repo.head.reset(index=True, working_tree=True)

    try:
        git_cleanup_repo(repo_path)
        git_update_submodules(repo_path)
    except Exception as e:
        msg = f"unable to clean up repo state after checkout: {e}"
        logger.error(msg)
        raise GitError(msg=msg) from None


def git_update_submodules(repo_path: Path) -> None:
    logger.debug("update submodules")
    repo = git.Repo(repo_path)
    try:
        repo.git.execute(  # pyright: ignore[reportCallIssue]
            ["git", "submodule", "update", "--init", "--recursive"],
            as_process=False,
            with_stdout=True,
        )
    except Exception as e:
        msg = f"unable to update repository's submodules: {e}"
        logger.error(msg)
        raise GitError(msg=msg) from None


def git_local_head_exists(repo_path: Path, name: str) -> bool:
    repo = git.Repo(repo_path)
    return name in repo.heads
