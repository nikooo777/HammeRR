import argparse
import hashlib
import io
import itertools
import json
import math
import struct
import zipfile
from pathlib import Path


COUNT = 32
POINTS = [(0, 0, 16), (1, 0, 16), (1, 1, 16), (0, 1, 16)]
HARDWARE = [2, 0, 2, 1, 3, 3]
FACE_FORMAT = '<HBBihhhh4Bif2i2ii2HI'
MAX_BSP_BYTES = 16 * 1024 * 1024
ROUNDING_INTERVAL = 1e-9


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data.encode() if isinstance(data, str) else data)


def fixtures(output, variant='base'):
    from prop_indirect_fixtures import box, entity, model, texture

    require(not output.exists() or (output.is_dir() and not any(output.iterdir())), 'Fixture output must be new or empty')
    require(variant in ('base', 'strong'), 'Unknown fixture variant')
    rho, intensity = (.875, 128) if variant == 'strong' else (.5, 256)
    game = output / 'game'
    model(game, 'receiver', [(0,0,0),(1,0,0),(1,1,0),(0,1,0)])
    vvd_path = game / 'models/oracle/receiver.vvd'
    vvd = bytearray(vvd_path.read_bytes())
    for index in range(4):
        struct.pack_into('<3f', vvd, 64 + index * 48 + 28, 0, 0, -1)
    write(vvd_path, vvd)
    for name, reflectivity in [('room', (rho,rho,rho)), ('white', (0,0,0))]:
        write(game / f'materials/hammerr628/{name}.vtf', texture(reflectivity))
        write(game / f'materials/hammerr628/{name}.vmt', f'LightmappedGeneric {{ $basetexture "hammerr628/{name}" }}\n')
    write(game / 'materials/models/oracle/receiver.vmt', 'VertexLitGeneric { $model 1 $basetexture "hammerr628/white" }\n')
    write(output / 'gameinfo.txt', 'GameInfo { game "HammeRR prop depth reference" FileSystem { SteamAppId 240 SearchPaths { game "|gameinfo_path|game" } } }\n')
    bounds = [((-272,-272,-16),(272,272,0)), ((-272,-272,256),(272,272,272)),
              ((-272,-272,0),(-256,272,256)), ((256,-272,0),(272,272,256)),
              ((-256,-272,0),(256,-256,256)), ((-256,256,0),(256,272,256))]
    solids = [box(100 + index, low, high, 'hammerr628/room', scale=64) for index, (low, high) in enumerate(bounds)]
    light = entity(9001, 'light', origin='0 0 128', _light=f'255 0 0 {intensity}', _lightHDR=f'0 0 255 {intensity * 2}',
                   _constant_attn='1', _linear_attn='0', _quadratic_attn='0', style='0')
    receiver = entity(9002, 'prop_static', origin='0 0 16', angles='0 0 0', model='models/oracle/receiver.mdl',
                      solid='0', skin='0', disableshadows='1', disablevertexlighting='0', disableselfshadowing='0')
    player = entity(9003, 'info_player_terrorist', origin='0 -192 8')
    vmf = 'versioninfo { "editorversion" "400" "mapversion" "1" "formatversion" "100" }\nworld { "id" "1" "classname" "worldspawn" "mapversion" "1"\n' + '\n'.join(solids) + '\n}\n' + '\n'.join([light, receiver, player]) + '\n'
    write(output / 'depth-room.vmf', vmf)
    contract = {
        'version': 1, 'variant': variant, 'quality': 'Fast32 only', 'world_sources': 'Separate bake-static-prop-vertices=false outputs W0 through W4',
        'meaning': 'Transfer contract PN=Dprop+Tprop(W(N-1)); the published world solver output is input, not independently validated here',
        'room': {'xy': [-256,256], 'z': [0,256], 'all_six_materials': 'hammerr628/room', 'rho': [rho,rho,rho], 'lightmap_scale': 64},
        'light': {'origin': [0,0,128], 'constant_attenuation': 1, 'linear_attenuation': 0, 'quadratic_attenuation': 0,
                  'ldr': [intensity,0,0], 'hdr': [0,0,intensity * 2], 'sun_and_sky': False},
        'receiver': {'points': POINTS, 'normal_authored_in_VVD': [0,0,-1], 'angles': [0,0,0], 'direct': [0,0,0],
                     'disableshadows': True, 'checksum': 619, 'hardware_root_ids': HARDWARE},
        'proof': 'Fast32 maximum floor radius including local extent is16*sqrt(63)+1<128<256; every ray first hits the one floor quad',
        'acceptance': 'Every channel/depth matches independent intervals; P2/P3/P4 differ from their predecessor and all wrong-final WN sources differ, including N4',
        'far_filter': 'Use published RGBExp32 face average, not a recomputed mean of quantized luxels',
        'roundoff_interval_linear': ROUNDING_INTERVAL,
    }
    write(output / 'contract.json', json.dumps(contract, indent=2) + '\n')
    files = sorted(path for path in output.rglob('*') if path.is_file())
    write(output / 'files.json', json.dumps({str(path.relative_to(output)): {'bytes': path.stat().st_size, 'sha256': digest(path.read_bytes())} for path in files}, indent=2) + '\n')


def read_contract(path):
    require(path.stat().st_size <= 8192, 'Contract exceeds fixture scope')
    raw = path.read_bytes()
    contract = json.loads(raw)
    room, light, receiver = (contract[key] for key in ('room','light','receiver'))
    require(contract['version'] == 1 and contract['quality'] == 'Fast32 only', 'Unsupported reference contract')
    require(room['xy'] == [-256,256] and room['z'] == [0,256] and room['lightmap_scale'] == 64
            and room['all_six_materials'] == 'hammerr628/room', 'Contract does not describe the known room')
    rho = room['rho'][0]
    require(rho in (.5,.875) and room['rho'] == [rho,rho,rho], 'Unsupported literal reflectivity')
    intensity = 256 if rho == .5 else 128
    require(light == {'origin': [0,0,128], 'constant_attenuation': 1, 'linear_attenuation': 0, 'quadratic_attenuation': 0,
                      'ldr': [intensity,0,0], 'hdr': [0,0,intensity * 2], 'sun_and_sky': False}, 'Contract lighting differs from the literal fixture')
    require(receiver == {'points': [list(point) for point in POINTS], 'normal_authored_in_VVD': [0,0,-1], 'angles': [0,0,0], 'direct': [0,0,0],
                         'disableshadows': True, 'checksum': 619, 'hardware_root_ids': HARDWARE}, 'Contract receiver differs from the literal fixture')
    manifest_path = path.parent / 'files.json'
    require(manifest_path.stat().st_size <= 16384, 'Fixture manifest exceeds reference scope')
    manifest = json.loads(manifest_path.read_bytes())
    require(manifest['contract.json'] == {'bytes': len(raw), 'sha256': digest(raw)}, 'Contract bytes differ from its fixture manifest')
    source = path.parent / 'game/materials/hammerr628/room.vtf'
    require(source.stat().st_size == 144, 'Reflectivity VTF differs from the literal source layout')
    vtf = source.read_bytes()
    require(vtf[:4] == b'VTF\0' and struct.unpack_from('<3I', vtf, 4) == (7,2,80)
            and struct.unpack_from('<3f', vtf, 32) == (rho,rho,rho), 'Reflectivity VTF does not match the contract')
    require(manifest['game/materials/hammerr628/room.vtf'] == {'bytes': len(vtf), 'sha256': digest(vtf)}, 'Reflectivity VTF bytes differ from the manifest')
    return {'variant': 'base' if rho == .5 else 'strong', 'rho': rho, 'sha256': digest(raw), 'vtf_sha256': digest(vtf)}


def lump(raw, index, versions=(0,)):
    offset, size, version, compression = struct.unpack_from('<4i', raw, 8 + index * 16)
    require(version in versions and compression == 0, f'Unsupported lump metadata: {index}')
    require(size >= 0 and 0 <= offset <= len(raw) and size <= len(raw) - offset, f'Invalid lump range: {index}')
    require(size == 0 or offset >= 1036, f'Lump overlaps BSP header: {index}')
    return memoryview(raw)[offset:offset + size]


def records(raw, index, layout, maximum, versions=(0,)):
    payload = lump(raw, index, versions)
    stride = struct.calcsize(layout)
    require(len(payload) % stride == 0 and len(payload) // stride <= maximum, f'Out-of-scope record count: {index}')
    return list(struct.iter_unpack(layout, payload))


def rgbexp(raw):
    red, green, blue, exponent = struct.unpack('<3Bb', raw)
    value = [math.ldexp(channel, exponent) for channel in (red, green, blue)]
    require(all(math.isfinite(channel) and 0 <= channel <= 4096 for channel in value), 'Source lighting is outside the fixture range')
    return value


def floor_source(raw, channel, rho):
    face_slot, light_slot = (7, 8) if channel == 'ldr' else (58, 53)
    faces = records(raw, face_slot, FACE_FORMAT, 64, (0,) if channel == 'ldr' else (1,))
    planes = records(raw, 1, '<4fi', 128)
    infos = records(raw, 6, '<16f2i', 64)
    vertices = records(raw, 3, '<3f', 128)
    edges = records(raw, 12, '<2H', 256)
    surfedges = records(raw, 13, '<i', 256)
    models = records(raw, 14, '<9f3i', 1)
    texdata = records(raw, 2, '<3f5i', 64)
    require(len(models) == 1, 'Reference requires exactly one world model')
    first, count = models[0][10:12]
    require(first >= 0 and count == 6 and first + count <= len(faces), 'Reference requires the sealed six-face world')
    selected = []
    for index in range(first, first + count):
        face = faces[index]
        require(face[0] < len(planes) and 0 <= face[5] < len(infos), 'Invalid face plane or texinfo')
        plane = planes[face[0]]
        if plane[:4] != (0.0,0.0,1.0,0.0):
            continue
        require(face[4] == 4 and face[3] >= 0 and face[3] + 4 <= len(surfedges), 'Floor is not one native quad')
        points = []
        for (signed,) in surfedges[face[3]:face[3] + 4]:
            require(abs(signed) < len(edges), 'Invalid floor edge')
            vertex = edges[abs(signed)][0 if signed >= 0 else 1]
            require(vertex < len(vertices), 'Invalid floor vertex')
            points.append(vertices[vertex])
        require(set(points) == {(-256.0,-256.0,0.0), (256.0,-256.0,0.0), (256.0,256.0,0.0), (-256.0,256.0,0.0)}, 'Floor does not match the fixture square')
        info = infos[face[5]]
        require(info[16] & (0x2 | 0x4 | 0x80 | 0x400 | 0x800) == 0, 'Floor sky/nodraw/unlit/bump layout is unsupported')
        require(face[6] == -1 and face[8:12] == (0,255,255,255), 'Displaced or styled floor is unsupported')
        require(0 <= info[17] < len(texdata) and texdata[info[17]][:3] == (rho,rho,rho), 'Floor reflectivity does not match the literal material')
        dimensions = [face[16] + 1, face[17] + 1]
        require(all(1 <= value <= 33 for value in dimensions), 'Floor luxel dimensions exceed fixture scope')
        lighting = lump(raw, light_slot, (1,))
        offset, total = face[12], dimensions[0] * dimensions[1]
        require(len(lighting) % 4 == 0 and offset >= 4 and offset % 4 == 0 and offset + total * 4 <= len(lighting), 'Invalid floor lighting range')
        samples = [rgbexp(lighting[offset + index * 4:offset + (index + 1) * 4]) for index in range(total)]
        average = rgbexp(lighting[offset - 4:offset])
        active = 0 if channel == 'ldr' else 2
        require(all(color[axis] == 0 for color in [average, *samples] for axis in range(3) if axis != active), 'World source channels are mixed')
        projection = [list(info[8:12]), list(info[12:16])]
        require(all(math.isfinite(value) for row in projection for value in row), 'Nonfinite lightmap projection')
        selected.append({'face': index, 'rho': rho, 'dimensions': dimensions, 'mins': list(face[14:16]), 'projection': projection,
                         'average': average, 'samples': samples, 'lighting_offset': offset,
                         'average_raw': bytes(lighting[offset - 4:offset]).hex(),
                         'samples_sha256': digest(lighting[offset:offset + total * 4])})
    require(len(selected) == 1, 'Reference requires exactly one native upward floor face')
    return selected[0]


def read_world(path, rho):
    require(path.stat().st_size <= MAX_BSP_BYTES, 'World BSP exceeds the 16 MiB reference limit')
    with path.open('rb') as stream:
        raw = stream.read(MAX_BSP_BYTES + 1)
    require(len(raw) <= MAX_BSP_BYTES, 'World BSP exceeds the 16 MiB reference limit')
    require(len(raw) >= 1036 and raw[:4] == b'VBSP' and struct.unpack_from('<i', raw, 4)[0] in (19,20), 'Unsupported BSP header')
    flags = lump(raw, 59)
    require(len(flags) in (0,4) and (not flags or struct.unpack('<I', flags)[0] & 3 == 0), 'World source advertises generated prop lighting')
    pak = lump(raw, 40)
    if pak:
        with zipfile.ZipFile(io.BytesIO(pak)) as archive:
            require(len(archive.infolist()) <= 128, 'World PAK exceeds fixture scope')
            require(all(not entry.filename.lower().endswith('.vhv') and entry.filename.lower() != 'hammerr/generated-prop-lighting.json' for entry in archive.infolist()), 'World reference contains generated prop lighting')
    return {'file': path.name, 'bytes': len(raw), 'sha256': digest(raw),
            'channels': {channel: floor_source(raw, channel, rho) for channel in ('ldr','hdr')}}


def floor_value(source, x, y, distance):
    width, height = source['dimensions']
    uv = []
    for row, minimum, maximum in zip(source['projection'], source['mins'], (width - 1, height - 1), strict=True):
        coordinate = row[0] * x + row[1] * y + row[3] - minimum
        uv.append(min(max(coordinate, 0), maximum))
    u, v = uv
    x0, y0 = math.floor(u), math.floor(v)
    x1, y1 = min(x0 + 1, width - 1), min(y0 + 1, height - 1)
    weights = [((x0,y0), (1-u+x0)*(1-v+y0)), ((x1,y0), (u-x0)*(1-v+y0)),
               ((x0,y1), (1-u+x0)*(v-y0)), ((x1,y1), (u-x0)*(v-y0))]
    local = [sum(source['samples'][yy * width + xx][axis] * weight for (xx, yy), weight in weights) for axis in range(3)]
    blend = min(1, max(0, (distance * (math.sqrt(63) / 31) - 20) / 20))
    return [source['rho'] * ((1 - blend) * local[axis] + blend * source['average'][axis]) for axis in range(3)]


def predict_linear(source, point):
    colors = []
    for index in range(COUNT):
        fraction = (2 * index + 1) / 64
        angle = index * 2.399963229728653
        radius = point[2] * math.sqrt(fraction / (1 - fraction))
        x, y = point[0] - radius * math.cos(angle), point[1] + radius * math.sin(angle)
        require(-256 < x < 256 and -256 < y < 256, 'A prop ray leaves the known floor quad')
        colors.append(floor_value(source, x, y, point[2] / math.sqrt(1 - fraction)))
    return [math.fsum(color[axis] for color in colors) / COUNT for axis in range(3)]


def f32(value):
    return struct.unpack('<f', struct.pack('<f', value))[0]


def packed_color(rgb, exponent):
    scale = f32(math.ldexp(1, exponent) / 255)
    result = []
    for mantissa in rgb:
        index = min(4095, max(0, round(f32(f32(mantissa * scale) * 1024))))
        value = f32((index / 1024) ** (1 / f32(2.2)))
        result.append(round(f32(f32(value * .5) * 255)))
    return [result[2], result[1], result[0], 255]


def exponent_for(value):
    return max(-128, math.frexp(value)[1] - 8) if value else 0


def encoded_interval(linear):
    require(all(math.isfinite(value) and value >= 0 for value in linear), 'Invalid reflected prediction')
    exponent = exponent_for(max(linear))
    rgb = [min(255, int(math.ldexp(value, -exponent))) for value in linear]
    lower = [max(0, value - ROUNDING_INTERVAL) for value in linear]
    upper = [value + ROUNDING_INTERVAL for value in linear]
    candidates = []
    for exp in sorted({exponent_for(max(lower)), exponent_for(max(upper))}):
        mantissas = [sorted({min(255, int(math.ldexp(value, -exp))) for value in interval}) for interval in zip(lower, upper, strict=True)]
        candidates.extend(packed_color(values, exp) for values in itertools.product(*mantissas))
    return {'linear': linear, 'rgbexp_rgb': rgb, 'rgbexp_exponent': exponent, 'bgra_exact': packed_color(rgb, exponent),
            'bgra_interval': [[min(color[axis] for color in candidates), max(color[axis] for color in candidates)] for axis in range(4)]}


def separated(previous, current, axis):
    witnesses = []
    for vertex, (left, right) in enumerate(zip(previous, current, strict=True)):
        a, b = left['bgra_interval'][axis], right['bgra_interval'][axis]
        if a[1] < b[0] or b[1] < a[0]:
            witnesses.append(vertex)
    return witnesses


def reference(paths, contract_path, output):
    require(len(paths) == 5, 'Supply exactly five separately compiled world BSPs, ordered W0 through W4')
    require(not output.exists(), 'Reference output already exists')
    contract = read_contract(contract_path)
    worlds = [read_world(path, contract['rho']) for path in paths]
    predictions = []
    for world in worlds:
        predictions.append({channel: [encoded_interval(predict_linear(world['channels'][channel], point)) for point in POINTS] for channel in ('ldr','hdr')})
    separation = {}
    for depth in range(2, 6):
        separation[f'P{depth}-P{depth-1}'] = {
            channel: separated(predictions[depth - 2][channel], predictions[depth - 1][channel], 2 if channel == 'ldr' else 0)
            for channel in ('ldr','hdr')}
    admitted = all(vertices for key, channels in separation.items() if key != 'P5-P4' for vertices in channels.values())
    timing_admitted = admitted and all(separation['P5-P4'].values())
    report = {'version': 1, 'quality': 32, 'contract': contract, 'caller_declared_world_order': 'W0,W1,W2,W3,W4; compiler commands/settings must be retained separately',
              'scope': 'Independent prop transfer from separately published world inputs, not an oracle for correctness of the world solver',
              'formula': 'PN=(rho/32)*sum((1-blend)*bilinear(decoded W(N-1))+blend*decoded stored face average); Dprop=0',
              'roundoff_interval_linear': ROUNDING_INTERVAL, 'hardware_root_ids': HARDWARE,
              'depth_separation_admitted': bool(admitted), 'all_wrong_final_sources_separated': bool(timing_admitted),
              'separation_witness_root_vertices': separation,
              'expected': {f'N{index+1}': {'world_source': index, 'root_vertices': prediction,
                                          'hardware_vertices': {channel: [prediction[channel][vertex] for vertex in HARDWARE] for channel in ('ldr','hdr')}} for index, prediction in enumerate(predictions[:4])},
              'wrong_final_source_predictions': {f'N{index}': predictions[index] for index in range(1,5)},
              'worlds': [{**{key: value for key, value in world.items() if key != 'channels'},
                          'floor': {channel: {key: value for key, value in source.items() if key != 'samples'} for channel, source in world['channels'].items()}} for world in worlds]}
    write(output, json.dumps(report, indent=2) + '\n')
    require(admitted, 'Fixture fails depth admission: at least one P2/P3/P4 channel overlaps its predecessor; see report')
    require(timing_admitted, 'Fixture fails timing admission: a wrong-final WN source has overlapping VHV intervals; see report')
    return report


def main():
    parser = argparse.ArgumentParser(description='Prepare and independently predict the Fast32 prop bounce-depth fixture.')
    commands = parser.add_subparsers(dest='command', required=True)
    prepare = commands.add_parser('fixtures')
    prepare.add_argument('--output', type=Path, required=True)
    prepare.add_argument('--variant', choices=('base','strong'), default='base')
    predict = commands.add_parser('reference')
    predict.add_argument('--world', type=Path, action='append', required=True, help='World-only BSP; repeat in order W0 through W4.')
    predict.add_argument('--contract', type=Path, required=True, help='Generated fixture contract.json.')
    predict.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.command == 'fixtures':
        fixtures(args.output.expanduser().resolve(), args.variant)
        print('Prepared depth fixture; world and prop compiler admission have not run.')
    else:
        report = reference([path.expanduser().resolve() for path in args.world], args.contract.expanduser().resolve(), args.output.expanduser().resolve())
        print(f'Depth intervals separated: {report["depth_separation_admitted"]}; wrong-final-source intervals separated: {report["all_wrong_final_sources_separated"]}.')


if __name__ == '__main__':
    main()
