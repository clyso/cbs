# image tool

At this point, this tool focuses on copying the images required by CES versions
to Clyso's Harbor registry.

CES versions can be specified as JSON files in the `desc/` directory, and will
need to map the images that need to be replicated from a source to a
destination.

Each descriptor file must also define to which versions it's applicable. File
names are not relevant, although they should reflect the versions they represent
as a matter of convention.

To run this tool, the following procedure can be performed:

```shell
# python3 -m venv venv
# source venv/bin/activate
# pip install -r requirements.txt
# source ces-env.sh
# ./image-tool.py sync <VERSION> (e.g., 24.11.0)
```

Note that it is recommended to source the `ces-env.sh` file (an example can be
found in `ces-env.sh.example`), with all the required environment variables
defined. These must allow access to Clyso's Hashicorp Vault instance, and the
corresponding role ID and secret ID must have permissions to obtain Harbor
credentials, as well as use the `ces-transit` transit mechanism (so images can
be signed).

The aim of this script is to be run automatically, periodically, ensuring we
have the required images for a release.
