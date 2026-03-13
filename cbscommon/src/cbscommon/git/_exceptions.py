# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) 2026 Clyso GmbH


from typing import override

from cbscommon.exceptions import CBSCommonError
from cbscommon.git._types import SHA


class GitError(CBSCommonError):
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
