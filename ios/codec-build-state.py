#!/usr/bin/env python3
import hashlib
import json
from pathlib import Path
import sys

IOS_ROOT = Path(__file__).resolve().parent
REPO_ROOT = IOS_ROOT.parent
STATE_NAME = "cw-build-state.json"


def fingerprint():
    inputs = [
        REPO_ROOT / "Cargo.toml",
        REPO_ROOT / "Cargo.lock",
        IOS_ROOT / "build-codec.sh",
        Path(__file__).resolve(),
        IOS_ROOT / "RustBridge/Cargo.toml",
        IOS_ROOT / "RustBridge/Cargo.lock",
    ]
    for directory in [REPO_ROOT / "src", IOS_ROOT / "RustBridge/src", IOS_ROOT / "RustBridge/include"]:
        inputs.extend(path for path in directory.rglob("*") if path.is_file())
    inputs.extend(REPO_ROOT.glob("vendor/**/Cargo.toml"))
    inputs.extend(REPO_ROOT.glob("vendor/**/*.rs"))
    for name in ["rust-toolchain", "rust-toolchain.toml", ".cargo/config", ".cargo/config.toml"]:
        path = REPO_ROOT / name
        if path.is_file():
            inputs.append(path)
    digest = hashlib.sha256()
    for path in sorted(inputs):
        relative = path.relative_to(REPO_ROOT).as_posix().encode()
        content = path.read_bytes()
        digest.update(len(relative).to_bytes(8, "little"))
        digest.update(relative)
        digest.update(len(content).to_bytes(8, "little"))
        digest.update(content)
    return digest.hexdigest()


def main():
    operation = sys.argv[1]
    if operation == "fingerprint":
        print(fingerprint())
        return
    framework = Path(sys.argv[2])
    state_file = framework / STATE_NAME
    if operation == "record":
        expected = sys.argv[3]
        if fingerprint() != expected:
            raise ValueError("Rust build inputs changed during compilation; rebuild the bridge.")
        state_file.write_text(json.dumps({
            "schema_version": 1,
            "inputs_sha256": expected,
            "targets": sys.argv[4:],
        }, indent=2) + "\n")
    elif operation == "check":
        state = json.loads(state_file.read_text())
        if state["schema_version"] != 1 or state["inputs_sha256"] != fingerprint():
            raise ValueError("Rust bridge inputs changed.")
        target = {
            "iphoneos": "aarch64-apple-ios",
            "iphonesimulator": "aarch64-apple-ios-sim",
        }[sys.argv[3]]
        if target not in state["targets"]:
            raise ValueError(f"Rust bridge is missing {target}.")
    else:
        raise ValueError(f"Unknown operation: {operation}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, IndexError) as error:
        print(f"error: {error} Run ./ios/build-codec.sh before building the app.", file=sys.stderr)
        sys.exit(1)
