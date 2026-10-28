// Disposable libretro-shaped host for the public SRAM launch/refresh test.
#include <dlfcn.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct game { const char *path; const void *data; size_t size; const char *meta; };

int main(int argc, char **argv) {
    if (argc != 6 || strcmp(argv[5], "normal")) return 2;
    FILE *config = fopen(argv[4], "rb");
    if (!config) return 3;
    fgetc(config);
    fclose(config);
    void *core = dlopen(argv[1], RTLD_NOW);
    if (!core) return 4;
    bool (*load)(const struct game *) = dlsym(core, "retro_load_game");
    void (*unload)(void) = dlsym(core, "retro_unload_game");
    void *(*data)(unsigned) = dlsym(core, "retro_get_memory_data");
    size_t (*size)(unsigned) = dlsym(core, "retro_get_memory_size");
    if (!load || !unload || !data || !size) return 5;
    FILE *file = fopen(argv[2], "rb");
    if (!file || fseek(file, 0, SEEK_END)) return 6;
    long length = ftell(file);
    if (length <= 0 || length > 1024 * 1024 || fseek(file, 0, SEEK_SET)) return 7;
    unsigned char *rom = malloc((size_t)length);
    if (!rom || fread(rom, 1, (size_t)length, file) != (size_t)length) return 8;
    fclose(file);
    struct game cartridge = { argv[2], rom, (size_t)length, NULL };
    if (!load(&cartridge)) return 9;
    void *ram = data(0);
    size_t ram_length = size(0);
    if (!ram || !ram_length) return 10;
    FILE *save = fopen(argv[3], "wb");
    if (!save) return 11;
    if (fwrite(ram, 1, ram_length, save) != ram_length || fclose(save)) return 12;
    unload();
    free(rom);
    dlclose(core);
    return 0;
}
