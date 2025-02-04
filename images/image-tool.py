#!/usr/bin/env python3

# pyright: reportUnknownMemberType=false
# pyright: reportUnknownVariableType=false
# pyright: reportExplicitAny=false
# pyright: reportUnknownArgumentType=false

import logging
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Any, override

import click
import hvac
import hvac.exceptions
import pydantic

ourdir = os.path.dirname(os.path.realpath(__file__))

logging.basicConfig(level=logging.INFO)
log = logging.getLogger("images")


class OurError(Exception):
    pass


class MalformedVersionError(OurError):
    @override
    def __str__(self) -> str:
        return "malformed version"


class NoSuchVersionError(OurError):
    @override
    def __str__(self) -> str:
        return "no such version"


class UnknownRepositoryError(OurError):
    repo: str

    def __init__(self, repo: str) -> None:
        super().__init__()
        self.repo = repo

    @override
    def __str__(self) -> str:
        return f"unknown repository: {self.repo}"


class AuthError(OurError):
    msg: str | None

    def __init__(self, msg: str | None) -> None:
        super().__init__()
        self.msg = msg

    @override
    def __str__(self) -> str:
        return "authentication error" + ("" if self.msg is None else f": {self.msg}")


class DescImage(pydantic.BaseModel):
    src: str
    dst: str


class Desc(pydantic.BaseModel):
    releases: list[str]
    images: list[DescImage]


class SkopeoTagListResult(pydantic.BaseModel):
    repository: str = pydantic.Field(alias="Repository")
    tags: list[str] = pydantic.Field(alias="Tags")


class AuthAndSignInfo:
    harbor_username: str
    harbor_password: str
    vault_addr: str
    vault_transit: str

    vault_client: hvac.Client

    def __init__(
        self,
        vault_addr: str,
        vault_role_id: str,
        vault_secret_id: str,
        vault_transit: str,
    ) -> None:
        self.vault_addr = vault_addr
        if self.vault_addr == "":
            raise AuthError("missing vault address")
        if vault_role_id == "":
            raise AuthError("missing vault role id")
        if vault_secret_id == "":
            raise AuthError("missing vault secret id")
        self.vault_transit = vault_transit
        if self.vault_transit == "":
            raise AuthError("missing vault transit")

        self.vault_login(vault_role_id, vault_secret_id)

    def vault_login(self, role_id: str, secret_id: str) -> None:
        self.vault_client = hvac.Client(url=self.vault_addr)

        try:
            self.vault_client.auth.approle.login(
                role_id=role_id,
                secret_id=secret_id,
                use_token=True,
            )
            log.info("logged in to vault")
        except hvac.exceptions.Forbidden:
            raise AuthError("permission denied logging in to vault")
        except Exception:
            raise AuthError("error logging in to vault")

        try:
            res: dict[str, Any] = self.vault_client.secrets.kv.v2.read_secret_version(
                path="creds/harbor.clyso.com:ces-build/ces-build-bot",
                mount_point="ces-kv",
                raise_on_deleted_version=False,
            )
            log.info("obtained harbor credentials from vault")
        except hvac.exceptions.Forbidden:
            raise AuthError("permission denied while obtainining harbor credentials")
        except Exception:
            raise AuthError("error obtaining harbor credentials")

        try:
            self.harbor_username = res["data"]["data"]["username"]
            self.harbor_password = res["data"]["data"]["password"]
        except KeyError as e:
            raise AuthError(f"missing key in harbor credentials: {e}")

        log.debug(
            f"harbor credentials: username = {self.harbor_username}, "
            + f"password = {self.harbor_password}"
        )

    @property
    def vault_token(self) -> str:
        return self.vault_client.token


def _get_version_desc(version: str) -> Desc:
    m = re.match(r"(\d+\.\d+\.\d+).*", version)
    if m is None:
        raise MalformedVersionError()

    candidates: list[Path] = []

    def _file_matches(f: str) -> bool:
        return f.startswith(f"ces-{m[1]}") and f.endswith(".json")

    def _gen_candidates(base_path: Path, files: list[str]) -> list[Path]:
        return [base_path.joinpath(f) for f in files if _file_matches(f)]

    desc_path = Path(ourdir).joinpath("desc")
    for cur_path, dirs, file_lst in desc_path.walk(top_down=True):
        log.debug(f"path: {cur_path}, dirs: {dirs}, files: {file_lst}")
        candidates.extend(_gen_candidates(cur_path, file_lst))

    log.debug(f"candidates: {candidates}")

    ces_version = f"ces-v{version}"

    desc: Desc | None = None
    found_at: Path | None = None
    for candidate in candidates:
        try:
            desc_raw = candidate.read_text()
            desc = Desc.model_validate_json(desc_raw)
        except Exception as e:
            log.debug(f"error loading desc file: {e}")
            raise e

        if ces_version in desc.releases:
            if found_at is not None:
                log.error(
                    f"error: potential conflict for version {ces_version} "
                    + f"between {found_at} and {candidate}"
                )
                raise OurError()
            found_at = candidate
            desc = desc
            log.debug(f"found candidate at {found_at}")

    if found_at is not None:
        assert desc is not None
        return desc

    raise NoSuchVersionError()


def _get_image(img: str) -> str:
    idx = img.find(":")
    return img[:idx] if idx > 0 else img


def _get_image_tag(img: str) -> str | None:
    idx = img.find(":")
    if idx > 0:
        tag = img[idx + 1 :]
    else:
        return None

    return tag if tag != "" else None


def run_cmd(cmd: list[str], env: dict[str, str] | None = None) -> tuple[int, str, str]:
    try:
        p = subprocess.run(cmd, env=env, capture_output=True)
    except OSError as e:
        log.error(f"error running '{cmd}': {e}")
        raise OurError()

    if p.returncode != 0:
        log.error(f"error running '{cmd}': retcode = {p.returncode}, res: {p.stderr}")
        return (p.returncode, "", p.stderr.decode("utf-8"))

    return (0, p.stdout.decode("utf-8"), p.stderr.decode("utf-8"))


def skopeo(args: list[str]) -> tuple[int, str, str]:
    cmd = ["skopeo"] + args
    return run_cmd(cmd)


def skopeo_get_tags(img: str) -> SkopeoTagListResult:
    img_base = _get_image(img)
    try:
        retcode, raw_out, err = skopeo(["list-tags", f"docker://{img_base}"])
    except OurError as e:
        log.error(f"error obtaining image tags for {img_base}")
        raise e

    if retcode != 0:
        m = re.match(r".*repository.*not found.*", err)
        if m is not None:
            raise UnknownRepositoryError(img_base)
        raise OurError()

    try:
        return SkopeoTagListResult.model_validate_json(raw_out)
    except pydantic.ValidationError as e:
        log.error(f"unable to parse resulting images list: {e}")
        raise OurError()


def sign(img: str, auth_info: AuthAndSignInfo) -> tuple[int, str, str]:
    cmd = [
        "cosign",
        "sign",
        "--key=hashivault://container-image-key",
        f"--registry-username={auth_info.harbor_username}",
        f"--registry-password={auth_info.harbor_password}",
        "--tlog-upload=false",
        "--upload=true",
        img,
    ]
    env = os.environ.copy()
    env.update(
        {
            "VAULT_ADDR": auth_info.vault_addr,
            "VAULT_TOKEN": auth_info.vault_token,
            "TRANSIT_SECRET_ENGINE_PATH": auth_info.vault_transit,
        }
    )
    return run_cmd(cmd, env=env)


def skopeo_copy(src: str, dst: str, auth_info: AuthAndSignInfo) -> None:
    log.info(f"copy '{src}' to '{dst}'")
    try:
        retcode, _, err = skopeo(
            [
                "copy",
                "--dest-creds",
                f"{auth_info.harbor_username}:{auth_info.harbor_password}",
                f"docker://{src}",
                f"docker://{dst}",
            ]
        )
    except OurError as e:
        log.error(f"error copying images: {e}")
        raise e

    if retcode != 0:
        log.error(f"error copying images: {err}")
        raise OurError()

    log.info(f"copied '{src}' to '{dst}'")

    try:
        retcode, out, err = sign(dst, auth_info)
    except OurError as e:
        log.error(f"error signing image '{dst}': {e}")
        raise e

    if retcode != 0:
        log.error(f"error signing image '{dst}': {err}")
        raise OurError()

    log.info(f"signed image '{dst}': {out}")


@click.group()
@click.option("-d", "--debug", is_flag=True)
def main(debug: bool) -> None:
    if debug:
        log.setLevel(logging.DEBUG)
    pass


@main.command()
@click.argument("version", type=str)
@click.option("-f", "--force", is_flag=True, default=False)
@click.option("--vault-addr", envvar="VAULT_ADDR", type=str, required=True)
@click.option("--vault-role-id", envvar="VAULT_ROLE_ID", type=str, required=True)
@click.option("--vault-secret-id", envvar="VAULT_SECRET_ID", type=str, required=True)
@click.option("--vault-transit", envvar="VAULT_TRANSIT", type=str, required=True)
def sync(
    version: str,
    force: bool,
    vault_addr: str,
    vault_role_id: str,
    vault_secret_id: str,
    vault_transit: str,
) -> None:
    try:
        desc = _get_version_desc(version)
    except OurError as e:
        click.echo(f"error: {e}")
        sys.exit(1)

    log.debug(f"desc: {desc}")

    for image in desc.images:
        src_tag = _get_image_tag(image.src)
        dst_tag = _get_image_tag(image.dst)

        if src_tag is None:
            log.error(f"missing tag for source image '{image.src}'")
            sys.exit(1)
        if dst_tag is None:
            log.info(f"missing tag for dest image '{image.dst}', assume '{src_tag}'")
            dst_tag = src_tag

        try:
            res_src = skopeo_get_tags(image.src)
        except UnknownRepositoryError as e:
            log.error(f"unable to obtain information for source repository: {e}")
            sys.exit(1)
        except Exception as e:
            log.error(f"unknown error: {e}")
            sys.exit(1)

        missing_dst_repo = False
        res_dst: SkopeoTagListResult | None = None
        try:
            res_dst = skopeo_get_tags(image.dst)
        except UnknownRepositoryError:
            missing_dst_repo = True
        except Exception as e:
            log.error(f"unknown error: {e}")
            sys.exit(1)

        if src_tag not in res_src.tags:
            log.error(f"error: missing source tag '{src_tag}' for '{image.src}'")
            raise OurError()

        if not missing_dst_repo and not force:
            assert res_dst is not None
            if dst_tag in res_dst.tags:
                log.info(f"nothing to do for tag '{dst_tag}' for '{image.dst}'")
                continue

        log.info(f"copying '{image.src}' to '{image.dst}")
        try:
            auth_info = AuthAndSignInfo(
                vault_addr, vault_role_id, vault_secret_id, vault_transit
            )
            skopeo_copy(image.src, image.dst, auth_info)
        except AuthError as e:
            log.error(f"authentication error: {e}")
            sys.exit(1)
        except OurError as e:
            log.error(f"error copying images: {e}")
            sys.exit(1)
        except Exception as e:
            log.error(f"unknown error: {e}")
            sys.exit(1)

        log.info(f"copied image from '{image.src}' to '{image.dst}'")


@main.command()
def verify() -> None:
    pass


if __name__ == "__main__":
    main()
