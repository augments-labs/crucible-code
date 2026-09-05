#!/bin/sh
# Native resource feasibility only. Disposable VM, synthetic state and owned UID.
set -eu
test "$(uname -s)" = Darwin
test "$(id -u)" != 0
probe_root=$(mktemp -d /tmp/crucible-macos-resources.XXXXXX)
probe_root=$(cd "$probe_root" && pwd -P)
printf '(version 1)\n(allow default)\n(deny network*)\n' > "$probe_root/network-deny.sb"
cat > "$probe_root/resources.c" <<'C'
#define _DARWIN_C_SOURCE 1
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <libproc.h>
#include <limits.h>
#include <pwd.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
extern char **environ;
static volatile sig_atomic_t stopped;
static void stop(int s) { (void)s; stopped = 1; }
static _Noreturn void die(const char *s) { perror(s); exit(77); }
/* Darwin API: XNU f6217f891ac0bb64f3d375211650a4c1ff8ca1ea:
 * libsyscall/wrappers/libproc/libproc.h declares proc_pidinfo (macOS 10.5+);
 * bsd/sys/proc_info.h defines PROC_PIDLISTFDS and struct proc_fdinfo.
 * bsd/kern/proc_info.c enumerates under proc_fdlock and may truncate at capacity.
 * The coordinator is single-threaded; signal handlers open no descriptors.
 * No descriptor numbers are guessed from a possibly lowered RLIMIT_NOFILE.
 */
static int list_self_fds(struct proc_fdinfo *fds, int capacity_bytes) {
    errno=0;
    int bytes=proc_pidinfo(getpid(),PROC_PIDLISTFDS,0,fds,capacity_bytes);
    if(bytes<=0 || bytes>=capacity_bytes || bytes%(int)sizeof(*fds)) {
        if(!errno) errno=EOVERFLOW;
        die("descriptor enumeration failed or filled bounded buffer");
    }
    return bytes/(int)sizeof(*fds);
}
static void close_extra_fds(void) {
    struct proc_fdinfo fds[4096];
    int n=list_self_fds(fds,sizeof fds);
    for(int i=0;i<n;i++) {
        if(fds[i].proc_fd<0) { errno=EINVAL; die("invalid descriptor entry"); }
        if(fds[i].proc_fd>=3 && close(fds[i].proc_fd)<0 && errno!=EBADF && errno!=EINTR)
            die("close inherited descriptor");
    }
    /* A second complete snapshot verifies closure, including close EINTR cases. */
    n=list_self_fds(fds,sizeof fds);
    for(int i=0;i<n;i++) if(fds[i].proc_fd<0 || fds[i].proc_fd>=3) {
        errno=EBUSY; die("extra descriptor remained after closure");
    }
}
static double now(void) { struct timespec t; if (clock_gettime(CLOCK_MONOTONIC,&t)) die("clock"); return t.tv_sec+t.tv_nsec/1e9; }
static void path(char *out, const char *a, const char *b) {
    if (snprintf(out,PATH_MAX,"%s/%s",a,b) >= PATH_MAX) die("path length");
}
static int scan(uid_t uid, int type, int verbose) {
    pid_t pids[65536]; errno=0;
    int bytes=proc_listpids(type,uid,pids,sizeof pids), error=errno;
    int complete=!(bytes<0 || (bytes==0 && error) || bytes%sizeof(pid_t) || bytes >= (int)sizeof pids);
    int n=complete?bytes/(int)sizeof(pid_t):-1;
    printf("UID-SCAN uid=%u filter=%d bytes=%d errno=%d count=%d complete=%d\n",uid,type,bytes,error,n,complete);
    if(!complete) return -1;
    if (verbose) for (int i=0;i<n;i++) printf("UID-MEMBER uid=%u pid=%d filter=%d\n",uid,pids[i],type);
    return n;
}
static int empty(uid_t uid) {
    int a=scan(uid,PROC_UID_ONLY,0), b=scan(uid,PROC_RUID_ONLY,0);
    if (a<0 || b<0) return -1;
    return a==0 && b==0;
}
static int absent(uid_t uid) {
    pid_t pids[65536]; errno=0;
    int bytes=proc_listpids(PROC_ALL_PIDS,0,pids,sizeof pids), error=errno;
    if(bytes<=0 || error || bytes%sizeof(pid_t) || bytes>=(int)sizeof pids) return -1;
    for(int i=0;i<bytes/(int)sizeof(pid_t);i++) {
        struct proc_bsdshortinfo p; errno=0;
        if(proc_pidinfo(pids[i],PROC_PIDT_SHORTBSDINFO,1,&p,sizeof p)!=(int)sizeof p) {
            if(errno==ESRCH) continue;
            return -1;
        }
        if(p.pbsi_uid==uid||p.pbsi_ruid==uid||p.pbsi_svuid==uid||
           p.pbsi_gid==uid||p.pbsi_rgid==uid||p.pbsi_svgid==uid) return 0;
    }
    return empty(uid);
}
static void verify(uid_t uid, const char *label) {
    struct proc_bsdshortinfo p; gid_t groups[64], directory_groups[64];
    /* Libc 71bbe350ab79eef58113991d817ccc6165061a64 include/unistd.h
     * maps _DARWIN_C_SOURCE getgroups to a directory-account extension.
     * sys/getgroups.c returns EINVAL for our deliberately accountless UID.
     * Keep that result as diagnostics; verify setgroups against XNU's actual
     * credential vector via SYS_getgroups (kern_prot.c / syscalls.master).
     * XNU rejects undersized buffers, not a capacity of 64.
     */
    errno=0; int directory_n=getgroups(64,directory_groups); int directory_errno=errno;
    errno=0; int n=(int)syscall(SYS_getgroups,64,groups); int groups_errno=errno;
    if (proc_pidinfo(getpid(),PROC_PIDT_SHORTBSDINFO,0,&p,sizeof p)!=(int)sizeof p) die("self info");
    printf("CREDENTIALS label=%s pid=%d uid=%u euid=%u suid=%u gid=%u egid=%u sgid=%u groups=%d groups_errno=%d darwin_getgroups=%d darwin_getgroups_errno=%d\n",
           label,getpid(),getuid(),geteuid(),p.pbsi_svuid,getgid(),getegid(),p.pbsi_svgid,n,groups_errno,directory_n,directory_errno);
    if (!uid || getuid()!=uid || geteuid()!=uid || p.pbsi_svuid!=uid ||
        getgid()!=uid || getegid()!=uid || p.pbsi_svgid!=uid || n!=1 || groups[0]!=uid) exit(77);
}
static void drop(uid_t uid,rlim_t process_limit,rlim_t file_limit) {
    gid_t group=uid;
    struct rlimit np={process_limit,process_limit}, nf={file_limit,file_limit}, fs={64*1024*1024,64*1024*1024}, core={0,0};
    if (setrlimit(RLIMIT_NPROC,&np)||setrlimit(RLIMIT_NOFILE,&nf)||
        setrlimit(RLIMIT_FSIZE,&fs)||setrlimit(RLIMIT_CORE,&core)||
        setgroups(1,&group)||setgid(uid)||setuid(uid)) die("drop");
    verify(uid,"after-drop"); errno=0;
    if (setuid(0)!=-1 || errno!=EPERM) exit(77);
    verify(uid,"root-regain-denied");
}
static int reap(pid_t p, double deadline, int *status) {
    while (now()<deadline) {
        pid_t r=waitpid(p,status,WNOHANG);
        if (r==p) return 1;
        if (r<0 && errno!=EINTR) return 0;
        usleep(50000);
    }
    return 0;
}
static int cleanup(uid_t uid) {
    double end=now()+8;
    do {
        int reaped_status; while(waitpid(-1,&reaped_status,WNOHANG)>0) {}
        int state=empty(uid); if (state<0) break;
        if (state) { printf("UID-EMPTY uid=%u\n",uid); return 1; }
        pid_t worker=fork(); if (worker<0) break;
        if (!worker) {
            alarm(2); close_extra_fds(); drop(uid,64,256);
            errno=0; int r=kill(-1,SIGKILL);
            _exit(r==0 || errno==ESRCH ? 0 : 77);
        }
        int status=0; errno=0;
        int waited=reap(worker,end,&status), wait_error=errno;
        printf("SIGNAL-WORKER pid=%d reaped=%d wait_errno=%d raw_status=%d exit=%d signal=%d\n",
               worker,waited,wait_error,status,
               waited&&WIFEXITED(status)?WEXITSTATUS(status):-1,
               waited&&WIFSIGNALED(status)?WTERMSIG(status):0);
        if(!waited) break;
        /* XNU kill wrapper uses posix=1 under __DARWIN_UNIX03;
         * kern_sig.c killpg1_allfilt then includes the sender.
         * Its expected SIGKILL is not a completion receipt: rescan the UID.
         */
        if(!(WIFEXITED(status)&&WEXITSTATUS(status)==0) &&
           !(WIFSIGNALED(status)&&WTERMSIG(status)==SIGKILL)) break;
        usleep(100000);
    } while (now()<end);
    printf("QUARANTINE uid=%u reason=incomplete-uid-teardown\n",uid);
    scan(uid,PROC_UID_ONLY,1); scan(uid,PROC_RUID_ONLY,1); return 0;
}
static void owned_dir(const char *name,uid_t uid) {
    if(mkdir(name,0700)||chown(name,uid,uid)) die("synthetic directory");
}

static void checked_limits(rlim_t processes,rlim_t files) {
    struct rlimit p,f;
    if(getrlimit(RLIMIT_NPROC,&p)||getrlimit(RLIMIT_NOFILE,&f)||p.rlim_cur!=processes||p.rlim_max!=processes||f.rlim_cur!=files||f.rlim_max!=files) die("inherited limit mismatch");
    struct rlimit raised={files+1,files+1}; errno=0;
    if(setrlimit(RLIMIT_NOFILE,&raised)!=-1||errno!=EPERM) die("file limit raise was not denied");
    raised.rlim_cur=raised.rlim_max=processes+1; errno=0;
    if(setrlimit(RLIMIT_NPROC,&raised)!=-1||errno!=EPERM) die("process limit raise was not denied");
    printf("RESOURCE-LIMITS processes=%llu files=%llu inherited=1 raises_denied=1\n",(unsigned long long)processes,(unsigned long long)files);
}
static int resource(uid_t uid,const char *mode,const char *self) {
    verify(uid,"resource-guest"); alarm(8);
    rlim_t processes=strncmp(mode,"nproc-",6)?64:(!strcmp(mode,"nproc-control")?64:8);
    rlim_t files=!strcmp(mode,"nofile-control")?256:4096;
    checked_limits(processes,files);
    if(!strcmp(mode,"nofile-exec")) {
        pid_t p=fork(); if(p<0) die("descriptor inheritance fork");
        if(!p) { char ids[32];snprintf(ids,sizeof ids,"%u",uid);execl(self,self,"resource",ids,"nofile-4096",(char *)NULL);_exit(77); }
        int status=0;
        if(!reap(p,now()+7,&status)||!WIFEXITED(status)||WEXITSTATUS(status)) return 77;
        puts("RESOURCE-NOFILE-EXEC observed=1"); return 0;
    }
    if(strncmp(mode,"nproc-",6)) {
        int fds[4100],count=0,error=0;
        while(count<4100) { int fd=open("/dev/null",O_RDONLY); if(fd<0) {error=errno;break;} fds[count++]=fd; }
        int closed=1;for(int i=0;i<count;i++) if(close(fds[i])) closed=0;
        printf("RESOURCE-NOFILE opened=%d expected=%llu errno=%d closed=%d\n",count,(unsigned long long)files-3,error,closed);
        return count==(int)files-3&&error==EMFILE&&closed?0:77;
    }
    int count=0,error=0;
    for(int i=0;i<12;i++) {
        pid_t p=-1;
        if(!strcmp(mode,"nproc-spawn")) {
            char ids[32];snprintf(ids,sizeof ids,"%u",uid);char *args[]={(char *)self,"hold",ids,NULL};
            int result=posix_spawn(&p,self,NULL,NULL,args,environ);if(result) {error=result;break;}
        } else {
            p=fork();if(p<0) {error=errno;break;}
            if(!p) {alarm(6);for(;;) pause();}
        }
        count++;
    }
    int control=!strcmp(mode,"nproc-control");
    printf("RESOURCE-NPROC mode=%s children=%d errno=%d parent_exits_with_live_children=1\n",mode,count,error);
    return control?(count==12&&!error?0:77):(count==7&&error==EAGAIN?0:77);
}
int main(int argc,char **argv) {
    setvbuf(stdout,NULL,_IONBF,0);
    if(argc==3&&!strcmp(argv[1],"hold")) {verify((uid_t)atoi(argv[2]),"spawn-held-child");alarm(6);for(;;) pause();}
    if(argc==4&&!strcmp(argv[1],"resource")) return resource((uid_t)atoi(argv[2]),argv[3],argv[0]);
    if(argc!=2||geteuid()!=0) return 77;
    close_extra_fds();signal(SIGINT,stop);signal(SIGTERM,stop);
    const char *root=argv[1];struct stat rst;
    const char *prefix="/private/tmp/crucible-macos-resources.";
    if(lstat(root,&rst)||!S_ISDIR(rst.st_mode)||strncmp(root,prefix,strlen(prefix))) die("fixture root");
    if(chown(root,0,0)||chmod(root,0755)||chdir(root)) die("fixture root ownership");
    uid_t uid=0;
    for(uid_t n=60000;n<60128;n++) {
        errno=0;struct passwd *pw=getpwuid(n);int pe=errno;
        errno=0;struct group *gr=getgrgid(n);int ge=errno;
        if(!pw&&!gr&&!pe&&!ge) {int a=absent(n);if(a<0) die("UID enumeration");if(a==1) {uid=n;break;}}
    }
    if(!uid) die("no unused fixture UID");
    printf("UID-LEASE uid=%u assumption=exclusive-disposable-VM-only\n",uid);
    const char *cases[]={"nofile-control","nofile-4096","nofile-exec","nproc-control","nproc-fork","nproc-spawn"};
    for(int c=0;c<6;c++) {
        if(stopped||empty(uid)!=1) {puts("QUARANTINE before next cell");return 77;}
        char work[PATH_MAX],self[PATH_MAX],prof[PATH_MAX],ids[32];
        path(work,root,cases[c]);owned_dir(work,uid);path(self,root,"resources");path(prof,root,"network-deny.sb");snprintf(ids,sizeof ids,"%u",uid);
        pid_t p=fork();if(p<0) {cleanup(uid);die("guest fork");}
        if(!p) {
            signal(SIGINT,SIG_DFL);signal(SIGTERM,SIG_DFL);close_extra_fds();
            rlim_t np=c==4||c==5?8:64,nf=c==0?256:4096;drop(uid,np,nf);
            if(chdir(work)) die("guest cwd");char *clean[]={NULL};environ=clean;
            alarm(8);execl("/usr/bin/sandbox-exec","sandbox-exec","-f",prof,self,"resource",ids,cases[c],(char *)NULL);die("guest exec");
        }
        int status=0,done=0;double end=now()+10;
        while(now()<end&&!stopped) {pid_t r=waitpid(p,&status,WNOHANG);if(r==p) {done=1;break;}if(r<0&&errno!=EINTR) break;usleep(20000);}
        int code=done&&WIFEXITED(status)?WEXITSTATUS(status):-1;
        printf("RESOURCE-RESULT case=%s reaped=%d exit=%d signal=%d\n",cases[c],done,code,done&&WIFSIGNALED(status)?WTERMSIG(status):0);
        int clean=cleanup(uid);
        if(!done) {pid_t r=waitpid(p,&status,WNOHANG);if(r!=p&&!(r<0&&errno==ECHILD)&&!reap(p,now()+3,&status)) {puts("QUARANTINE unreaped direct child");return 77;}}
        if(!clean||!done||code) return 77;
        printf("RESOURCE-CASE-CLEAN case=%s uid_empty=1\n",cases[c]);
    }
    puts("RESOURCE-FEASIBILITY-COMPLETE cases=6 full_sandbox_tested=0");return 0;
}
C
/usr/bin/xcrun clang -O0 -Wall -Wextra -Werror -Wno-deprecated-declarations "$probe_root/resources.c" -o "$probe_root/resources"
chmod 0755 "$probe_root/resources"
chmod 0644 "$probe_root/network-deny.sb"
/usr/bin/sw_vers
/usr/bin/uname -mrv
/usr/bin/shasum -a 256 "$probe_root/resources.c" "$probe_root/resources" "$probe_root/network-deny.sb"
printf 'RESOURCE-PROBE six cells; synthetic UID/rlimits/owned signals only; no accounts/services/host ACL changes\n'
sudo -n /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin "$probe_root/resources" "$probe_root" < /dev/null
