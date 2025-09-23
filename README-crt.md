# Ceph Release Tool (CRT)

The Ceph Release Tool (CRT) helps handling and maintaining the Ceph component of
CES releases. Through this tool, we can logically create a release, and manage
its patches. A branch representing the release state can be created for
building.

## Requirements

The following options will be required to be provided to CRT at runtime:

- `--github-token TOKEN`, a personal access token for GitHub
- `--patches-repo-path`, the path to the `ces-patches` git repository

Additionally, many commands take an additional `--ceph-repo-path` option,
referring to the repository where git operations over a Ceph repository will be
performed.

To make things easier, these can all be specified as environment variables, such
as:

```bash
CRT_GITHUB_TOKEN=ghp_yourtokenhere
CRT_CEPH_REPO_PATH=/path/to/ceph/repo
CRT_PATCHES_REPO_PATH=/path/to/patches/repo

export CRT_GITHUB_TOKEN CRT_CEPH_REPO_PATH CRT_PATCHES_REPO_PATH
```

We supply a `crt.env.example` file with the recommended environment variables.
We recommend renaming said file to `crt.env`, adjusting values as needed, and
sourcing it before running `crt` commands. Consuming these values from
environment variables significantly reduces the noise on the command line.

## Concepts

We rely on three main concepts when operating CRT: releases, manifests, and
patch sets. These are essentially metadata, kept in JSON files within the
patches repository. Except for patch sets, which, alongside its metadata, also
include the actual patches associated with said patch set.

Let us start bottom-up.

### Patch Sets

CRT understands two types of patch sets at this point in time: GitHub patch
sets, obtained from GitHub pull requests, and Custom patch sets, which are
created and defined by the user.

A patch set is a collection of one or more patches. These exist as independent
units, and are not associated with any specific release by themselves -- we'll
require manifests for that. They do, however, contain metadata that can tell us
from which release these patches were obtained, so we can decide whether they
are applicable to whatever release we are working on.

### Manifests

Manifests represent a given point in the release cycle. They aggregate patch
sets, and keep them in the order they were added to the manifest. It's the
manifest that represents how a given release branch will look like in the end.

A manifest has a defined base reference and repository, from which it starts,
and a corresponding destination branch and repository, onto which its results
will be published. These are always associated with a release. We don't define
the base and destination references or repositories for the manifest
individually -- these are defined on a per-release basis --, thus ensuring we
guarantee consistency across manifests for a given release, reducing margin for
error.

A given manifest will usually represent an iteration within a release cycle. For
example, a manifest may represent a release candidate for a given release. It
will contain a collection of patch sets, internally organized in stages, that
will be published for this specific release iteration. In the event we want
additional iterations, we can simply create a new manifest based on a previous
manifest, thus inheriting its stages and patch sets.

A manifest stage is nothing but a collection of patch sets that have been
organized in a bite-sized unit. We'll usually organize patch sets in stages of
differing levels of confidence. That way, should a given stage introduce a
regression or unwanted behavior, we can simply remove it in a subsequent
iteration. Manifests will contain one or more manifests, but a given manifest
will always require one active stage before patch sets can be added to it.

### Releases

A release is an iteration in the product development cycle.

These can be based off of a given upstream tag or branch, or a previous release
tag or branch. They will target a destination branch, most likely in
[`clyso/ceph`][_clyso_ceph].

Releases only contain metadata referring to where to find the base reference for
said release, and where to push it to once it's finished. Although manifests are
associated with releases, releases keep no references to manifests. The only
time a release cares about manifests is when the release is finalized and
published, given it will obtain the final release branch from the manifest being
marked as the final iteration.

## Repositories

CRT will operate over two git repositories: the Ceph repository, for branch and
patch related operations, and the Patches repository, for metadata operations
and where we will be keeping the actual patch sets to be applied to releases.

### Ceph repository

In order to obtain the various patches for the patch sets we will need to
populate a release, we need a local Ceph repository. This repository will
usually have two remotes: the upstream Ceph repository, at
[`ceph/ceph`][_ceph_ceph], and the Clyso Ceph repository at
[`clyso/ceph`][_clyso_ceph]. Given we will have to perform authenticated
requests with these repositories, we will require a GitHub Personal Access Token
-- this is one of the required parameters to CRT, and can be defined as an
environment variable.

Additionally, it is through this git repository that we will push branches and
tags for releases and their iterations. Pushing will target the
[`clyso/ceph`][_clyso_ceph] remote by default.

### Patches repository

We maintain the state for all releases in a git repository. For official CES
purposes, the patches repository can be found at
[clyso/ces-patches][_clyso_ces_patches].

The patches repository follows a specific hierarchical structure:

- Metadata and patches go under `ceph/`
- Published releases go under `ces/` (for Clyso Enterprise Storage), or `ccs/`
  (for Clyso Community Storage)
- Release notes will by default go under `release-notes/`

#### `ceph/` hierarchy and contents

Within the `ceph/` tree, we will find the following directories:

- `manifests/`, where all manifests will be stored by their UUIDs; these will
  also be stored by their names under `manifests/by_name/`, as symlinks to their
  UUID counterparts
- `releases/`, containing the various releases metadata JSON files, by name
- `patches/`, containing the various patch sets

The `patches` directory is the most heavily populated one. At its root, it will
contain all patch sets in their `.patch` form, individually named by their own
UUIDs. Under `patches/meta/` we will find each patch sets' metadata JSON files,
accordingly named using the patch sets' UUIDs. Additionally, we will have
directories for different GitHub repositories, so we can map GitHub pull
requests to their individual patch sets.

For example, for a given GitHub pull request from the upstream Ceph repository,
[`ceph/ceph#63711`](https://github.com/ceph/ceph/pull/63711), we will find its
reference in `patches/meta/ceph/ceph/63711/`. This directory will contain one or
more files named after the pull request's head commit's SHA, and a `latest`
symbolic link to the latest one. If a given pull request has been merged by the
time we obtain it, we will always see only one commit in this directory;
however, if the pull request has not been merged, and has been updated one or
more times between being added to different manifests, we will have multiple
files in this directory. Each one of these files will simply contain a UUID,
referring to the patch set UUID it corresponds to. This approach ensures that we
will maintain state for existing manifests, while keeping track of which pull
requests these patch sets belong to.

#### `ces/` and `ccs/` hierarchy and contents

Clyso Enterprise Storage (CES) and Clyso Community Storage (CCS) releases are
kept under `ces/` and `ccs/` respectively. They follow the same hierarchical
structure.

For example, for `ces-v99.99.1`, we may find the hierarchy to be:

- `ces/ces-v99/ces-99.99.1/ces-99.99/ces-v99.99.1-rc.1/`
- `ces/ces-v99/ces-99.99.1/ces-99.99/ces-v99.99.1-rc.2/`
- `ces/ces-v99/ces-99.99.1/ces-99.99/ces-v99.99.1-rc.2-dev.1/`

At the end of the tree, we will have symbolic links to the patch sets that
compose the corresponding release.

Both `ces/` and `ccs/` directories hierarchy are provided mostly for human
consumption. Patches manually applied from these directories will result in the
same state as the one resulting from using CRT. However, CRT does not apply
patches from either the `ces/` or the `ccs/` directories when building a release
branch.

## Usage

This section presumes the user has populated the environment variables described
in the beginning of this document.

### Setting up the environment

If running from the source git repository, the following should be run at the
repository's root:

```shell
# uv sync --all-packages
# source .venv/bin/activate
```

### Starting a release

A release is required to create iterations (through manifests, as previously
discussed), and is expected to provide the base reference and repository on
which the release will be based. This can be an upstream Ceph tag or branch, or
a previous release tag or branch. Additionally, the destination repository and
branch need to be specified, so the release can be published once finalized.

```shell
$ crt/crt.py release start --ref v18.2.7 --ref-rel-name reef ces-v99.99.1
        Release Name  ces-v99.99.1
    Destination Repo  clyso/ceph
      Ceph Repo Path  /mnt/pci5-dev/clyso/clyso-ceph
       From Manifest  n/a
 From Base Reference  v18.2.7 from ceph/ceph
 Release base branch  release-base/ces-v99.99.1
    Release base tag  release-base-ces-v99.99.1
```

Above we state that we are starting a release `ces-v99.99.1` for a `reef`
release, based off of upstream's `v18.2.7`.

Alternatively, we could specify we want to start a new release based on a
previously released version:

```shell
$ crt/crt.py release start --from ces-v25.03.2-rc.9 ces-v80.80.1
        Release Name  ces-v80.80.1
    Destination Repo  clyso/ceph
      Ceph Repo Path  /mnt/pci5-dev/clyso/clyso-ceph
       From Manifest  ces-v25.03.2-rc.9
 From Base Reference  release/ces-v25.03.2-rc.9 from clyso/ceph
 Release base branch  release-base/ces-v80.80.1
    Release base tag  release-base-ces-v80.80.1
```

### Creating a new release manifest

To start a new iteration for the release, we will need to first create a new
release manifest.

```shell
$ crt/crt.py new -r ces-v99.99.1 ces-v99.99.1-rc.1
             name  ces-v99.99.1-rc.1
     base release  reef
  base repository  clyso/ceph
         base ref  release-base/ces-v99.99.1
  dest repository  clyso/ceph
      dest branch  n/a
    creation date  2025-09-22 08:33:55.551657+00:00
    manifest uuid  22556695-86fc-4d79-a61f-de3e66cb77b4
           stages  0
        published  no
```

In the above case we are creating a new release candidate iteration for
`ces-v99.99.1`. Manifests are always named with suffixes representing the
iteration. These suffixes are free-form, but we do recommend certain suffixes to
be used for consistency -- these can be found in the versioning guide in this
repository.

### Adding patch sets

Before we can start adding patch sets to the manifest from the previous section,
we will need to first create a new stage.

```shell
$ crt/crt.py stage new \
    --author "Joao Eduardo Luis" \
    --email "joao@clyso.com" \
    -m ces-v99.99.1-rc.1

       uuid  ad0643db-982a-4c35-bb59-79a0356fcb14
     author  Joao Eduardo Luis <joao@clyso.com>
    created  2025-09-22 08:35:23.639600+00:00
       tags  None
 patch sets  0
  published  no
```

We can now start adding patch sets to the manifest. However, patch sets do not
have names, and are solely identified by their UUIDs. This means we will have to
know the individual patch sets UUIDs to add them to the manifest stage. To learn
which patch sets we currently have, we can simply list them:

<!-- markdownlint-disable MD013 -->

```shell
$ crt/crt.py patchset list

  19fa4ba8-02b3-47d3-b5c4-9b106d1a08eb   single   common/options: increase mds_cache_trim_threshold 2x
                                                  3758bb9bd2578b1bfa061b25d13df3ed7af145a1  version: n/a
                                                  2023-10-04 12:01:48-07:00  Dan van der Ster <dan.vanderster@clyso.com>

  db2bf7a7-7197-43a6-b9ab-65420c62bbcd   single   osd/scrub: increasing max_osd_scrubs to 3
                                                  36198beeee8f3557ffdac4fb1a94ade4ce88a758  version: n/a
                                                  2023-05-22 18:09:28+03:00  Ronen Friedman <rfriedma@redhat.com>
  [...]
  f4c6fd0f-65ab-4c0e-afbe-a2e8b7dfe317   single   cephadm: use clyso's v25.03.2 image as default
                                                  90693977d9bea8e500546ed57148646bd818404e  version: n/a
                                                  2025-08-27 14:37:52+00:00  Joao Eduardo Luis <joao@clyso.com>

  6225af48-c31b-4c7a-bba3-10e5b1b2a3f8   gh       mgr/dashboard: add reef downstream branding
                                                  2025-05-08 15:41:00+00:00  Tatjana Dehler <unknown>
                                                  clyso/ceph #206 (not merged)  version: ces-base-v25.03.1-3
                                                  updated: 2025-08-26 10:27:23+00:00  merged: n/a
```

<!-- markdownlint-enable MD013 -->

We see two kinds of patch sets in this list: `single` and `gh`. The former means
it's a single patch, directly imported from a git repository; the latter was
imported as a GitHub pull request. We are phasing out `single` patch sets in
favour of `custom` patch sets. While `single` patch sets are essentially a
single patch, `custom` patch sets may be a collection of patches that are
imported from one or more git remotes.

We can thus add a given patch set to the on-going release iteration (i.e., to
the active manifest stage), by running:

<!-- markdownlint-disable MD013 -->

```shell
$ crt/crt.py add -P 19fa4ba8-02b3-47d3-b5c4-9b106d1a08eb -m ces-v99.99.1-rc.1

  apply patch set to manifest's repository
  successfully applied patch set to manifest
  ✓ adding from patch set '19fa4ba8-02b3-47d3-b5c4-9b106d1a08eb' 0:00:00
  ✓ applying patch set to manifest                               0:00:14
  patch set single patch set uuid 19fa4ba8-02b3-47d3-b5c4-9b106d1a08eb (3758bb9bd2578b1bfa061b25d13df3ed7af145a1) added to manifest 'ces-v99.99.1-rc.1'
```

<!-- markdownlint-disable MD013 -->

Alternatively, we can consume a GitHub pull request directly:

```shell
$ crt/crt.py add --from-gh 56834 -m ces-v99.99.1-rc.1
  found patch set
  apply patch set to manifest's repository
  successfully applied patch set to manifest
  ✓ adding from github pull request                  0:00:00
  ✓ obtaining pull request info from ceph/ceph#56834 0:00:00
  ✓ fetch patch set for ceph/ceph#56834              0:00:00
  ✓ applying patch set to manifest                   0:00:15
  patch set pr ceph/ceph#56834 added to manifest 'ces-v99.99.1-rc.1'
```

By obtaining the information about the `ces-v99.99.1-rc.1` manifest, we can now
see the various patch sets that were added to its active stage by running:

```shell
crt/crt.py info -m ces-v99.99.1-rc.1
```

### Publishing a release iteration

In order to build and test the release iteration we have been working on, we
first need to publish it. This step will ensure a git branch containing the
various patches is pushed to the release's destination repository (by default,
`clyso/ceph`), and the various patch sets composing the release are symlinked in
the patches repository.

```shell
$ crt/crt.py publish ces-v99.99.1-rc.1
  ✓ executing manifest         0:00:11
  ✓ publishing                 0:00:02
  ✓ publishing manifest stages 0:00:00
  ✓ publish branch             0:00:02

  Branch 'ces-v99.99.1-rc.1-exec-20250923T033611' published to 'clyso/ceph'
          remote  clyso/ceph
  remote updated  False
   heads updated  release-dev/ces-v99.99.1-rc.1
```

The branch has now been pushed to the `clyso/ceph` remote, published as
`release-dev/ces-v99.99.1-rc.1`. This branch can now be built and tested.

If we are not satisfied with the result, or require additional patch sets, we
can perform the same process as described until now, by creating a new iteration
and go through its lifecyle. However, if we are at a point where we are
satisfied with the release iteration, we can proceed to finishing the release.

### Finishing a release

Once we are satisfied, we can mark a given iteration as the final version for a
release:

```shell
$ crt/crt.py release finish -m ces-v99.99.1-rc.1 ces-v99.99.1
  ✓ prepare repositories 0:00:04
  ✓ publish release      0:00:04
  release 'ces-v99.99.1' successfully published to 'clyso/ceph' branch 'release/ces-v99.99.1'
```

And with that, we just finished the `ces-v99.99.1` release. This results in a
branch, `release/ces-v99.99.1`, in the release's destination repository (in our
case, `clyso/ceph`), alongside with an annotated tag (`ces-v99.99.1`) pointing
to the branch's HEAD commit.

```shell
$ git show ces-v99.99.1
tag ces-v99.99.1
Tagger: Joao Eduardo Luis <joao@clyso.com>
Date:   Tue Sep 23 04:11:43 2025 +0000

Release 'ces-v99.99.1'

commit 7ab9542b76add5d57027f45ae8ee06d48de1c937
[...]
```

### Committing metadata

While CRT will perform all operations under the Ceph git repositories, and
modify its remotes, it will not perform any git operations on the patches
repository. This ensures that any release-related changes are only codified for
eternity once the tool's operator believes them to be correct. This will also
allow us, in the future, to perform some of these actions automatically using CI
tools, upon review and approval of other interested parties.

As such, after each major action (e.g., release start, iteration creation,
etc.), an accompanying commit should happen in the patches repository, followed
by a push to its corresponding remote.

[_clyso_ceph]: https://github.com/clyso/ceph
[_ceph_ceph]: https://github.com/ceph/ceph
[_clyso_ces_patches]: https://github.com/clyso/ces-patches.git
