#define _DARWIN_C_SOURCE 1
#include <sys/types.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <libproc.h>
#include <limits.h>
#include <pthread.h>
#include <pwd.h>
#include <signal.h>
#include <spawn.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#define UID 60000
struct Oracle { _Atomic unsigned generation, acknowledgement, entered, route_hits; };
static struct Oracle *oracle;
static pid_t (*compat_vfork)(void);
static char self[PATH_MAX];
static char *guest_env[] = { "PATH=/usr/bin:/bin", NULL };
static double now(void) {
    struct timespec t;
    if (clock_gettime(CLOCK_MONOTONIC, &t)) _exit(90);
    return (double)t.tv_sec + (double)t.tv_nsec / 1e9;
}
static void fail(const char *why) { perror(why); exit(77); }
static void cap(int resource, rlim_t value) {
    struct rlimit limit={value,value}; if (setrlimit(resource,&limit)) _exit(91);
}
static void identities(void) {
    gid_t group=UID;
    if (setgroups(1,&group)||setgid(UID)||setuid(UID)) _exit(92);
    struct proc_bsdshortinfo info;
    if (proc_pidinfo(getpid(),PROC_PIDT_SHORTBSDINFO,0,&info,sizeof info)!=(int)sizeof info) _exit(93);
    if (getuid()!=UID||geteuid()!=UID||getgid()!=UID||getegid()!=UID||info.pbsi_svuid!=UID||info.pbsi_svgid!=UID) _exit(94);
    errno=0; if (setuid(0)==0||errno!=EPERM) _exit(95);
}
static int census(void) {
    pid_t pids[4096]; errno=0;
    int bytes=proc_listpids(PROC_RUID_ONLY,UID,pids,sizeof pids);
    if (bytes<0||(bytes==0&&errno)||bytes>=(int)sizeof pids||bytes%(int)sizeof(pid_t)) fail("census");
    return bytes/(int)sizeof(pid_t);
}
static void pulse(void) {
    unsigned g=atomic_load(&oracle->generation);
    if (g) atomic_store(&oracle->acknowledgement,g);
}
static void leaf(void) {
    alarm(3);
    (void)setsid();
    atomic_fetch_add(&oracle->entered,1);
    for (;;) { pulse(); usleep(100); }
}
static void *creator(void *argument) {
    const char *route=argument;
    for (int i=0;i<8;i++) {
        pid_t child;
        if (!strcmp(route,"spawn")) {
            char *args[]={self,"guest","leaf",NULL};
            int result=posix_spawn(&child,self,NULL,NULL,args,guest_env);
            if (result) { errno=result; _exit(96); }
        } else if (!strcmp(route,"vfork")) {
            child=compat_vfork();
            if (!child) { execle(self,self,"guest","leaf",(char *)NULL,guest_env); _exit(97); }
            if (child<0) _exit(98);
        } else {
            child=fork();
            if (!child) leaf();
            if (child<0) _exit(99);
        }
        atomic_fetch_add(&oracle->route_hits,1);
        pulse(); usleep(50);
    }
    return NULL;
}
static int guest(const char *route) {
    alarm(3);
    if (getuid()!=UID||geteuid()!=UID) return 100;
    oracle=mmap(NULL,4096,PROT_READ|PROT_WRITE,MAP_SHARED,3,0);
    if (oracle==MAP_FAILED) return 101;
    if (!atomic_is_lock_free(&oracle->generation)) return 102;
    if (!strcmp(route,"leaf")) leaf();
    atomic_fetch_add(&oracle->entered,1);
    if (!strcmp(route,"exec")) {
        pthread_t first,second;
        if (pthread_create(&first,NULL,creator,"fork")||pthread_create(&second,NULL,creator,"spawn")) return 103;
        while (atomic_load(&oracle->route_hits)<2) { pulse(); usleep(20); }
        execle(self,self,"guest","leaf",(char *)NULL,guest_env); return 104;
    }
    creator((void *)route);
    for (;;) { pulse(); usleep(100); }
}
static int reap(pid_t child, int *reaped) {
    if (*reaped) return 1;
    int status=0; pid_t result=waitpid(child,&status,WNOHANG);
    if (result==child) { *reaped=1; return 1; }
    if (result<0&&errno!=EINTR) fail("waitpid owned child");
    return 0;
}
static void stop_scope(pid_t leader, int *leader_reaped) {
    double until=now()+8;
    do {
        reap(leader,leader_reaped);
        if (!census()&&*leader_reaped) return;
        pid_t cleaner=fork(); if (cleaner<0) fail("cleanup fork");
        if (!cleaner) { identities(); (void)kill(-1,SIGKILL); _exit(0); }
        int reaped=0;
        while (!reap(cleaner,&reaped)&&now()<until) usleep(1000);
        if (!reaped) fail("cleanup helper not reaped; quarantine");
        usleep(2000);
    } while (now()<until);
    fail("UID not empty; quarantine");
}
int main(int argc,char **argv) {
    setvbuf(stdout,NULL,_IONBF,0);
    // vfork is deliberately part of this compatibility challenge despite its
    // deprecation for new applications. Resolve the actual native symbol;
    // absence is a failed prerequisite, never a replacement with fork/spawn.
    compat_vfork=(pid_t (*)(void))dlsym(RTLD_DEFAULT,"vfork");
    if (!compat_vfork) return 111;
    if (!realpath(argv[0],self)) fail("program path");
    if (argc==3&&!strcmp(argv[1],"guest")) return guest(argv[2]);
    if (argc!=2||geteuid()!=0) return 105;
    if (getpwuid(UID)||getgrgid(UID)||census()) return 106;
    char path[PATH_MAX];
    if (snprintf(path,sizeof path,"%s/oracle",argv[1])>=(int)sizeof path) return 107;
    int fd=open(path,O_RDWR|O_CREAT|O_EXCL,0600); if(fd<0||ftruncate(fd,4096)) fail("oracle");
    oracle=mmap(NULL,4096,PROT_READ|PROT_WRITE,MAP_SHARED,fd,0); if(oracle==MAP_FAILED) fail("oracle mapping");
    if (!atomic_is_lock_free(&oracle->generation)) return 108;
    const char *routes[]={"fork","vfork","spawn","exec"};
    unsigned generation=0;
    for (unsigned route=0;route<4;route++) for (int round=0;round<8;round++) {
        if(census()) fail("UID reused before empty");
        atomic_store(&oracle->generation,0); atomic_store(&oracle->acknowledgement,0);
        atomic_store(&oracle->entered,0); atomic_store(&oracle->route_hits,0);
        pid_t child=fork(); if(child<0) fail("leader fork");
        if(!child) {
            cap(RLIMIT_NPROC,48); cap(RLIMIT_NOFILE,128); cap(RLIMIT_FSIZE,1048576); cap(RLIMIT_CORE,0);
            if(fd!=3&&dup2(fd,3)!=3) _exit(109);
            for(int i=4;i<4096;i++) close(i);
            identities();
            execle(self,self,"guest",routes[route],(char *)NULL,guest_env); _exit(110);
        }
        int reaped=0,ok=1;
        double until=now()+5;
        while((atomic_load(&oracle->entered)<2||!atomic_load(&oracle->route_hits))&&now()<until) {
            if(reap(child,&reaped)) { ok=0; break; }
            usleep(100);
        }
        if(atomic_load(&oracle->entered)<2||!atomic_load(&oracle->route_hits)||!census()) ok=0;
        unsigned control=++generation;
        atomic_store(&oracle->generation,control);
        until=now()+1;
        while(atomic_load(&oracle->acknowledgement)!=control&&now()<until) usleep(100);
        if(atomic_load(&oracle->acknowledgement)!=control) ok=0;
        // This coordinator never starts another guest until cleanup and the
        // post-zero window finish. Cleanup helpers cannot create guest work.
        stop_scope(child,&reaped);
        if(census()||!reaped) fail("completion not established");
        unsigned challenge=++generation;
        atomic_store(&oracle->generation,challenge);
        until=now()+0.1;
        while(now()<until) {
            if(atomic_load(&oracle->acknowledgement)==challenge||census()) { ok=0; break; }
            usleep(1000);
        }
        stop_scope(child,&reaped);
        printf("route=%s round=%d positive=%u post_zero=%u ack=%u uid_empty=%d leader_reaped=%d pass=%d\n",
            routes[route],round,control,challenge,atomic_load(&oracle->acknowledgement),!census(),reaped,ok);
        if(!ok) return 77;
    }
    puts("all_32_cases_pass=1");
    return 0;
}
