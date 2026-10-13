#define _GNU_SOURCE
#include <dlfcn.h>
#include <unistd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>
int fsync(int fd) {
 int (*real)(int)=dlsym(RTLD_NEXT,"fsync");
 char link[64], path[4096]; snprintf(link,sizeof link,"/proc/self/fd/%d",fd);
 ssize_t n=readlink(link,path,sizeof path-1);if(n>=0){path[n]=0;
 const char *target=getenv("R3_SYNC_FAULT");
 if(target && !strcmp(path,target)){fprintf(stderr,"FIRED directory fsync EIO: %s\n",path);errno=EIO;return -1;}}
 return real(fd);
}
