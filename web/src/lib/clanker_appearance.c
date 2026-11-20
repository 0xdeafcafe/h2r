// Name-based Clanker looks for the game, Live and the browser. Input is the
// normalized UTF-8 gamertag. Output is fg, bg and the four profile colours.
#include <stddef.h>
#include <stdint.h>

static void colour_pair(const uint8_t group[4], uint32_t bits, uint8_t out[2]) {
    unsigned first = bits & 3;
    unsigned second = (first + 1 + (bits >> 2) % 3) % 4;
    out[0] = group[first];
    out[1] = group[second];
}

void clanker_appearance(const uint8_t *name, size_t len, uint8_t out[6]) {
    uint32_t h = 2166136261u;
    for (size_t i = 0; i < len; ++i) {
        h = (h ^ name[i]) * 16777619u;
    }
    h ^= h >> 15;
    h *= 0x2c1b3c6du;
    h ^= h >> 12;

    // Retail profile palette indices, grouped by the extracted swatch luminance.
    static const uint8_t light[4] = {0, 4, 13, 16};
    static const uint8_t dark[4] = {2, 7, 10, 11};
    int inverse = (h & (1u << 11)) != 0;
    out[0] = h & 63;
    out[1] = (h >> 6) & 31;
    colour_pair(inverse ? dark : light, h >> 12, out + 2);
    colour_pair(inverse ? light : dark, h >> 20, out + 4);
}
