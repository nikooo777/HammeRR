import argparse
import hashlib
import json
import math
import struct
from pathlib import Path


EXPECTED = {
    'version': 2,
    'status': 'Prepared inputs; compiler admission and lighting observations are required',
    'scope': 'N1 energy and source timing; no general N2..4 depth or native acceptance claim',
    'source': {
        'sun_seek_direction_proportional': [-1024, 0, 1],
        'authored_white_floor_ldr': [64.125, 64.125, 64.125],
        'authored_white_floor_hdr': [128.125, 128.125, 128.125],
        'white_floor_W0_ldr': [64, 64, 64],
        'white_floor_W0_hdr': [128, 128, 128],
        'floor_rho': [0.5, 0.25, 0.125],
        'required_observation': 'Every sample on every source reflector face must have the expected W0, including perimeter samples and CSG subdivisions',
    },
    'geometry': {
        'main_room_xy': [-2048, 2048], 'room_z': [0, 256],
        'main_reflector_xy': [-1024, 1024], 'reflector_to_sky_wall_gap': 1024,
        'floor_partition': 'Central reflector plus four nonoverlapping nodraw ring boxes at z=0, with distinct materials',
        'main_ray_bound': '24*sqrt(1023)+1 < 768.625 < reflector half-width1024',
        'sun_height_bound': '0.5+4096/1024 = 4.5 < blocker minimum z8',
        'blocker_ray_bound': '4*sqrt(1023)+1 < 128.938 < plate half-width256',
    },
    'white_N1': {
        'ldr': {'linear': [32, 16, 8], 'rgbexp_rgb': [128, 64, 32], 'rgbexp_exponent': -2,
                'bgra_exact': [26, 36, 50, 255], 'bgra_interval': [[26, 26], [36, 36], [49, 50], [255, 255]]},
        'hdr': {'linear': [64, 32, 16], 'rgbexp_rgb': [128, 64, 32], 'rgbexp_exponent': -1,
                'bgra_exact': [36, 50, 68, 255], 'bgra_interval': [[36, 36], [49, 50], [68, 68], [255, 255]]},
    },
    'red_blue_N1': {
        'floor_W0_ldr': [64, 0, 0], 'floor_W0_hdr': [0, 0, 128],
        'ldr': {'linear': [32, 0, 0], 'rgbexp_rgb': [128, 0, 0], 'rgbexp_exponent': -2,
                'bgra_exact': [0, 0, 50, 255], 'bgra_interval': [[0, 0], [0, 0], [49, 50], [255, 255]]},
        'hdr': {'linear': [0, 0, 16], 'rgbexp_rgb': [0, 0, 128], 'rgbexp_exponent': -3,
                'bgra_exact': [36, 0, 0, 255], 'bgra_interval': [[36, 36], [0, 0], [0, 0], [255, 255]]},
    },
    'blocked_N1': {'linear': [0, 0, 0], 'both_channels_bgra': [0, 0, 0, 255],
                   'required_observation': 'Source floor W0 remains uniform white64 LDR/128 HDR despite receiver-path plate'},
    'up_normal': {'indirect_linear': [0, 0, 0], 'ldr_direct_linear': [64.125, 64.125, 64.125],
                  'hdr_direct_linear': [128.125, 128.125, 128.125],
                  'ldr_bgra': [68, 68, 68, 255], 'hdr_bgra': [93, 93, 93, 255]},
    'sky_control': {'floor_rho': [0, 0, 0], 'indirect_linear': [0, 0, 0],
                    'ldr_linear': [0, 0, 64.125], 'hdr_linear': [0, 128.125, 0],
                    'ldr_bgra': [68, 0, 0, 255], 'hdr_bgra': [0, 93, 0, 255]},
    'timing': {
        'quality': 32, 'reflector_xy': [-192, 192], 'reflector_lightmap_scale': 64,
        'positive_X_wall': 256, 'reflector_to_wall_gap': 64, 'wall_rho': [1, 1, 1],
        'wall_direct_ldr': [65664, 65664, 65664], 'wall_W0_ldr': [65536, 65536, 65536],
        'wall_direct_hdr': [131200, 131200, 131200], 'wall_W0_hdr': [131072, 131072, 131072],
        'prop_ray_bound': '16*sqrt(63)+1 < 127.997 < reflector half-width192',
        'continuous_positive_wall_path': {'origin': [0, 0, 0.5], 'direction_proportional': [2, 0, 1], 'first_hit': [256, 0, 128.5]},
        'required_observations': ['Uniform reflector W0', 'All prop Fast32 rays first hit reflector',
                                  'Central floor W1>W0 in each channel', 'Wrong-W1 source would change prop bytes beyond white_N1 intervals'],
        'W1_exact_value': 'Not simulated or claimed',
    },
    'profiles': {
        'a-white': {'quality': [32, 128, 512], 'N1': 'white_N1', 'N0': 'black'},
        'a-height-8': {'quality': [32, 128, 512], 'N1': 'white_N1', 'N0': 'black'},
        'a-height-24': {'quality': [32, 128, 512], 'N1': 'white_N1', 'N0': 'black'},
        'a-near-source': {'quality': [32, 128, 512], 'N1': 'white_N1', 'N0': 'black'},
        'a-red-blue': {'quality': [32, 128, 512], 'N1': 'red_blue_N1', 'N0': 'black'},
        'a-block-world': {'quality': [32, 128, 512], 'N1': 'blocked_N1', 'N0': 'black'},
        'a-block-terrain': {'quality': [32, 128, 512], 'N1': 'blocked_N1', 'N0': 'black'},
        'a-block-prop': {'quality': [32, 128, 512], 'N1': 'blocked_N1', 'N0': 'black'},
        'a-up-normal': {'quality': [32, 128, 512], 'N1': 'up_normal', 'N0': 'up_normal'},
        'b-sky-no-doublecount': {'quality': [32, 128, 512], 'N1': 'sky_control', 'N0': 'sky_control'},
        'a-source-timing-fast': {'quality': [32], 'N1': 'white_N1', 'N0': 'black', 'additional': 'timing'},
    },
    'black': [0, 0, 0, 255],
    'near_source': {'height': 0.00001, 'native_f32_height': 0.000009999999747378752,
                    'vertical_source_fraction': 1.5258788677030566e-10, 'receiver_bias': 0},
    'quantization_interval': 'Only +/-1e-9 linear roundoff around the independent constant field before RGBExp32 truncation; no broadened energy tolerance',
    'hardware': {'checksum': 619, 'receiver_ordinal': 0, 'root_vertex_references': [2, 0, 2, 1, 3, 3],
                 'all_six_colors_must_match': True, 'blocker_ordinal': 1, 'blocker_receives_vertex_lighting': False},
}


def write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data.encode() if isinstance(data, str) else data)


def put(data, offset, value):
    struct.pack_into('<i', data, offset, value)


def texture(reflectivity):
    data = bytearray(144)
    data[:4] = b'VTF\0'
    struct.pack_into('<3I', data, 4, 7, 2, 80)
    struct.pack_into('<2H', data, 16, 4, 4)
    struct.pack_into('<2H', data, 24, 1, 0)
    struct.pack_into('<3f', data, 32, *reflectivity)
    struct.pack_into('<fI', data, 48, 1, 0)
    data[56] = 1
    struct.pack_into('<i', data, 57, -1)
    struct.pack_into('<H', data, 63, 1)
    data[80:] = b'\xff' * 64
    return data


def model(game, stem, points):
    mdl = bytearray(1024)
    mdl[:4] = b'IDST'
    fields = [(4, 48), (8, 619), (76, 1024), (152, 16), (204, 1), (208, 256),
              (212, 1), (216, 384), (220, 1), (224, 1), (228, 388), (232, 1),
              (236, 400), (256, 464), (384, 752), (404, 1), (408, 1), (412, 16),
              (488, 1), (492, 160), (496, 4), (580, -160), (584, 4), (628, 4)]
    for offset, value in fields:
        put(mdl, offset, value)
    low = [min(point[i] for point in points) for i in range(3)]
    high = [max(point[i] for point in points) for i in range(3)]
    low[2], high[2] = -0.125, 0.125
    for offset in (104, 128):
        struct.pack_into('<6f', mdl, offset, *low, *high)
    mdl[720:729], mdl[752:766] = b'receiver\0', b'models/oracle\0'
    vvd = bytearray(256)
    vvd[:4] = b'IDSV'
    for offset, value in [(4, 4), (8, 619), (12, 1), (16, 4), (56, 64)]:
        put(vvd, offset, value)
    for index, point in enumerate(points):
        struct.pack_into('<6f', vvd, 64 + index * 48 + 16, *point, 0, 0, 1)
    vtx = bytearray(349)
    for offset, value in [(0, 7), (16, 619), (20, 1), (24, 173), (28, 1), (32, 36),
                          (36, 1), (40, 8), (44, 1), (48, 8), (52, 1), (56, 12),
                          (64, 1), (68, 9), (73, 6), (77, 183), (81, 6), (85, 237),
                          (89, 1), (93, 249), (322, 6), (330, 6)]:
        put(vtx, offset, value)
    vtx[97], vtx[340] = 2, 1
    for index, original in enumerate((2, 0, 2, 1, 3, 3)):
        struct.pack_into('<H', vtx, 260 + index * 9, original)
    struct.pack_into('<6H', vtx, 310, 1, 3, 0, 1, 0, 4)
    for suffix, data in [('mdl', mdl), ('vvd', vvd), ('dx90.vtx', vtx)]:
        write(game / f'models/oracle/{stem}.{suffix}', data)


def vector(values):
    return ' '.join(str(value) for value in values)


def box(identifier, low, high, material, terrain=False, scale=256):
    x0, y0, z0 = low
    x1, y1, z1 = high
    planes = [((x0,y0,z0),(x0,y1,z0),(x0,y1,z1)), ((x1,y1,z0),(x1,y0,z0),(x1,y0,z1)),
              ((x1,y0,z0),(x0,y0,z0),(x0,y0,z1)), ((x0,y1,z0),(x1,y1,z0),(x1,y1,z1)),
              ((x0,y0,z0),(x1,y0,z0),(x1,y1,z0)), ((x1,y0,z1),(x0,y0,z1),(x0,y1,z1))]
    sides = []
    for index, points in enumerate(planes):
        disp = ''
        if terrain and index == 5:
            normals = '\n'.join(f'"row{i}" "' + ' '.join(['0 0 1'] * 5) + '"' for i in range(5))
            distances = '\n'.join(f'"row{i}" "0 0 0 0 0"' for i in range(5))
            disp = f'dispinfo {{ "power" "2" "startposition" "[{x1} {y0} {z1}]" "elevation" "0" "subdiv" "0" normals {{ {normals} }} distances {{ {distances} }} }}'
        plane = ' '.join(f'({vector(point)})' for point in points)
        axis = index // 2
        uaxis = ['[0 1 0 0]', '[1 0 0 0]', '[1 0 0 0]'][axis]
        vaxis = ['[0 0 -1 0]', '[0 0 -1 0]', '[0 -1 0 0]'][axis]
        sides.append(f'side {{ "id" "{identifier*10+index}" "plane" "{plane}" "material" "{material}" "uaxis" "{uaxis} 0.25" "vaxis" "{vaxis} 0.25" "rotation" "0" "lightmapscale" "{scale}" "smoothing_groups" "0" {disp} }}')
    return f'solid {{ "id" "{identifier}" ' + '\n'.join(sides) + ' }'


def entity(identifier, classname, **properties):
    return 'entity { ' + '\n'.join(f'"{key}" "{value}"' for key, value in {'id': identifier, 'classname': classname, **properties}.items()) + ' }'


def scene(output, name, white=True, blocker=None, height=16, up=False, sky_control=False, timing=False):
    half, east = (192, 256) if timing else (1024, 2048)
    floor = 'hammerr619/black' if sky_control else 'hammerr619/reflector'
    bounds = [((-half,-half,-16),(half,half,0),floor),
              ((-2064,-2064,-16),(-half,2064,0),'hammerr619/nodraw'),
              ((half,-2064,-16),(east+16,2064,0),'hammerr619/nodraw'),
              ((-half,-2064,-16),(half,-half,0),'hammerr619/nodraw'),
              ((-half,half,-16),(half,2064,0),'hammerr619/nodraw'),
              ((-2064,-2064,256),(east+16,2064,272),'hammerr619/sky'),
              ((-2064,-2064,0),(-2048,2064,256),'hammerr619/sky'),
              ((east,-2064,0),(east+16,2064,256),'hammerr619/timing_wall' if timing else 'hammerr619/sky'),
              ((-2048,-2064,0),(east,-2048,256),'hammerr619/sky'),
              ((-2048,2048,0),(east,2064,256),'hammerr619/sky')]
    solids = [box(100 + index, *bound, scale=64 if timing and index == 0 else 256) for index, bound in enumerate(bounds)]
    if blocker in ('world', 'terrain'):
        solids.append(box(120, (-256,-256,8), (256,256,12), 'hammerr619/nodraw' if blocker == 'world' else 'hammerr619/black', terrain=blocker == 'terrain'))
    target = entity(9001, 'info_target', origin='128 0 63.875' if timing else '1024 0 63', targetname='oracle_sun_target')
    if sky_control:
        light = entity(9002, 'light_environment', origin='0 0 64', target='oracle_sun_target', _light='0 0 0 0', _lightHDR='0 0 0 0', _ambient='0 0 255 64.125', _ambientHDR='0 255 0 128.125', SunSpreadAngle='0')
    else:
        rgb0, rgb1 = ('255 255 255', '255 255 255') if white else ('255 0 0', '0 0 255')
        gain = math.sqrt(1024*1024+1)
        light = entity(9002, 'light_environment', origin='0 0 64', target='oracle_sun_target', _light=f'{rgb0} {64.125*gain:.17g}', _lightHDR=f'{rgb1} {128.125*gain:.17g}', _ambient='0 0 0 0', _ambientHDR='0 0 0 0', SunSpreadAngle='0')
    receiver = entity(9003, 'prop_static', origin=f'0 0 {height}', angles='0 0 0' if (up or sky_control) else '180 0 0', model='models/oracle/receiver.mdl', solid='0', skin='0', disableshadows='1', disablevertexlighting='0', disableselfshadowing='0')
    extras = [entity(9004, 'info_player_terrorist', origin='0 -512 8')]
    if blocker == 'prop':
        extras.append(entity(9005, 'prop_static', origin='0 0 12', model='models/oracle/blocker.mdl', solid='0', disablevertexlighting='1', disableshadows='0'))
    text = 'versioninfo { "editorversion" "400" "mapversion" "1" "formatversion" "100" }\nworld { "id" "1" "classname" "worldspawn" "mapversion" "1" "skyname" "sky_day01_01"\n' + '\n'.join(solids) + '\n}\n' + '\n'.join([target, light, receiver, *extras]) + '\n'
    write(output / f'{name}.vmf', text)


def generate(output):
    game = output / 'game'
    model(game, 'receiver', [(0,0,0),(1,0,0),(1,1,0),(0,1,0)])
    model(game, 'blocker', [(-256,-256,0),(256,-256,0),(256,256,0),(-256,256,0)])
    for name, rho in [('reflector', (.5,.25,.125)), ('black', (0,0,0)), ('white', (0,0,0)), ('timing_wall', (1,1,1))]:
        write(game / f'materials/hammerr619/{name}.vtf', texture(rho))
        write(game / f'materials/hammerr619/{name}.vmt', f'LightmappedGeneric {{ $basetexture "hammerr619/{name}" }}\n')
    write(game / 'materials/models/oracle/receiver.vmt', 'VertexLitGeneric { $model 1 $basetexture "hammerr619/white" }\n')
    write(game / 'materials/hammerr619/sky.vmt', 'UnlitGeneric { %compilesky 1 }\n')
    write(game / 'materials/hammerr619/nodraw.vmt', 'LightmappedGeneric { %compilenodraw 1 }\n')
    write(output / 'gameinfo.txt', 'GameInfo { game "HammeRR prop indirect oracle" FileSystem { SteamAppId 240 SearchPaths { game "|gameinfo_path|game" } } }\n')
    scene(output, 'a-white')
    scene(output, 'a-red-blue', white=False)
    scene(output, 'a-up-normal', up=True)
    scene(output, 'a-near-source', height=1e-5)
    for height in (8, 24):
        scene(output, f'a-height-{height}', height=height)
    for blocker in ('world', 'terrain', 'prop'):
        scene(output, f'a-block-{blocker}', blocker=blocker)
    scene(output, 'b-sky-no-doublecount', sky_control=True)
    scene(output, 'a-source-timing-fast', timing=True)
    write(output / 'expected.json', json.dumps(EXPECTED, indent=2) + '\n')
    files = sorted(path for path in output.rglob('*') if path.is_file())
    manifest = {str(path.relative_to(output)): {'bytes': path.stat().st_size, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()} for path in files}
    write(output / 'files.json', json.dumps({'version': 2, 'files': manifest}, indent=2) + '\n')


def main():
    parser = argparse.ArgumentParser(description='Prepare sealed N1 prop indirect lighting fixtures and literal expected values.')
    parser.add_argument('--output', required=True, type=Path, help='Fresh fixture package directory.')
    args = parser.parse_args()
    output = args.output.expanduser().resolve()
    if output.exists() and (not output.is_dir() or any(output.iterdir())):
        parser.error('--output must name a new or empty directory')
    output.mkdir(parents=True, exist_ok=True)
    generate(output)
    print(f'Prepared {len(EXPECTED["profiles"])} fixtures in {output}; compiler admission has not run.')


if __name__ == '__main__':
    main()
