# /// script
# dependencies = ["pillow==11.3.0"]
# ///

import hashlib
import io
import json
import pathlib
import struct
import urllib.request

from PIL import Image


ROOT = pathlib.Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "crates/render/assets/lightbulb"
CACHE = ROOT / "target/lightbulb-source"
BASE_URL = "https://dl.polyhaven.org/file/ph-assets/Models"
SOURCES = {
    "lightbulb_led.gltf": (
        "gltf/1k/lightbulb_led/lightbulb_led_1k.gltf",
        "ae23f72de657c1c25d4558d86a9e8b593c608360933f9748a91c3124d600ddb3",
    ),
    "lightbulb_led.bin": (
        "gltf/4k/lightbulb_led/lightbulb_led.bin",
        "7f0d1072f911f53fabee9d48579b9fe14c8425222062958d6fe50bc5d072a446",
    ),
    "diffuse.jpg": (
        "jpg/1k/lightbulb_led/lightbulb_led_diff_1k.jpg",
        "ace68a18d6eed07ee182ae49e8e44e60f895046204ccef633cc6f0eda5aa30e8",
    ),
    "normal.jpg": (
        "jpg/1k/lightbulb_led/lightbulb_led_nor_gl_1k.jpg",
        "70988facb59f508e47e392c5c9ced86ef61586dc49656ee06bade8ab43f9ff53",
    ),
    "arm.jpg": (
        "jpg/1k/lightbulb_led/lightbulb_led_arm_1k.jpg",
        "2c16a50d7b052d54d9f0ccf00c74dda3740430b855eae30abb9cfa439d879320",
    ),
}


def source(name):
    path, digest = SOURCES[name]
    cached = CACHE / name
    if cached.exists():
        data = cached.read_bytes()
    else:
        with urllib.request.urlopen(f"{BASE_URL}/{path}", timeout=30) as response:
            data = response.read()
        cached.write_bytes(data)
    if hashlib.sha256(data).hexdigest() != digest:
        raise ValueError(f"Source checksum changed: {name}")
    return data


def accessor(model, data, index, fmt):
    definition = model["accessors"][index]
    view = model["bufferViews"][definition["bufferView"]]
    start = view.get("byteOffset", 0) + definition.get("byteOffset", 0)
    stride = view.get("byteStride", struct.calcsize(fmt))
    return [
        struct.unpack_from(fmt, data, start + i * stride)
        for i in range(definition["count"])
    ]


def main():
    CACHE.mkdir(parents=True, exist_ok=True)
    OUTPUT.mkdir(parents=True, exist_ok=True)
    model = json.loads(source("lightbulb_led.gltf"))
    data = source("lightbulb_led.bin")
    primitive = model["meshes"][0]["primitives"][0]
    assert model["nodes"] == [{"mesh": 0, "name": "lightbulb_led"}]
    attributes = primitive["attributes"]
    positions = accessor(model, data, attributes["POSITION"], "<3f")
    normals = accessor(model, data, attributes["NORMAL"], "<3f")
    uvs = accessor(model, data, attributes["TEXCOORD_0"], "<2f")
    indices = [i[0] for i in accessor(model, data, primitive["indices"], "<H")]
    low = min(p[1] for p in positions)
    high = max(p[1] for p in positions)
    center = (low + high) / 2
    scale = 32 / (high - low)
    mesh = bytearray(struct.pack("<4sII", b"HRM1", len(positions), len(indices)))
    for (x, y, z), (nx, ny, nz), (u, v) in zip(positions, normals, uvs):
        globe = 255 if y >= 0.049 else 0
        mesh.extend(struct.pack(
            "<8f4B", x * scale, -z * scale, (y - center) * scale,
            nx, -nz, ny, u, v, 255, 255, 255, globe,
        ))
    for a, b, c in zip(indices[::3], indices[1::3], indices[2::3]):
        mesh.extend(struct.pack("<3I", a, c, b))
    (OUTPUT / "bulb.mesh").write_bytes(mesh)
    for name in ["diffuse", "normal", "arm"]:
        image = Image.open(io.BytesIO(source(f"{name}.jpg"))).convert("RGBA")
        image.resize((512, 512), Image.Resampling.LANCZOS).save(OUTPUT / f"{name}.png")
    print(f"Imported {len(positions)} vertices, {len(indices) // 3} triangles")


if __name__ == "__main__":
    main()
