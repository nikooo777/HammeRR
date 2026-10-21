#include <cstddef>
#include <cstdint>
#include <type_traits>

namespace {

struct Vector {
    float values[3];
};

struct IndexedLine {
    std::uint16_t vertices[2];
};

struct LineReference {
    std::uint16_t line;
    std::uint8_t endpoint;
    std::uint8_t padding;
};

struct IndexedPolygon {
    std::uint16_t first;
    std::uint16_t count;
    Vector normal;
};

struct PolyhedronInput {
    Vector *vertices;
    IndexedLine *lines;
    LineReference *indices;
    IndexedPolygon *polygons;
    std::uint16_t vertex_count;
    std::uint16_t line_count;
    std::uint16_t index_count;
    std::uint16_t polygon_count;
};

class CPolyhedron {
public:
    Vector *vertices = nullptr;
    IndexedLine *lines = nullptr;
    LineReference *indices = nullptr;
    IndexedPolygon *polygons = nullptr;
    std::uint16_t vertex_count = 0;
    std::uint16_t line_count = 0;
    std::uint16_t index_count = 0;
    std::uint16_t polygon_count = 0;
    virtual ~CPolyhedron() = default;
    virtual void Release() = 0;
};

class Polyhedron final : public CPolyhedron {
public:
    bool released = false;
    void Release() override { released = true; }
};

static_assert(sizeof(void *) == 8);
static_assert(sizeof(float) == 4);
static_assert(sizeof(Vector) == 12 && alignof(Vector) == 4);
static_assert(sizeof(IndexedLine) == 4 && alignof(IndexedLine) == 2);
static_assert(sizeof(LineReference) == 4 && alignof(LineReference) == 2);
static_assert(offsetof(LineReference, endpoint) == 2);
static_assert(sizeof(IndexedPolygon) == 16 && alignof(IndexedPolygon) == 4);
static_assert(offsetof(IndexedPolygon, normal) == 4);
static_assert(std::is_standard_layout_v<PolyhedronInput>);
static_assert(sizeof(PolyhedronInput) == 40 && alignof(PolyhedronInput) == 8);
static_assert(offsetof(PolyhedronInput, vertices) == 0);
static_assert(offsetof(PolyhedronInput, lines) == 8);
static_assert(offsetof(PolyhedronInput, indices) == 16);
static_assert(offsetof(PolyhedronInput, polygons) == 24);
static_assert(offsetof(PolyhedronInput, vertex_count) == 32);
static_assert(offsetof(PolyhedronInput, line_count) == 34);
static_assert(offsetof(PolyhedronInput, index_count) == 36);
static_assert(offsetof(PolyhedronInput, polygon_count) == 38);
static_assert(sizeof(CPolyhedron) == 48 && alignof(CPolyhedron) == 8);

using Construct = void *(*)(void *, const CPolyhedron &);

}

extern "C" int hammerr_convex_from_polyhedron(
    Construct construct, void *collision, const PolyhedronInput *input,
    void **output) noexcept {
    *output = nullptr;
    Polyhedron polyhedron;
    const auto *base = reinterpret_cast<const char *>(
        static_cast<const CPolyhedron *>(&polyhedron));
    if (reinterpret_cast<const char *>(&polyhedron.vertices) - base != 8 ||
        reinterpret_cast<const char *>(&polyhedron.lines) - base != 16 ||
        reinterpret_cast<const char *>(&polyhedron.indices) - base != 24 ||
        reinterpret_cast<const char *>(&polyhedron.polygons) - base != 32 ||
        reinterpret_cast<const char *>(&polyhedron.vertex_count) - base != 40 ||
        reinterpret_cast<const char *>(&polyhedron.line_count) - base != 42 ||
        reinterpret_cast<const char *>(&polyhedron.index_count) - base != 44 ||
        reinterpret_cast<const char *>(&polyhedron.polygon_count) - base != 46) {
        return 1;
    }
    polyhedron.vertices = input->vertices;
    polyhedron.lines = input->lines;
    polyhedron.indices = input->indices;
    polyhedron.polygons = input->polygons;
    polyhedron.vertex_count = input->vertex_count;
    polyhedron.line_count = input->line_count;
    polyhedron.index_count = input->index_count;
    polyhedron.polygon_count = input->polygon_count;
    try {
        *output = construct(collision, polyhedron);
    } catch (...) {
        return 3;
    }
    return polyhedron.released ? 2 : 0;
}
