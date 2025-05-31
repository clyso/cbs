#!/usr/bin/env python3

import abc
import contextlib
import datetime
import errno
import re
import string
import sys
import uuid
from datetime import datetime as dt
from pathlib import Path
from random import choices
from typing import Annotated, Any, cast, override

import click
import httpx
import pydantic

SHA = str


class NoSuchManifestError(Exception):
    pass


class MalformedManifestError(Exception):
    pass


class PatchSetError(Exception):
    msg: str

    def __init__(self, msg: str) -> None:
        super().__init__()
        self.msg = msg

    @override
    def __str__(self) -> str:
        return f"patch set error: {self.msg}"


class NoSuchPatchSetError(PatchSetError):
    @override
    def __str__(self) -> str:
        return f"patch set does not exists: {self.msg}"


class MalformedPatchSetError(PatchSetError):
    @override
    def __str__(self) -> str:
        return f"malformed patch set: {self.msg}"


class PatchSetMismatchError(PatchSetError):
    @override
    def __str__(self) -> str:
        return f"mismatch patch set type: {self.msg}"


class PatchError(Exception):
    msg: str

    def __init__(self, msg: str) -> None:
        super().__init__()
        self.msg = msg


class PatchExistsError(PatchError):
    def __init__(self, sha: str, patch_uuid: uuid.UUID) -> None:
        super().__init__(msg=f"sha'{sha}' uuid '{patch_uuid}'")

    @override
    def __str__(self) -> str:
        return f"patch already exists: {self.msg}"


class NoSuchPatchError(PatchError):
    def __init__(self, patch_uuid: uuid.UUID) -> None:
        super().__init__(msg=f"uuid '{patch_uuid}'")

    @override
    def __str__(self) -> str:
        return f"patch not found: {self.msg}"


class MalformedPatchError(PatchError):
    def __init__(self, patch_uuid: uuid.UUID) -> None:
        super().__init__(msg=f"uuid '{patch_uuid}'")

    @override
    def __str__(self) -> str:
        return f"malformed patch: {self.msg}"


class AuthorData(pydantic.BaseModel):
    """Represents an author."""

    user: str
    email: str


class Patch(pydantic.BaseModel):
    """Represents a singular patch."""

    sha: SHA
    author: AuthorData
    author_date: dt
    commit_author: AuthorData | None
    commit_date: dt | None
    title: str
    message: str
    cherry_picked_from: list[str]
    related_to: list[str]

    repo_url: str
    patch_id: SHA
    patch_uuid: uuid.UUID = pydantic.Field(default_factory=lambda: uuid.uuid4())
    patchset_uuid: uuid.UUID | None


class PatchSetBase(pydantic.BaseModel, abc.ABC):  # pyright: ignore[reportUnsafeMultipleInheritance]
    """Represents a set of related patches."""

    author: AuthorData
    creation_date: dt
    title: str
    related_to: list[str]
    patches: list[Patch]

    patchset_uuid: uuid.UUID = pydantic.Field(default_factory=lambda: uuid.uuid4())


class GitHubPullRequest(PatchSetBase):
    """Represents a GitHub Pull Request, containing one or more patches."""

    org_name: str
    repo_name: str
    repo_url: str
    pull_request_id: int
    merge_date: dt | None
    merged: bool
    target_branch: str


def _patchset_discriminator(v: Any) -> str:  # pyright: ignore[reportExplicitAny, reportAny]
    if isinstance(v, GitHubPullRequest):
        return "gh"
    elif isinstance(v, dict):
        if "pull_request_id" in v:
            return "gh"
        else:
            return "vanilla"
    else:
        return "vanilla"


class PatchSet(pydantic.BaseModel):
    info: Annotated[
        Annotated[GitHubPullRequest, pydantic.Tag("gh")]
        | Annotated[PatchSetBase, pydantic.Tag("vanilla")],
        pydantic.Discriminator(_patchset_discriminator),
    ]


class ReleaseManifest(pydantic.BaseModel):
    name: str
    base_release_name: str
    base_ref_org: str
    base_ref_repo: str
    base_ref: str

    patchsets: list[uuid.UUID] = pydantic.Field(default=[])
    patches: list[uuid.UUID] = pydantic.Field(default=[])

    creation_date: dt = pydantic.Field(default_factory=lambda: dt.now(datetime.UTC))
    release_uuid: uuid.UUID = pydantic.Field(default_factory=lambda: uuid.uuid4())
    release_git_uid: str = pydantic.Field(
        default_factory=lambda: "".join(choices(string.ascii_letters, k=6))  # noqa: S311
    )

    def contains_patchset(self, patchset: PatchSetBase) -> bool:
        """Check if the release manifest contains a given patch set."""
        return patchset.patchset_uuid in self.patchsets

    def add_patchset(
        self, patchset: PatchSetBase
    ) -> tuple[bool, list[Patch], list[Patch]]:
        """
        Add a patch set to this release manifest.

        Returns a tuple containing:
        - `bool`, indicating whether the patch set was added or not.
        - `list[Patch]`, with the patches that were added to the release manifest.
        - `list[Patch]`, with the patches that were skipped and not added to the
                         release manifest.
        """
        if self.contains_patchset(patchset):
            return (False, [], [])

        self.patchsets.append(patchset.patchset_uuid)

        skipped: list[Patch] = []
        added: list[Patch] = []
        for patch in patchset.patches:
            is_added = self.add_patch(patch)
            if not is_added:
                skipped.append(patch)
            else:
                added.append(patch)

        return (True, added, skipped)

    def contains_patch(self, patch: Patch) -> bool:
        """Check if the release manifest contains a given patch."""
        return patch.patch_uuid in self.patches

    def add_patch(self, patch: Patch) -> bool:
        """
        Add a given patch to the release manifest.

        Returns a boolean, referring to whether the patch was added to the release
        manifest or not.
        """
        if self.contains_patch(patch):
            return False
        self.patches.append(patch.patch_uuid)
        return True


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


class _GitHubCommit(pydantic.BaseModel):
    sha: str
    commit: _GitHubCommitInfo


class _GitHubMessageBody(pydantic.BaseModel):
    title: str | None
    desc: str
    signed_off_by: list[AuthorData]
    cherry_picked_from: list[str]
    fixes: list[str]


class ReleasesDB:
    """
    On-disk representation of the releases database.

    For a release db root at '$root', the on-disk format is as follows:

    $root/manifests         - stores release manifests as JSON files, identified by
                              their UUIDs.
    $root/patchsets         - stores patchsets for releases, identified by their UUIDs.
    $root/patchsets/gh      - stores pull requests from GitHub, mapping them to the
                              corresponding patch set UUIDs.
    $root/patches/by_uuid   - stores patches information as JSON files, identified by
                              their UUIDs.
    $root/patches/by_sha    - stores patches' SHAs, mapping them to the corresponding
                              patch UUID.
    """

    db_path: Path

    def __init__(self, path: Path) -> None:
        self.db_path = path
        self._init_tree()

    def _init_tree(self) -> None:
        self.manifests_path.mkdir(exist_ok=True, parents=True)
        self.gh_prs_path.mkdir(exist_ok=True, parents=True)
        self.patches_by_uuid_path.mkdir(exist_ok=True, parents=True)
        self.patches_by_sha_path.mkdir(exist_ok=True, parents=True)

    @property
    def manifests_path(self) -> Path:
        return self.db_path.joinpath("manifests")

    @property
    def patchsets_path(self) -> Path:
        return self.db_path.joinpath("patchsets")

    @property
    def gh_prs_path(self) -> Path:
        return self.patchsets_path.joinpath("gh")

    @property
    def patches_path(self) -> Path:
        return self.db_path.joinpath("patches")

    @property
    def patches_by_uuid_path(self) -> Path:
        return self.patches_path.joinpath("by_uuid")

    @property
    def patches_by_sha_path(self) -> Path:
        return self.patches_path.joinpath("by_sha")

    def list_manifests_uuids(self) -> list[uuid.UUID]:
        """Obtain the UUIDs for all known release manifests."""
        uuids_lst: list[uuid.UUID] = []
        for entry in self.manifests_path.glob("*.json"):
            try:
                entry_uuid = uuid.UUID(entry.stem)
            except Exception:  # noqa: S112
                # malformed UUID, ignore.
                continue
            uuids_lst.append(entry_uuid)

        return uuids_lst

    def load_manifest(self, uuid: uuid.UUID) -> ReleaseManifest:
        """Load a release manifest from disk."""
        manifest_path = self.manifests_path.joinpath(f"{uuid}.json")
        if not manifest_path.exists():
            raise NoSuchManifestError()

        try:
            with manifest_path.open("r") as fd:
                manifest = ReleaseManifest.model_validate_json(fd.read())
        except pydantic.ValidationError:
            raise MalformedManifestError() from None
        # propagate further exceptions
        return manifest

    def store_manifest(self, manifest: ReleaseManifest) -> None:
        """Store a release manifest to disk."""
        manifest_path = self.manifests_path.joinpath(f"{manifest.release_uuid}.json")
        _ = manifest_path.write_text(manifest.model_dump_json(indent=2))

    def get_patchset_path(self, uuid: uuid.UUID) -> Path:
        return self.patchsets_path.joinpath(f"{uuid}.json")

    def load_patchset(self, uuid: uuid.UUID) -> PatchSetBase:
        """Obtain a patch set by its UUID."""
        patchset_path = self.patchsets_path.joinpath(f"{uuid}.json")
        if not patchset_path.exists():
            raise NoSuchPatchSetError(msg=f"uuid '{uuid}'")

        try:
            patchset_ctr = PatchSet.model_validate_json(patchset_path.read_text())
        except pydantic.ValidationError:
            raise MalformedPatchSetError(msg=f"uuid '{uuid}'") from None

        return patchset_ctr.info

    def load_gh_pr(self, org: str, repo: str, pr_id: int) -> GitHubPullRequest:
        """Load a patch set's information, as a GitHub pull request, from disk."""
        pr_path = self.gh_prs_path.joinpath(f"{org}/{repo}/{pr_id}")
        click.echo(f"pr path: {pr_path}")
        if not pr_path.exists():
            raise NoSuchPatchSetError(f"gh/{org}/{repo}/{pr_id}")

        try:
            patchset_uuid = uuid.UUID(pr_path.read_text())
        except Exception as e:
            raise PatchSetError(
                msg=f"missing uuid for 'gh/{org}/{repo}/{pr_id}: {e}"
            ) from None

        patchset_path = self.patchsets_path.joinpath(f"{patchset_uuid}.json")
        if not patchset_path.exists():
            raise NoSuchPatchSetError(msg=f"uuid '{patchset_uuid}'")

        try:
            patchset = PatchSet.model_validate_json(patchset_path.read_text())
        except pydantic.ValidationError:
            raise MalformedPatchSetError(msg=f"uuid '{patchset_uuid}'") from None
        # propagate further exceptions

        if not isinstance(patchset.info, GitHubPullRequest):
            raise PatchSetMismatchError(msg=f"uuid '{patchset_uuid}' expected github")
        return patchset.info

    def store_gh_patchset(self, patchset: GitHubPullRequest) -> None:
        """Store a GitHub pull request's information as a patch set to disk."""
        pr_base_path = self.gh_prs_path.joinpath(patchset.org_name).joinpath(
            patchset.repo_name
        )
        pr_base_path.mkdir(exist_ok=True, parents=True)
        pr_path = pr_base_path.joinpath(f"{patchset.pull_request_id}")
        patchset_path = self.patchsets_path.joinpath(f"{patchset.patchset_uuid}.json")

        patchset_ctr = PatchSet(info=patchset)
        # propagate exceptions
        _ = patchset_path.write_text(patchset_ctr.model_dump_json(indent=2))
        _ = pr_path.write_text(str(patchset.patchset_uuid))

        for patch in patchset.patches:
            with contextlib.suppress(PatchExistsError):
                self.store_patch(patch)

    def load_patch(self, patch_uuid: uuid.UUID) -> Patch:
        """Load a patch's information from disk, by its UUID."""
        patch_path = self.patches_by_uuid_path.joinpath(f"{patch_uuid}.json")
        if not patch_path.exists():
            raise NoSuchPatchError(patch_uuid)

        try:
            patch = Patch.model_validate_json(patch_path.read_text())
        except pydantic.ValidationError:
            raise MalformedPatchError(patch_uuid) from None

        return patch

    def store_patch(self, patch: Patch) -> None:
        """Store a patch's information to disk."""
        sha_path = self.patches_by_sha_path.joinpath(patch.sha)
        uuid_path = self.patches_by_uuid_path.joinpath(f"{patch.patch_uuid}.json")

        if sha_path.exists() or uuid_path.exists():
            raise PatchExistsError(patch.sha, patch.patch_uuid)

        # propagate exceptions
        _ = sha_path.write_text(str(patch.patch_uuid))
        _ = uuid_path.write_text(patch.model_dump_json(indent=2))


class Ctx:
    db: ReleasesDB
    release_uuid: uuid.UUID | None
    github_token: str | None

    def __init__(self) -> None:
        self.db = ReleasesDB(Path.cwd().joinpath(".releases"))
        self.release_uuid = None
        self.github_token = None

    @property
    def db_path(self) -> Path:
        return self.db.db_path

    @db_path.setter
    def db_path(self, path: Path) -> None:
        self.db.db_path = path


pass_ctx = click.make_pass_decorator(Ctx, ensure=True)


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
        click.echo(f"error: unable to connect to github: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)
    except Exception as e:
        click.echo(f"error: unable to obtain user info: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    if not user_info_res.is_success:
        click.echo(f"error: unable to obtain user info: {user_info_res.text}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        user_info = _GitHubUserInfo.model_validate(user_info_res.json())
    except pydantic.ValidationError:
        click.echo("error: malformed user info", err=True)
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
        click.echo(f"error: unable to connect to github: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)
    except Exception as e:
        click.echo(f"error: unable to obtain PR commits: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    if not pr_commits_res.is_success:
        click.echo(
            f"error: unable to obtain PR commits: {pr_commits_res.text}",
            err=True,
        )
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        ta = pydantic.TypeAdapter(list[_GitHubCommit])
        commits = ta.validate_python(pr_commits_res.json())
    except pydantic.ValidationError:
        click.echo("error: malformed github PR commits response", err=True)
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
        click.echo(f"error: unable to connect to github: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)
    except Exception as e:
        click.echo(f"error: unable to obtain PR {pr_id}: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    if not pr_res.is_success:
        click.echo(f"error: unable to obtain PR {pr_id}: {pr_res.text}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    try:
        pr = _GitHubPullRequestInfo.model_validate(pr_res.json())
    except pydantic.ValidationError as e:
        click.echo(f"error: malformed github PR response: {e}", err=True)
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


@click.group()
@click.option(
    "--db",
    "db_path",
    type=click.Path(
        exists=False,
        file_okay=False,
        dir_okay=True,
        resolve_path=True,
        readable=True,
        writable=True,
        path_type=Path,
    ),
    metavar="DIR",
    required=False,
    help="Specify manifest database path.",
)
@click.option(
    "-r",
    "--release-uuid",
    "release_uuid",
    type=str,
    metavar="UUID",
    required=False,
    help="Specify release UUID to use.",
)
@click.option(
    "--github-token",
    type=str,
    metavar="TOKEN",
    envvar="GITHUB_TOKEN",
    required=False,
    help="Specify GitHub Token to use.",
)
@pass_ctx
def main(
    ctx: Ctx, db_path: Path | None, release_uuid: str | None, github_token: str | None
) -> None:
    if db_path:
        ctx.db_path = db_path
    ctx.db_path.mkdir(exist_ok=True)

    if release_uuid:
        ctx.release_uuid = uuid.UUID(release_uuid)

    ctx.github_token = github_token

    click.echo(f"releases db path: {ctx.db_path}")
    click.echo(f"  manifests path: {ctx.db.manifests_path}")
    click.echo(f" patch sets path: {ctx.db.patchsets_path}")
    click.echo(f"    patches path: {ctx.db.patches_path}")
    click.echo(f"has github token: {github_token is not None}")


@main.group("manifest", help="Manifest operations.")
def cmd_manifest() -> None:
    pass


def _gen_manifest_header(manifest: ReleaseManifest) -> str:
    return f"""           name: {manifest.name}
   base release: {manifest.base_release_name}
base repository: {manifest.base_ref_org}/{manifest.base_ref_repo}
       base ref: {manifest.base_ref}
  creation date: {manifest.creation_date}
  manifest uuid: {manifest.release_uuid}"""


def _get_manifests_from_uuids(
    db: ReleasesDB, uuid_lst: list[uuid.UUID]
) -> list[ReleaseManifest]:
    lst: list[ReleaseManifest] = []
    for entry in uuid_lst:
        try:
            manifest = db.load_manifest(entry)
        except NoSuchManifestError:
            click.echo(f"error: manifest uuid '{entry}' not found", err=True)
            continue
        except MalformedManifestError:
            click.echo(f"error: malformed manifest uuid '{entry}'", err=True)
            continue
        lst.append(manifest)

    return sorted(lst, key=lambda e: e.creation_date)


@cmd_manifest.command("create", help="Create a new release manifest.")
@click.argument("name", type=str, required=True, metavar="NAME")
@click.argument("base_release", type=str, required=True, metavar="BASE_RELEASE")
@click.argument("base_ref", type=str, required=True, metavar="[REPO@]REF")
@pass_ctx
def cmd_manifest_create(ctx: Ctx, name: str, base_release: str, base_ref: str) -> None:
    m = re.match(r"(?:(.+)@)?([\w\d_.-]+)", base_ref)
    if not m:
        click.echo("error: malformed BASE_REF")
        sys.exit(errno.EINVAL)

    base_repo_str = cast(str | None, m.group(1))
    base_ref_str = cast(str, m.group(2))
    if not base_repo_str:
        base_repo_str = "clyso/ceph"

    m = re.match(r"([\w\d_.-]+)/([\w\d_.-]+)", base_repo_str)
    if not m:
        click.echo("error: malformed REPO")
        sys.exit(errno.EINVAL)

    base_repo_org = cast(str, m.group(1))
    base_repo = cast(str, m.group(2))

    manifest = ReleaseManifest(
        name=name,
        base_release_name=base_release,
        base_ref_org=base_repo_org,
        base_ref_repo=base_repo,
        base_ref=base_ref_str,
    )

    manifest_path = ctx.db.manifests_path.joinpath(f"{manifest.release_uuid}.json")
    if manifest_path.exists():
        click.echo(
            "error: conflicting manifest UUID, "
            + f"'{manifest.release_uuid}' already exists",
            err=True,
        )
        sys.exit(errno.EEXIST)

    try:
        ctx.db.store_manifest(manifest)
    except Exception as e:
        click.echo(f"error: unable to write manifest to disk: {e}")
        sys.exit(errno.ENOTRECOVERABLE)

    click.echo(f"""
Manifest created
-----------------
{_gen_manifest_header(manifest)}

You can now modify this release using its UUID.
""")


@cmd_manifest.command("list", help="List existing release manifest.")
@pass_ctx
def cmd_manifest_list(ctx: Ctx) -> None:
    lst = _get_manifests_from_uuids(ctx.db, ctx.db.list_manifests_uuids())
    for manifest in lst:
        click.echo(f"""Manifest {manifest.release_uuid}
----------------------------------------------
{_gen_manifest_header(manifest)}

""")


@cmd_manifest.command("info", help="Show information about release manifests.")
@click.option(
    "-m",
    "--manifest-uuid",
    required=False,
    type=uuid.UUID,
    metavar="UUID",
    help="Manifest UUID for which information will be shown.",
)
@pass_ctx
def cmd_manifest_info(ctx: Ctx, manifest_uuid: uuid.UUID | None) -> None:
    db = ctx.db

    manifest_uuids_lst = [manifest_uuid] if manifest_uuid else db.list_manifests_uuids()
    lst = _get_manifests_from_uuids(db, manifest_uuids_lst)

    for manifest in lst:
        click.echo(f"""Manifest {manifest.release_uuid}
----------------------------------------------
{_gen_manifest_header(manifest)}
""")
        click.echo("  Patch Sets:")
        # FIXME: don't assume just GitHub patch sets
        for patchset_uuid in manifest.patchsets:
            try:
                patchset = db.load_patchset(patchset_uuid)
            except (PatchSetError, Exception) as e:
                click.echo(
                    f"error: unable to load patch set uuid '{patchset_uuid}': {e}",
                    err=True,
                )
                sys.exit(errno.ENOTRECOVERABLE)

            click.echo(f"""    \u29bf {patchset.title}
      \u276f author: {patchset.author.user} <{patchset.author.email}>
      \u276f created: {patchset.creation_date}
      \u276f related: {patchset.related_to}""")

            if isinstance(patchset, GitHubPullRequest):
                click.echo(f"      \u276f repo: {patchset.repo_url}")
                click.echo(f"      \u276f pr id: {patchset.pull_request_id}")
                click.echo(f"      \u276f target: {patchset.target_branch}")
                click.echo(f"      \u276f merged: {patchset.merge_date}")

            click.echo("      \u276f patches:")
            for patch in patchset.patches:
                click.echo(f"        \u2022 {patch.title}")

        click.echo("\n  Patches:")
        for patch_uuid in manifest.patches:
            try:
                patch = db.load_patch(patch_uuid)
            except (PatchError, Exception) as e:
                click.echo(
                    f"error: unable to load patch uuid '{patch_uuid}': {e}", err=True
                )
                sys.exit(errno.ENOTRECOVERABLE)

            click.echo(f"""    \u29bf {patch.title}
      \u276f author: {patch.author.user} <{patch.author.email}>
      \u276f date: {patch.author_date}
      \u276f related: {patch.related_to}
      \u276f cherry-picked from: {patch.cherry_picked_from}""")

    pass


@main.group("patchset", help="Handle patch sets.")
def cmd_patchset() -> None:
    pass


@cmd_patchset.group("add", help="Add a patch set to a release.")
def cmd_patchset_add() -> None:
    pass


@cmd_patchset_add.command("gh", help="Add patch set from GitHub")
@click.argument(
    "pr_id",
    type=int,
    required=True,
    metavar="PR-ID",
)
@click.option(
    "-m",
    "--manifest-uuid",
    required=True,
    type=uuid.UUID,
    metavar="UUID",
    help="Manifest UUID to which the patch set should be added.",
)
@click.option(
    "-r",
    "--repo",
    required=False,
    type=str,
    metavar="ORG/REPO",
    default="ceph/ceph",
    help="Specify the repository to obtain patch set from (default: ceph/ceph).",
)
@pass_ctx
def cmd_patchset_add_gh(
    ctx: Ctx, pr_id: int, manifest_uuid: uuid.UUID, repo: str
) -> None:
    m = re.match(r"([\w\d_.-]+)/([\w\d_.-]+)", repo)
    if not m:
        click.echo("error: malformed ORG/REPO", err=True)
        sys.exit(errno.EINVAL)

    org = cast(str, m.group(1))
    repo_name = cast(str, m.group(2))

    db = ctx.db

    try:
        manifest = db.load_manifest(manifest_uuid)
    except NoSuchManifestError:
        click.echo(f"error: unable to find manifest '{manifest_uuid}' in db", err=True)
        sys.exit(errno.ENOENT)
    except MalformedManifestError:
        click.echo(f"error: malformed manifest '{manifest_uuid}'", err=True)
        sys.exit(errno.EINVAL)
    except Exception as e:
        click.echo(f"error: unable to obtain manifest '{manifest_uuid}': {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    patchset: GitHubPullRequest | None = None
    try:
        patchset = db.load_gh_pr(org, repo_name, pr_id)
    except NoSuchPatchSetError:
        click.echo("patch set not found, obtain from github")
    except PatchSetError as e:
        click.echo(f"error: unable to obtain patch set: {e}")
        sys.exit(errno.ENOTRECOVERABLE)
    except Exception as e:
        click.echo(f"error: {e}", err=True)
        sys.exit(errno.ENOTRECOVERABLE)

    if not patchset:
        patchset = gh_get_pr(org, repo_name, pr_id, token=ctx.github_token)
        click.echo(f"patchset:\n{patchset}")

        try:
            db.store_gh_patchset(patchset)
        except Exception as e:
            click.echo(
                f"error: unable to write patch set '{patchset.patchset_uuid}' "
                + f"to disk: {e}",
                err=True,
            )
            sys.exit(errno.ENOTRECOVERABLE)

    added, patches_added, patches_skipped = manifest.add_patchset(patchset)
    if not added:
        click.echo(
            f"patch set '{patchset.patchset_uuid}' already exists in release manifest"
        )
        return

    for patch in patchset.patches:
        if patch in patches_added:
            click.echo(
                f"added patch sha '{patch.sha}' uuid '{patch.patch_uuid}' "
                + f"title '{patch.title}' to release manifest"
            )
        elif patch in patches_skipped:
            click.echo(
                f"skipped existing patch sha '{patch.sha}' "
                + f"uuid '{patch.patch_uuid}' title '{patch.title}'"
            )
        else:
            click.echo(
                f"error: missing patch '{patch.patch_uuid}' from manifest!", err=True
            )
            sys.exit(errno.ENOTRECOVERABLE)

    try:
        db.store_manifest(manifest)
    except Exception as e:
        click.echo(f"error: unable to write manifest '{manifest_uuid}' to db: {e}")
        sys.exit(errno.ENOTRECOVERABLE)


if __name__ == "__main__":
    main()
