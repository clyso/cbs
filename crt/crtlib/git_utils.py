# crt - utils
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

import logging
import re
import sys
import tempfile
from pathlib import Path
from typing import cast, override

import git
from crtlib.errors import CRTError
from crtlib.logger import logger as parent_logger

logger = parent_logger.getChild("git")


SHA = str


class GitError(CRTError):
    @override
    def __str__(self) -> str:
        return "git error" + (f": {self.msg}" if self.msg else "")


class GitIsTagError(GitError):
    def __init__(self, tag: str) -> None:
        super().__init__(msg=tag)

    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("found unexpected tag")


class GitPatchDiffError(GitError):
    @override
    def __str__(self) -> str:
        return "patches diff error" + (f": {self.msg}" if self.msg else "")


class GitEmptyPatchDiffError(GitPatchDiffError):
    pass


class GitCherryPickError(GitError):
    @override
    def __str__(self) -> str:
        return "cherry-pick error" + (f": {self.msg}" if self.msg else "")


class GitCherryPickConflictError(GitCherryPickError):
    sha: SHA
    conflicts: list[str]

    def __init__(self, sha: SHA, files: list[str] | None = None) -> None:
        super().__init__(msg=f"conflict occurred on sha '{sha}'")
        self.sha = sha
        self.conflicts = files if files else []


class GitAMApplyError(GitError):
    pass


class GitMissingRemoteError(GitError):
    remote_name: str

    def __init__(self, remote_name: str) -> None:
        super().__init__()
        self.remote_name = remote_name

    @override
    def __str__(self) -> str:
        return f"missing remote '{self.remote_name}'"


class GitMissingBranchError(GitError):
    def __init__(self, branch_name: str) -> None:
        super().__init__(msg=branch_name)

    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("missing branch")


class GitCreateHeadExistsError(GitError):
    def __init__(self, name: str) -> None:
        super().__init__(msg=name)

    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("head already exists")


class GitHeadNotFoundError(GitError):
    def __init__(self, name: str) -> None:
        super().__init__(msg=name)

    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("head not found")


class GitFetchError(GitError):
    remote_name: str
    from_ref: str
    to_ref: str

    def __init__(self, remote_name: str, from_ref: str, to_ref: str) -> None:
        super().__init__()
        self.remote_name = remote_name
        self.from_ref = from_ref
        self.to_ref = to_ref

    @override
    def __str__(self) -> str:
        return (
            f"failed fetching from remote '{self.remote_name}': "
            + f"from '{self.from_ref}' to '{self.to_ref}'"
        )


class GitFetchHeadNotFoundError(GitFetchError):
    def __init__(self, remote_name: str, head: str) -> None:
        super().__init__(remote_name, head, "")

    @override
    def __str__(self) -> str:
        return f"head '{self.from_ref}' not found in remote '{self.remote_name}'"


class GitPushError(GitError):
    def __init__(self, branch: str, dst_branch: str, remote_name: str) -> None:
        super().__init__(
            msg=f"from branch '{branch}' to '{dst_branch}' on remote '{remote_name}'"
        )

    @override
    def __str__(self) -> str:
        return self.with_maybe_msg("unable to push")


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


def git_cherry_pick(repo_path: Path, sha: SHA) -> None:
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


def git_abort_cherry_pick(repo_path: Path) -> None:
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
) -> git.Remote:
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

    return remote


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


def _get_remote_ref(
    repo_path: Path, ref_name: str, remote_name: str
) -> git.RemoteReference | None:
    repo = git.Repo(repo_path)

    try:
        remote = repo.remote(remote_name)
    except ValueError:
        logger.error(f"remote '{remote_name}' not found")
        raise GitMissingRemoteError(remote_name) from None

    for ref in remote.refs:
        if _get_remote_ref_name(remote_name, ref.name, ref_name=ref_name):
            return ref

    return None


def git_pull_ref(repo_path: Path, from_ref: str, to_ref: str, remote_name: str) -> bool:
    repo = git.Repo(repo_path)
    if repo.active_branch.name != to_ref:
        return False

    if not _get_remote_ref(repo_path, from_ref, remote_name):
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


def git_get_local_head(repo_path: Path, name: str) -> git.Head | None:
    repo = git.Repo(repo_path)
    for head in repo.heads:
        if head.name == name:
            return head
    return None


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

    logger.debug(f"repo active: {repo.active_branch}")

    if repo.active_branch.name == to_ref:
        logger.warning(f"checked out branch is '{to_ref}', pull instead.")
        return git_pull_ref(repo_path, from_ref, to_ref, remote_name)

    # check whether 'from_ref' is a tag
    if _get_tag(repo_path, from_ref):
        logger.warning(f"can't fetch tag '{from_ref}' from remote '{remote_name}'")
        raise GitIsTagError(from_ref)

    # check whether 'from_ref' is a remote head
    if not _get_remote_ref(repo_path, from_ref, remote_name):
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
            if git_get_local_head(repo_path, target_branch):
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
    if head := git_get_local_head(repo_path, target_branch):
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

    head = git_get_local_head(repo_path, target_branch)
    if not head:
        msg = f"unexpected missing local head '{target_branch}'"
        logger.error(msg)
        raise GitError(msg)

    repo.head.reference = head
    _ = repo.head.reset(index=True, working_tree=True)


def git_branch_delete(repo_path: Path, branch: str) -> None:
    """Delete a local branch."""
    repo = git.Repo(repo_path)
    if repo.active_branch.name == branch:
        git_cleanup_repo(repo_path)
        repo.head.reference = repo.heads["main"]

    repo.git.branch(["-D", branch])  # pyright: ignore[reportAny]


def git_push(
    repo_path: Path,
    branch: str,
    remote_name: str,
    *,
    branch_to: str | None = None,
) -> tuple[bool, list[str], list[str]]:
    dst_branch = branch_to if branch_to else branch

    head = git_get_local_head(repo_path, branch)
    if not head:
        logger.error(f"unable to find branch '{branch}' to push")
        raise GitHeadNotFoundError(branch)

    repo = git.Repo(repo_path)
    try:
        remote = repo.remote(remote_name)
    except ValueError:
        logger.error(f"unable to find remote '{remote_name}'")
        raise GitMissingRemoteError(remote_name) from None

    try:
        info = remote.push(f"{branch}:{dst_branch}")
    except git.CommandError as e:
        msg = f"unable to push '{branch}' to '{dst_branch}': {e}"
        logger.error(msg)
        logger.error(e.stderr)
        raise GitPushError(branch, dst_branch, remote_name) from None

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


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("error: missing repo path argument")
        sys.exit(1)

    logger.setLevel(logging.DEBUG)

    repo_path = Path(sys.argv[1])

    print("checkout refs")
    try:
        git_checkout_ref(repo_path, "foobar")
    except Exception as e:
        print(f"error getting 'foobar': {e}")

    try:
        git_checkout_ref(
            repo_path, "main", update_from_remote=True, remote_name="clyso/ceph"
        )
    except Exception as e:
        print(f"error getting 'foobar': {e}")

    try:
        git_checkout_ref(
            repo_path,
            "tentacle",
            update_from_remote=True,
            remote_name="clyso/ceph",
        )
    except Exception as e:
        print(f"error getting 'tentacle': {e}")

    try:
        git_checkout_ref(
            repo_path,
            "v18.2.7",
            to_branch="test-v18.2.7",
            remote_name="ceph/ceph",
            update_from_remote=True,
            fetch_if_not_exists=True,
        )
    except Exception as e:
        print(f"error checking out 'v18.2.7' to 'test-v18.2.7': {e}")
