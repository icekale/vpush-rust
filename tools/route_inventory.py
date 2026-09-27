#!/usr/bin/env python3
"""Build a redacted Python/Rust route and configuration inventory."""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

METHODS = {"get", "post", "put", "delete", "patch", "head", "options", "trace"}
ENV_RE = re.compile(r"\b(?:std::env::var|env::var|var|dotenvy::var)\s*\(\s*[\"']([A-Z][A-Z0-9_]*)[\"']")
PY_ENV_RE = re.compile(r"\b(?:os\.getenv|os\.environ\.get|os\.environ\.__getitem__|getenv)\s*\(\s*[\"']([A-Z][A-Z0-9_]*)[\"']")
PARAM_RE = re.compile(r"\{[^}/]+\}")
PY_ROUTE_RE = re.compile(
    r"@(?:\w+\.)?router\.(get|post|put|delete|patch|head|options)\(\s*[\"']([^\"']+)[\"']",
    re.MULTILINE,
)


def line_number(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def scan_call(text: str, start: int) -> tuple[str, int]:
    """Return the balanced argument text and closing offset for a call."""
    open_at = text.find("(", start)
    if open_at < 0:
        raise ValueError(f"missing '(' after offset {start}")
    depth = 0
    quote: str | None = None
    escaped = False
    for index in range(open_at, len(text)):
        char = text[index]
        if quote:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == quote:
                quote = None
            continue
        if char in "\"'":
            quote = char
        elif char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
            if depth == 0:
                return text[open_at + 1 : index], index + 1
    raise ValueError(f"unclosed call at line {line_number(text, start)}")


def rust_routes(path: Path) -> list[tuple[str, str, int]]:
    text = path.read_text()
    routes: list[tuple[str, str, int]] = []
    cursor = 0
    while True:
        match = re.search(r"\.route\s*\(", text[cursor:])
        if not match:
            break
        start = cursor + match.start()
        args, end = scan_call(text, start)
        path_match = re.match(r'\s*"([^"\\]*(?:\\.[^"\\]*)*)"\s*,', args, re.S)
        if not path_match:
            raise ValueError(f"cannot parse route path at {path}:{line_number(text, start)}")
        route_path = bytes(path_match.group(1), "utf-8").decode("unicode_escape")
        methods = [method for method in METHODS if re.search(rf"\b{method}\s*\(", args)]
        if not methods:
            raise ValueError(f"cannot parse route method at {path}:{line_number(text, start)}")
        routes.extend((method.upper(), route_path, line_number(text, start)) for method in methods)
        cursor = end
    return sorted(set(routes), key=lambda item: (item[1], item[0]))


def python_routes(path: Path) -> list[tuple[str, str, int]]:
    text = path.read_text()
    return sorted(
        {(match.group(1).upper(), match.group(2), line_number(text, match.start())) for match in PY_ROUTE_RE.finditer(text)},
        key=lambda item: (item[1], item[0]),
    )


def config_names(paths: list[Path], pattern: re.Pattern[str]) -> set[str]:
    names: set[str] = set()
    for path in paths:
        if path.exists():
            names.update(pattern.findall(path.read_text()))
    return names


def canonical_route(method: str, route: str, prefix: str) -> tuple[str, str]:
    route = route if route.startswith("/") else f"/{route}"
    prefix = prefix.rstrip("/")
    if prefix and route.startswith(prefix + "/"):
        route = route[len(prefix) :]
    route = PARAM_RE.sub("{}", route.rstrip("/") or "/")
    return method.upper(), route


def route_table(title: str, routes: list[tuple[str, str, int]]) -> list[str]:
    rows = [f"## {title}", "", "| Method | Path | Line |", "|---|---|---:|"]
    rows.extend(f"| `{method}` | `{route}` | {line} |" for method, route, line in routes)
    return rows + [""]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--python", type=Path, required=True)
    parser.add_argument("--rust", type=Path, required=True)
    parser.add_argument("--rust-root", type=Path, default=Path("src"))
    parser.add_argument("--python-prefix", default="/api")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    try:
        py_routes = python_routes(args.python)
        rs_routes = rust_routes(args.rust)
    except (OSError, ValueError) as error:
        print(f"route inventory failed: {error}", file=sys.stderr)
        return 2

    py_keys = {canonical_route(method, route, args.python_prefix) for method, route, _ in py_routes}
    rs_keys = {canonical_route(method, route, args.python_prefix) for method, route, _ in rs_routes}
    rust_files = sorted(args.rust_root.glob("**/*.rs"))
    py_env = config_names([args.python], PY_ENV_RE)
    rust_env = config_names(rust_files, ENV_RE)

    lines = [
        "# Python/Rust Route Inventory",
        "",
        "> Generated from source. Configuration values are intentionally excluded.",
        "",
        f"- Python source: `{args.python}`",
        f"- Rust source: `{args.rust}`",
        f"- Python routes: **{len(py_routes)}**",
        f"- Rust routes: **{len(rs_routes)}**",
        f"- Python route prefix removed for comparison: `{args.python_prefix}`",
        f"- Exact route matches: **{len(py_keys & rs_keys)}**",
        f"- Python-only routes: **{len(py_keys - rs_keys)}**",
        f"- Rust-only routes: **{len(rs_keys - py_keys)}**",
        "",
        "## Python-only",
        "",
        "| Method | Path |",
        "|---|---|",
    ]
    lines.extend(f"| `{method}` | `{route}` |" for method, route in sorted(py_keys - rs_keys))
    lines += ["", "## Rust-only", "", "| Method | Path |", "|---|---|"]
    lines.extend(f"| `{method}` | `{route}` |" for method, route in sorted(rs_keys - py_keys))
    lines += ["", "## Configuration Names", "", "### Python", ""]
    lines.extend(f"- `{name}`" for name in sorted(py_env))
    lines += ["", "### Rust", ""]
    lines.extend(f"- `{name}`" for name in sorted(rust_env))
    lines += ["", "## Source Routes", ""]
    lines += route_table("Python", py_routes)
    lines += route_table("Rust", rs_routes)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text("\n".join(lines))
    print(f"wrote {args.output} ({len(py_routes)} Python routes, {len(rs_routes)} Rust routes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
