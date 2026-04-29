from pathlib import Path

from click.testing import CliRunner
from crt.cmds.crt import cmd_crt
from crt.crtlib.models.release import Release
from crt.crtlib.paths import patch_meta_dir
from crt.crtlib.release import store_release


def _write_config(store: Path, *, component: str = "rados") -> None:
    _ = store.joinpath("crt.config.yaml").write_text(
        f"""
component: {component}
namespaces:
  enterprise:
    description: Enterprise releases
    channels:
      ces:
        description: CES channel
        release_repo: clyso/ceph
        branding:
          product_name: Clyso Enterprise Storage
          short_name: CES
          vendor: Clyso
  community:
    description: Community releases
    channels:
      ccs:
        description: CCS channel
        release_repo: clyso/ceph-community
        branding:
          product_name: Clyso Community Storage
          short_name: CCS
          vendor: Clyso
""".strip(),
        encoding="utf-8",
    )


def _store_release(store: Path, name: str = "ces-v25.03.3") -> Release:
    release = Release(
        name=name,
        base_release_name="v19.2.1",
        base_release_ref="main",
        base_repo="ceph/ceph",
        release_repo="clyso/ceph",
        release_base_branch=f"release-base/{name}",
        release_base_tag=f"release-base-{name}",
        release_branch=f"release/{name}",
        store_branch=f"release/enterprise/{name}",
    )
    store_release(store, "enterprise", "ces", release)
    return release


def test_paths_use_configured_component(tmp_path: Path) -> None:
    _write_config(tmp_path, component="rados")

    assert patch_meta_dir(tmp_path) == tmp_path / "rados" / "patches" / "meta"


def test_release_list_reads_local_registry_without_token(tmp_path: Path) -> None:
    _write_config(tmp_path)
    release = _store_release(tmp_path)

    result = CliRunner().invoke(cmd_crt, ["-p", str(tmp_path), "release", "list"])

    assert result.exit_code == 0, result.output
    assert release.name in result.output
    assert "unpublished" in result.output


def test_manifest_new_rejects_release_channel_mismatch(tmp_path: Path) -> None:
    _write_config(tmp_path)
    release = _store_release(tmp_path)

    result = CliRunner().invoke(
        cmd_crt,
        [
            "-p",
            str(tmp_path),
            "manifest",
            "new",
            "--release",
            release.name,
            "ccs-v25.03.3-dev.1",
        ],
    )

    assert result.exit_code != 0
    assert "resolves to community/ccs" in result.output
    assert "resolves to enterprise/ces" in result.output
