#!/usr/bin/env python3
"""Builds the MineHouse resource pack from the assets of our Fabric mods.

On the Fabric server Polymer assembles this pack at startup. Steel has no Polymer,
so the pack is built ahead of time and offered through [server.resource_pack].

Usage: build_pack.py LANDING_REPO OUT.zip

LANDING_REPO is a checkout of minehouse-landing, where the mods live. The zip is
reproducible: the same assets give the same bytes, so the printed SHA-1 only
changes when the pack does.
"""
import hashlib
import itertools
import json
import sys
import zipfile
from pathlib import Path

# Resource pack format of Minecraft 26.3.
PACK_FORMAT = 97
DESCRIPTION = "MineHouse"

# The mods whose assets Polymer puts into the server pack (they call
# PolymerResourcePackUtils.addModAssets), in load order: on a conflict that cannot
# be merged the later mod wins, and the build reports it.
MODS = [
    ("tweaks-mod", "src/main/resources/assets"),
    ("bank-mod", "src/main/resources/assets"),
    ("herbs-mod", "src/main/resources/assets"),
    ("paradise-mod", "src/main/resources/assets"),
    ("guides-mod", "src/main/resources/assets"),
    ("events-mod", "src/main/resources/assets"),
    ("cosmetics-mod", "src/main/resources/assets"),
    ("rift-mod", "src/main/resources/assets"),
    ("paint-mod", "src/main/resources/assets"),
    ("ambient-mod", "resourcepack/assets"),
]


def merge_json(path, old, new):
    """Merges two versions of a file that several mods may add to."""
    parts = path.split("/")
    old, new = json.loads(old), json.loads(new)
    if len(parts) >= 3 and parts[1] == "atlases":
        merged = {"sources": old.get("sources", []) + [s for s in new.get("sources", []) if s not in old.get("sources", [])]}
    elif len(parts) >= 3 and parts[1] == "font":
        merged = {"providers": old.get("providers", []) + [p for p in new.get("providers", []) if p not in old.get("providers", [])]}
    elif (len(parts) >= 3 and parts[1] == "lang") or parts[-1] == "sounds.json":
        merged = {**old, **new}
    else:
        return None
    return (json.dumps(merged, ensure_ascii=False, indent=2) + "\n").encode()


def collect(landing):
    """Reads the assets of every mod. Returns {path in pack: bytes} and the conflicts."""
    files, conflicts = {}, []
    for mod, assets in MODS:
        root = landing / mod / assets
        if not root.is_dir():
            sys.exit(f"{root} is missing: is {landing} a minehouse-landing checkout?")
        for file in sorted(p for p in root.rglob("*") if p.is_file()):
            path = file.relative_to(root).as_posix()
            data = file.read_bytes()
            if path in files and files[path] != data:
                merged = merge_json(path, files[path], data) if path.endswith(".json") else None
                if merged is None:
                    conflicts.append(f"{path}: {mod} replaces an earlier mod's file")
                else:
                    data = merged
            files[path] = data
    return files, conflicts


def client_blocks(files):
    """Draws our blocks over the vanilla states the server sends for them.

    client_blocks.json, next to this script, says which state of which carrier block (a
    vanilla block whose states all look alike) is whose; the server reads the same file.
    Every other state of a carrier keeps the vanilla model: a blockstate file has to name
    them all.
    """
    table = json.loads((Path(__file__).parent / "client_blocks.json").read_text())
    for carrier, entry in table["carriers"].items():
        properties = entry["properties"]
        names = sorted(properties)
        models = {}
        for slot in entry["slots"]:
            if set(slot["state"]) != set(names):
                sys.exit(f"client_blocks.json: {slot['block']} must name every property of {carrier}")
            key = tuple(slot["state"][name] for name in names)
            if key in models:
                sys.exit(f"client_blocks.json: repeated {carrier} state for {slot['block']}")
            models[key] = {"model": slot["model"]}
            # A block that faces a side is its model turned around the vertical axis.
            if slot.get("y"):
                models[key]["y"] = slot["y"]
        variants = {}
        for key in itertools.product(*(properties[name] for name in names)):
            state = ",".join(f"{name}={value}" for name, value in zip(names, key))
            variants[state] = models.pop(key, {"model": entry["vanilla_model"]})
        if models:
            sys.exit(f"client_blocks.json: no such {carrier} state: {sorted(models)}")
        path = f"minecraft/blockstates/{carrier}.json"
        if path in files:
            sys.exit(f"{path}: a mod ships its own, it would be replaced")
        files[path] = (json.dumps({"variants": variants}, separators=(",", ":")) + "\n").encode()


def resource(reference, kind, extension):
    """The pack path of a `namespace:path` reference, e.g. a model or a texture."""
    namespace, _, path = reference.rpartition(":")
    return f"{namespace or 'minecraft'}/{kind}/{path}{extension}"


def references(value, key=None):
    """Yields (key, string) for every string inside a JSON value."""
    if isinstance(value, dict):
        for k, v in value.items():
            yield from references(v, k)
    elif isinstance(value, list):
        for v in value:
            yield from references(v, key)
    elif isinstance(value, str):
        yield key, value


def check(files):
    """Finds references to models, textures and sounds of ours that the pack lacks.

    Vanilla's own resources are not in the pack, so only our namespaces are checked.
    """
    namespaces = {path.split("/")[0] for path in files} - {"minecraft"}
    missing = []

    def need(source, reference, kind, extension):
        target = resource(reference, kind, extension)
        if target.split("/")[0] in namespaces and target not in files:
            missing.append(f"{source}: {reference} ({target} is not in the pack)")

    for path, data in files.items():
        if not path.endswith(".json"):
            continue
        parts = path.split("/")
        try:
            content = json.loads(data)
        except ValueError as error:
            missing.append(f"{path}: not valid JSON ({error})")
            continue
        if parts[1] == "items":
            for key, value in references(content):
                if key == "model" and ":" in value:
                    need(path, value, "models", ".json")
        elif parts[1] == "blockstates":
            for key, value in references(content):
                if key == "model":
                    need(path, value, "models", ".json")
        elif parts[1] == "models":
            if isinstance(content.get("parent"), str):
                need(path, content["parent"], "models", ".json")
            for value in content.get("textures", {}).values():
                if isinstance(value, str) and not value.startswith("#"):
                    need(path, value, "textures", ".png")
        elif parts[1] == "font":
            for provider in content.get("providers", []):
                if isinstance(provider.get("file"), str):
                    need(path, provider["file"], "textures", "")
        elif parts[-1] == "sounds.json":
            for event in content.values():
                for sound in event.get("sounds", []):
                    name = sound if isinstance(sound, str) else sound.get("name", "")
                    need(path, name if ":" in name else f"minecraft:{name}", "sounds", ".ogg")
    return missing


def write(files, out):
    """Writes a zip whose bytes depend only on the files."""
    mcmeta = {"pack": {"description": DESCRIPTION, "min_format": PACK_FORMAT, "max_format": PACK_FORMAT}}
    entries = {"pack.mcmeta": (json.dumps(mcmeta, ensure_ascii=False, indent=2) + "\n").encode()}
    entries.update({f"assets/{path}": data for path, data in files.items()})
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as pack:
        for name in sorted(entries):
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            pack.writestr(info, entries[name])


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    landing, out = Path(sys.argv[1]), Path(sys.argv[2])
    files, conflicts = collect(landing)
    client_blocks(files)
    problems = conflicts + check(files)
    for problem in problems:
        print(f"error: {problem}", file=sys.stderr)
    if problems:
        sys.exit(1)
    write(files, out)
    sha1 = hashlib.sha1(out.read_bytes()).hexdigest()
    print(f"{out}: {len(files)} files, {out.stat().st_size} bytes")
    print(f"sha1 = \"{sha1}\"")


if __name__ == "__main__":
    main()
