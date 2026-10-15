#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#define HARDWARE_VERSION "MaterialSystemHardwareConfig012"
#define MATERIAL_VERSION "VMaterialSystem082"

enum {
    HARDWARE_CONFIG_SLOT = 14,
    DX_LEVEL_SLOT = 32,
    HDR_TYPE_SLOT = 54,
    HARDWARE_HDR_TYPE_SLOT = 55,
    HDR_ENABLED_SLOT = 69,
    WAIT_ATTEMPTS = 240,
    ACQUIRE_ATTEMPTS = 12,
    SAMPLE_COUNT = 8,
};

typedef void *(*Factory)(const char *, int *);
typedef void *(*HardwareConfig)(void *, const char *, int *);
typedef int (*IntQuery)(const void *);
typedef bool (*BoolQuery)(const void *);

static pthread_t observer_thread;
static atomic_bool stopping;
static bool thread_started;
static int report_fd = -1;
static char library_directory[PATH_MAX];
static char ready_path[PATH_MAX];
static void *module_handles[3];
static const char *module_names[] = {"materialsystem.so", "shaderapivk.so", "shaderapidx9.so"};

static bool mapped(const void *pointer, size_t size, char permission) {
    uintptr_t address = (uintptr_t)pointer;
    if (!address || size > UINTPTR_MAX - address) return false;
    FILE *maps = fopen("/proc/self/maps", "re");
    if (!maps) return false;
    char line[PATH_MAX + 128], permissions[5];
    unsigned long start, end;
    bool found = false;
    while (fgets(line, sizeof(line), maps)) {
        if (sscanf(line, "%lx-%lx %4s", &start, &end, permissions) != 3) continue;
        if (address >= start && address + size <= end && strchr(permissions, permission)) {
            found = true;
            break;
        }
    }
    fclose(maps);
    return found;
}

static bool game_code(const void *pointer) {
    Dl_info info;
    char path[PATH_MAX];
    if (!mapped(pointer, 1, 'x') || !dladdr(pointer, &info) || !info.dli_fname) return false;
    if (!realpath(info.dli_fname, path)) return false;
    size_t length = strlen(library_directory);
    return !strncmp(path, library_directory, length) && path[length] == '/';
}

static void *slot(const void *object, size_t index) {
    void **vtable;
    void *function;
    if (!mapped(object, sizeof(vtable), 'r')) return NULL;
    memcpy(&vtable, object, sizeof(vtable));
    if (!mapped(vtable, (index + 1) * sizeof(function), 'r')) return NULL;
    memcpy(&function, vtable + index, sizeof(function));
    return game_code(function) ? function : NULL;
}

static void pause_observer(void) {
    struct timespec delay = {.tv_sec = 0, .tv_nsec = 250000000};
    while (nanosleep(&delay, &delay) && errno == EINTR && !atomic_load(&stopping)) {}
}

static void *acquire_hardware(void) {
    char path[PATH_MAX];
    for (size_t index = 0; index < sizeof(module_handles) / sizeof(module_handles[0]); index++) {
        if (!module_handles[index]) {
            int length = snprintf(path, sizeof(path), "%s/%s", library_directory, module_names[index]);
            if (length < 0 || (size_t)length >= sizeof(path)) return NULL;
            module_handles[index] = dlopen(path, RTLD_LAZY | RTLD_NOLOAD);
        }
    }
    for (size_t index = 0; index < sizeof(module_handles) / sizeof(module_handles[0]); index++) {
        if (!module_handles[index]) continue;
        Factory factory = (Factory)dlsym(module_handles[index], "CreateInterface");
        if (!game_code((const void *)factory)) {
            dprintf(report_fd, "{\"event\":\"invalid_factory\",\"module\":\"%s\"}\n", module_names[index]);
            return NULL;
        }
        int code = -1;
        void *hardware;
        if (index == 0) {
            void *material = factory(MATERIAL_VERSION, &code);
            dprintf(report_fd, "{\"event\":\"factory\",\"module\":\"%s\",\"version\":\"%s\",\"code\":%d,\"present\":%s}\n", module_names[index], MATERIAL_VERSION, code, material ? "true" : "false");
            if (!material || code != 0) continue;
            HardwareConfig get_config = (HardwareConfig)slot(material, HARDWARE_CONFIG_SLOT);
            if (!get_config) {
                dprintf(report_fd, "{\"event\":\"invalid_material_vtable\"}\n");
                return NULL;
            }
            code = -1;
            hardware = get_config(material, HARDWARE_VERSION, &code);
        } else {
            hardware = factory(HARDWARE_VERSION, &code);
        }
        dprintf(report_fd, "{\"event\":\"hardware_factory\",\"module\":\"%s\",\"route\":\"%s\",\"version\":\"%s\",\"code\":%d,\"present\":%s}\n", module_names[index], index == 0 ? MATERIAL_VERSION ".GetHardwareConfig" : "CreateInterface", HARDWARE_VERSION, code, hardware ? "true" : "false");
        if (hardware && code == 0) return hardware;
    }
    return NULL;
}

static void *observe(void *unused) {
    (void)unused;
    for (int attempt = 0; attempt < WAIT_ATTEMPTS && !atomic_load(&stopping); attempt++) {
        if (access(ready_path, F_OK) == 0) break;
        pause_observer();
    }
    if (atomic_load(&stopping) || access(ready_path, F_OK) != 0) {
        dprintf(report_fd, "{\"event\":\"ready_timeout\"}\n");
        return NULL;
    }
    void *hardware = NULL;
    for (int attempt = 0; attempt < ACQUIRE_ATTEMPTS && !atomic_load(&stopping); attempt++) {
        hardware = acquire_hardware();
        if (hardware) break;
        pause_observer();
    }
    if (!hardware || atomic_load(&stopping)) {
        dprintf(report_fd, "{\"event\":\"acquisition_failed\"}\n");
        return NULL;
    }
    IntQuery dx_level = (IntQuery)slot(hardware, DX_LEVEL_SLOT);
    IntQuery hdr_type = (IntQuery)slot(hardware, HDR_TYPE_SLOT);
    IntQuery hardware_type = (IntQuery)slot(hardware, HARDWARE_HDR_TYPE_SLOT);
    BoolQuery enabled = (BoolQuery)slot(hardware, HDR_ENABLED_SLOT);
    if (!dx_level || !hdr_type || !hardware_type || !enabled) {
        dprintf(report_fd, "{\"event\":\"invalid_hardware_vtable\"}\n");
        return NULL;
    }
    Dl_info implementation;
    if (!dladdr((const void *)hdr_type, &implementation) || !implementation.dli_fname) return NULL;
    const char *basename = strrchr(implementation.dli_fname, '/');
    dprintf(report_fd, "{\"event\":\"hardware_implementation\",\"module\":\"%s\"}\n", basename ? basename + 1 : implementation.dli_fname);
    for (int sample = 0; sample < SAMPLE_COUNT && !atomic_load(&stopping); sample++) {
        struct timespec timestamp;
        clock_gettime(CLOCK_MONOTONIC, &timestamp);
        int dx = dx_level(hardware), active = hdr_type(hardware), capable = hardware_type(hardware);
        bool active_enabled = enabled(hardware);
        dprintf(report_fd, "{\"event\":\"sample\",\"sequence\":%d,\"monotonic_ns\":%lld,\"dx_support_level\":%d,\"hdr_type\":%d,\"hardware_hdr_type\":%d,\"hdr_enabled\":%s}\n", sample, (long long)timestamp.tv_sec * 1000000000LL + timestamp.tv_nsec, dx, active, capable, active_enabled ? "true" : "false");
        pause_observer();
    }
    dprintf(report_fd, "{\"event\":\"complete\"}\n");
    return NULL;
}

static bool private_command(const char *game) {
    char command[32768];
    int fd = open("/proc/self/cmdline", O_RDONLY | O_CLOEXEC);
    if (fd < 0) return false;
    ssize_t size = read(fd, command, sizeof(command));
    close(fd);
    if (size <= 0 || (size_t)size == sizeof(command) || command[size - 1] != '\0') return false;
    bool insecure = false, private_game = false;
    for (size_t offset = 0; offset < (size_t)size; offset += strlen(command + offset) + 1) {
        if (!strcmp(command + offset, "-insecure")) insecure = true;
        if (!strcmp(command + offset, "-game")) {
            size_t next = offset + strlen(command + offset) + 1;
            if (next < (size_t)size && !strcmp(command + next, game)) private_game = true;
        }
    }
    return insecure && private_game;
}

__attribute__((constructor)) static void start_probe(void) {
    const char *expected = getenv("HAMMERR_HDR_EXPECTED_EXE");
    const char *libraries = getenv("HAMMERR_HDR_LIBRARY_DIR");
    const char *game = getenv("HAMMERR_HDR_PRIVATE_GAME");
    const char *ready = getenv("HAMMERR_HDR_READY");
    const char *report = getenv("HAMMERR_HDR_REPORT");
    char executable[PATH_MAX];
    ssize_t length = readlink("/proc/self/exe", executable, sizeof(executable) - 1);
    if (!expected || !libraries || !game || !ready || !report || length < 0) return;
    executable[length] = '\0';
    if (strcmp(executable, expected) || !private_command(game)) return;
    if (strlen(libraries) >= sizeof(library_directory) || strlen(ready) >= sizeof(ready_path)) return;
    strcpy(library_directory, libraries);
    strcpy(ready_path, ready);
    report_fd = open(report, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600);
    if (report_fd < 0) return;
    dprintf(report_fd, "{\"event\":\"start\",\"pid\":%ld,\"hardware_version\":\"%s\"}\n", (long)getpid(), HARDWARE_VERSION);
    int code = pthread_create(&observer_thread, NULL, observe, NULL);
    thread_started = code == 0;
    if (!thread_started) dprintf(report_fd, "{\"event\":\"thread_failed\",\"code\":%d}\n", code);
}

__attribute__((destructor)) static void stop_probe(void) {
    atomic_store(&stopping, true);
    if (thread_started) pthread_join(observer_thread, NULL);
    for (size_t index = 0; index < sizeof(module_handles) / sizeof(module_handles[0]); index++) {
        if (module_handles[index]) dlclose(module_handles[index]);
    }
    if (report_fd >= 0) close(report_fd);
}
