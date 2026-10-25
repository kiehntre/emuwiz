// Disposable libretro-shaped fixture: no ROM emulation or installed profile.
#include <stdbool.h>
#include <stddef.h>
#include <string.h>
struct game {const char *path;const void *data;size_t size;const char *meta;};
static unsigned char ram[8192];
bool retro_load_game(const struct game*g){memset(ram,0x39,sizeof ram);return g&&g->data&&g->size;}
void retro_unload_game(void){}
void *retro_get_memory_data(unsigned id){return id==0?ram:0;}
size_t retro_get_memory_size(unsigned id){return id==0?sizeof ram:0;}
bool retro_unserialize(const void*p,size_t n){return true;}
