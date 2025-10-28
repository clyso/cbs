# CES Build Service

## Prepare the environment

```shell
# uv sync --all-packages --no-dev
# source .venv/bin/activate
# cbsbuild ...
```

### Requirements

The CES Build System and its various build-related tools rely on `podman` and
`buildah`. We expect these to be installed and available to be used in a
[rootless environment][_podman_rootless].

Additionally, Python 3.13 or greater is necessary. If this is not available to
the user, please consider using the `--python VER` flag for `uv`. It will ensure
the specified Python version is installed for this specific environment.

[_podman_rootless]:
  https://github.com/containers/podman/blob/main/docs/tutorials/rootless_tutorial.md

## Creating a version to build

Version descriptors can be generated using the `cbsbuild` tool, which will
generate a `JSON` file under `_versions/`, with the metadata required to build
the specified version. This metadata include the components to build, and their
versions.

Creating a new version is as simple as running

```shell
$ cbsbuild versions create -c ceph@adf123 ces-v12.3.4

version: ces-v12.3.4-rc.1-dev.2
version title: Release ces-v12.3.4
... JSON ...
written to .../ces.git/versions/testing/ces-v12.3.4-rc.1-dev.2.json
```

This will be the file descriptor to use when building `ces-v12.3.4`. The tool
accepts more options, but these will be the most often used ones:

## Local builds

### Initialize the configuration file

To build using `cbsbuild`, we first need a configuration file that will be
specifying the various necessary paths for the build process.

Creating this configuration file can be achieved by running
`cbsbuild config-init`, which will interactively populate the configuration
file.

We highly recommend having a dedicated, high-throughput partition or disk to
hold the scratch directory and the `ccache` directory, although not necessarily
required.

### Building using `cbsbuild`

Building is performed using `cbsbuild build`. This command takes one argument,
expecting the path to a version descriptor. If you haven't created one yet,
please see how to do it in the
[corresponding section](#creating-a-version-to-build).

To build a given version, the following can be used (please note the `-d` option
is used for debug output and can be omitted):

```shell
$ cbs/ces-build.py -d \
  build \
  ./_versions/release/ces-v12.3.4.json
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
