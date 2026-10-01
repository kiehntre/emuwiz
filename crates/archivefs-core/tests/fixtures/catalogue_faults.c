/* Disposable review fixtures only: inject deterministic filesystem faults. */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <sys/syscall.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/mount.h>
#include <unistd.h>
#include <sys/wait.h>

static int replaced=0, enumerated=0, enumeration_failed=0, probed=0;
static void replace_at_probe_t(const char *path,const char *tag);
#define replace_at_probe(p) replace_at_probe_t(p,__func__)
static void replace_at_probe_t(const char *path,const char *tag) {
    const char *trigger=getenv("EMUWIZ_FAULT_PATH"), *root=getenv("EMUWIZ_FAULT_ROOT");
    if (replaced || !trigger || !root || strncmp(root,"/tmp/",5)) return;
    const char *trace=getenv("EMUWIZ_FAULT_TRACE");
    if (trace && strstr(path,strcmp(trace,"1")?trace:"SNES")) fprintf(stderr,"TRACE %s %s\n",tag,path);
    if (strcmp(path,trigger)) return;
    // Arm only after N hardened (openat2) probes of the trigger: the fault then
    // fires at the next resolution, whatever ordinary or hardened call it is.
    static int armed=0; const char *arm=getenv("EMUWIZ_FAULT_ARM_OPENAT2");
    if(arm && armed<atoi(arm)){ if(!strcmp(tag,"syscall")) armed++; return; }
    const char *phase=getenv("EMUWIZ_FAULT_PROBE_NUMBER");
    if(phase && ++probed<atoi(phase))return;
    replaced=1;
    const char *database=getenv("EMUWIZ_FAULT_SQL_DATABASE");
    if(database) {
        if(strncmp(database,"/tmp/",5))abort();
        pid_t child=fork(); if(child<0)abort();
        if(child==0) {
            execlp("python3","python3","-c",
                "import os,sqlite3; c=sqlite3.connect(os.environ['EMUWIZ_FAULT_SQL_DATABASE']); c.execute(\"UPDATE source_folders SET removed_from_config_at='changed-during-probe'\"); c.commit()",
                (char*)NULL);
            _exit(127);
        }
        int status; if(waitpid(child,&status,0)!=child || status)abort();
        return;
    }
    const char *parent=getenv("EMUWIZ_FAULT_PARENT_LINK"), *outside=getenv("EMUWIZ_FAULT_LINK_TARGET");
    if(parent && outside){
        size_t n=strlen(root);
        if(strncmp(parent,root,n)||parent[n]!='/'||strncmp(outside,"/tmp/",5))abort();
        if(rename(parent,outside)||symlink(outside,parent))abort();return;
    }
    const char *leaf=getenv("EMUWIZ_FAULT_LEAF_LINK");
    if(leaf){
        size_t n=strlen(root);
        if(strncmp(path,root,n)||path[n]!='/'||strncmp(leaf,"/tmp/",5))abort();
        if(rename(path,leaf)||symlink(leaf,path))abort();return;
    }
    if(parent && getenv("EMUWIZ_FAULT_RECREATE")){
        char tmp[4096];snprintf(tmp,sizeof(tmp),"%s.moved",parent);size_t n=strlen(root);
        if(strncmp(parent,root,n)||parent[n]!='/')abort();
        if(rename(parent,tmp)||mkdir(parent,0700))abort();
        FILE *f=fopen(path,"w");if(!f)abort();fputs("different bytes",f);fclose(f);return;
    }
    if(getenv("EMUWIZ_FAULT_UNMOUNT")){if(umount2(root,MNT_DETACH))abort();return;}
    char saved[4096]; snprintf(saved,sizeof(saved),"%s.saved-original",root);
    if(rename(root,saved) || mkdir(root,0700)) abort();
}
int statx(int fd, const char *path, int flags, unsigned mask, struct statx *buf) {
    const char *unreadable=getenv("EMUWIZ_FAULT_STAT_EIO");
    if(unreadable && !strncmp(unreadable,"/tmp/",5) && !strcmp(path,unreadable)){errno=EIO;return -1;}
    replace_at_probe(path);
    int (*real)(int,const char*,int,unsigned,struct statx*)=dlsym(RTLD_NEXT,"statx");
    return real(fd,path,flags,mask,buf);
}
int lstat64(const char *path, struct stat64 *buf) {
    const char *unreadable=getenv("EMUWIZ_FAULT_STAT_EIO");
    if(unreadable && !strncmp(unreadable,"/tmp/",5) && !strcmp(path,unreadable)){errno=EIO;return -1;}
    replace_at_probe(path);
    int (*real)(const char*,struct stat64*)=dlsym(RTLD_NEXT,"lstat64");
    return real(path,buf);
}
int stat64(const char *path, struct stat64 *buf) {
    const char *unreadable=getenv("EMUWIZ_FAULT_STAT_EIO");
    if(unreadable && !strncmp(unreadable,"/tmp/",5) && !strcmp(path,unreadable)){errno=EIO;return -1;}
    int (*real)(const char*,struct stat64*)=dlsym(RTLD_NEXT,"stat64");
    return real(path,buf);
}
struct dirent64 *readdir64(DIR *dir) {
    const char *root=getenv("EMUWIZ_FAULT_ENUM");
    if(root && !strncmp(root,"/tmp/",5) && !enumeration_failed) {
        char fd[80],path[4096]; snprintf(fd,sizeof(fd),"/proc/self/fd/%d",dirfd(dir));
        ssize_t n=readlink(fd,path,sizeof(path)-1);
        if(n>=0){path[n]=0;if(!strcmp(path,root) && ++enumerated==4){enumeration_failed=1;errno=EIO;return NULL;}}
    }
    struct dirent64 *(*real)(DIR*)=dlsym(RTLD_NEXT,"readdir64");
    return real(dir);
}

int openat(int fd, const char *name, int flags, ...) {
    char base[4096], full[8192], link[80];
    const char *path=name;
    if(name[0]!='/' && fd!=AT_FDCWD) {
        snprintf(link,sizeof(link),"/proc/self/fd/%d",fd);
        ssize_t n=readlink(link,base,sizeof(base)-1);
        if(n>=0) { base[n]=0; snprintf(full,sizeof(full),"%s/%s",base,name); path=full; }
    }
    replace_at_probe(path);
    const char *unreadable=getenv("EMUWIZ_FAULT_STAT_EIO");
    if(unreadable && !strncmp(unreadable,"/tmp/",5) && !strcmp(path,unreadable)) { errno=EIO;return -1; }
    int (*real)(int,const char*,int,...)=dlsym(RTLD_NEXT,"openat");
    if(flags&O_CREAT) {va_list ap;va_start(ap,flags);int mode=va_arg(ap,int);va_end(ap);return real(fd,name,flags,mode);}
    return real(fd,name,flags);
}

long syscall(long number, ...) {
    long (*real)(long,...)=dlsym(RTLD_NEXT,"syscall");
    va_list ap;va_start(ap,number);
    if(number==SYS_openat2) {
        int fd=va_arg(ap,int); const char *name=va_arg(ap,const char*);
        const void *how=va_arg(ap,const void*); size_t size=va_arg(ap,size_t);
        va_end(ap);
        char base[4096],full[8192],link[80]; const char *path=name;
        if(name[0]!='/' && fd!=AT_FDCWD) {
            snprintf(link,sizeof(link),"/proc/self/fd/%d",fd);
            ssize_t n=readlink(link,base,sizeof(base)-1);
            if(n>=0){base[n]=0;snprintf(full,sizeof(full),"%s/%s",base,name);path=full;}
        }
        replace_at_probe(path);
        const char *unreadable=getenv("EMUWIZ_FAULT_STAT_EIO");
        if(unreadable && !strncmp(unreadable,"/tmp/",5) && !strcmp(path,unreadable)){errno=EIO;return -1;}
        return real(number,fd,name,how,size);
    }
    // Linux syscall ABI passes six machine-word argument slots. Unused slots
    // are ignored by the kernel; only the disposable test process is wrapped.
    long args[6]; for(int i=0;i<6;i++)args[i]=va_arg(ap,long);va_end(ap);
    return real(number,args[0],args[1],args[2],args[3],args[4],args[5]);
}
