#!/usr/bin/env python3
"""Prepare immutable ctypst packages, then publish those same bytes.

The provider supplies authentication, committed source and resource admission.
This command never installs tools or changes registry publishing policy.
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
import struct
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = "corbet-labs/ctypst"
CRATES = "https://crates.io/api/v1"
GITHUB = "https://api.github.com"
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
                          not (relative_directory == Path(".") and name in (".git", "target"))]
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
    require(package.get("repository") == "https://github.com/" + REPOSITORY, "Unexpected crate repository")
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
        run(["cargo", "build", "--locked", "--release", "--target", "wasm32-unknown-unknown", "--no-default-features", "--features", "wasm"], env=env)
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


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, new_url):
        return None


def request(url, *, token=None, method="GET", data=None, missing=False, binary=False):
    headers = {"User-Agent": "ctypst-release (https://github.com/" + REPOSITORY + ")", "Cache-Control": "no-cache"}
    if binary and url.startswith(f"{GITHUB}/repos/{REPOSITORY}/releases/assets/"):
        headers["Accept"] = "application/octet-stream"
    if token:
        origin = urllib.parse.urlsplit(url).netloc
        require(origin in ("crates.io", "api.github.com", "uploads.github.com"), "Refusing credential delivery to an unexpected host")
        headers["Authorization"] = token if origin == "crates.io" else "Bearer " + token
    if data is not None:
        headers["Content-Type"] = "application/octet-stream" if isinstance(data, bytes) else "application/json"
    payload = data if isinstance(data, bytes) else json.dumps(data).encode() if data is not None else None
    opener = urllib.request.build_opener(NoRedirect()) if token else urllib.request.build_opener()
    try:
        with opener.open(urllib.request.Request(url, data=payload, headers=headers, method=method), timeout=60) as response:
            body = response.read(200_000_001)
            require(len(body) <= 200_000_000, "Remote artifact exceeds size limit")
            return body if binary else json.loads(body)
    except urllib.error.HTTPError as error:
        if error.code == 302 and method == "GET" and binary and token and url.startswith(f"{GITHUB}/repos/{REPOSITORY}/releases/assets/"):
            location = error.headers.get("Location", "")
            parsed = urllib.parse.urlsplit(location)
            require(parsed.scheme == "https" and parsed.netloc.endswith(".githubusercontent.com"), "Unexpected GitHub artifact download origin")
            # Authentication is never forwarded across the download redirect.
            return request(location, binary=True)
        if error.code == 404 and missing and method == "GET":
            return None
        raise Failure(f"{method} {urllib.parse.urlsplit(url).netloc} returned HTTP {error.code}; no retry attempted") from None
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError):
        raise Failure("Remote response is unavailable or ambiguous; reconcile without repeating a mutation") from None


def remote_cargo(directory, identity, manifest):
    version = identity["version"]
    result = request(f"{CRATES}/crates/ctypst/{version}", missing=True)
    if result is None:
        return False
    filename = f"ctypst-{version}.crate"
    require(result["version"].get("yanked") is False, "Published crate is yanked")
    content = request(f"https://static.crates.io/crates/ctypst/{filename}", binary=True)
    require(digest(content) == manifest["artifacts"][filename], "Existing crates.io bytes differ from the prepared artifact")
    return True


def mutation(directory, name, identity, expected, verify, upload):
    journal = directory / "publication" / (name + ".json")
    record = {**stable_identity(identity), "destination": name, "expected": expected}
    if journal.exists():
        prior = read_json(journal)
        require(all(prior.get(key) == value for key, value in record.items()), "Publication journal belongs to another artifact")
        require(verify(), "A prior publication attempt is unresolved; inspect the destination without another upload")
        write_json(journal, {**record, "status": "verified"})
        return
    if verify():
        write_json(journal, {**record, "status": "verified"})
        return
    # Persist intent before the irreversible operation, including failure or
    # lost responses. No later invocation repeats this operation blindly.
    write_json(journal, {**record, "status": "attempted"})
    upload()
    for attempt in range(10):
        if verify():
            break
        if attempt == 9:
            raise Failure("Upload sent; registry visibility is pending. Reconcile this same journal; do not upload again")
        time.sleep(2)
    write_json(journal, {**record, "status": "verified"})


def cargo_authorization():
    token = os.environ.get("CARGO_REGISTRY_TOKEN", "")
    require(token, "CARGO_REGISTRY_TOKEN is required for an unpublished crate")
    mode = os.environ.get("RELEASE_CARGO_AUTH", "token")
    require(mode in ("token", "trusted"), "RELEASE_CARGO_AUTH must be token or trusted")
    if mode == "token":
        crate = request(f"{CRATES}/crates/ctypst", token=token)["crate"]
        require(crate.get("trustpub_only") is False, "ctypst has trusted-publishing-only enabled: Crow token publication is unavailable until the operator configures compatible registry permissions; this command never changes that setting")
    return token


def publish_cargo(directory, identity):
    manifest = load_component(directory, identity, "cargo")
    if remote_cargo(directory, identity, manifest):
        print("crates.io already contains the exact prepared crate")
        return
    token = cargo_authorization()
    archive = directory / f"ctypst-{identity['version']}.crate"
    metadata = json.dumps(crate_metadata(archive_files(archive, f"ctypst-{identity['version']}/"), identity["version"]), separators=(",", ":")).encode()
    content = archive.read_bytes()
    payload = struct.pack("<I", len(metadata)) + metadata + struct.pack("<I", len(content)) + content
    mutation(directory, "cargo", identity, manifest["artifacts"],
             lambda: remote_cargo(directory, identity, manifest),
             lambda: request(f"{CRATES}/crates/new", token=token, method="PUT", data=payload))


def remote_javascript(directory, identity, manifest, registry):
    version = identity["version"]
    if registry == "npm":
        result = request(f"https://registry.npmjs.org/@corbet-labs%2Fctypst/{version}", missing=True)
        if result is None:
            return False
        require(result["name"] == "@corbet-labs/ctypst" and result["version"] == version, "Unexpected npm release identity")
        url = result["dist"]["tarball"]
        require(url.startswith("https://registry.npmjs.org/@corbet-labs/ctypst/-/"), "Unexpected npm download origin")
        expected = next(value for name, value in manifest["artifacts"].items() if name.endswith(".tgz"))
        require(digest(request(url, binary=True)) == expected, "Existing npm archive differs from prepared bytes")
    else:
        result = request(f"https://jsr.io/@corbet-labs/ctypst/{version}_meta.json", missing=True)
        if result is None:
            return False
        local = archive_files(directory / f"ctypst-{version}-jsr.tar.gz")
        require(set(result["manifest"]) == {"/" + name for name in local}, "Existing JSR file inventory differs")
        for name, content in local.items():
            require(result["manifest"]["/" + name]["checksum"] == "sha256-" + digest(content), "Existing JSR file checksum differs")
            url = f"https://jsr.io/@corbet-labs/ctypst/{version}/" + urllib.parse.quote(name, safe="/")
            require(request(url, binary=True) == content, "Existing JSR downloaded bytes differ")
    return True


def publish_javascript(directory, identity):
    manifest = load_component(directory, identity, "javascript")
    for registry in ("npm", "jsr"):
        if remote_javascript(directory, identity, manifest, registry):
            print(f"{registry} already contains the exact prepared package")
            continue
        token = os.environ.get("NPM_TOKEN" if registry == "npm" else "JSR_TOKEN", "")
        oidc = bool(os.environ.get("ACTIONS_ID_TOKEN_REQUEST_URL"))
        require(token or oidc, f"A {registry} credential is required")
        with tempfile.TemporaryDirectory(prefix="ctypst-publish-", dir=os.environ.get("TMPDIR")) as temporary:
            work = Path(temporary)
            env = {key: value for key, value in os.environ.items() if key not in TOKENS}
            if registry == "npm":
                require(shutil.which("npm"), "Provisioned npm is required")
                archive = next(directory / name for name in manifest["artifacts"] if name.endswith(".tgz"))
                (work / "npmrc").write_text("//registry.npmjs.org/:_authToken=${NPM_TOKEN}\n" if token else "")
                (work / "npmrc").chmod(0o600)
                env.update(npm_config_userconfig=str(work / "npmrc"), npm_config_fetch_retries="0")
                if token:
                    env["NPM_TOKEN"] = token
                command = ["npm", "publish", str(archive), "--ignore-scripts", "--access=public", "--provenance=" + ("true" if oidc else "false")]
            else:
                require(shutil.which("deno"), "Provisioned Deno is required")
                unpack(archive_files(directory / f"ctypst-{identity['version']}-jsr.tar.gz"), work)
                command = ["deno", "publish", "--no-check", "--allow-dirty", "--node-modules-dir=none"]
                if token:
                    command += ["--no-provenance", "--token", token]
            mutation(directory, registry, identity, manifest["artifacts"],
                     lambda registry=registry: remote_javascript(directory, identity, manifest, registry),
                     lambda: run(command, cwd=work, env=env, capture=True))


def tag_commit(tag, token):
    obj = request(f"{GITHUB}/repos/{REPOSITORY}/git/ref/tags/{urllib.parse.quote(tag, safe='')}", token=token)["object"]
    for _ in range(4):
        if obj["type"] == "commit":
            return obj["sha"]
        require(obj["type"] == "tag", "Release tag does not identify a commit")
        obj = request(f"{GITHUB}/repos/{REPOSITORY}/git/tags/{obj['sha']}", token=token)["object"]
    raise Failure("Release tag indirection exceeds the supported limit")


def verify_tag(manifests, identity, token):
    expected = {manifest["tag_commit"] for manifest in manifests}
    require(len(expected) == 1, "Package receipts disagree on the immutable release tag")
    require(tag_commit(identity["tag"], token) == next(iter(expected)), "Remote release tag differs from the prepared package identity")


def hosted_attempt(identity):
    """Ephemeral hosted jobs cannot safely invent a fresh journal on rerun."""
    if not os.environ.get("GITHUB_RUN_ID"):
        return
    require(os.environ.get("GITHUB_RUN_ATTEMPT") == "1", "Hosted publication rerun refused; reconcile the retained bundle and journals through Crow")
    run_id = int(os.environ["GITHUB_RUN_ID"])
    runs = request(f"{GITHUB}/repos/{REPOSITORY}/actions/workflows/release.yml/runs?head_sha={identity['driver_commit']}&per_page=100", token=os.environ.get("GH_TOKEN"))
    require(runs["total_count"] <= 100 and all(item["id"] == run_id for item in runs["workflow_runs"]),
            "An earlier hosted release run exists for this source; reconcile its artifacts and journals before using the Crow fallback")


def publish_github(directory, identity):
    manifests = [load_component(directory, identity, component) for component in ("cargo", "javascript")]
    token = os.environ.get("GH_TOKEN", "")
    require(token, "GH_TOKEN is required for GitHub publication")
    verify_tag(manifests, identity, token)
    # This is the complete release, after all registries have been verified.
    require(remote_cargo(directory, identity, manifests[0]), "Cargo publication remains incomplete")
    for registry in ("npm", "jsr"):
        require(remote_javascript(directory, identity, manifests[1], registry), f"{registry} publication remains incomplete")
    lines = (ROOT / "CHANGELOG.md").read_text().splitlines()
    heading = next((index for index, line in enumerate(lines) if line.startswith("## " + identity["version"] + " ")), None)
    require(heading is not None, "Release changelog section is missing")
    end = next((index for index in range(heading + 1, len(lines)) if lines[index].startswith("## ")), len(lines))
    notes = "\n".join(lines[heading + 1:end]).strip()
    require(notes, "Release notes are empty")
    notes += "\n\nPackage provenance:\n" + "\n".join(
        f"- {name}: source `{manifest['producing_commit']}`, source SHA-256 `{manifest['producing_source_sha256']}`."
        for name, manifest in zip(("Cargo", "JavaScript"), manifests))
    notes += f"\n- Immutable tag `{identity['tag']}`: `{manifests[0]['tag_commit']}`.\n"
    title = "ctypst " + identity["version"]
    endpoint = f"{GITHUB}/repos/{REPOSITORY}/releases/tags/{identity['tag']}"
    def verify_release():
        release = request(endpoint, token=token, missing=True)
        if release is None:
            return False
        require(release["tag_name"] == identity["tag"] and release["name"] == title and release["body"] == notes, "Existing GitHub release differs; it will not be edited or overwritten")
        return True
    mutation(directory, "github-release", identity, {"title": title, "body": notes}, verify_release,
             lambda: request(f"{GITHUB}/repos/{REPOSITORY}/releases", token=token, method="POST",
                             data={"tag_name": identity["tag"], "name": title, "body": notes, "draft": True}))
    release = request(endpoint, token=token)
    assets = {name: checksum for manifest in manifests for name, checksum in manifest["artifacts"].items()}
    for component in ("cargo", "javascript"):
        name = component + ".json"
        assets[name] = digest((directory / name).read_bytes())
    inventory = {asset["name"]: asset for asset in release["assets"]}
    require(len(inventory) == len(release["assets"]) and set(inventory) <= set(assets), "Existing release has duplicate or unexpected assets; publication refused")
    require(release["draft"] or set(inventory) == set(assets), "Already-public release is incomplete; it will not be modified")
    for name, checksum in assets.items():
        def verify_asset(name=name, checksum=checksum):
            current = request(endpoint, token=token)
            found = [asset for asset in current["assets"] if asset["name"] == name]
            require(len(found) <= 1, "Duplicate GitHub release asset")
            if not found:
                return False
            url = found[0]["url"]
            require(url.startswith(f"{GITHUB}/repos/{REPOSITORY}/releases/assets/"), "Unexpected release asset endpoint")
            require(digest(request(url, token=token, binary=True)) == checksum, "Existing GitHub asset differs; overwrite refused")
            return True
        upload = f"https://uploads.github.com/repos/{REPOSITORY}/releases/{release['id']}/assets?name=" + urllib.parse.quote(name, safe="")
        mutation(directory, "github-" + name, identity, {name: checksum}, verify_asset,
                 lambda upload=upload, name=name: request(upload, token=token, method="POST", data=(directory / name).read_bytes()))
    def verify_public_release():
        current = request(endpoint, token=token)
        inventory = {asset["name"]: asset for asset in current["assets"]}
        require(len(inventory) == len(current["assets"]) and set(inventory) == set(assets), "Release asset inventory differs; publication refused")
        for name, checksum in assets.items():
            if current["draft"]:
                url = inventory[name]["url"]
                require(url.startswith(f"{GITHUB}/repos/{REPOSITORY}/releases/assets/"), "Unexpected release asset endpoint")
                content = request(url, token=token, binary=True)
            else:
                url = inventory[name]["browser_download_url"]
                require(url.startswith(f"https://github.com/{REPOSITORY}/releases/download/"), "Unexpected release download origin")
                content = request(url, binary=True)
            require(digest(content) == checksum, "Release download differs; publication refused")
        return not current["draft"]
    mutation(directory, "github-publish", identity, assets, verify_public_release,
             lambda: request(f"{GITHUB}/repos/{REPOSITORY}/releases/{release['id']}", token=token,
                             method="PATCH", data={"draft": False}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=("source", "prepare", "status", "preflight", "publish"))
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
        if args.stage == "prepare":
            require(not any(os.environ.get(name) for name in TOKENS), "Publication credentials must be absent from preparation")
            for component in components:
                (prepare_cargo if component == "cargo" else prepare_javascript)(directory, identity)
        else:
            manifests = [load_component(directory, identity, component) for component in components]
            verify_tag(manifests, identity, os.environ.get("GH_TOKEN"))
            if args.stage == "status":
                require(args.component == "cargo", "The status selector currently accepts Cargo only")
                present = {"cargo": remote_cargo(directory, identity, manifests[0])}
                if os.environ.get("GITHUB_OUTPUT"):
                    with Path(os.environ["GITHUB_OUTPUT"]).open("a") as stream:
                        stream.write("published=" + str(present["cargo"]).lower() + "\n")
                print(json.dumps(present))
                return
            if args.stage == "preflight":
                if "cargo" in components and not remote_cargo(directory, identity, manifests[0]):
                    cargo_authorization()
                if "javascript" in components:
                    javascript = manifests[components.index("javascript")]
                    for registry, tool, credential in (("npm", "npm", "NPM_TOKEN"), ("jsr", "deno", "JSR_TOKEN")):
                        if not remote_javascript(directory, identity, javascript, registry):
                            require(shutil.which(tool), f"Provisioned {tool} is required")
                            require(os.environ.get(credential) or os.environ.get("ACTIONS_ID_TOKEN_REQUEST_URL"), f"{registry} authentication is unavailable")
                if args.component == "all":
                    require(os.environ.get("GH_TOKEN"), "GH_TOKEN is required for GitHub publication")
                print("Selected artifact identities, existing registry bytes and credential availability checked; token permissions other than Cargo policy are proved only by the destination. No mutation performed")
                return
            hosted_attempt(identity)
            for component in components:
                (publish_cargo if component == "cargo" else publish_javascript)(directory, identity)
            if args.component == "all":
                publish_github(directory, identity)
    print(json.dumps({"stage": args.stage, "component": args.component, "artifacts": str(directory), **identity}))


if __name__ == "__main__":
    try:
        main()
    except (Failure, OSError, KeyError, ValueError, subprocess.CalledProcessError) as error:
        # Avoid printing command arguments: JSR's supported token argument is
        # secret, and child diagnostics can contain authentication context.
        print(str(error) if isinstance(error, Failure) else f"Release failed ({type(error).__name__}); retained artifacts and journals were not removed", file=sys.stderr)
        sys.exit(1)
