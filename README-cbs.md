# CES Build Service

## Prepare the environment

```shell
# uv sync --all-packages --no-dev
# source .venv/bin/activate
# cbs/ces-build.py ... or whatever
```

### Requirements

The CES Build Service and its various build-related tools rely on `podman` and
`buildah`. We expect these to be installed and available to be used in a
[rootless environment][_podman_rootless].

Additionally, Python 3.13 or greater is necessary.

[_podman_rootless]:
  https://github.com/containers/podman/blob/main/docs/tutorials/rootless_tutorial.md

## Creating a version to build

Version descriptors can be generated using the `versions.py` tool, found in
`cbs/versions.py`.

The `versions.py` tool will generate a `JSON` file, under `versions/`, with the
metadata required to build the specified version. This metadata include the
components to build, and their versions.

Creating a new version is as simple as running

```shell
$ cbs/versions.py create ces-v12.3.4 \
  -t rc=1 -t dev=2 \
  -c ceph@adf123

version: ces-v12.3.4-rc.1-dev.2
version title: Release ces-v12.3.4, Release Candidate #1 Development release #2
... JSON ...
written to .../ces.git/versions/testing/ces-v12.3.4-rc.1-dev.2.json
```

This will be the file descriptor to use when building `ces-v12.3.4-rc.1-dev.2`.
The tool accepts more options, but these will be the most often used ones:

- `-t | --type TYPE=N`: Specifies the type of the build and its iteration (e.g.,
  `ga` for General Availability, `rc` for Release Candidate, etc...).

- `-c | --component NAME@VERSION`: Specifies a component to be built, and the
  version at which to build said component.

## Local CES builds

### `ces-build.py` options

- `--secrets PATH`: Path to the secrets mapping file, located in
  `versions/secrets.json`.

- `--scratch-dir PATH`: Path to directory where all scratch artefacts will be
  created.

- `--scratch-containers-dir PATH`: Path to directory where generated container
  artefacts will be kept.

- `--components-dir PATH`: Path to directory where component descriptors can be
  found, located in `components/`.

- `--ccache-dir PATH`: Path to directory to be used for the compiler cache.

- `--containers-dir PATH`: Path to the directory where container descriptors can
  be found, located in `containers/`.

- `--vault-addr URL`: HTTPS address to the HashiCorp vault holding required
  credentials. Should be passed as an environment variable, as `VAULT_ADDR`.

- `--vault-role-id ROLE_ID`: HashiCorp's Vault Role ID to authenticate with.
  Should be passed as an environment variable, as `VAULT_ROLE_ID`.

- `--vault-secret-id SECRET_ID`: HashiCorp's Vault Secret ID to authenticate
  with. Should be passed as an environment variable, as `VAULT_SECRET_ID`.

- `--vault-transit NAME`: HashiCorp's Vault Transit to use for signing
  artefacts. Should be passed as environment variable, as `VAULT_TRANSIT`.

- `--upload / --no-upload`: Whether to upload resulting artefacts (i.e., RPMs)
  to Clyso's S3. Defaults to `True`.

### Running `ces-build.py`

The `ces-build.py` program takes one argument, expecting the path to a version
descriptor. If you haven't created one yet, please see how to do it in the
[corresponding section](#creating-a-version-to-build).

When running `ces-build.py`, we recommend having a given `env.sh` file where the
various Vault environment variables are defined, which should be sourced before
running the tool. While we plan to allow individual user credentials to be used
in the near future, at time of writing this hasn't been implemented yet, and
thus `approle` role and secret IDs are required. This file will generally look
like the following:

```bash
VAULT_ADDR="https://dev.vault.clyso.cloud"
VAULT_ROLE_ID="0b165ba5-8479-609f-1cae-7a78deadbeef"
VAULT_SECRET_ID="a8ddf000-ac00-88ec-07f5-e7f0beefdead"
VAULT_TRANSIT="ces-transit"

export VAULT_ADDR VAULT_ROLE_ID VAULT_SECRET_ID VAULT_TRANSIT
```

We highly recommend having a dedicated, high-throughput partition or disk to
hold the scratch directory and the `ccache` directory, although not necessarily
required.

To build a given version, the following can be used (please note the `-d` option
is used for debug output and can be omitted):

```shell
$ cbs/ces-build.py -d \
  build \
  --secrets ./versions/secrets.json \
  --scratch-dir /mnt/pci4-scratch/ces-scratch \
  --scratch-containers-dir /mnt/pci4-scratch/ces-scratch/containers \
  --components-dir ./components \
  --ccache-dir /mnt/pci4-scratch/ces-scratch/ccache \
  --containers-dir ./containers \
  ./versions/testing/ces-v12.3.4-rc.1-dev.2.json
```

The build process is fully automated, and will result in the following
artefacts:

- RPMs will be found at `${scratch_dir}/rpms`, split across the various
  components and their versions (e.g.,
  `${scratch_dir}/ceph/18.2.2-0-g531c0d11a1c`).

- The various components' repositories will also be found in `${scratch_dir}`.

- The resulting image will be in the local host's `podman` registry.

- The resulting will also have been pushed to Clyso's container registry at
  `harbor.clyso.com/ces/`.

- RPMs and release metadata artefacts will have been uploaded to Clyso's S3, in
  bucket `ces-packages`. Metadata will be under `ces-packages/releases/`, while
  RPMs will be under `ces-packages/${component}/` (e.g.,
  `ces-packages/ceph/rpm-19.2.2-1-gadd17dc4301/`).
