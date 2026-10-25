/* Native glibc/x86-64 observer. Never changes a core return value, save buffer,
 * file contents or emulator configuration. Missing evidence disables capture.
 * The selected native host/core are trusted code, not a hostile-code sandbox. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <link.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>
#include <poll.h>
#define MAX_RAM (1024u*1024u)
#define MAX_ROM (8u*1024u*1024u)
#define SLOTS 256
struct header { uint32_t kind, size, pid, reserved; };
struct stamp { uint64_t v[10]; };
struct slot { int used, fd; uint64_t token, dev, ino; };
static atomic_int channel=-1;
static const char *root, *core_path, *config_path;
static pthread_mutex_t mutex=PTHREAD_MUTEX_INITIALIZER;
static __thread int inside;
static atomic_int invalid;
static atomic_int relevant_io;
static bool active;
static void *ram;
static _Atomic(void*) core_handle;
static size_t ram_size;
static uint64_t sequence;
static struct slot slots[SLOTS];
static void *real_symbol(const char *n) { void *(*lookup)(void*,const char*)=dlvsym(RTLD_NEXT,"dlsym","GLIBC_2.2.5"); return lookup(RTLD_NEXT,n); }
static int64_t millis(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC,&t); return t.tv_sec*1000+t.tv_nsec/1000000; }
static bool transfer(const void *b,size_t z) {
    int64_t deadline=millis()+1000;
    while(z && channel>=0) {
        ssize_t n=send(channel,b,z,MSG_NOSIGNAL|MSG_DONTWAIT);
        if(n>0) { b=(const char*)b+n;z-=(size_t)n;continue; }
        if(n<0 && errno==EINTR) continue;
        if(n<0 && (errno==EAGAIN||errno==EWOULDBLOCK) && millis()<deadline) {
            struct pollfd p={channel,POLLOUT,0}; poll(&p,1,10);continue;
        }
        atomic_store(&invalid,1);syscall(SYS_close,channel);channel=-1;return false;
    }
    return z==0;
}
static bool frame(uint32_t kind,const void *b,size_t z) {
    if(channel<0)return false;
    struct header h={kind,(uint32_t)z,(uint32_t)getpid(),0};
    return transfer(&h,sizeof h) && (!z||transfer(b,z));
}
static bool ack(void) {
    int64_t end=millis()+1000;unsigned char b=0;
    while(channel>=0 && millis()<end) {
        ssize_t n=recv(channel,&b,1,MSG_DONTWAIT);
        if(n==1)return b==1;
        if(n<0 && (errno==EAGAIN||errno==EINTR)){struct pollfd p={channel,POLLIN,0};poll(&p,1,10);continue;}
        break;
    }
    atomic_store(&invalid,1);return false;
}
static void stat_wire(struct stamp *w,const struct stat *s) {
    uint64_t v[10]={s->st_dev,s->st_ino,s->st_size,(uint64_t)s->st_mtim.tv_sec,s->st_mtim.tv_nsec,(uint64_t)s->st_ctim.tv_sec,s->st_ctim.tv_nsec,s->st_nlink,s->st_mode,s->st_uid};memcpy(w->v,v,sizeof v);
}
static bool related(int fd,char path[4096],struct stat *s) {
    if(fd<0 || fstat(fd,s) || !S_ISREG(s->st_mode))return false;
    char p[64];snprintf(p,sizeof p,"/proc/self/fd/%d",fd);
    ssize_t n=readlink(p,path,4095);if(n<1)return false;path[n]=0;
    size_t r=root?strlen(root):0;
    return (r && !strncmp(path,root,r) && path[r]=='/') || ((size_t)n>4 && !strcmp(path+n-4,".srm"));
}
static struct slot *find(int fd) {for(unsigned i=0;i<SLOTS;i++)if(slots[i].used&&slots[i].fd==fd)return &slots[i];return NULL;}
static struct slot *track(int fd,const struct stat *s) {
    struct slot *t=find(fd);
    if(!t)for(unsigned i=0;i<SLOTS;i++)if(!slots[i].used){t=&slots[i];break;}
    if(!t){atomic_store(&invalid,1);return NULL;}
    *t=(struct slot){1,fd,++sequence,s->st_dev,s->st_ino};return t;
}
static void file_event(unsigned kind,int fd,unsigned flags,const struct stat *s,const char *path,const void *bytes,size_t len) {
    struct slot *t=find(fd);if(!t){atomic_store(&invalid,1);return;}
    if(t->dev!=s->st_dev||t->ino!=s->st_ino){atomic_store(&invalid,1);return;}
    uint32_t plen=(uint32_t)strlen(path), n=(uint32_t)len;
    size_t z=16+sizeof(struct stamp)+4+plen+4+len;
    unsigned char *b=malloc(z);if(!b){atomic_store(&invalid,1);return;}
    struct stamp w;stat_wire(&w,s);uint32_t f=(uint32_t)fd;
    memcpy(b,&f,4);memcpy(b+4,&flags,4);memcpy(b+8,&t->token,8);memcpy(b+16,&w,sizeof w);
    memcpy(b+16+sizeof w,&plen,4);memcpy(b+20+sizeof w,path,plen);memcpy(b+20+sizeof w+plen,&n,4);
    if(len)memcpy(b+24+sizeof w+plen,bytes,len);
    frame(kind,b,z);free(b);
}
static void opened_inner(int fd,bool read_mode) {
    if(inside||channel<0)return;
    inside=1;pthread_mutex_lock(&mutex);
    char path[4096];struct stat s;
    if(related(fd,path,&s)){if(track(fd,&s))file_event(4,fd,read_mode?1:0,&s,path,NULL,0);}
    // Bind the selected configuration actually opened by the native host.
    if(read_mode&&config_path&&fd>=0&&!fstat(fd,&s)&&S_ISREG(s.st_mode)){
        char link[64];snprintf(link,sizeof link,"/proc/self/fd/%d",fd);
        ssize_t n=readlink(link,path,sizeof path-1);
        if(n>0){path[n]=0;if(!strcmp(path,config_path)){struct stamp w;stat_wire(&w,&s);frame(11,&w,sizeof w);}}
    }
    pthread_mutex_unlock(&mutex);inside=0;
}
static void opened(int fd,bool read_mode){int saved=errno;opened_inner(fd,read_mode);errno=saved;}
static void fork_parent(void){atomic_store(&invalid,1);}
static void fork_child(void){if(channel>=0)syscall(SYS_close,channel);channel=-1;}
__attribute__((constructor))static void start_observer(void) {
    const char *fd=getenv("EMUWIZ_SRAM_OBSERVER_FD");root=getenv("EMUWIZ_SRAM_ROOT");core_path=getenv("EMUWIZ_SRAM_CORE");config_path=getenv("EMUWIZ_SRAM_CONFIG");
    if(!fd||!root||!core_path)return;
    char *end;long f=strtol(fd,&end,10);if(*end||f<3||f>1048576)return;channel=(int)f;
    // The environment survives exec into descendants; the descriptor does not.
    // Authenticate its socket identity before touching a possibly reused fd.
    const char *expected=getenv("EMUWIZ_SRAM_COOKIE");
    uint64_t cookie=0; socklen_t length=sizeof cookie;
    char *tail=NULL; unsigned long long wanted=expected?strtoull(expected,&tail,10):0;
    int type=0; socklen_t type_length=sizeof type;
    if(!expected||*tail||!wanted||getsockopt(channel,SOL_SOCKET,SO_COOKIE,&cookie,&length)||
       cookie!=wanted||getsockopt(channel,SOL_SOCKET,SO_TYPE,&type,&type_length)||type!=SOCK_STREAM){channel=-1;return;}
    fcntl(channel,F_SETFD,FD_CLOEXEC);pthread_atfork(NULL,fork_parent,fork_child);frame(1,NULL,0);
}
__attribute__((destructor))static void finish_observer(void) {
    if(channel<0)return;
    inside=1;pthread_mutex_lock(&mutex);
    uint32_t bad=(uint32_t)(atomic_load(&invalid)||atomic_load(&relevant_io));frame(9,&bad,4);syscall(SYS_close,channel);channel=-1;
    pthread_mutex_unlock(&mutex);
}
struct game {const char *path;const void *data;size_t size;const char *meta;};
static _Atomic(bool(*)(const struct game*)) load_game;
static _Atomic(void(*)(void)) unload_game;
static _Atomic(void*(*)(unsigned)) memory_data;
static _Atomic(size_t(*)(unsigned)) memory_size;
static _Atomic(bool(*)(const void*,size_t)) unserialize_game;
static _Atomic(bool(*)(unsigned,const struct game*,size_t)) load_special;
static void restore_errno(int *saved){errno=*saved;}
#define ERRNO_GUARD int saved_errno __attribute__((cleanup(restore_errno)))=errno
static bool observed_load(const struct game *g) {
    ERRNO_GUARD;
    if(channel<0){errno=saved_errno;bool result=load_game(g);saved_errno=errno;return result;}
    if(atomic_load(&relevant_io))atomic_store(&invalid,1);
    pthread_mutex_lock(&mutex);inside=1;active=false;ram=NULL;ram_size=0;
    bool supported=g&&g->data&&g->size&&g->size<=MAX_ROM;
    if(!supported || !frame(3,g->data,g->size) || !ack())atomic_store(&invalid,1);
    inside=0;pthread_mutex_unlock(&mutex);
    errno=saved_errno;bool result=load_game(g);saved_errno=errno;
    pthread_mutex_lock(&mutex);inside=1;uint32_t yes=result;active=result;
    frame(6,&yes,4);inside=0;pthread_mutex_unlock(&mutex);return result;
}
static void observed_unload(void){
    ERRNO_GUARD;
    if(channel>=0){if(atomic_load(&relevant_io))atomic_store(&invalid,1);pthread_mutex_lock(&mutex);active=false;frame(7,NULL,0);pthread_mutex_unlock(&mutex);}
    errno=saved_errno;unload_game();saved_errno=errno;
}
static void *observed_data(unsigned id){
    ERRNO_GUARD;errno=saved_errno;void *p=memory_data(id);saved_errno=errno;
    if(channel>=0&&id==0){if(atomic_load(&relevant_io))atomic_store(&invalid,1);pthread_mutex_lock(&mutex);ram=p;pthread_mutex_unlock(&mutex);}return p;
}
static size_t observed_size(unsigned id){
    ERRNO_GUARD;errno=saved_errno;size_t n=memory_size(id);saved_errno=errno;
    if(channel>=0&&id==0){if(atomic_load(&relevant_io))atomic_store(&invalid,1);pthread_mutex_lock(&mutex);ram_size=n;pthread_mutex_unlock(&mutex);}return n;
}
static bool observed_unserialize(const void *p,size_t n){atomic_store(&invalid,1);return unserialize_game(p,n);}
static bool observed_special(unsigned id,const struct game *g,size_t n){atomic_store(&invalid,1);return load_special(id,g,n);}

// glibc resolves special dlsym handles using its caller's return address.
// A C forwarding frame changes RTLD_NEXT/default lookup scope. This x86-64
// entry restores the original stack and tail-jumps for both special handles.
// The resolver is deliberately lazy: dependency constructors can call dlsym
// before this observer's constructor has run. Only two integer arguments are
// part of dlsym's ABI; stack alignment follows the System V x86-64 ABI.
__attribute__((visibility("hidden"),noinline))
void *emuwiz_real_dlsym(void){
    int saved=errno;void *p=dlvsym(RTLD_NEXT,"dlsym","GLIBC_2.2.5");errno=saved;return p;
}
__asm__(
    ".text\n"
    ".globl dlsym\n.type dlsym,@function\n"
    "dlsym:\nendbr64\n"
    "testq %rdi,%rdi\nje 1f\n"
    "cmpq $-1,%rdi\nje 1f\n"
    "jmp emuwiz_dlsym_explicit\n"
    "1:\npushq %rdi\npushq %rsi\nsubq $8,%rsp\n"
    "call emuwiz_real_dlsym\n"
    "addq $8,%rsp\npopq %rsi\npopq %rdi\njmp *%rax\n"
    ".size dlsym,.-dlsym\n"
);
__attribute__((visibility("hidden")))
void *emuwiz_dlsym_explicit(void *handle,const char *name) {
    ERRNO_GUARD;
    int entry_errno=errno;void *(*real)(void*,const char*)=real_symbol("dlsym");errno=entry_errno;void *p=real(handle,name);saved_errno=errno;
    if(inside||channel<0||strncmp(name,"retro_",6)||!p||handle==RTLD_DEFAULT||handle==RTLD_NEXT)return p;
    struct link_map *map=NULL;if(dlinfo(handle,RTLD_DI_LINKMAP,&map)||!map)return p;
    if(strcmp(map->l_name,core_path)){atomic_store(&invalid,1);return p;}
    if(core_handle&&core_handle!=handle){atomic_store(&invalid,1);return p;}
    core_handle=handle;
    if(!strcmp(name,"retro_load_game")){
        inside=1;pthread_mutex_lock(&mutex);load_game=p;uint64_t addr=(uint64_t)(uintptr_t)p;
        if(!frame(2,&addr,8)||!ack())atomic_store(&invalid,1);
        pthread_mutex_unlock(&mutex);inside=0;return observed_load;
    }
    if(!strcmp(name,"retro_unload_game")){unload_game=p;return observed_unload;}
    if(!strcmp(name,"retro_get_memory_data")){memory_data=p;return observed_data;}
    if(!strcmp(name,"retro_get_memory_size")){memory_size=p;return observed_size;}
    if(!strcmp(name,"retro_unserialize")){unserialize_game=p;return observed_unserialize;}
    if(!strcmp(name,"retro_load_game_special")){load_special=p;return observed_special;}
    return p;
}
#define OPEN_WRAP(NAME) \
int NAME(const char *p,int flags,...){ERRNO_GUARD;mode_t mode=0;if((flags&O_CREAT)||((flags&O_TMPFILE)==O_TMPFILE)){va_list a;va_start(a,flags);mode=va_arg(a,int);va_end(a);} \
 int(*real)(const char*,int,...)=real_symbol(#NAME);errno=saved_errno;int fd=real(p,flags,mode);saved_errno=errno;opened(fd,(flags&O_ACCMODE)!=O_WRONLY);return fd;}
#include <stdarg.h>
OPEN_WRAP(open)
OPEN_WRAP(open64)
#define OPENAT_WRAP(NAME) \
int NAME(int d,const char *p,int flags,...){ERRNO_GUARD;mode_t mode=0;if((flags&O_CREAT)||((flags&O_TMPFILE)==O_TMPFILE)){va_list a;va_start(a,flags);mode=va_arg(a,int);va_end(a);} \
 int(*real)(int,const char*,int,...)=real_symbol(#NAME);errno=saved_errno;int fd=real(d,p,flags,mode);saved_errno=errno;opened(fd,(flags&O_ACCMODE)!=O_WRONLY);return fd;}
OPENAT_WRAP(openat)
OPENAT_WRAP(openat64)
#define FOPEN_WRAP(NAME) \
FILE *NAME(const char*p,const char*m){ERRNO_GUARD;FILE*(*real)(const char*,const char*)=real_symbol(#NAME);int old=inside;inside=1;errno=saved_errno;FILE*f=real(p,m);saved_errno=errno;inside=old;if(f)opened(fileno(f),strchr(m,'r')||strchr(m,'+'));return f;}
FOPEN_WRAP(fopen)
FOPEN_WRAP(fopen64)
static void written(int fd,const void *b,size_t n,ssize_t result,off_t offset,const unsigned char *copy,bool qualifies) {
    char path[4096];struct stat s;if(!related(fd,path,&s))return;
    if(!find(fd)){atomic_store(&invalid,1);return;}
    if(result!=(ssize_t)n||offset!=0){atomic_store(&invalid,1);qualifies=false;}
    if(qualifies&&memcmp(copy,b,n)){atomic_store(&invalid,1);qualifies=false;}
    file_event(8,fd,qualifies?2:0,&s,path,qualifies?copy:NULL,qualifies?n:0);
}
// Unrelated host I/O bypasses capture entirely. Never hold the capture lock
// over real I/O: a blocked host fd must not stop the emulation thread.
static bool begin_io(int fd){
    if(inside||channel<0)return false;
    int saved=errno;char path[4096];struct stat stat;
    bool yes=related(fd,path,&stat);errno=saved;
    if(yes&&atomic_fetch_add(&relevant_io,1)!=0)atomic_store(&invalid,1);
    return yes;
}
static void end_io(void){atomic_fetch_sub(&relevant_io,1);}
size_t fwrite(const void *b,size_t size,size_t count,FILE *f) {
    ERRNO_GUARD;
    size_t(*real)(const void*,size_t,size_t,FILE*)=real_symbol("fwrite");
    int fd=fileno(f);bool observe=begin_io(fd);
    if(!observe){errno=saved_errno;size_t result=real(b,size,count,f);saved_errno=errno;return result;}
    if(size&&count>SIZE_MAX/size){atomic_store(&invalid,1);end_io();errno=saved_errno;size_t result=real(b,size,count,f);saved_errno=errno;return result;}
    size_t n=size*count;inside=1;pthread_mutex_lock(&mutex);off_t off=ftello(f);
    bool q=active&&b==ram&&n==ram_size&&n&&n<=MAX_RAM;
    unsigned char *copy=q?malloc(n):NULL;if(q&&!copy){q=false;atomic_store(&invalid,1);}if(q)memcpy(copy,b,n);
    pthread_mutex_unlock(&mutex);
    errno=saved_errno;size_t result=real(b,size,count,f);saved_errno=errno;
    pthread_mutex_lock(&mutex);written(fd,b,n,(ssize_t)(result*size),off,copy,q);free(copy);pthread_mutex_unlock(&mutex);
    inside=0;end_io();return result;
}
ssize_t write(int fd,const void*b,size_t n) {
    ERRNO_GUARD;ssize_t(*real)(int,const void*,size_t)=real_symbol("write");
    bool observe=begin_io(fd);if(!observe){errno=saved_errno;ssize_t result=real(fd,b,n);saved_errno=errno;return result;}
    inside=1;pthread_mutex_lock(&mutex);off_t off=lseek(fd,0,SEEK_CUR);
    bool q=active&&b==ram&&n==ram_size&&n&&n<=MAX_RAM;
    unsigned char *copy=q?malloc(n):NULL;if(q&&!copy){q=false;atomic_store(&invalid,1);}if(q)memcpy(copy,b,n);
    pthread_mutex_unlock(&mutex);
    errno=saved_errno;ssize_t result=real(fd,b,n);saved_errno=errno;
    pthread_mutex_lock(&mutex);written(fd,b,n,result,off,copy,q);free(copy);pthread_mutex_unlock(&mutex);
    inside=0;end_io();return result;
}
static void read_event(int fd,const void*b,size_t n){
    char path[4096];struct stat s;
    if(related(fd,path,&s)){if(find(fd))file_event(5,fd,0,&s,path,n<=MAX_RAM?b:NULL,n<=MAX_RAM?n:0);else atomic_store(&invalid,1);}
}
size_t fread(void*b,size_t size,size_t count,FILE*f){
    ERRNO_GUARD;size_t(*real)(void*,size_t,size_t,FILE*)=real_symbol("fread");
    int fd=fileno(f);bool observe=begin_io(fd);if(observe)inside=1;
    errno=saved_errno;size_t n=real(b,size,count,f);saved_errno=errno;
    if(observe){pthread_mutex_lock(&mutex);if(size&&n>SIZE_MAX/size)atomic_store(&invalid,1);else read_event(fd,b,n*size);pthread_mutex_unlock(&mutex);inside=0;end_io();}return n;
}
ssize_t read(int fd,void*b,size_t z){
    ERRNO_GUARD;ssize_t(*real)(int,void*,size_t)=real_symbol("read");
    bool observe=begin_io(fd);if(observe)inside=1;
    errno=saved_errno;ssize_t n=real(fd,b,z);saved_errno=errno;
    if(observe){pthread_mutex_lock(&mutex);if(n>0)read_event(fd,b,(size_t)n);pthread_mutex_unlock(&mutex);inside=0;end_io();}return n;
}
static int finish_fd(int fd,FILE*f,bool stream){
    ERRNO_GUARD;int(*realclose)(int)=real_symbol("close");int(*realfclose)(FILE*)=real_symbol("fclose");
    bool observe=begin_io(fd);
    if(!observe){errno=saved_errno;int result=stream?realfclose(f):realclose(fd);saved_errno=errno;return result;}
    inside=1;pthread_mutex_lock(&mutex);struct slot *t=find(fd);
    uint64_t token=t?t->token:0;int duplicate=t?fcntl(fd,F_DUPFD_CLOEXEC,3):-1;
    if(!t){atomic_store(&invalid,1);}
    pthread_mutex_unlock(&mutex);
    errno=saved_errno;int result=stream?realfclose(f):realclose(fd);saved_errno=errno;
    pthread_mutex_lock(&mutex);t=find(fd);
    if(t&&t->token==token){struct stat s;char path[4096];if(result||duplicate<0||!related(duplicate,path,&s)){atomic_store(&invalid,1);}else file_event(10,fd,0,&s,path,NULL,0);t->used=0;}else atomic_store(&invalid,1);
    if(duplicate>=0)syscall(SYS_close,duplicate);
    pthread_mutex_unlock(&mutex);inside=0;end_io();return result;
}
int fclose(FILE*f){return finish_fd(fileno(f),f,true);}
int close(int fd){return finish_fd(fd,NULL,false);}
// These operations break the bounded open/write/close witness. They still run
// unchanged; association is refused if they touch a potentially relevant file.
static void unsupported_fd(int fd){
    if(inside||channel<0)return;
    int saved=errno;char path[4096];struct stat stat;
    if(related(fd,path,&stat))atomic_store(&invalid,1);
    errno=saved;
}
int dup(int fd){ERRNO_GUARD;unsupported_fd(fd);int(*real)(int)=real_symbol("dup");errno=saved_errno;int r=real(fd);saved_errno=errno;return r;}
int dup2(int fd,int to){ERRNO_GUARD;unsupported_fd(fd);unsupported_fd(to);int(*real)(int,int)=real_symbol("dup2");errno=saved_errno;int r=real(fd,to);saved_errno=errno;return r;}
int dup3(int fd,int to,int flags){ERRNO_GUARD;unsupported_fd(fd);unsupported_fd(to);int(*real)(int,int,int)=real_symbol("dup3");errno=saved_errno;int r=real(fd,to,flags);saved_errno=errno;return r;}
FILE *freopen(const char*p,const char*m,FILE*f){ERRNO_GUARD;unsupported_fd(fileno(f));int old=inside;inside=1;FILE*(*real)(const char*,const char*,FILE*)=real_symbol("freopen");errno=saved_errno;FILE*r=real(p,m,f);saved_errno=errno;inside=old;if(r){unsupported_fd(fileno(r));}return r;}
ssize_t pwrite(int fd,const void*b,size_t n,off_t off){ERRNO_GUARD;unsupported_fd(fd);ssize_t(*real)(int,const void*,size_t,off_t)=real_symbol("pwrite");errno=saved_errno;ssize_t r=real(fd,b,n,off);saved_errno=errno;return r;}
#include <sys/uio.h>
ssize_t writev(int fd,const struct iovec*v,int n){ERRNO_GUARD;unsupported_fd(fd);ssize_t(*real)(int,const struct iovec*,int)=real_symbol("writev");errno=saved_errno;ssize_t r=real(fd,v,n);saved_errno=errno;return r;}
#include <sys/mman.h>
void *mmap(void*a,size_t n,int prot,int flags,int fd,off_t off){ERRNO_GUARD;unsupported_fd(fd);void*(*real)(void*,size_t,int,int,int,off_t)=real_symbol("mmap");errno=saved_errno;void*r=real(a,n,prot,flags,fd,off);saved_errno=errno;return r;}

#include <spawn.h>
int posix_spawn(pid_t*p,const char*path,const posix_spawn_file_actions_t*a,const posix_spawnattr_t*attr,char*const argv[],char*const env[]){
    ERRNO_GUARD;if(channel>=0)atomic_store(&invalid,1);
    int(*real)(pid_t*,const char*,const posix_spawn_file_actions_t*,const posix_spawnattr_t*,char*const[],char*const[])=real_symbol("posix_spawn");
    errno=saved_errno;int r=real(p,path,a,attr,argv,env);saved_errno=errno;return r;
}
int posix_spawnp(pid_t*p,const char*path,const posix_spawn_file_actions_t*a,const posix_spawnattr_t*attr,char*const argv[],char*const env[]){
    ERRNO_GUARD;if(channel>=0)atomic_store(&invalid,1);
    int(*real)(pid_t*,const char*,const posix_spawn_file_actions_t*,const posix_spawnattr_t*,char*const[],char*const[])=real_symbol("posix_spawnp");
    errno=saved_errno;int r=real(p,path,a,attr,argv,env);saved_errno=errno;return r;
}
int system(const char*command){ERRNO_GUARD;if(channel>=0)atomic_store(&invalid,1);int(*real)(const char*)=real_symbol("system");errno=saved_errno;int r=real(command);saved_errno=errno;return r;}
