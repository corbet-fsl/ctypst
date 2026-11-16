#!/usr/bin/env python3
"""Prepare the immutable ctypst packages of a release (the crate and the Wasm npm package).

The release run (`ccid release build`, .ci/release-build.sh) calls this; publication is the
platform publisher's, from the release's files. This command never installs tools.
"""
import argparse
import contextlib
import fcntl
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = "corbet-libs/ctypst"
TOKENS = ("CARGO_REGISTRY_TOKEN", "NPM_TOKEN", "NODE_AUTH_TOKEN", "JSR_TOKEN", "GH_TOKEN")


class Failure(Exception):
    pass


def require(condition, message):
    if not condition:
        raise Failure(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def read_json(path):
    return json.loads(path.read_text())


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    with temporary.open("w") as stream:
        json.dump(value, stream, indent=2, sort_keys=True)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    temporary.replace(path)
    directory = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def run(command, *, cwd=ROOT, env=None, capture=False):
    result = subprocess.run(command, cwd=cwd, env=env, text=True,
                            stdout=subprocess.PIPE if capture else None,
                            stderr=subprocess.STDOUT if capture else None)
    require(result.returncode == 0, f"{Path(command[0]).name} failed (exit {result.returncode}); no publication retry was attempted")
    return result.stdout.strip() if capture else None


def archive_files(path, prefix=""):
    options = {"fileobj": io.BytesIO(path)} if isinstance(path, bytes) else {"name": path}
    with tarfile.open(mode="r:*", **options) as archive:
        files = {}
        total = 0
        for member in archive.getmembers():
            name = PurePosixPath(member.name)
            require(not name.is_absolute() and ".." not in name.parts, "Unsafe archive path")
            require(member.isfile() or member.isdir(), "Archive contains a link or special file")
            if member.isdir():
                continue
            require(member.name.startswith(prefix), "Unexpected archive prefix")
            relative = member.name[len(prefix):]
            require(relative and relative not in files, "Duplicate archive file")
            total += member.size
            require(total < 200_000_000, "Archive exceeds the expected package size")
            files[relative] = archive.extractfile(member).read()
        return files


def unpack(files, destination):
    for name, data in files.items():
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)


def working_source_files():
    files = {}
    for directory, directories, filenames in os.walk(ROOT):
        relative_directory = Path(directory).relative_to(ROOT)
        directories[:] = [name for name in directories if name != "__pycache__" and
                          not (relative_directory == Path(".") and name in (".git", ".ccid", "target"))]
        for name in directories:
            require(not (Path(directory) / name).is_symlink(), "Release source contains a symbolic link")
        for name in filenames:
            path = Path(directory) / name
            relative = path.relative_to(ROOT)
            if relative.parts[0] == ".git":
                continue
            require(not path.is_symlink(), "Release source contains a symbolic link")
            files[relative.as_posix()] = path.read_bytes()
    return files


def source_content_identity(source_bytes, files):
    # Git and Crow can encode identical sources into different tar bytes.
    # Bind every path, executable bit and payload, retaining the raw transport
    # checksum separately rather than forcing a rebuild after a provider switch.
    with tarfile.open(fileobj=io.BytesIO(source_bytes), mode="r:*") as archive:
        executable = {member.name: member.mode & 0o111 for member in archive.getmembers() if member.isfile()}
    for name, mode in executable.items():
        require((ROOT / name).stat().st_mode & 0o111 == mode, f"Release source executable mode differs: {name}")
    return digest(json.dumps([(name, executable[name], digest(data)) for name, data in sorted(files.items())], separators=(",", ":")).encode())


def stable_identity(identity):
    return {key: value for key, value in identity.items() if key != "driver_source_sha256"}


def source_identity():
    package = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]
    version = package["version"]
    require(package["name"] == "ctypst" and re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?", version), "Unexpected release identity")
    commit = os.environ.get("CI_COMMIT_SHA") or run(["git", "rev-parse", "HEAD"], capture=True)
    require(re.fullmatch(r"[0-9a-f]{40}", commit), "A full source commit is required")
    tag = os.environ.get("RELEASE_TAG", "")
    require(tag == "v" + version, "RELEASE_TAG must match the committed package version")
    archive = os.environ.get("SOURCE_ARCHIVE")
    if archive:
        source_bytes = Path(archive).read_bytes()
        source_hash = digest(source_bytes)
        require(source_hash == os.environ.get("SOURCE_SHA256"), "Source archive hash mismatch")
    else:
        source_bytes = subprocess.check_output(["git", "archive", "--format=tar", commit], cwd=ROOT)
        source_hash = digest(source_bytes)
    archive_commit = subprocess.check_output(["git", "get-tar-commit-id"], input=source_bytes).decode().strip()
    require(archive_commit == commit, "Source archive commit mismatch")
    expected_files = archive_files(source_bytes)
    actual_files = working_source_files()
    require(set(actual_files) == set(expected_files), "Release source file inventory differs from its committed archive")
    for name, data in actual_files.items():
        require(expected_files[name] == data, f"Release source differs from its committed archive: {name}")
    for path in ("package.json", "jsr.json"):
        require(read_json(ROOT / "js/@corbet-labs/ctypst" / path)["version"] == version, "JavaScript version differs from Cargo")
    return {"schema": 1, "repository": REPOSITORY, "version": version, "tag": tag,
            "driver_commit": commit, "driver_source_sha256": source_hash,
            "driver_content_sha256": source_content_identity(source_bytes, expected_files),
            "lock_sha256": digest((ROOT / "Cargo.lock").read_bytes())}


def artifacts_directory(identity):
    supplied = os.environ.get("RELEASE_ARTIFACT_DIR")
    if supplied:
        directory = Path(supplied)
        require(directory.is_absolute(), "RELEASE_ARTIFACT_DIR must be absolute")
    else:
        target = os.environ.get("CARGO_TARGET_DIR")
        require(target and Path(target).is_absolute(), "An absolute persistent CARGO_TARGET_DIR or RELEASE_ARTIFACT_DIR is required")
        directory = Path(target) / "releases" / identity["driver_content_sha256"]
    directory.mkdir(parents=True, exist_ok=True)
    return directory


@contextlib.contextmanager
def release_lock(directory):
    with (directory / "release.lock").open("a") as stream:
        fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield


def dependencies(manifest):
    result = []
    def append(table, target=None):
        for section, kind in (("dependencies", "normal"), ("dev-dependencies", "dev"), ("build-dependencies", "build")):
            for name, value in table.get(section, {}).items():
                spec = {"version": value} if isinstance(value, str) else value
                require(isinstance(spec.get("version"), str) and not any(key in spec for key in ("path", "git", "registry", "registry-index", "workspace")), "Unresolved registry dependency")
                result.append({"name": spec.get("package", name), "version_req": spec["version"],
                               "features": spec.get("features", []), "optional": spec.get("optional", False),
                               "default_features": spec.get("default-features", True), "target": target,
                               "kind": kind, "registry": None, "explicit_name_in_toml": name if spec.get("package") else None})
    append(manifest)
    for target, table in manifest.get("target", {}).items():
        append(table, target)
    return sorted(result, key=lambda row: (row["target"] or "", row["kind"], row["name"]))


def crate_metadata(files, version):
    manifest = tomllib.loads(files["Cargo.toml"].decode())
    original = tomllib.loads(files["Cargo.toml.orig"].decode())
    package = manifest["package"]
    require(package["name"] == "ctypst" and package["version"] == version, "Crate version mismatch")
    for key in ("name", "version", "edition", "rust-version", "authors", "description", "repository", "homepage", "documentation", "license", "license-file", "keywords", "categories", "links"):
        require(package.get(key) == original["package"].get(key), f"Normalized crate metadata differs: {key}")
    require(dependencies(manifest) == dependencies(original), "Normalized crate dependencies differ")
    require(manifest.get("features", {}) == original.get("features", {}), "Normalized features differ")
    require(not manifest.get("patch") and not manifest.get("replace"), "Local overrides in crate")
    require(package.get("repository") == "https://git.corbet.ch/corbet-fsl/ctypst", "Unexpected crate repository")
    return {"name": "ctypst", "vers": version, "deps": dependencies(manifest),
            "features": manifest.get("features", {}), "authors": package.get("authors", []),
            "description": package.get("description"), "documentation": package.get("documentation"),
            "homepage": package.get("homepage"), "readme": files[package["readme"]].decode(),
            "readme_file": package["readme"], "keywords": package.get("keywords", []),
            "categories": package.get("categories", []), "license": package.get("license"),
            "license_file": package.get("license-file"), "repository": package["repository"],
            "links": package.get("links"), "rust_version": package.get("rust-version"), "badges": manifest.get("badges", {})}


def check_crate(path, source, version, producing_commit):
    files = archive_files(path, f"ctypst-{version}/")
    crate_metadata(files, version)
    for name, data in files.items():
        if name == "Cargo.toml":
            continue
        if name == ".cargo_vcs_info.json":
            vcs = json.loads(data)
            require(vcs.get("git", {}).get("sha1") == producing_commit and not vcs["git"].get("dirty"), "Cargo VCS provenance differs from the clean producing commit")
            continue
        original_name = "Cargo.toml" if name == "Cargo.toml.orig" else name
        require(source.get(original_name) == data, f"Crate member differs from producing source: {name}")
    require(files.get("Cargo.lock") == source.get("Cargo.lock"), "Crate lock differs from producing source")


def load_component(directory, identity, component):
    manifest = read_json(directory / f"{component}.json")
    require(all(manifest.get(key) == value for key, value in stable_identity(identity).items()), "Artifact receipt belongs to another source or lock")
    for name, expected in manifest["artifacts"].items():
        require(Path(name).name == name and name not in (".", ".."), "Unsafe artifact name")
        path = directory / name
        require(path.is_file() and not path.is_symlink() and digest(path.read_bytes()) == expected, f"Artifact changed: {name}")
    return manifest


def prepare_cargo(directory, identity):
    if (directory / "cargo.json").exists():
        load_component(directory, identity, "cargo")
        print("Reusing verified Cargo artifact")
        return
    version = identity["version"]
    target = directory / f"ctypst-{version}.crate"
    imported = os.environ.get("RELEASE_REUSE_CRATE")
    if imported:
        # Import carries its own audited producing-source receipt, not a claim
        # that a new helper commit or a historic tag produced the old bytes.
        proof_path = Path(os.environ["RELEASE_REUSE_RECEIPT"])
        require(digest(proof_path.read_bytes()) == os.environ.get("RELEASE_REUSE_RECEIPT_SHA256"), "Imported proof checksum mismatch")
        proof = read_json(proof_path)
        for key in ("producing_commit", "tag_commit"):
            require(re.fullmatch(r"[0-9a-f]{40}", proof.get(key, "")), "Imported proof needs producing and tag commits")
        require(proof.get("version") == version and proof.get("checks_passed") is True and proof.get("evidence"), "Imported artifact lacks reviewed validation evidence")
        require(digest(Path(imported).read_bytes()) == proof.get("crate_sha256"), "Imported crate checksum mismatch")
        source_path = Path(os.environ["RELEASE_REUSE_SOURCE_ARCHIVE"])
        require(digest(source_path.read_bytes()) == proof.get("source_sha256"), "Imported source checksum mismatch")
        producing_commit = subprocess.check_output(["git", "get-tar-commit-id"], input=source_path.read_bytes()).decode().strip()
        require(producing_commit == proof["producing_commit"], "Imported source archive commit mismatch")
        source = archive_files(source_path)
        require(source.get("Cargo.toml") == (ROOT / "Cargo.toml").read_bytes() and source.get("Cargo.lock") == (ROOT / "Cargo.lock").read_bytes(), "Imported package graph differs from release driver source")
        runtime_prefixes = ("src/", "fonts/", "typst/", "protocol/")
        imported_runtime = {name: data for name, data in source.items() if name.startswith(runtime_prefixes)}
        driver_runtime = {path.relative_to(ROOT).as_posix(): path.read_bytes()
                          for prefix in runtime_prefixes for path in (ROOT / prefix).rglob("*") if path.is_file()}
        require(imported_runtime == driver_runtime, "Imported runtime inventory or input differs from release driver source")
        check_crate(Path(imported), source, version, producing_commit)
        shutil.copyfile(imported, target)
        provenance = {"producing_commit": proof["producing_commit"], "producing_source_sha256": proof["source_sha256"],
                      "tag_commit": proof["tag_commit"], "verification": proof["evidence"], "import_receipt_sha256": digest(proof_path.read_bytes())}
    else:
        run(["bash", "scripts/ci.sh", "quality"])
        run(["bash", "scripts/ci.sh", "test"])
        cargo_target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
        # cargo publish --dry-run retains the exact upload archive here.
        candidates = [cargo_target / "package/tmp-crate" / target.name,
                      cargo_target / "package" / target.name,
                      cargo_target / "package/tmp-registry" / target.name]
        candidate = next((path for path in candidates if path.is_file()), None)
        require(candidate is not None, "Cargo dry-run did not retain an upload crate; preserve the job and inspect it")
        source = archive_files(Path(os.environ["SOURCE_ARCHIVE"])) if os.environ.get("SOURCE_ARCHIVE") else {
            name: subprocess.check_output(["git", "show", f"{identity['driver_commit']}:{name}"], cwd=ROOT)
            for name in run(["git", "ls-files"], capture=True).splitlines()}
        check_crate(candidate, source, version, identity["driver_commit"])
        shutil.copyfile(candidate, target)
        provenance = {"producing_commit": identity["driver_commit"], "producing_source_sha256": identity["driver_source_sha256"],
                      "tag_commit": identity["driver_commit"], "verification": ["quality", "test", "cargo-publish-dry-run"],
                      "rustc": run(["rustc", "--version", "--verbose"], capture=True), "cargo": run(["cargo", "--version"], capture=True)}
    write_json(directory / "cargo.json", {**identity, **provenance, "artifacts": {target.name: digest(target.read_bytes())}})


def prepare_javascript(directory, identity):
    if (directory / "javascript.json").exists():
        load_component(directory, identity, "javascript")
        print("Reusing verified JavaScript artifacts")
        return
    for tool in ("bun", "wasm-bindgen", "wasm-opt", "deno"):
        require(shutil.which(tool), f"Provisioned {tool} is required; this release command does not install tools")
    bindgen = {item["version"] for item in tomllib.loads((ROOT / "Cargo.lock").read_text())["package"] if item["name"] == "wasm-bindgen"}
    require(len(bindgen) == 1 and run(["wasm-bindgen", "--version"], capture=True).split()[-1] in bindgen, "Provisioned wasm-bindgen must match Cargo.lock")
    with tempfile.TemporaryDirectory(prefix="ctypst-release-js-", dir=os.environ.get("TMPDIR")) as temporary:
        stage = Path(temporary)
        # Keep Bun's generated files out of the Cargo package source tree.
        for name in ("js", "typst", "fonts", "protocol"):
            shutil.copytree(ROOT / name, stage / name, ignore=shutil.ignore_patterns("node_modules", "dist", "wasm", "*.tgz"))
        shutil.copyfile(ROOT / "Cargo.toml", stage / "Cargo.toml")
        package = stage / "js/@corbet-labs/ctypst"
        run(["bun", "install", "--frozen-lockfile"], cwd=package)
        run(["bash", "scripts/sync-assets.sh"], cwd=package)
        run(["bun", "./node_modules/typescript/bin/tsc", "--noEmit", "-p", "tsconfig.json"], cwd=package)
        run(["bun", "scripts/conformance.mts"], cwd=package)
        env = os.environ.copy()
        env["RUSTFLAGS"] = "-C opt-level=z -C codegen-units=1"
        run(["cargo", "rustc", "--locked", "--release", "--target", "wasm32-unknown-unknown", "--no-default-features", "--features", "wasm", "--crate-type", "cdylib"], env=env)
        target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
        wasm = target / "wasm32-unknown-unknown/release/ctypst.wasm"
        optimized = stage / "ctypst.wasm"
        run(["wasm-opt", "--enable-bulk-memory", "--enable-nontrapping-float-to-int", "-Oz", str(wasm), "-o", str(optimized)])
        require(optimized.stat().st_size < 26_214_400, "WASM exceeds the 25 MiB distribution budget")
        for kind in ("web", "nodejs"):
            run(["wasm-bindgen", "--target", kind, "--out-dir", str(package / "wasm" / kind), str(optimized)])
        # wasm-bindgen emits CommonJS for nodejs; the enclosing package is ESM.
        (package / "wasm/nodejs/package.json").write_text('{"type":"commonjs"}\n')
        packed = stage / "packed"
        packed.mkdir()
        run(["bun", "pm", "pack", "--destination", str(packed)], cwd=package)
        archives = list(packed.glob("*.tgz"))
        require(len(archives) == 1, "Expected exactly one npm archive")
        npm = archive_files(archives[0], "package/")
        require(json.loads(npm["package.json"])["version"] == identity["version"], "npm package version mismatch")
        require(sum(name.endswith(".ttf") for name in npm) == 16, "npm font inventory mismatch")
        require(all(name in npm for name in ("wasm/web/ctypst_bg.wasm", "wasm/nodejs/ctypst_bg.wasm", "wasm/nodejs/package.json")), "Packaged WASM outputs are missing")
        smoke = stage / "smoke"
        unpack(npm, smoke)
        run(["bun", str(ROOT / "scripts/release-wasm-smoke.mjs"), str(smoke), str(ROOT / "protocol/measure-v1/requests.json")])
        npm_target = directory / archives[0].name
        shutil.copyfile(archives[0], npm_target)
        jsr = stage / "jsr"
        jsr.mkdir()
        for name in ("jsr.json", "README.md"):
            shutil.copyfile(package / name, jsr / name)
        shutil.copytree(package / "src", jsr / "src")
        run(["deno", "publish", "--dry-run", "--allow-dirty", "--no-provenance", "--node-modules-dir=none"], cwd=jsr)
        jsr_target = directory / f"ctypst-{identity['version']}-jsr.tar.gz"
        with tarfile.open(jsr_target, "w:gz") as archive:
            for path in sorted(jsr.rglob("*")):
                if path.is_file():
                    archive.add(path, arcname=path.relative_to(jsr), recursive=False)
    write_json(directory / "javascript.json", {**identity, "producing_commit": identity["driver_commit"],
               "producing_source_sha256": identity["driver_source_sha256"], "tag_commit": identity["driver_commit"],
               "verification": ["typescript", "measurement-conformance", "packaged-wasm-smoke", "jsr-dry-run"],
               "tools": {tool: run([tool, "--version"], capture=True) for tool in ("rustc", "cargo", "bun", "wasm-bindgen", "wasm-opt", "deno")},
               "artifacts": {path.name: digest(path.read_bytes()) for path in (npm_target, jsr_target)}})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=("source", "prepare"))
    parser.add_argument("component", choices=("cargo", "javascript", "all"), nargs="?", default="all")
    args = parser.parse_args()
    if args.stage == "source" and not os.environ.get("RELEASE_TAG"):
        os.environ["RELEASE_TAG"] = "v" + tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    identity = source_identity()
    if args.stage == "source":
        print(json.dumps(identity))
        return
    directory = artifacts_directory(identity)
    components = ("cargo", "javascript") if args.component == "all" else (args.component,)
    with release_lock(directory):
        require(not any(os.environ.get(name) for name in TOKENS), "Publication credentials must be absent from preparation")
        for component in components:
            (prepare_cargo if component == "cargo" else prepare_javascript)(directory, identity)
    print(json.dumps({"stage": args.stage, "component": args.component, "artifacts": str(directory), **identity}))


if __name__ == "__main__":
    try:
        main()
    except (Failure, OSError, KeyError, ValueError, subprocess.CalledProcessError) as error:
        # Avoid printing command arguments: JSR's supported token argument is
        # secret, and child diagnostics can contain authentication context.
        print(str(error) if isinstance(error, Failure) else f"Release failed ({type(error).__name__}); retained artifacts and journals were not removed", file=sys.stderr)
        sys.exit(1)
