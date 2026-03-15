#!/usr/bin/env python3
# Copyright (C) 2026  Clyso
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

"""CBS core build wrapper for cbsd-rs worker."""

import json
import os
import sys


def main():
    """Read build descriptor from stdin and execute the build."""
    input_data = json.load(sys.stdin)
    descriptor = input_data["descriptor"]
    component_path = input_data["component_path"]
    trace_id = input_data.get("trace_id", "")

    # Set trace ID for logging.
    os.environ["CBS_TRACE_ID"] = trace_id

    # TODO: Import and call cbscore.runner.runner() here.
    # For now, just print a message and exit successfully.
    print(
        f"cbscore-wrapper: would build {descriptor.get('channel', 'unknown')} "
        f"v{descriptor.get('version', 'unknown')} from {component_path}"
    )
    print(f"cbscore-wrapper: trace_id={trace_id}")

    # Emit structured result line.
    result = {"type": "result", "exit_code": 0, "error": None}
    print(json.dumps(result))

    sys.exit(0)


if __name__ == "__main__":
    main()
