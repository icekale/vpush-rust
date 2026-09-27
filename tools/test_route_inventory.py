#!/usr/bin/env python3
"""Small regression check for route_inventory.py."""
from pathlib import Path
from tempfile import TemporaryDirectory

from route_inventory import canonical_route, python_routes, rust_routes


with TemporaryDirectory() as directory:
    root = Path(directory)
    py = root / "api.py"
    rs = root / "main.rs"
    py.write_text(
        '@router.post("/api/items/{item_id}")\n'
        '@router.get(\n'
        '    "/api/items/{item_id}"\n'
        ')\n'
    )
    rs.write_text(
        '.route(\n'
        '    "/api/items/{id}",\n'
        '    get(read).post(write),\n'
        ')\n'
    )
    assert {canonical_route(*route[:2], "/api") for route in python_routes(py)} == {
        ("GET", "/items/{}"),
        ("POST", "/items/{}"),
    }
    assert {canonical_route(*route[:2], "/api") for route in rust_routes(rs)} == {
        ("GET", "/items/{}"),
        ("POST", "/items/{}"),
    }
print("route-inventory-tests-ok")
