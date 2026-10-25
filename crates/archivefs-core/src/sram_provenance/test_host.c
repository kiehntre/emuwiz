#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
#include <pthread.h>
#include <unistd.h>
struct game {const char *path;const void *data;size_t size;const char *meta;};
static int pipe_fd[2];
static void *blocked_reader(void *ignored){char c;read(pipe_fd[0],&c,1);return NULL;}
int main(int argc,char **argv){
    if(argc!=6)return 2;
    const char *mode=argv[5];
    FILE *cfg=fopen(argv[4],"rb");if(!cfg)return 3;fgetc(cfg);fclose(cfg);
    void *core=dlopen(argv[1],RTLD_NOW);if(!core)return 4;
    bool(*load)(const struct game*)=dlsym(core,"retro_load_game");
    void(*unload)(void)=dlsym(core,"retro_unload_game");
    void*(*data)(unsigned)=dlsym(core,"retro_get_memory_data");
    size_t(*size)(unsigned)=dlsym(core,"retro_get_memory_size");
    bool(*state)(const void*,size_t)=dlsym(core,"retro_unserialize");
    unsigned char rom[4];FILE *in=fopen(argv[2],"rb");if(!in||fread(rom,1,4,in)!=4)return 5;fclose(in);
    struct game g={argv[2],rom,sizeof rom,NULL};if(!load(&g))return 6;
    void *ram=data(0);size_t n=size(0);
    if(!strcmp(mode,"historical")){FILE*f=fopen(argv[3],"rb");unsigned char b[8192];if(f){fread(b,1,sizeof b,f);fclose(f);}}
    if(!strcmp(mode,"state"))state(ram,n);
    if(!strcmp(mode,"mixed"))load(&g);
    pthread_t thread;
    if(!strcmp(mode,"blocking")){if(pipe(pipe_fd))return 7;pthread_create(&thread,NULL,blocked_reader,NULL);usleep(30000);}
    FILE *out=fopen(argv[3],"wb");if(!out)return 8;
    if(!strcmp(mode,"shadow")){unsigned char b[8192];memcpy(b,ram,n);fwrite(b,1,n,out);}else fwrite(ram,1,!strcmp(mode,"partial")?n/2:n,out);
    if(fclose(out))return 9;
    if(!strcmp(mode,"blocking")){write(pipe_fd[1],"x",1);pthread_join(thread,NULL);close(pipe_fd[0]);close(pipe_fd[1]);}
    if(!strcmp(mode,"errno")){char b;errno=0;ssize_t r=read(-1,&b,1);if(r!=-1||errno!=EBADF)return 10;}
    if(strcmp(mode,"interrupted"))unload();
    dlclose(core);return 0;
}
