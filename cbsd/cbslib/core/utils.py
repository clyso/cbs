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
from pathlib import Path
from typing import Any, Final


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


# Generated using GitHub Copilot, Claude Code Sonnet 4.5
#   on Jan 17 2026, by Joao Eduardo Luis <joao@clyso.com>
#
# Edited to make more sense and adjust to code base.
#
# Efficient file tail reading utility for Python 3.13+
#
# This function provides functionality to read the last N lines from
# text files efficiently, without loading the entire file into memory.
#


class FileTailError(Exception):
    """Base exception for file tail operations."""

    pass


class FileNotFoundError(FileTailError):
    """Raised when the specified file does not exist."""

    pass


class InvalidLineCountError(FileTailError):
    """Raised when an invalid line count is provided."""

    pass


# Default buffer size for reading chunks (8KB is optimal for most filesystems)
_DEFAULT_BUFFER_SIZE: Final[int] = 8192


def tail_file(
    filepath: Path | str,
    n: int,
    *,
    encoding: str = "utf-8",
    buffer_size: int = _DEFAULT_BUFFER_SIZE,
    errors: str = "strict",
) -> list[str]:
    """
    Read the last N lines from a text file efficiently.

    This function reads the file backwards in chunks, making it memory-efficient
    for large files. Similar to the Unix 'tail -n' command.

    Args:
        filepath: Path to the file to read (Path object or string)
        n: Number of lines to read from the end of the file
        encoding: Text encoding to use when decoding bytes (default: 'utf-8')
        buffer_size: Size of chunks to read in bytes (default: 8192)
        errors: How to handle encoding errors ('strict', 'ignore', 'replace')

    Returns:
        A list containing the last N lines from the file, in their original order.
        Lines do not include the trailing newline character.

    Raises:
        FileNotFoundError: If the file does not exist
        InvalidLineCountError: If n is negative
        PermissionError: If the file cannot be read due to permissions
        UnicodeDecodeError: If the file cannot be decoded with the specified encoding
        OSError: For other I/O related errors

    Examples:
        >>> from pathlib import Path
        >>> tail(Path("app.log"), 10)  # Last 10 lines
        ['line 991', 'line 992', ..., 'line 1000']

        >>> tail("data.txt", 5, encoding="latin-1")  # Last 5 lines with custom encoding
        ['data line 96', 'data line 97', ..., 'data line 100']

        >>> tail(Path("empty.txt"), 10)  # File with fewer than N lines
        []

    Performance:
        - Time complexity: O(bytes_needed) where bytes_needed ≈ N x avg_line_length
        - Space complexity: O(N) for storing the N lines
        - Does not load the entire file into memory

    """
    # Validate inputs
    if n < 0:
        raise InvalidLineCountError(f"Line count must be non-negative, got {n}")

    if n == 0:
        return []

    # Convert string path to Path object
    path = Path(filepath) if isinstance(filepath, str) else filepath

    # Validate file exists and is readable
    if not path.exists():
        raise FileNotFoundError(f"File not found: {path}")

    if not path.is_file():
        raise FileTailError(f"Path is not a file: {path}")

    # Get file size first to handle edge cases
    file_size = path.stat().st_size

    if file_size == 0:
        return []

    # Read file backwards in chunks
    lines_found: list[bytes] = []
    try:
        with path.open("rb") as file:
            position = file_size
            buffer = b""

            # Read backwards until we have enough lines or reach the start
            while position > 0 and len(lines_found) <= n:
                # Calculate chunk size (don't read past the beginning)
                chunk_size = min(buffer_size, position)
                position -= chunk_size

                # Seek to position and read chunk
                _ = file.seek(position)
                chunk = file.read(chunk_size)

                # Prepend chunk to buffer
                buffer = chunk + buffer

                # Split buffer into lines
                parts = buffer.split(b"\n")

                if position == 0:
                    # At the beginning of file, all parts are complete lines
                    lines_found = parts + lines_found
                    buffer = b""
                else:
                    # First part might be incomplete, keep it in buffer
                    buffer = parts[0]
                    lines_found = parts[1:] + lines_found

    except PermissionError as e:
        raise PermissionError(f"Permission denied reading file: {path}") from e
    except OSError as e:
        raise OSError(f"Error reading file {path}: {e}") from e

    # Handle trailing newline: if file ends with \n, last element is empty
    if lines_found and lines_found[-1] == b"":
        _ = lines_found.pop()

    # Take only the last N lines
    result_bytes = lines_found[-n:] if len(lines_found) > n else lines_found

    try:
        # Decode bytes to strings
        return [line.decode(encoding, errors=errors) for line in result_bytes]
    except UnicodeDecodeError as e:
        raise UnicodeDecodeError(
            e.encoding,
            e.object,
            e.start,
            e.end,
            f"Failed to decode file {path} with encoding {encoding}: {e.reason}",
        ) from e


if __name__ == "__main__":
    print(format_to_str("build-{Y}{m}{d}-{H}{M}{S}", {}))
    print(format_to_str("{version}-{DT}", {"version": "foo-v18.2.2"}))

    # Example usage and basic testing for 'tail_file()'

    # Create a test file
    test_file = Path("test_tail.txt")
    _ = test_file.write_text("\n".join(f"Line {i}" for i in range(1, 101)))

    print("Last 5 lines:")
    for line in tail_file(test_file, 5):
        print(f"  {line}")

    print("\nLast 10 lines:")
    for line in tail_file(test_file, 10):
        print(f"  {line}")

    # Clean up
    test_file.unlink()
