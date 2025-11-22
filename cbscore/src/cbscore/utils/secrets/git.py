# CES library - secrets utilities - secrets manager (git)
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

import random
import re
import string
import subprocess
from collections.abc import Generator
from contextlib import contextmanager
from pathlib import Path
from typing import cast

from cbscore.utils import MaybeSecure, Password, SecureURL
from cbscore.utils.secrets import SecretsMgrError
from cbscore.utils.secrets import logger as parent_logger
from cbscore.utils.secrets.models import (
    GitHTTPSSecret,
    GitSecret,
    GitSSHSecret,
    GitTokenSecret,
    GitVaultHTTPSSecret,
    GitVaultSSHSecret,
)
from cbscore.utils.secrets.utils import find_best_secret_candidate
from cbscore.utils.vault import Vault, VaultError

logger = parent_logger.getChild("git")


# github copilot generated pattern to validate git urls, for either file access,
# or http/https and ssh with optional username/password, or ssh key
GIT_URL_PATTERN = re.compile(
    r"""
    # File protocol: file:///path/to/repo(.git)?
    (file:///(?P<file_path>(?:[\w\-/]+/)*[\w\-]+(?:\.git)?)) |

    # Git protocol: git://host.xz[:port]/path/to/repo(.git)?
    (git://
        (?P<git_host>[\w\.\-]+)
        (:(?P<git_port>\d+))?
        /(?P<git_path>(?:[\w\-/]+/)*[\w\-]+(?:\.git)?)
    ) |

    # HTTP/HTTPS/SSH protocol: [user[:password]@]host.xz[:port]/path/to/repo(.git)?
    ((?P<http_protocol>https?|ssh)://
        (?:(?P<user>[\w\-\.]+)
            (?::(?P<password>[^@]+))?
            @
        )?
        (?P<http_host>[\w\.\-]+)
        (:(?P<http_port>\d+))?
        /(?P<http_path>(?:[\w\-/]+/)*[\w\-]+(?:\.git)?)
    )
    """,
    re.VERBOSE,
)


@contextmanager
def _ssh_git_url_for(
    url: str,
    entry: GitSSHSecret | GitVaultSSHSecret,
    vault: Vault | None,
) -> Generator[str]:
    """Obtain URL for an SSH-based git access, either local or from vault."""
    homedir = Path.home()
    if not homedir.exists():
        msg = f"cannot determine home directory for ssh key git secret for url '{url}'"
        logger.error(msg)
        raise SecretsMgrError(msg)
    ssh_config_dir = homedir / ".ssh"
    ssh_config_dir.mkdir(mode=0o700, exist_ok=True)

    remote_name = "".join(random.choices(string.ascii_letters, k=10))  # noqa: S311
    m = re.match(GIT_URL_PATTERN, url)
    if not m:
        msg = f"cannot parse git url '{url}' for ssh key git secret"
        logger.error(msg)
        raise SecretsMgrError(msg)

    ssh_host = cast(str, m.group("http_host"))
    ssh_port = int(m.group("http_port")) if m.group("http_port") else 22
    target_repo = cast(str, m.group("http_path"))

    # obtain target host key, stash it
    try:
        p = subprocess.run(  # noqa: S603
            ["ssh-keyscan", "-t", "rsa", ssh_host],  # noqa: S607
            capture_output=True,
        )
    except Exception as e:
        msg = f"error running ssh-keyscan for git url '{ssh_host}': {e}"
        logger.error(msg)
        raise SecretsMgrError(msg) from e

    if p.returncode != 0 or not p.stdout:
        msg = (
            f"error obtaining ssh host key for git url '{ssh_host}': "
            + f"{p.stderr.decode('utf-8').strip()}"
        )
        logger.error(msg)
        raise SecretsMgrError(msg)

    with ssh_config_dir.joinpath("known_hosts").open("a") as f:
        _ = f.write(p.stdout.decode("utf-8"))

    if isinstance(entry, GitSSHSecret):
        ssh_key = entry.ssh_key
        ssh_username = entry.username
    else:  # GitVaultSSHSecret
        if not vault:
            msg = f"no vault configured for git vault ssh secret for git url '{url}'"
            logger.error(msg)
            raise SecretsMgrError(msg)

        try:
            ssh_secret = vault.read_secret(entry.key)
        except VaultError as e:
            msg = f"error obtaining ssh key from vault for git url '{url}': {e}"
            logger.error(msg)
            raise SecretsMgrError(msg) from e

        try:
            ssh_key = ssh_secret[entry.ssh_key].rstrip()
            ssh_username = ssh_secret[entry.username].rstrip()
        except KeyError as e:
            msg = f"missing field in vault secret for git url '{url}'"
            logger.error(msg)
            raise SecretsMgrError(msg) from e

    ssh_key_path = ssh_config_dir / f"id_{remote_name}"
    with ssh_key_path.open("w") as f:
        _ = f.write(ssh_key)
        _ = f.write("\n")
    ssh_key_path.chmod(0o600)
    ssh_host_config = f"""
Host {remote_name}
Hostname {ssh_host}
User {ssh_username}
Port {ssh_port}
IdentityFile {ssh_key_path.as_posix()}

"""

    ssh_config_path = ssh_config_dir / "config"
    with ssh_config_path.open("a") as f:
        _ = f.write(ssh_host_config)

    yield f"{remote_name}:{target_repo}"

    ssh_key_path.unlink(missing_ok=True)


def _https_git_url_for(
    url: str, entry: GitHTTPSSecret | GitVaultHTTPSSecret, vault: Vault | None
) -> MaybeSecure:
    """Obtain URL for an HTTPS-based git access, either local or from vault."""
    m = re.match(GIT_URL_PATTERN, url)
    if not m:
        msg = f"cannot parse git url '{url}' for https git secret"
        logger.error(msg)
        raise SecretsMgrError(msg)

    https_host = cast(str, m.group("http_host"))
    https_port = cast(str, m.group("http_port")) if m.group("http_port") else ""
    http_path = cast(str, m.group("http_path"))

    if isinstance(entry, GitHTTPSSecret):
        username = entry.username
        password = entry.password

    else:  # GitVaultHTTPSSecret
        if not vault:
            msg = f"no vault configured for git vault https secret for git url '{url}'"
            logger.error(msg)
            raise SecretsMgrError(msg)

        try:
            https_secret = vault.read_secret(entry.key)
        except VaultError as e:
            msg = f"error obtaining https creds from vault for git url '{url}': {e}"
            logger.error(msg)
            raise SecretsMgrError(msg) from e

        try:
            username = https_secret[entry.username].rstrip()
            password = https_secret[entry.password].rstrip()
        except KeyError as e:
            msg = f"missing field in vault secret for git url '{url}'"
            logger.error(msg)
            raise SecretsMgrError(msg) from e

    return SecureURL(
        "https://{username}:{password}@{host_with_port}/{path}",
        username=username,
        password=Password(password),
        host_with_port=f"{https_host}{':' + https_port if https_port else ''}",
        path=http_path,
    )


def _token_git_url_for(url: str, entry: GitTokenSecret) -> MaybeSecure:
    """Obtain URL for a token-based git access."""
    m = re.match(GIT_URL_PATTERN, url)
    if not m:
        msg = f"cannot parse git url '{url}' for token git secret"
        logger.error(msg)
        raise SecretsMgrError(msg)

    https_host = cast(str, m.group("http_host"))
    https_port = cast(str, m.group("http_port")) if m.group("http_port") else ""
    http_path = cast(str, m.group("http_path"))

    return SecureURL(
        "https://{username}:{token}@{host_with_port}/{path}",
        username=entry.username,
        token=Password(entry.token),
        host_with_port=f"{https_host}{':' + https_port if https_port else ''}",
        path=http_path,
    )


@contextmanager
def git_url_for(
    url: str, secrets: dict[str, GitSecret], vault: Vault | None
) -> Generator[MaybeSecure]:
    """Obtain URL for git access."""
    url_m = re.match(GIT_URL_PATTERN, url)
    if not url_m:
        msg = f"invalid git url '{url}'"
        logger.error(msg)
        raise SecretsMgrError(msg)

    best_entry = find_best_secret_candidate(list(secrets.keys()), url)
    if not best_entry:
        m = re.match(GIT_URL_PATTERN, url)
        if not m:
            msg = f"no git secret found for url '{url}'"
            logger.warning(msg)
            raise SecretsMgrError(msg)

        yield SecureURL(url)
        return

    entry = secrets[best_entry]
    if isinstance(entry, GitSSHSecret | GitVaultSSHSecret):
        with _ssh_git_url_for(url, entry, vault) as ssh_url:
            yield ssh_url
    elif isinstance(entry, GitHTTPSSecret | GitVaultHTTPSSecret):
        yield _https_git_url_for(url, entry, vault)
    else:  # GitTokenSecret
        assert isinstance(entry, GitTokenSecret)
        yield _token_git_url_for(url, entry)


#
# kludge to test git patterns.
#
check_mark = "\u2714"  # ✔
error_mark = "\u274c"  # ❌


def _test_git_uri_patterns():
    # most test uris generated by github copilot
    test_uris = [
        # Valid URIs (should match)
        ("file:///home/user/repo.git", True),
        ("file:///home/user/repo", True),
        ("git://github.com/user/repo.git", True),
        ("git://github.com/user/repo", True),
        ("https://github.com/user/repo.git", True),
        ("https://github.com/user/repo", True),
        ("http://user:pass@github.com/user/repo.git", True),
        ("http://user@github.com/user/repo", True),
        ("ssh://user@host.xz:22/path/to/repo.git", True),
        ("ssh://host.xz/path/to/repo", True),
        ("ssh://user:pass@host.xz:22/path/to/repo.git", True),
        # Invalid URIs (should not match)
        ("file://home/user/repo.git", False),  # missing third slash
        ("git:/github.com/user/repo.git", False),  # missing one slash
        ("https:/github.com/user/repo", False),  # missing one slash
        ("ftp://github.com/user/repo.git", False),  # unsupported protocol
        ("git://github.com/.git", False),  # missing repo name before .git
    ]

    for uri in test_uris:
        match = GIT_URL_PATTERN.match(uri[0])
        if (match is not None) != uri[1]:
            print(f"{error_mark} {uri[0]}")
        else:
            print(f"{check_mark} {uri[0]}")


def _test_git_uri_groups():
    m = re.match(GIT_URL_PATTERN, "file:///home/user/repo.git")
    groups = m.groupdict() if m else {}
    if "file_path" in groups and groups["file_path"] == "home/user/repo.git":
        print(f"{check_mark} file URI groups")
    else:
        print(f"{error_mark} file URI groups")

    m = re.match(GIT_URL_PATTERN, "git://github.com/user/repo.git")
    groups = m.groupdict() if m else {}
    if "git_host" in groups and groups["git_host"] == "github.com":
        print(f"{check_mark} git URI groups")
    else:
        print(f"{error_mark} git URI groups")

    m = re.match(GIT_URL_PATTERN, "https://user:pass@github.com/user/repo")
    groups = m.groupdict() if m else {}
    if (
        "http_protocol" in groups
        and groups["http_protocol"] == "https"
        and "user" in groups
        and groups["user"] == "user"
        and "password" in groups
        and groups["password"] == "pass"  # noqa: S105
    ):
        print(f"{check_mark} http URI groups")
    else:
        print(f"{error_mark} http URI groups")

    m = re.match(GIT_URL_PATTERN, "ssh://user@host.xz:22/path/to/repo.git")
    groups = m.groupdict() if m else {}
    if "http_protocol" in groups and "user" in groups and groups["user"] == "user":
        print(f"{check_mark} ssh URI groups")
    else:
        print(f"{error_mark} ssh URI groups")


if __name__ == "__main__":
    print("test git url patterns:")
    _test_git_uri_patterns()

    print("\ncheck git url groups:")
    _test_git_uri_groups()
