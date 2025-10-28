# CES Build System (CBS)

A collection of tools to simplify and automate creating, and releasing,
containers for Ceph and other desirable components.

## Repository organization

CBS is composed of three main tools:

- `cbsbuild`, a CLI tool to build containers, part of the `cbscore` package.
- `crt`, a CLI tool to manage Ceph releases and their life cycles.
- `cbs`, essentially `cbsbuild` as a service, being a REST server with a work
  queue, scheduling builds on worker nodes.

Additionally,

- in `components/` we will find the definition of the `ceph` component, and the
  various files and descriptors required to building various Ceph versions.
- in `images/` we will find an old attempt at keeping Ceph release's dependency
  images sync'ed across repositories. Feel free to ignore it.

We will also find a `podman-compose.cbs.yaml` file, which will set up a local
`cbs` -- server, worker, and the required `redis` broker for the server's work
queue. Using the `do-cbs-compose.sh` script we can easily run this setup.

## Contributing

We haven't quite defined all the requirements for contributing to this
repository, but we tend to keep our commits formatted in-line with what the Ceph
project practices.

We do required DCO on all commits, and GPG-signed commits.

## Licensing

Various projects within this repository may be licensed differently.

- `cbs` is AGPLv3
- CLI tools such as `cbscore`/`cbsbuild`, and `crt` are GPLv3
