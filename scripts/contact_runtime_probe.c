#define _GNU_SOURCE
#include <dlfcn.h>
#include <elf.h>
#include <fcntl.h>
#include <limits.h>
#include <link.h>
#include <math.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

enum {
    MAX_MAPS = 4096,
    MAX_ENVIRONMENTS = 16,
    MAX_OBJECTS = 65536,
    MAX_CONTACTS = 4096,
    MAX_REPORT_BYTES = 32 * 1024 * 1024,
    SAMPLE_STRIDE = 7,
    TRACKED_COUNT = 7,
    STABLE_COUNT = 3,
    SOLID_NONE = 0,
    SOLID_LAST = 7,
    FSOLID_NOT_SOLID = 4
};

typedef void *(*Factory)(const char *, int *);
typedef struct {
    uintptr_t low, high;
    bool executable;
} Mapping;
typedef struct {
    const char *name, *build_id;
    char path[PATH_MAX];
    void *handle;
    uintptr_t base;
} Module;
typedef struct {
    const char *name;
    void *entity, *edict, *object, *environment, *collide, *collideable;
    int index, object_count;
    bool seen, freed, present;
} Tracked;
typedef struct {
    float x, y, z;
} Vector;

static Mapping mappings[MAX_MAPS];
static size_t mapping_count, written;
static Module engine = {.name = "engine.so",
                        .build_id = "a897e8fd69fff915bd84769509436d61cdd528c7"};
static Module server = {.name = "server.so",
                        .build_id = "87d084fa011e265ede4aa65eb02a04112499aa7c"};
static Module physics = {.name = "vphysics.so",
                         .build_id = "ab4c2d0c4c56edf61af5a7501907d1b6aba51324"};
static Tracked tracked[TRACKED_COUNT] = {
    {.name = "blocked_crate"},
    {.name = "blocked_door"},
    {.name = "clear_door"},
    {.name = "blocked_door_side_brace"},
    {.name = "blocked_door_back_brace"},
    {.name = "clear_door_side_brace"},
    {.name = "clear_door_back_brace"},
};
static void *tools, *entities, *engine_server, *physics_system;
static int report_fd = -1;
static bool invalidated, activated, entered, loaded, level_init_seen, initial_shutdown_seen;
static unsigned long frame_count, sample_count;
static long simulation_thread;

static void report_failure(void) {
    static const char message[] = "{\"event\":\"invalidated\",\"reason\":\"report_integrity\"}\n";
    if (!invalidated && report_fd >= 0) {
        ssize_t count = write(report_fd, message, sizeof(message) - 1);
        if (count > 0)
            written += (size_t)count;
    }
    invalidated = true;
}

static bool emit(const char *format, ...) {
    if (report_fd < 0 || written >= MAX_REPORT_BYTES - 256) {
        report_failure();
        return false;
    }
    char buffer[2048];
    va_list args;
    va_start(args, format);
    int size = vsnprintf(buffer, sizeof(buffer), format, args);
    va_end(args);
    if (size <= 0 || (size_t)size >= sizeof(buffer) ||
        (size_t)size > MAX_REPORT_BYTES - 256 - written) {
        report_failure();
        return false;
    }
    ssize_t count = write(report_fd, buffer, (size_t)size);
    if (count != size) {
        report_failure();
        return false;
    }
    written += (size_t)size;
    return true;
}

static void fail(const char *reason) {
    if (!invalidated)
        emit("{\"event\":\"invalidated\",\"reason\":\"%s\",\"frame\":%lu,\"sample\":%lu}\n", reason,
             frame_count, sample_count);
    invalidated = true;
}

static bool refresh_mappings(void) {
    FILE *stream = fopen("/proc/self/maps", "re");
    if (!stream)
        return false;
    mapping_count = 0;
    char line[PATH_MAX + 256], permissions[5];
    unsigned long low, high;
    while (fgets(line, sizeof(line), stream)) {
        if (sscanf(line, "%lx-%lx %4s", &low, &high, permissions) != 3)
            continue;
        if (permissions[0] != 'r')
            continue;
        if (mapping_count == MAX_MAPS) {
            fclose(stream);
            return false;
        }
        mappings[mapping_count++] = (Mapping){low, high, strchr(permissions, 'x') != NULL};
    }
    bool success = !ferror(stream);
    fclose(stream);
    return success;
}

static bool mapped(const void *pointer, size_t bytes, bool executable) {
    uintptr_t address = (uintptr_t)pointer;
    if (!address || bytes > UINTPTR_MAX - address)
        return false;
    size_t low = 0, high = mapping_count;
    while (low < high) {
        size_t middle = low + (high - low) / 2;
        Mapping *region = &mappings[middle];
        if (address < region->low)
            high = middle;
        else if (address >= region->high)
            low = middle + 1;
        else
            return address + bytes <= region->high && (!executable || region->executable);
    }
    return false;
}

static void *method(void *object, size_t table_offset, size_t slot, Module *module, size_t target) {
    void **table;
    if (!mapped(object, sizeof(table), false)) {
        fail("unmapped_object");
        return NULL;
    }
    memcpy(&table, object, sizeof(table));
    if ((uintptr_t)table != module->base + table_offset ||
        !mapped(table, (slot + 1) * sizeof(void *), false)) {
        emit("{\"event\":\"abi_mismatch\",\"module\":\"%s\",\"slot\":%zu,\"expected_vtable\":\"%"
             "p\",\"actual_vtable\":\"%p\"}\n",
             module->name, slot, (void *)(module->base + table_offset), (void *)table);
        fail("unexpected_public_vtable");
        return NULL;
    }
    void *function = table[slot];
    if ((uintptr_t)function != module->base + target || !mapped(function, 1, true)) {
        fail("unexpected_public_method");
        return NULL;
    }
    return function;
}

static int match_build_id(struct dl_phdr_info *info, size_t size, void *data) {
    (void)size;
    Module *module = data;
    if ((uintptr_t)info->dlpi_addr != module->base)
        return 0;
    for (ElfW(Half) index = 0; index < info->dlpi_phnum; index++) {
        const ElfW(Phdr) *header = &info->dlpi_phdr[index];
        if (header->p_type != PT_NOTE || header->p_memsz > 65536)
            continue;
        const unsigned char *start = (const void *)(module->base + header->p_vaddr);
        if (!mapped(start, header->p_memsz, false))
            continue;
        size_t offset = 0;
        while (header->p_memsz - offset >= sizeof(ElfW(Nhdr))) {
            ElfW(Nhdr) note;
            memcpy(&note, start + offset, sizeof(note));
            offset += sizeof(note);
            size_t names = ((size_t)note.n_namesz + 3) & ~(size_t)3;
            size_t desc = ((size_t)note.n_descsz + 3) & ~(size_t)3;
            if (names > header->p_memsz - offset || desc > header->p_memsz - offset - names)
                break;
            if (note.n_type == NT_GNU_BUILD_ID && note.n_namesz == 4 &&
                !memcmp(start + offset, "GNU", 4) && note.n_descsz == 20) {
                char hex[41];
                for (size_t byte = 0; byte < 20; byte++)
                    snprintf(hex + byte * 2, 3, "%02x", start[offset + names + byte]);
                return !strcmp(hex, module->build_id) ? 1 : -1;
            }
            offset += names + desc;
        }
    }
    return -1;
}

static bool bind_module(Module *module, const char *path) {
    if (!realpath(path, module->path))
        return false;
    module->handle = dlopen(module->path, RTLD_NOW | RTLD_NOLOAD);
    if (!module->handle)
        return false;
    void *factory = dlsym(module->handle, "CreateInterface");
    Dl_info info;
    char actual[PATH_MAX];
    if (!factory || !dladdr(factory, &info) || !realpath(info.dli_fname, actual) ||
        strcmp(actual, module->path))
        return false;
    module->base = (uintptr_t)info.dli_fbase;
    return mapped(factory, 1, true) && dl_iterate_phdr(match_build_id, module) == 1;
}

static bool factory_origin(Factory factory, Module *module) {
    Dl_info info;
    size_t expected = module == &engine ? 0x666b80 : module == &server ? 0x938070 : 0x58b20;
    return mapped((const void *)factory, 1, true) && dladdr((const void *)factory, &info) &&
           (uintptr_t)info.dli_fbase == module->base &&
           (uintptr_t)factory == module->base + expected;
}

static void *interface(Factory factory, const char *version, Module *module, size_t table) {
    if (!factory_origin(factory, module)) {
        fail("unexpected_factory_origin");
        return NULL;
    }
    int result = -1;
    void *object = factory(version, &result);
    void *vtable = NULL;
    if (!object || result || !mapped(object, sizeof(vtable), false)) {
        fail("missing_public_interface");
        return NULL;
    }
    memcpy(&vtable, object, sizeof(vtable));
    if ((uintptr_t)vtable != module->base + table) {
        fail("unexpected_interface_vtable");
        return NULL;
    }
    emit("{\"event\":\"interface\",\"version\":\"%s\",\"module\":\"%s\",\"factory_elf\":%lu,"
         "\"vtable_elf\":%lu}\n",
         version, module->name, (unsigned long)((uintptr_t)factory - module->base),
         (unsigned long)table);
    return object;
}

static bool private_command(const char *game) {
    char command[32768];
    int fd = open("/proc/self/cmdline", O_RDONLY | O_CLOEXEC);
    if (fd < 0)
        return false;
    ssize_t count = read(fd, command, sizeof(command));
    close(fd);
    if (count <= 0 || (size_t)count == sizeof(command) || command[count - 1])
        return false;
    bool insecure = false, defaults = false, private_game = false;
    for (size_t offset = 0; offset < (size_t)count; offset += strlen(command + offset) + 1) {
        if (!strcmp(command + offset, "-insecure"))
            insecure = true;
        if (!strcmp(command + offset, "-default"))
            defaults = true;
        if (!strcmp(command + offset, "-game")) {
            size_t next = offset + strlen(command + offset) + 1;
            if (next < (size_t)count && !strcmp(command + next, game))
                private_game = true;
        }
    }
    return insecure && defaults && private_game;
}

static bool process_guard(void) {
    const char *expected = getenv("HAMMERR_CONTACT_EXPECTED_EXE");
    const char *game = getenv("HAMMERR_CONTACT_PRIVATE_GAME");
    const char *report = getenv("HAMMERR_CONTACT_REPORT");
    const char *root = getenv("HAMMERR_CONTACT_GAME_ROOT");
    char executable[PATH_MAX], actual_game[PATH_MAX], actual_root[PATH_MAX],
        expected_report[PATH_MAX], path[PATH_MAX];
    ssize_t count = readlink("/proc/self/exe", executable, sizeof(executable) - 1);
    if (!expected || !game || !report || !root || count <= 0 || !realpath(game, actual_game) ||
        strcmp(game, actual_game) || !realpath(root, actual_root) || strcmp(root, actual_root))
        return false;
    executable[count] = 0;
    if (strcmp(expected, executable) || !private_command(game))
        return false;
    size_t root_length = strlen(root), game_length = strlen(game);
    if ((!strncmp(game, root, root_length) && (game[root_length] == '/' || !game[root_length])) ||
        (!strncmp(root, game, game_length) && (root[game_length] == '/' || !root[game_length])))
        return false;
    int length = snprintf(path, sizeof(path), "%s/cstrike_linux64", root);
    if (length <= 0 || (size_t)length >= sizeof(path) || !realpath(path, actual_root) ||
        strcmp(actual_root, executable))
        return false;
    const char *end = strrchr(game, '/');
    if (!end || strcmp(end, "/cstrike"))
        return false;
    length = snprintf(expected_report, sizeof(expected_report), "%.*s/contact-observer.jsonl",
                      (int)(end - game), game);
    if (length <= 0 || (size_t)length >= sizeof(expected_report) || strcmp(report, expected_report))
        return false;
    length = snprintf(path, sizeof(path), "%s/addons/hammerr_contact_observer.so", game);
    Dl_info own;
    if (length <= 0 || (size_t)length >= sizeof(path) ||
        !dladdr((const void *)process_guard, &own) || !realpath(own.dli_fname, actual_root) ||
        strcmp(path, actual_root))
        return false;
    length = snprintf(path, sizeof(path), "%s/gameinfo.txt", game);
    if (length <= 0 || (size_t)length >= sizeof(path))
        return false;
    int info_fd = open(path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (info_fd < 0)
        return false;
    char info[16385];
    ssize_t info_size = read(info_fd, info, sizeof(info) - 1);
    close(info_fd);
    if (info_size <= 0 || (size_t)info_size == sizeof(info) - 1)
        return false;
    info[info_size] = 0;
    if (!strstr(info, "game \"HammeRR validation\""))
        return false;
    if (!refresh_mappings())
        return false;
    report_fd = open(report, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600);
    if (report_fd < 0)
        return false;
    emit("{\"event\":\"start\",\"schema\":1,\"pid\":%ld,\"observation_only\":true}\n",
         (long)getpid());
    Module *modules[] = {&engine, &server, &physics};
    const char *relative[] = {"bin/linux64/engine.so", "cstrike/bin/linux64/server.so",
                              "bin/linux64/vphysics.so"};
    for (size_t index = 0; index < 3; index++) {
        length = snprintf(path, sizeof(path), "%s/%s", root, relative[index]);
        if (length <= 0 || (size_t)length >= sizeof(path) || !bind_module(modules[index], path)) {
            fail("module_fingerprint_or_origin");
            return false;
        }
        emit("{\"event\":\"module\",\"name\":\"%s\",\"build_id\":\"%s\"}\n", modules[index]->name,
             modules[index]->build_id);
    }
    return true;
}

static bool query_key(void *entity, const char *field, char *value, int capacity, bool *returned) {
    typedef bool (*Query)(void *, void *, const char *, char *, int);
    memset(value, 0, (size_t)capacity);
    if (returned)
        *returned = false;
    Query query = (Query)method(tools, 0xb5bcc8, 11, &server, 0x7b4e20);
    if (!query)
        return false;
    bool result = query(tools, entity, field, value, capacity);
    if (returned)
        *returned = result;
    return result && memchr(value, 0, (size_t)capacity);
}

static bool key(void *entity, const char *field, char *value, int capacity) {
    return query_key(entity, field, value, capacity, NULL);
}

static void query_diagnostic(const Tracked *item, void *entity, int index, const char *field,
                             const char *value, size_t capacity, bool returned,
                             const char *expected) {
    if (capacity > 256) {
        fail("diagnostic_capacity");
        return;
    }
    size_t length = strnlen(value, capacity), preview_length = length < 64 ? length : 64;
    char hex[513], preview[385];
    for (size_t offset = 0; offset < length; offset++)
        snprintf(hex + offset * 2, 3, "%02x", (unsigned char)value[offset]);
    hex[length * 2] = 0;
    size_t used = 0;
    for (size_t offset = 0; offset < preview_length; offset++) {
        unsigned char character = (unsigned char)value[offset];
        if (character >= 32 && character <= 126 && character != '"' && character != '\\')
            preview[used++] = (char)character;
        else {
            snprintf(preview + used, 7, "\\u%04x", character);
            used += 6;
        }
    }
    preview[used] = 0;
    emit(
        "{\"event\":\"entity_query\",\"sample\":%lu,\"tracked\":\"%s\",\"entity\":\"%p\",\"index\":%d,\"field\":\"%s\",\"query_returned\":%s,\"nul_terminated\":%s,\"capacity\":%zu,\"value_bytes\":%zu,\"value_hex\":\"%s\",\"preview\":\"%s\",\"preview_complete\":%s,\"expected_checked\":%s,\"expected\":\"%s\"}\n",
        sample_count, item->name, entity, index, field, returned ? "true" : "false",
        length < capacity ? "true" : "false", capacity, length, hex, preview,
        preview_length == length ? "true" : "false", expected ? "true" : "false",
        expected ? expected : "");
}

static void *find(const char *name, void *after) {
    typedef void *(*Query)(void *, void *, const char *, void *, void *, void *, void *);
    Query query = (Query)method(tools, 0xb5bcc8, 38, &server, 0x7b53b0);
    return query ? query(tools, after, name, NULL, NULL, NULL, NULL) : NULL;
}

static bool entity_roundtrip(void *entity, void **edict, int *index) {
    typedef bool (*IsEntity)(void *, void *);
    typedef void *(*PointerQuery)(void *, void *);
    typedef void *(*IndexQuery)(void *, int);
    typedef int (*EdictIndex)(void *, const void *);
    IsEntity live = (IsEntity)method(tools, 0xb5bcc8, 36, &server, 0x7b5390);
    PointerQuery to_edict = (PointerQuery)method(entities, 0xb54ef8, 5, &server, 0x59b5b0);
    PointerQuery to_entity = (PointerQuery)method(entities, 0xb54ef8, 6, &server, 0x59cdc0);
    IndexQuery by_index = (IndexQuery)method(engine_server, 0x8e1770, 19, &engine, 0x49aee0);
    IndexQuery base_by_index = (IndexQuery)method(tools, 0xb5bcc8, 21, &server, 0x7b5560);
    EdictIndex get_index = (EdictIndex)method(engine_server, 0x8e1770, 18, &engine, 0x49a430);
    if (!live || !to_edict || !to_entity || !by_index || !base_by_index || !get_index || !entity ||
        !live(tools, entity))
        return false;
    void *candidate = to_edict(entities, entity);
    if (!candidate || !mapped(candidate, 32, false))
        return false;
    int number = get_index(engine_server, candidate);
    if (number < 0 || by_index(engine_server, number) != candidate ||
        to_entity(entities, candidate) != entity || base_by_index(tools, number) != entity)
        return false;
    *edict = candidate;
    *index = number;
    return true;
}

static bool resolve_tracked(void) {
    for (size_t index = 0; index < TRACKED_COUNT; index++) {
        Tracked *item = &tracked[index];
        void *entity = find(item->name, NULL);
        if (invalidated)
            return false;
        item->present = entity != NULL;
        item->object_count = 0;
        if (!entity) {
            if (index < STABLE_COUNT || !item->seen) {
                fail("missing_tracked_entity");
                return false;
            }
            continue;
        }
        void *edict = NULL;
        int number = -1;
        if (find(item->name, entity) || !entity_roundtrip(entity, &edict, &number) || number <= 0) {
            fail("ambiguous_entity_or_roundtrip");
            return false;
        }
        const char *expected = index == 0             ? "prop_physics"
                               : index < STABLE_COUNT ? "func_door_rotating"
                                                      : "func_brush";
        char name[128] = {0}, classname[128] = {0};
        bool name_returned = false, class_returned = false;
        bool name_ok = query_key(entity, "targetname", name, sizeof(name), &name_returned);
        bool class_ok = !invalidated && query_key(entity, "classname", classname, sizeof(classname),
                                                  &class_returned);
        bool identity_ok = name_ok && !strcmp(name, item->name) && class_ok;
        bool class_matches = class_ok && !strcmp(classname, expected);
        if (!item->seen || !identity_ok || !class_matches) {
            query_diagnostic(item, entity, number, "targetname", name, sizeof(name), name_returned,
                             item->name);
            query_diagnostic(item, entity, number, "classname", classname, sizeof(classname),
                             class_returned, expected);
        }
        if (!identity_ok) {
            fail("ambiguous_entity_or_roundtrip");
            return false;
        }
        if (!class_matches) {
            fail("unexpected_entity_class");
            return false;
        }
        if (item->seen && (item->freed || item->entity != entity || item->edict != edict ||
                           item->index != number)) {
            fail("tracked_entity_replaced");
            return false;
        }
        item->entity = entity;
        item->edict = edict;
        item->index = number;
        item->seen = true;
    }
    return !invalidated;
}

static void *object_owner(void *object) {
    typedef void *(*Query)(const void *);
    Query query = (Query)method(object, 0x1948a8, 18, &physics, 0x36e70);
    return query ? query(object) : NULL;
}

static bool enumerate_objects(size_t *environment_count, size_t *object_count) {
    typedef void *(*Environment)(void *, int);
    typedef bool (*InSimulation)(const void *);
    typedef const void **(*ObjectList)(const void *, int *);
    typedef void *(*Collide)(const void *);
    Environment environment_at =
        (Environment)method(physics_system, 0x193c98, 7, &physics, 0x1cc80);
    if (!environment_at)
        return false;
    *environment_count = 0;
    *object_count = 0;
    for (int index = 0; index <= MAX_ENVIRONMENTS; index++) {
        void *environment = environment_at(physics_system, index);
        if (!environment)
            return true;
        if (index == MAX_ENVIRONMENTS) {
            fail("environment_budget");
            return false;
        }
        (*environment_count)++;
        InSimulation simulating =
            (InSimulation)method(environment, 0x198288, 36, &physics, 0x2bb80);
        ObjectList list = (ObjectList)method(environment, 0x198288, 48, &physics, 0x2bb00);
        if (!simulating || !list || simulating(environment)) {
            fail("unsafe_simulation_phase");
            return false;
        }
        int count = -1;
        const void **objects = list(environment, &count);
        if (count < 0 || count > MAX_OBJECTS || (size_t)count > MAX_OBJECTS - *object_count ||
            (count && !mapped(objects, (size_t)count * sizeof(void *), false))) {
            fail("object_list_budget_or_storage");
            return false;
        }
        *object_count += (size_t)count;
        for (int object_index = 0; object_index < count; object_index++) {
            void *object = (void *)objects[object_index];
            void *owner = object_owner(object);
            if (invalidated)
                return false;
            for (size_t tracked_index = 0; tracked_index < TRACKED_COUNT; tracked_index++) {
                Tracked *item = &tracked[tracked_index];
                if (!item->present || owner != item->entity)
                    continue;
                item->object_count++;
                Collide collide_query = (Collide)method(object, 0x1948a8, 74, &physics, 0x370e0);
                if (!collide_query)
                    return false;
                void *collide = collide_query(object);
                if (item->object && (item->object != object || item->environment != environment ||
                                     item->collide != collide)) {
                    fail("tracked_physics_identity_changed");
                    return false;
                }
                item->object = object;
                item->environment = environment;
                item->collide = collide;
            }
        }
        int final_count = -1;
        if (list(environment, &final_count) != objects || final_count != count ||
            simulating(environment)) {
            fail("object_list_changed_during_observation");
            return false;
        }
    }
    return false;
}

static bool finite_vector(Vector vector) {
    return isfinite(vector.x) && isfinite(vector.y) && isfinite(vector.z);
}

static bool observe_brace_solid(Tracked *brace) {
    typedef void *(*PointerQuery)(void *);
    typedef int (*SolidQuery)(const void *);
    PointerQuery base_at = (PointerQuery)method(brace->entity, 0xbc6ef8, 6, &server, 0x3a9960);
    PointerQuery collideable_at =
        (PointerQuery)method(brace->entity, 0xbc6ef8, 4, &server, 0x3a9940);
    if (!base_at || !collideable_at)
        return false;
    if (base_at(brace->entity) != brace->entity) {
        fail("brace_public_base_identity");
        return false;
    }
    void *collideable = collideable_at(brace->entity);
    PointerQuery handle_at = (PointerQuery)method(collideable, 0xb7c000, 0, &server, 0x521400);
    SolidQuery solid_at = (SolidQuery)method(collideable, 0xb7c000, 13, &server, 0x521430);
    SolidQuery flags_at = (SolidQuery)method(collideable, 0xb7c000, 14, &server, 0x4dc250);
    if (!handle_at || !solid_at || !flags_at)
        return false;
    void *handle = handle_at(collideable);
    if (handle != brace->entity || (brace->collideable && brace->collideable != collideable)) {
        fail("brace_collideable_identity_changed");
        return false;
    }
    int solid = solid_at(collideable), flags = flags_at(collideable);
    if (solid < SOLID_NONE || solid >= SOLID_LAST || flags < 0 || flags > UINT16_MAX) {
        fail("invalid_brace_solid_values");
        return false;
    }
    brace->collideable = collideable;
    bool not_solid = (flags & FSOLID_NOT_SOLID) != 0;
    emit(
        "{\"event\":\"brace_solid\",\"sample\":%lu,\"name\":\"%s\",\"owner\":\"%p\",\"edict\":\"%p\",\"index\":%d,\"collideable\":\"%p\",\"entity_handle\":\"%p\",\"solid_type\":%d,\"solid_flags\":%d,\"not_solid\":%s,\"is_solid\":%s}\n",
        sample_count, brace->name, brace->entity, brace->edict, brace->index, collideable, handle,
        solid, flags, not_solid ? "true" : "false",
        solid != SOLID_NONE && !not_solid ? "true" : "false");
    return !invalidated;
}

static bool copy_mapped_string(const char *source, char *value, size_t capacity, size_t *copied) {
    uintptr_t start = (uintptr_t)source;
    *copied = 0;
    memset(value, 0, capacity);
    if (!source)
        return false;
    for (size_t index = 0; index < capacity; index++) {
        if (index > UINTPTR_MAX - start)
            return false;
        const void *at = (const void *)(start + index);
        if (!mapped(at, 1, false))
            return false;
        memcpy(value + index, at, 1);
        *copied = index + 1;
        if (!value[index])
            return true;
    }
    return false;
}

static bool verify_physics_name(const Tracked *crate) {
    typedef const char *(*Name)(const void *);
    Name name_at = (Name)method(crate->object, 0x1948a8, 75, &physics, 0x370f0);
    if (!name_at)
        return false;
    const char *address = name_at(crate->object);
    char name[256];
    size_t copied;
    bool readable = copy_mapped_string(address, name, sizeof(name), &copied);
    const char *expected = "models/props_junk/wood_crate001a.mdl";
    bool matches = readable && !strcmp(name, expected);
    if (readable && (!sample_count || !matches))
        query_diagnostic(crate, crate->entity, crate->index, "physics_debug_name", name,
                         sizeof(name), true, expected);
    if (!matches) {
        emit(
            "{\"event\":\"physics_name_failure\",\"sample\":%lu,\"object\":\"%p\",\"owner\":\"%p\",\"returned_pointer\":\"%p\",\"bounded_read_success\":%s,\"bytes_read\":%zu}\n",
            sample_count, crate->object, crate->entity, (const void *)address,
            readable ? "true" : "false", copied);
        fail("unexpected_physics_debug_name");
        return false;
    }
    emit(
        "{\"event\":\"physics_name\",\"sample\":%lu,\"object\":\"%p\",\"owner\":\"%p\",\"collide\":\"%p\",\"returned_pointer\":\"%p\",\"name\":\"%s\",\"bounded_read_success\":true,\"bytes_read\":%zu,\"fixture_label_only\":true}\n",
        sample_count, crate->object, crate->entity, crate->collide, (const void *)address, name,
        copied);
    return !invalidated;
}

static bool contacts(void *object, size_t *count) {
    typedef void *(*Create)(void *);
    typedef void (*Destroy)(void *, void *);
    typedef bool (*Valid)(void *);
    typedef void *(*Object)(void *, int);
    typedef void (*VectorQuery)(void *, Vector *);
    typedef void (*Next)(void *);
    typedef bool (*Asleep)(const void *);
    typedef void *(*Collide)(const void *);
    Create create = (Create)method(object, 0x1948a8, 80, &physics, 0x37270);
    Destroy destroy = (Destroy)method(object, 0x1948a8, 81, &physics, 0x37280);
    if (!create || !destroy)
        return false;
    void *snapshot = create(object);
    *count = 0;
    if (!snapshot) {
        fail("null_friction_snapshot");
        return false;
    }
    if (!refresh_mappings()) {
        fail("snapshot_mapping_refresh");
        destroy(object, snapshot);
        return false;
    }
    Valid valid = (Valid)method(snapshot, 0x194648, 2, &physics, 0x33270);
    Object get_object = (Object)method(snapshot, 0x194648, 3, &physics, 0x33280);
    VectorQuery point_at = (VectorQuery)method(snapshot, 0x194648, 5, &physics, 0x332b0);
    VectorQuery normal_at = (VectorQuery)method(snapshot, 0x194648, 6, &physics, 0x333a0);
    Next next = (Next)method(snapshot, 0x194648, 13, &physics, 0x33310);
    if (valid && get_object && point_at && normal_at && next) {
        while (!invalidated && valid(snapshot)) {
            if (*count == MAX_CONTACTS) {
                fail("contact_budget");
                break;
            }
            void *source = get_object(snapshot, 0), *other = get_object(snapshot, 1);
            if (source != object || !other) {
                fail("invalid_snapshot_object_pair");
                break;
            }
            void *owner = object_owner(other), *edict = NULL;
            Asleep other_asleep = (Asleep)method(other, 0x1948a8, 3, &physics, 0x37a20);
            Collide other_collide_at = (Collide)method(other, 0x1948a8, 74, &physics, 0x370e0);
            if (!other_asleep || !other_collide_at)
                break;
            bool other_sleeping = other_asleep(other);
            void *other_collide = other_collide_at(other);
            int index = -1;
            char name[128], classname[128];
            Vector point = {NAN, NAN, NAN}, normal = {NAN, NAN, NAN};
            point_at(snapshot, &point);
            normal_at(snapshot, &normal);
            if (invalidated || !entity_roundtrip(owner, &edict, &index) ||
                !key(owner, "targetname", name, sizeof(name)) ||
                !key(owner, "classname", classname, sizeof(classname)) || !finite_vector(point) ||
                !finite_vector(normal) || !other_collide) {
                fail("unresolved_contact_owner_or_values");
                break;
            }
            bool representable = true;
            const char *values[] = {name, classname};
            for (size_t field = 0; field < 2; field++) {
                for (const unsigned char *character = (const unsigned char *)values[field];
                     *character; character++) {
                    if (*character < 32 || *character > 126 || *character == '"' ||
                        *character == '\\')
                        representable = false;
                }
            }
            if (!representable) {
                fail("unrepresentable_contact_name");
                break;
            }
            emit(
                "{\"event\":\"contact\",\"sample\":%lu,\"source\":\"%p\",\"other\":\"%p\","
                "\"owner\":\"%p\",\"edict\":\"%p\",\"index\":%d,\"name\":\"%s\",\"classname\":\"%"
                "s\",\"other_awake\":%s,\"other_collide\":\"%p\",\"point\":[%.9g,%.9g,%.9g],\"normal\":[%.9g,%.9g,%.9g]}\n",
                sample_count, source, other, owner, edict, index, name, classname,
                other_sleeping ? "false" : "true", other_collide, point.x, point.y, point.z,
                normal.x, normal.y, normal.z);
            (*count)++;
            next(snapshot);
        }
    }
    destroy(object, snapshot);
    return !invalidated;
}

static void observe(void) {
    if (!refresh_mappings() || !resolve_tracked()) {
        fail("observation_mapping_failed");
        return;
    }
    size_t environment_count, object_count;
    if (!enumerate_objects(&environment_count, &object_count))
        return;
    for (size_t index = 0; index < TRACKED_COUNT; index++) {
        Tracked *item = &tracked[index];
        if (item->present && (item->object_count != 1 || !item->collide)) {
            fail("ambiguous_or_missing_physics_owner");
            return;
        }
        if (index >= STABLE_COUNT && item->present && !observe_brace_solid(item))
            return;
        emit("{\"event\":\"entity\",\"sample\":%lu,\"name\":\"%s\",\"present\":%s,\"entity\":\"%"
             "p\",\"edict\":\"%p\",\"index\":%d,\"object_count\":%d,\"object\":\"%p\",\"collide\":"
             "\"%p\"}\n",
             sample_count, item->name, item->present ? "true" : "false", item->entity, item->edict,
             item->index, item->object_count, item->object, item->collide);
    }
    Tracked *crate = &tracked[0];
    if (!verify_physics_name(crate))
        return;
    typedef bool (*Asleep)(const void *);
    typedef void (*Velocity)(const void *, Vector *, Vector *);
    typedef float (*SimulationTime)(const void *);
    Asleep asleep = (Asleep)method(crate->object, 0x1948a8, 3, &physics, 0x37a20);
    Velocity velocity_at = (Velocity)method(crate->object, 0x1948a8, 52, &physics, 0x37cc0);
    SimulationTime time_at =
        (SimulationTime)method(crate->environment, 0x198288, 39, &physics, 0x2bb40);
    if (!asleep || !velocity_at || !time_at)
        return;
    Vector velocity = {NAN, NAN, NAN}, angular = {NAN, NAN, NAN};
    velocity_at(crate->object, &velocity, &angular);
    float time = time_at(crate->environment);
    if (!finite_vector(velocity) || !finite_vector(angular) || !isfinite(time)) {
        fail("nonfinite_crate_state");
        return;
    }
    bool sleeping = asleep(crate->object);
    size_t contact_count;
    if (!contacts(crate->object, &contact_count))
        return;
    emit("{\"event\":\"sample\",\"sample\":%lu,\"frame\":%lu,\"thread\":%ld,\"simulation_time\":%."
         "9g,\"in_simulation\":false,\"environments\":%zu,\"objects\":%zu,\"crate_index\":%d,"
         "\"object\":\"%p\",\"owner\":\"%p\",\"collide\":\"%p\",\"awake\":%s,\"velocity\":[%.9g,%."
         "9g,%.9g],\"angular_velocity\":[%.9g,%.9g,%.9g],\"contacts\":%zu,\"snapshot_destroyed\":"
         "true}\n",
         sample_count, frame_count, simulation_thread, time, environment_count, object_count,
         crate->index, crate->object, crate->entity, crate->collide, sleeping ? "false" : "true",
         velocity.x, velocity.y, velocity.z, angular.x, angular.y, angular.z, contact_count);
    sample_count++;
}

static bool plugin_load(void *self, Factory engine_factory, Factory server_factory) {
    (void)self;
    (void)engine_factory;
    if (loaded || !process_guard())
        return false;
    tools = interface(server_factory, "VSERVERTOOLS003", &server, 0xb5bcc8);
    if (!tools || invalidated)
        return false;
    entities = interface(server_factory, "ServerGameEnts001", &server, 0xb54ef8);
    if (!entities || invalidated)
        return false;
    Factory direct_engine_factory = (Factory)dlsym(engine.handle, "CreateInterface");
    engine_server = interface(direct_engine_factory, "VEngineServer023", &engine, 0x8e1770);
    if (!engine_server || invalidated)
        return false;
    Factory physics_factory = (Factory)dlsym(physics.handle, "CreateInterface");
    physics_system = interface(physics_factory, "VPhysics031", &physics, 0x193c98);
    loaded = tools && entities && engine_server && physics_system && !invalidated;
    emit("{\"event\":\"loaded\",\"success\":%s}\n", loaded ? "true" : "false");
    return loaded;
}

static void plugin_unload(void *self) {
    (void)self;
    activated = false;
    fail("plugin_unloaded");
    emit("{\"event\":\"unload\"}\n");
}
static void plugin_pause(void *self) {
    (void)self;
    fail("plugin_paused");
}
static void plugin_unpause(void *self) {
    (void)self;
    fail("plugin_unpaused_after_pause");
}
static const char *plugin_description(void *self) {
    (void)self;
    return "HammeRR private read-only contact observer";
}
static void plugin_level_init(void *self, const char *name) {
    (void)self;
    if (!name || strcmp(name, "hammerr_runtime") || level_init_seen || activated || sample_count) {
        fail("unexpected_level_init");
        return;
    }
    level_init_seen = true;
    emit("{\"event\":\"level_init\",\"map\":\"hammerr_runtime\"}\n");
}
static void plugin_activate(void *self, void *edicts, int count, int clients) {
    (void)self;
    (void)edicts;
    if (!loaded || !level_init_seen || activated || invalidated || count <= 0 || clients <= 0) {
        fail("unexpected_activation");
        return;
    }
    activated = true;
    emit("{\"event\":\"activate\",\"edicts\":%d,\"clients\":%d}\n", count, clients);
}
static void plugin_frame(void *self, bool simulating) {
    (void)self;
    if (!loaded || !activated || invalidated || !simulating)
        return;
    long thread = syscall(SYS_gettid);
    if (entered || (simulation_thread && thread != simulation_thread)) {
        fail("callback_thread_or_reentrancy");
        return;
    }
    simulation_thread = thread;
    entered = true;
    if (frame_count++ % SAMPLE_STRIDE == 0)
        observe();
    entered = false;
}
static void plugin_shutdown(void *self) {
    (void)self;
    if (loaded && !invalidated && !level_init_seen && !activated && !entered && !frame_count &&
        !sample_count && !initial_shutdown_seen) {
        initial_shutdown_seen = true;
        emit(
            "{\"event\":\"initial_shutdown\",\"frame\":0,\"sample\":0,\"before_target_init\":true}\n");
        return;
    }
    activated = false;
    fail("level_shutdown");
}
static void plugin_entity(void *self, void *entity) {
    (void)self;
    (void)entity;
}
static void plugin_put(void *self, void *entity, const char *name) {
    (void)self;
    (void)entity;
    (void)name;
}
static void plugin_command_client(void *self, int index) {
    (void)self;
    (void)index;
}
static int plugin_connect(void *self, bool *allow, void *entity, const char *name,
                          const char *address, char *reject, int capacity) {
    (void)self;
    (void)allow;
    (void)entity;
    (void)name;
    (void)address;
    (void)reject;
    (void)capacity;
    return 0;
}
static int plugin_command(void *self, void *entity, const void *command) {
    (void)self;
    (void)entity;
    (void)command;
    return 0;
}
static int plugin_network(void *self, const char *name, const char *id) {
    (void)self;
    (void)name;
    (void)id;
    return 0;
}
static void plugin_cvar(void *self, int cookie, void *entity, int status, const char *name,
                        const char *value) {
    (void)self;
    (void)cookie;
    (void)entity;
    (void)status;
    (void)name;
    (void)value;
}
static void plugin_allocated(void *self, void *edict) {
    (void)self;
    (void)edict;
}
static void plugin_freed(void *self, const void *edict) {
    (void)self;
    for (size_t index = 0; index < TRACKED_COUNT; index++) {
        Tracked *item = &tracked[index];
        if (!item->seen || item->edict != edict)
            continue;
        emit("{\"event\":\"edict_freed\",\"name\":\"%s\",\"edict\":\"%p\",\"index\":%d,\"frame\":%"
             "lu}\n",
             item->name, edict, item->index, frame_count);
        item->freed = true;
        if (index < STABLE_COUNT)
            fail("stable_tracked_edict_freed");
    }
}

static void *callbacks[] = {
    plugin_load,       plugin_unload,   plugin_pause,          plugin_unpause,   plugin_description,
    plugin_level_init, plugin_activate, plugin_frame,          plugin_shutdown,  plugin_entity,
    plugin_entity,     plugin_put,      plugin_command_client, plugin_entity,    plugin_connect,
    plugin_command,    plugin_network,  plugin_cvar,           plugin_allocated, plugin_freed,
};
static struct {
    void **vtable;
} plugin = {callbacks};

__attribute__((visibility("default"))) void *CreateInterface(const char *name, int *result) {
    bool supported = name && !strcmp(name, "ISERVERPLUGINCALLBACKS003");
    if (result)
        *result = supported ? 0 : 1;
    return supported ? &plugin : NULL;
}
