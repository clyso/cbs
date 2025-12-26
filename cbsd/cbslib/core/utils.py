# CBS service library - core - utilities
# Copyright (C) 2025  Clyso GmbH
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU Affero General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU Affero General Public License for more details.


import datetime
from datetime import datetime as dt
from typing import Any


def format_to_str(format_str: str, vars: dict[str, Any]) -> str:  # pyright: ignore[reportExplicitAny]
    now = dt.now(datetime.UTC)

    base_vars = {
        "H": now.strftime("%H"),
        "M": now.strftime("%M"),
        "S": now.strftime("%S"),
        "d": now.strftime("%d"),
        "m": now.strftime("%m"),
        "Y": now.strftime("%Y"),
        "DT": now.strftime("%Y%m%dT%H%M%S"),
        **vars,
    }
    return format_str.format(**base_vars)


if __name__ == "__main__":
    print(format_to_str("build-{Y}{m}{d}-{H}{M}{S}", {}))
    print(format_to_str("{version}-{DT}", {"version": "foo-v18.2.2"}))
