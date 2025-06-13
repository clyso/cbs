# Ceph Release Tool - db commands
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


import click
from crtlib.db.s3 import S3DB
from rich.progress import Progress, SpinnerColumn, TextColumn, TimeElapsedColumn

from . import Ctx, console, pass_ctx, pass_s3db, pinfo
from . import logger as parent_logger

logger = parent_logger.getChild("db")


@click.group("db", help="Database operations.")
def cmd_db() -> None:
    pass


@cmd_db.command("sync", help="Synchronize database with S3.")
@pass_s3db
@pass_ctx
def cmd_db_sync(ctx: Ctx, s3db: S3DB) -> None:
    pinfo("synchronize S3 db")

    progress = Progress(
        SpinnerColumn(),
        TextColumn("[progress.description]{task.description}"),
        TimeElapsedColumn(),
        console=console,
    )

    _ = progress.add_task("synchronizing")
    progress.start()
    s3db.sync()
    progress.stop()

    pass
