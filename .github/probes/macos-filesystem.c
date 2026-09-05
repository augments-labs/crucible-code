#define _DARWIN_C_SOURCE 1
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <libproc.h>
#include <limits.h>
#include <pwd.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <unistd.h>

static const uid_t guest_uid=60000;
static void fail(const char *why) { perror(why); exit(77); }
static void path(char *out, const char *root, const char *suffix) {
    int n=snprintf(out,PATH_MAX,"%s/%s",root,suffix);
    if(n<0 || n>=PATH_MAX) { errno=ENAMETOOLONG; fail("path"); }
}
static void identity(void) {
    struct proc_bsdinfo ids;
    if(proc_pidinfo(getpid(),PROC_PIDTBSDINFO,0,&ids,sizeof(ids))!=(int)sizeof(ids)) fail("identity query");
    if(ids.pbi_ruid!=guest_uid || ids.pbi_uid!=guest_uid || ids.pbi_svuid!=guest_uid ||
       ids.pbi_rgid!=guest_uid || ids.pbi_gid!=guest_uid || ids.pbi_svgid!=guest_uid) {
        fprintf(stderr,"IDENTITY-MISMATCH real=%u effective=%u saved=%u\n",ids.pbi_ruid,ids.pbi_uid,ids.pbi_svuid); exit(77);
    }
    gid_t groups[64];
    int (*kernel_call)(int, ...)=dlsym(RTLD_DEFAULT,"syscall");
    if(!kernel_call) fail("kernel group API");
    int count=kernel_call(SYS_getgroups,64,groups);
    if(count!=1 || groups[0]!=guest_uid) fail("kernel groups");
    errno=0;
    if(setuid(0)!=-1 || errno!=EPERM) fail("root regain");
}
static void empty_uid(void) {
    if(getpwuid(guest_uid)!=NULL) { errno=EEXIST; fail("fixture UID account exists"); }
    pid_t pids[4096]; errno=0;
    int n=proc_listpids(PROC_RUID_ONLY,guest_uid,pids,sizeof(pids));
    if(n<0 || n>=(int)sizeof(pids) || n%(int)sizeof(pid_t) || (n==0 && errno)) fail("UID enumeration");
    if(n!=0) { fprintf(stderr,"UID not empty bytes=%d\n",n); exit(77); }
    puts("UID-EMPTY");
}
static void launch(int argc, char **argv) {
    if(argc!=7 || geteuid()!=0) exit(77);
    gid_t group=guest_uid;
    struct rlimit files={128,128}, processes={8,8}, size={1024*1024,1024*1024}, core={0,0};
    if(setrlimit(RLIMIT_NOFILE,&files) || setrlimit(RLIMIT_NPROC,&processes) ||
       setrlimit(RLIMIT_FSIZE,&size) || setrlimit(RLIMIT_CORE,&core) ||
       setgroups(1,&group) || setgid(guest_uid) || setuid(guest_uid)) fail("permanent drop");
    identity(); alarm(10);
    struct proc_fdinfo descriptors[128];
    int bytes=proc_pidinfo(getpid(),PROC_PIDLISTFDS,0,descriptors,sizeof(descriptors));
    if(bytes<=0 || bytes>=(int)sizeof(descriptors) || bytes%(int)sizeof(descriptors[0])) fail("descriptor inventory");
    for(int i=0;i<bytes/(int)sizeof(descriptors[0]);i++) if(descriptors[i].proc_fd>2) { errno=EBADF; fail("unexpected inherited descriptor"); }
    if(strcmp(argv[2],"confined")==0) {
        FILE *f=fopen(argv[3],"rb"); if(!f) fail("profile read");
        char profile[16385]; size_t n=fread(profile,1,sizeof(profile)-1,f);
        if(ferror(f) || !feof(f) || !n) fail("profile bounds");
        profile[n]=0; fclose(f);
        int (*initialize)(const char *, unsigned long, char **)=dlsym(RTLD_DEFAULT,"sandbox_init");
        void (*free_error)(char *)=dlsym(RTLD_DEFAULT,"sandbox_free_error");
        if(!initialize || !free_error) fail("sandbox API unavailable");
        char *error=NULL;
        puts("LAUNCH before-profile");
        if(initialize(profile,0,&error)) { fprintf(stderr,"PROFILE %.256s\n",error?error:"unknown"); if(error)free_error(error); exit(77); }
        puts("LAUNCH after-profile");
    } else if(strcmp(argv[2],"control")!=0) exit(77);
    /* No untrusted code runs in this pre-policy image. Fresh exec is mandatory. */
    char *args[]={argv[0],"guest",argv[2],argv[4],argv[5],argv[6],NULL};
    char *environment[]={"PATH=/usr/bin:/bin","LANG=C",NULL};
    puts("LAUNCH before-exec");
    execve(argv[0],args,environment); fail("guest exec");
}
static int operation(const char *name, const char *root, const char *outside) {
    char a[PATH_MAX],b[PATH_MAX];
    if(!strcmp(name,"allowed-read")) { path(a,root,"ro/payload"); return open(a,O_RDONLY); }
    if(!strcmp(name,"allowed-write")) { path(a,root,"rw/new"); return open(a,O_CREAT|O_EXCL|O_WRONLY,0600); }
    if(!strcmp(name,"source-read")) return open(outside,O_RDONLY);
    if(!strcmp(name,"source-write")) return open(outside,O_WRONLY);
    if(!strcmp(name,"readonly-write")) { path(a,root,"ro/payload"); return open(a,O_WRONLY); }
    if(!strcmp(name,"protected-write")) { path(a,root,"rw/nested/.git/config"); return open(a,O_WRONLY); }
    if(!strcmp(name,"protected-unlink")) { path(a,root,"rw/nested/.git/config"); return unlink(a); }
    if(!strcmp(name,"protected-rename")) { path(a,root,"rw/nested/.git"); path(b,root,"rw/moved-git"); return rename(a,b); }
    if(!strcmp(name,"ancestor-rename")) { path(a,root,"rw/nested"); path(b,root,"rw/moved-parent"); return rename(a,b); }
    if(!strcmp(name,"protected-link")) { path(a,root,"rw/nested/.git/config"); path(b,root,"rw/linked-config"); return link(a,b); }
    if(!strcmp(name,"protected-symlink")) { path(a,root,"rw/protected-alias"); return open(a,O_WRONLY); }
    if(!strcmp(name,"source-symlink")) { path(a,root,"rw/source-alias"); return open(a,O_RDONLY); }
    if(!strcmp(name,"unreadable-read")) { path(a,root,"rw/secret"); return open(a,O_RDONLY); }
    if(!strcmp(name,"unreadable-alias")) { path(a,root,"rw/secret-alias"); return open(a,O_RDONLY); }
    if(!strcmp(name,"unreadable-case")) { path(a,root,"rw/SECRET"); return open(a,O_RDONLY); }
    if(!strcmp(name,"unreadable-create")) { path(a,root,"rw/absent-secret"); return open(a,O_CREAT|O_EXCL|O_WRONLY,0600); }
    if(!strcmp(name,"unreadable-rename")) { path(a,root,"rw/plain"); path(b,root,"rw/absent-secret"); return rename(a,b); }
    if(!strcmp(name,"device")) { path(a,root,"rw/null-device"); return open(a,O_RDWR); }
    if(!strcmp(name,"setuid")) {
        path(a,root,"rw/setuid-worker"); execl(a,a,"identity",NULL); return -1;
    }
    errno=EINVAL; return -1;
}
int main(int argc,char **argv) {
    setvbuf(stdout,NULL,_IONBF,0);
    if(argc==2 && !strcmp(argv[1],"empty")) { empty_uid(); return 0; }
    if(argc==2 && !strcmp(argv[1],"identity")) { alarm(10); identity(); puts("SETUID-REFUSED"); return 0; }
    if(argc==3 && !strcmp(argv[1],"flags")) {
        struct statfs fs; if(statfs(argv[2],&fs)) fail("statfs");
        printf("FLAGS nosuid=%d nodev=%d readonly=%d\n",!!(fs.f_flags&MNT_NOSUID),!!(fs.f_flags&MNT_NODEV),!!(fs.f_flags&MNT_RDONLY));
        return (fs.f_flags&(MNT_NOSUID|MNT_NODEV))==(MNT_NOSUID|MNT_NODEV)?0:77;
    }
    if(argc==3 && !strcmp(argv[1],"unmount")) { if(unmount(argv[2],0)) fail("nonforced unmount"); return 0; }
    if(argc>1 && !strcmp(argv[1],"launch")) launch(argc,argv);
    if(argc!=6 || strcmp(argv[1],"guest")) return 77;
    puts("GUEST entered");
    alarm(10); identity();
    errno=0; int result=operation(argv[3],argv[4],argv[5]); int error=errno;
    int allow=!strcmp(argv[2],"control") || !strcmp(argv[3],"allowed-read") || !strcmp(argv[3],"allowed-write");
    if(!strcmp(argv[3],"device")) allow=0;
    /* setuid success replaces this process; only refusal to exec reaches here. */
    printf("CASE mode=%s name=%s result=%d errno=%d expected=%s\n",argv[2],argv[3],result,error,allow?"allow":"deny");
    if(allow) return result>=0?0:1;
    return result==-1 && (error==EACCES || error==EPERM || (!strcmp(argv[3],"device") && error==ENXIO))?0:1;
}
