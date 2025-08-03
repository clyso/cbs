# Ceph Release Tool (CRT)

The Ceph Release Tool (CRT) helps handling and maintaining the Ceph component of
CES releases. Through this tool, we can logically create a release, and manage
its patches. A branch representing the release state can be created for
building.

**NOTE:** quite a few operations are still in flux, some are actively broken. We
will be improving the tool piecemeal.

## Requirements

The following options will be required to be provided to CRT at runtime:

- `--github-token TOKEN`, a personal access token for GitHub
- `--vault-addr ADDR`, specifying the address of the CES vault instance
- `--vault-role-id ROLE-ID`, specifying the role ID to use to access the CES
  vault instance
- `--vault-secret-id SECRET-ID`, specifying the secret ID to use to access the
  CES vault instance
- `--secrets-path FILE`, the file containing the secrets mapping to vault

To make things easier, these can all be specified as environment variables, such
as:

```bash
VAULT_ADDR="https://dev.vault.clyso.cloud"
VAULT_ROLE_ID="foobar"
VAULT_SECRET_ID="barbaz"

GITHUB_TOKEN="my-gh-token"
CES_SECRETS_PATH="/a/b/ces.git/versions/secrets.json"

export VAULT_ADDR VAULT_ROLE_ID VAULT_SECRET_ID \
  GITHUB_TOKEN CES_SECRETS_PATH
```

## Concepts

### Patches repository

We maintain the state for all releases in a git repository. For official CES
purposes, the patches repository can be found at
[clyso/ces-patches](https://github.com/clyso/ces-patches.git).

The patches repository follows a specific hierarchical structure:

- Individual patches and patch sets go into `ceph/patches/`
- References from GitHub pull requests to their corresponding patch sets are
  maintained in `ceph/patches/{owner}/{repo}/{id}`
- Metadata for each patch or patch set is kept under `ceph/patches/meta/`

Additionally, CES releases are kept under `ces/`, and follow a tree hierarchy
according to the release version. E.g., for `ces-v25.03.2` we may find a
hierarchy such as:

- `ces/ces-v25.03/ces-v25.03.2/ces-v25.03.2-rc.1/`
- `ces/ces-v25.03/ces-v25.03.2/ces-v25.03.2-rc.2/`
- `ces/ces-v25.03/ces-v25.03.2/ces-v25.03.2-rc.3-dev.1/`

Each release directory under `ces/` will contain symbolic links to its
corresponding patches. The patches themselves will be under `ceph/patches/`.
Patches will be enumerated according to their position within the release's
manifest's stages. A later stage will inevitably include all patches that come
before it as well -- i.e., `ces-v25.03.2-rc.2` will contain the patches for
`rc.2` plus the patches for `rc.1`.

The `ces/` directory hierarchy is provided mostly for human consumption. Patches
manually applied from these directories will result in the same state as the one
resulting from using CRT. However, CRT does not apply patches from the `ces/`
directory when building a release branch.

### Release Manifest

Each release is defined by a manifest. For instance, the `v25.03.2` CES release
has a manifest defining its life cycle. We should aim to keeping it unique, but
exceptions may be opened if we want to release out-of-band updates to
`v25.03.2`. **This is not currently supported**, and creating a duplicate
manifest for a given release is likely to break the state.

A release manifest is split into _Stages_, which define a point in time for
changes for a given release.

### Stages

A stage contains patches and patch sets. It should also contain tags that
represent what the stage refers to. The patch state will be represented
according to the release's corresponding stages and the patches it contains.

For instance, while developing the `v25.03.2` release, we will create several
stages. We can start with the first release candidate, `rc.1`, to which we will
add several patches. These patches will be verified as applying cleanly to the
final release branch automatically, as they are added. Once we are satisfied
with the state of the stage, we can commit it.

Committing a stage will ensure that the patch hierarchy for the given release
will be populated. A committed stage should not be changed, even though we do
allow some of it to be amended after the fact (e.g., author's name and email).

The resulting branch name will also reflect the stage it's generated for. If the
release's latest stage happens to be tagged with `rc.4 dev.1`, the resulting
branch will be named as `ces-v25.03.2-rc.1-dev.4`.

### Patches and Patch Sets

Stages contain patches and patch sets. The difference between the two is that
patches are individual patches, whereas patch sets include multiple patches.
Both are represented in the patches repository as a single patch file, but the
latter will include multiple patches instead of a single patch.

Patches and patch sets will be applied in the same sequence they were added to
the stage.

## Creating a release

A release starts by creating a manifest, such as

```shell
# crt/crt.py new \
  --patches-repo /a/b/ces-patches.git \
  --dst-repo clyso/ceph \
  ces-v25.03.2 reef ceph/ceph@v17.2.7
```

This command outputs the new release manifest's UUID. We will use if from now on
for all operations pertaining to this release. If needed, we can list all
existing manifests with `crt/crt.py list -p /a/b/ces-patches.git`. Information
about all manifests, or a specific manifest, can be obtained with
`crt/crt.py info [-m UUID] -p /a/b/ces-patches.git`.

## Adding patches to a release

To add patches to a release we first need to have an active stage -- the stage
which we're currently developing. If no active stage exists, we are not able to
add patches to a given release.

```shell
# crt/crt.py stage new \
  -p /a/b/ces-patches.git \
  -m cf3d61c1-bbee-4884-927a-6919812d17c6 \
  --author "Joao Eduardo Luis" \
  --email "joao@clyso.com" \
  -t rc=1
```

To verify the new stage has been created, we can run

```shell
# crt/crt.py info \
  -p /a/b/ces-patches.git \
  -m cf3d61c1-bbee-4884-927a-6919812d17c6 \
  --extended
```

Which will show all the information about a release manifest, including its
stages. Alternatively, we can also run

```shell
# crt/crt.py stage info \
  -p /a/b/ces-patches.git \
  -m cf3d61c1-bbee-4884-927a-6919812d17c6 \
  --extended
```

Which will show information about the manifest's stages. Specifying
`--stage UUID` will obtain information about a specific stage.

With the new stage created, we can start adding patches and patch sets.

### Adding a discrete patch

To add a single, discrete patch, we will use the `patch` commands.

```shell
# crt/crt.py patch add \
  --patches-repo /a/b/ces-patches.git \
  --ceph-repo /a/b/ceph.git \
  -m cf3d61c1-bbee-4884-927a-6919812d17c6 \
  d3749b0 09a6cab
```

This will add two patches (with SHAs `d3749b0` and `09a6cab`) to the specified
release manifest. The provided `--ceph-repo` path refers to the repository where
the release branches will be built. In this case, it is assumed provided SHAs
will be known within the context of this repository. However, we can
alternatively specify `--src-ceph-repo PATH` if the patches are living in a
different repository.

This command also accepts a `--src-gh-repo OWNER/NAME`, should the patch come
from a specific upstream GitHub repository, and a `--src-version NAME`, which,
albeit optional, is definitely encouraged (e.g., `reef`).

### Adding a patch set

For now, we only support GitHub patch sets -- i.e., GitHub pull requests.

To add a GitHub pull request, we rely on the `patchset` command.

```shell
# crt/crt.py patchset add \
  -p /a/b/ces-patches.git \
  -c /a/b/ceph.git \
  --from-gh 1234 \
  --from-gh-repo clyso/ceph \
  -m cf3d61c1-bbee-4884-927a-6919812d17c6
```

This command will fetch the specified pull request (`#1234`) from the
`clyso/ceph` repository on GitHub. The set of patches will then be processed and
a single `.patch` file containing all of them will be created. If the patch set
is cleanly applied to the release branch, the patch set is added to the
manifest. Otherwise, an error will be raised to the user.

## Publishing the release

**Note:** this is still under active development. Automatically publishing
releases is not currently supported, and is thus involves manual steps.

### Commit the stage

The currently active stage will need to be committed. We assume patches have
been added to the stage, otherwise the tool will refuse to perform the
operation.

```shell
# crt/crt.py stage commit \
  -p /a/b/ces-patches.git \
  -m cf3d61c1-bbee-4884-927a-6919812d17c6
```

This will create the patch hierarchy [previously discussed](#concepts).

This is the right time to commit the changes in the CES patches repository. This
is currently a manual step.

### Validate the release

For now, building the release branch is performed by validating the release
without cleaning up afterwards.

This is done by running the following:

```shell
# crt/crt.py validate \
  -p /a/b/ces-patches.git \
  -c /a/b/ceph.git \
  --no-cleanup
  cf3d61c1-bbee-4884-927a-6919812d17c6
```

A branch such as `ces-v25.03.2-rc.4-msLbyF-exec-20250803T123911` will be
created. This is the branch that should be manually pushed to the ceph git
repository from which branches are built.
