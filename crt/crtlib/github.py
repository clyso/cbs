# crt - github adapter
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

import errno
import re
import sys
import uuid
from datetime import datetime as dt
from typing import override

import httpx
import pydantic
from crtlib.errors import CRTError
from crtlib.logger import logger as parent_logger
from crtlib.models.patch import AuthorData, Patch
from crtlib.models.patchset import GitHubPullRequest

logger = parent_logger.getChild("gh")


class GitHubError(CRTError):
    @override
    def __str__(self) -> str:
        return "github error" + (f": {self.msg}" if self.msg else "")


class _GitHubUser(pydantic.BaseModel):
    login: str
    url: str


class _GitHubUserInfo(pydantic.BaseModel):
    login: str
    name: str | None
    email: str | None


class _GitHubPullRequestBase(pydantic.BaseModel):
    ref: str


class _GitHubPullRequestInfo(pydantic.BaseModel):
    url: str = pydantic.Field(alias="html_url")
    pr_id: int = pydantic.Field(alias="number")
    state: str
    title: str
    user: _GitHubUser
    created_at: dt
    closed_at: dt
    merged_at: dt
    base: _GitHubPullRequestBase
    body: str
    merged: bool


class _GitHubAuthor(pydantic.BaseModel):
    name: str
    email: str
    date: dt


class _GitHubCommitInfo(pydantic.BaseModel):
    author: _GitHubAuthor
    committer: _GitHubAuthor
    message: str


class _GitHubCommitParent(pydantic.BaseModel):
    sha: str


class _GitHubCommit(pydantic.BaseModel):
    sha: str
    commit: _GitHubCommitInfo
    parents: list[_GitHubCommitParent]


class _GitHubMessageBody(pydantic.BaseModel):
    title: str | None
    desc: str
    signed_off_by: list[AuthorData]
    cherry_picked_from: list[str]
    fixes: list[str]


def gh_get_user_info(url: str, *, token: str | None = None) -> AuthorData:
    headers = {
        # return the PR's body's markdown as plain text
        "Accept": "application/vnd.github.text+json",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"

    try:
        user_info_res = httpx.get(url, headers=headers)
    except httpx.ConnectError as e:
        logger.error(f"error: unable to connect to github: {e}")
        sys.exit(errno.ENOTRECOVERABLE)
    except Exception as e:
        logger.error(f"error: unable to obtain user info: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if not user_info_res.is_success:
        logger.error(f"error: unable to obtain user info: {user_info_res.text}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        user_info = _GitHubUserInfo.model_validate(user_info_res.json())
    except pydantic.ValidationError:
        logger.error("error: malformed user info")
        sys.exit(errno.EINVAL)

    user_name = user_info.name if user_info.name else user_info.login
    user_email = user_info.email if user_info.email else "unknown"

    return AuthorData(user=user_name, email=user_email)


def _gh_commit_to_patch(
    repo_url: str, commit: _GitHubCommit, patchset_uuid: uuid.UUID
) -> Patch:
    """Translate a GitHub commits's information to a patch."""
    commit_message_body = _gh_parse_message_body(
        commit.commit.message, first_line_is_title=True
    )
    title = "<no title>" if not commit_message_body.title else commit_message_body.title
    # click.echo(f"commit message body: {commit_message_body}")

    if len(commit.parents) > 1:
        raise GitHubError(msg="multiple parents found, merge commit?")

    parent = next(iter(commit.parents)).sha

    return Patch(
        sha=commit.sha,
        author=AuthorData(
            user=commit.commit.author.name,
            email=commit.commit.author.email,
        ),
        author_date=commit.commit.author.date,
        commit_author=AuthorData(
            user=commit.commit.committer.name,
            email=commit.commit.committer.email,
        ),
        commit_date=commit.commit.committer.date,
        title=title,
        message=commit_message_body.desc,
        cherry_picked_from=commit_message_body.cherry_picked_from,
        related_to=commit_message_body.fixes,
        # FIXME: calc patch id
        repo_url=repo_url,
        parent=parent,
        patch_id="qwe",
        patchset_uuid=patchset_uuid,
    )


def gh_pr_get_patches(
    url: str, patchset_uuid: uuid.UUID, *, repo_url: str, token: str | None = None
) -> list[Patch]:
    """Obtain commits from GitHub and translate them into patches."""
    headers = {
        "Accept": "application/vnd.github.raw+json",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"

    try:
        pr_commits_res = httpx.get(url, headers=headers)
    except httpx.ConnectError as e:
        logger.error(f"error: unable to connect to github: {e}")
        sys.exit(errno.ENOTRECOVERABLE)
    except Exception as e:
        logger.error(f"error: unable to obtain PR commits: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if not pr_commits_res.is_success:
        logger.error(
            f"error: unable to obtain PR commits: {pr_commits_res.text}",
        )
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        ta = pydantic.TypeAdapter(list[_GitHubCommit])
        commits = ta.validate_python(pr_commits_res.json())
    except pydantic.ValidationError:
        logger.error("error: malformed github PR commits response")
        sys.exit(errno.EINVAL)

    patches: list[Patch] = [
        _gh_commit_to_patch(repo_url, commit, patchset_uuid) for commit in commits
    ]
    return patches


def _gh_parse_message_body(
    body: str, *, first_line_is_title: bool = False
) -> _GitHubMessageBody:
    """
    Parse a message body from a commit from the Ceph repository.

    Assumes a certain format is in place for the commit's message body, according to
    the Ceph upstream's commit guidelines.
    """
    idx = body.find("<!--")
    body = body if idx < 0 else body[:idx]

    desc_lst: list[str] = []
    signed_offs_lst: list[AuthorData] = []
    cherry_picks_lst: list[str] = []
    fixes_lst: list[str] = []

    sign_off_re = re.compile(r"[sS]igned-[oO]ff-[bB]y:\s+(.*)\s+<(.*)>")
    cherry_picked_re = re.compile(r"\(cherry picked from commit ([\w\d]+)\)")
    fixes_re = re.compile(r"[fF]ixes: (.*)|[rR]esolves: (.*)")

    end_of_desc = False
    for line in body.splitlines():
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

    # if we expect a title on the first line, obtain it
    title = (
        None
        if not first_line_is_title
        else (None if not len(desc_lst) else desc_lst.pop(0))
    )

    # remove newlines from end of description
    desc = "".join(desc_lst).strip()

    return _GitHubMessageBody(
        title=title,
        desc=desc,
        signed_off_by=signed_offs_lst,
        cherry_picked_from=cherry_picks_lst,
        fixes=fixes_lst,
    )


def gh_get_pr(
    org: str, repo: str, pr_id: int, *, token: str | None = None
) -> GitHubPullRequest:
    """Obtain a pull request's information from GitHub."""
    headers = {
        # return the PR's body's markdown as plain text
        "Accept": "application/vnd.github.raw+json",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"

    pr_base_url = f"https://api.github.com/repos/{org}/{repo}/pulls/{pr_id}"
    pr_commits_url = f"{pr_base_url}/commits"

    try:
        pr_res = httpx.get(pr_base_url, headers=headers)
    except httpx.ConnectError as e:
        logger.error(f"error: unable to connect to github: {e}")
        sys.exit(errno.ENOTRECOVERABLE)
    except Exception as e:
        logger.error(f"error: unable to obtain PR {pr_id}: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    if not pr_res.is_success:
        logger.error(f"error: unable to obtain PR {pr_id}: {pr_res.text}")
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        pr = _GitHubPullRequestInfo.model_validate(pr_res.json())
    except pydantic.ValidationError as e:
        logger.error(f"error: malformed github PR response: {e}")
        sys.exit(errno.EINVAL)

    pr_body = _gh_parse_message_body(pr.body)
    pr_user = gh_get_user_info(pr.user.url, token=token)

    repo_url = f"https://github.com/{org}/{repo}"
    patchset = GitHubPullRequest(
        org_name=org,
        repo_name=repo,
        repo_url=repo_url,
        pull_request_id=pr_id,
        merge_date=pr.merged_at,
        merged=pr.merged,
        target_branch=pr.base.ref,
        # for PatchSet
        author=pr_user,
        creation_date=pr.created_at,
        title=pr.title,
        related_to=pr_body.fixes,
        patches=[],
    )

    pr_commits = gh_pr_get_patches(
        pr_commits_url, patchset.patchset_uuid, repo_url=repo_url, token=token
    )
    patchset.patches = pr_commits

    return patchset
