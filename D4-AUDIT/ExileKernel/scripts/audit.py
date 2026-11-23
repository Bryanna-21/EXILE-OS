#!/usr/bin/env python3
"""Enforce EUK dependency, documentation, and license declaration gates."""
import pathlib

r = pathlib.Path(__file__).resolve().parents[1]
required = [
    "LICENSE",
    "NOTICE.md",
    "THIRD_PARTY.md",
    "docs/PROVENANCE.md",
    "docs/THREAT_MODEL.md",
]
for name in required:
    path = r / name
    assert path.is_file() and path.stat().st_size > 0, name

license_text = (r / "LICENSE").read_text(encoding="utf-8")
assert "Apache License" in license_text, "Apache license text missing"
assert "Version 2.0, January 2004" in license_text, "Unexpected license version"
assert "END OF TERMS AND CONDITIONS" in license_text, "License text incomplete"
assert not list(r.rglob("Cargo.toml")), (
    "New Cargo dependencies require explicit audit changes"
)

print("EUK dependency/documentation/Apache-2.0 declaration gate PASS.")
print("Third-party and binary-distribution obligations still require review.")
