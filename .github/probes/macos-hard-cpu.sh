#!/bin/sh
# macos-hard-cpu-v1: two cases, no full compatibility or sandbox claim.
# DISPOSABLE macOS VM ONLY. Numeric UID noncollision is a VM fixture assumption.
# Privileged effects are confined to new synthetic directories and owned children.
set -eu
test "$(uname -s)" = Darwin || { echo 'Requires native macOS' >&2; exit 77; }
test "$(id -u)" != 0 || { echo 'Start as the VM runner, not root' >&2; exit 77; }
probe_developer_dir=$(/usr/bin/xcode-select --print-path)
case "$probe_developer_dir" in /*) ;; *) exit 77 ;; esac
probe_developer_dir=$(cd "$probe_developer_dir" && pwd -P)
probe_clang=$(/usr/bin/env -i PATH=/usr/bin:/bin DEVELOPER_DIR="$probe_developer_dir" /usr/bin/xcrun --find clang)
probe_sdk=$(/usr/bin/env -i PATH=/usr/bin:/bin DEVELOPER_DIR="$probe_developer_dir" /usr/bin/xcrun --sdk macosx --show-sdk-path)
for probe_path in "$probe_clang" "$probe_sdk"; do
    case "$probe_path" in /*) ;; *) echo 'Nonabsolute tool/SDK' >&2; exit 77 ;; esac
done
test -x "$probe_clang" && test -d "$probe_sdk"
probe_root=$(mktemp -d /tmp/crucible-macos-cpu.XXXXXX)
probe_root=$(cd "$probe_root" && pwd -P)
printf 'FIXTURE %s\n' "$probe_root"
cat > "$probe_root/network-deny.sb" <<'SB'
(version 1)
(allow default)
(deny network*)
SB
cat > "$probe_root/cpu.c" <<'C'
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
static void drop(uid_t uid) {
    gid_t group=uid;
    struct rlimit np={64,64}, nf={256,256}, fs={64*1024*1024,64*1024*1024}, core={0,0};
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
            alarm(2); close_extra_fds(); drop(uid);
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

static volatile sig_atomic_t cpu_signals;
static void cpu_signal(int sig) { (void)sig; cpu_signals++; }
static double used_cpu(const struct rusage *u) {
    return (double)u->ru_utime.tv_sec+u->ru_utime.tv_usec/1e6+
           (double)u->ru_stime.tv_sec+u->ru_stime.tv_usec/1e6;
}
static int burn(uid_t uid, const char *mode) {
    verify(uid,"sandbox-cpu-guest"); alarm(9);
    struct rlimit limit;
    if(getrlimit(RLIMIT_CPU,&limit)) die("read CPU limit");
    printf("CPU-LIMIT case=%s soft=%llu hard=%llu\n",mode,
           (unsigned long long)limit.rlim_cur,(unsigned long long)limit.rlim_max);
    if(limit.rlim_cur!=1 || limit.rlim_max!=1) return 77;
    struct sigaction action; memset(&action,0,sizeof action);
    action.sa_handler=!strcmp(mode,"handler")?cpu_signal:SIG_DFL;
    if(sigemptyset(&action.sa_mask)||sigaction(SIGXCPU,&action,NULL)) die("SIGXCPU disposition");
    printf("CPU-BURN-START case=%s wall_alarm=9 target_cpu=3\n",mode);
    volatile unsigned long value=1;
    for(;;) {
        for(unsigned int i=0;i<1000000;i++) value=value*1664525UL+1013904223UL;
        struct rusage usage;
        if(getrusage(RUSAGE_SELF,&usage)) die("guest CPU observation");
        double cpu=used_cpu(&usage);
        if(cpu>=3.0) {
            printf("HARD-CPU-BYPASS case=%s cpu=%.6f signal_count=%d\n",mode,cpu,(int)cpu_signals);
            return 42;
        }
    }
}
int main(int argc,char **argv) {
    setvbuf(stdout,NULL,_IONBF,0);
    if(argc==3 && !strcmp(argv[1],"fd-selftest")) {
        int fd=open(argv[2],O_RDONLY);
        if(fd<3) die("self-test fixture descriptor");
        int high=fcntl(fd,F_DUPFD,128); if(high<128) die("self-test high descriptor");
        close_extra_fds();
        errno=0; if(fcntl(fd,F_GETFD)!=-1 || errno!=EBADF) return 77;
        errno=0; if(fcntl(high,F_GETFD)!=-1 || errno!=EBADF) return 77;
        puts("DARWIN-FD-CLOSE-SELFTEST-PASS"); return 0;
    }
    if(argc==4 && !strcmp(argv[1],"guest")) {
        if(strcmp(argv[3],"default") && strcmp(argv[3],"handler")) return 77;
        return burn((uid_t)atoi(argv[2]),argv[3]);
    }
    if(argc!=2 || geteuid()!=0) { fputs("root coordinator requires fixture root\n",stderr); return 77; }
    close_extra_fds(); signal(SIGINT,stop); signal(SIGTERM,stop);
    const char *root=argv[1]; struct stat rst;
    const char *prefix="/private/tmp/crucible-macos-cpu.";
    if(lstat(root,&rst)||!S_ISDIR(rst.st_mode)||strncmp(root,prefix,strlen(prefix))) die("fixture root");
    if(chown(root,0,0)||chmod(root,0755)||chdir(root)) die("fixture root ownership");
    uid_t uid=0;
    for(uid_t n=60000;n<60128;n++) {
        errno=0; struct passwd *pw=getpwuid(n); int pe=errno;
        errno=0; struct group *gr=getgrgid(n); int ge=errno;
        if(!pw&&!gr&&!pe&&!ge) {
            int a=absent(n); if(a<0) die("candidate process enumeration");
            if(a==1) { uid=n; break; }
        }
    }
    if(!uid) die("no demonstrably unused fixture UID");
    printf("UID-LEASE uid=%u assumption=exclusive-disposable-VM-only\n",uid);
    const char *cases[]={"default","handler"}; int control_ok=0, bypass=0, hard_kill=0;
    for(int c=0;c<2;c++) {
        if(stopped) { cleanup(uid); return 77; }
        if(empty(uid)!=1) { puts("QUARANTINE reason=not-empty-before-guest"); return 77; }
        char work[PATH_MAX],home[PATH_MAX],tmp[PATH_MAX],self[PATH_MAX],prof[PATH_MAX],ids[32];
        path(work,root,cases[c]); owned_dir(work,uid);
        path(home,work,"home"); path(tmp,work,"tmp"); owned_dir(home,uid); owned_dir(tmp,uid);
        path(self,root,"cpu"); path(prof,root,"network-deny.sb"); snprintf(ids,sizeof ids,"%u",uid);
        pid_t p=fork(); if(p<0) { cleanup(uid); die("guest fork"); }
        if(!p) {
            signal(SIGINT,SIG_DFL); signal(SIGTERM,SIG_DFL); close_extra_fds(); drop(uid);
            if(chdir(work)) die("guest cwd");
            char *clean[]={NULL}; environ=clean;
            if(setenv("HOME",home,1)||setenv("TMPDIR",tmp,1)||setenv("PATH","/usr/bin:/bin",1)) die("environment");
            struct rlimit cpu={1,1}; if(setrlimit(RLIMIT_CPU,&cpu)) die("CPU ceiling");
            alarm(9);
            execl("/usr/bin/sandbox-exec","sandbox-exec","-f",prof,self,"guest",ids,cases[c],(char *)NULL);
            die("sandbox exec");
        }
        int status=0,done=0; struct rusage usage; memset(&usage,0,sizeof usage);
        double started=now(),end=started+10;
        while(now()<end&&!stopped) {
            pid_t r=wait4(p,&status,WNOHANG,&usage);
            if(r==p) { done=1; break; }
            if(r<0&&errno!=EINTR) break;
            usleep(50000);
        }
        double cpu=done?used_cpu(&usage):-1.0;
        int code=done&&WIFEXITED(status)?WEXITSTATUS(status):-1;
        int sig=done&&WIFSIGNALED(status)?WTERMSIG(status):0;
        printf("CPU-RESULT case=%s reaped=%d exit=%d signal=%d cpu=%.6f wall=%.6f\n",
               cases[c],done,code,sig,cpu,now()-started);
        int clean=cleanup(uid);
        if(!done) {
            pid_t r=waitpid(p,&status,WNOHANG);
            if(r!=p && !(r<0&&errno==ECHILD) && !reap(p,now()+3,&status)) {
                puts("QUARANTINE reason=unreaped-coordinator-child"); return 77;
            }
        }
        if(!clean) return 77;
        printf("CPU-CASE-CLEAN case=%s uid_empty=1\n",cases[c]);
        if(c==0) control_ok=done&&(sig==SIGXCPU||sig==SIGKILL)&&cpu>=0.5&&cpu<3.0;
        else {
            bypass=done&&code==42&&cpu>=3.0;
            hard_kill=done&&sig==SIGKILL&&cpu>=0.5&&cpu<3.0;
        }
    }
    if(control_ok&&bypass) {
        puts("HARD-CPU-UNSUPPORTED mechanism=RLIMIT_CPU control=terminated handler=survived-three-cpu-seconds full_sandbox_tested=0");
        return 1;
    }
    if(control_ok&&hard_kill) {
        puts("HARD-CPU-OBSERVED native-path-only requires-source-reconciliation-and-inheritance-tests full_sandbox_tested=0");
        return 0;
    }
    printf("HARD-CPU-INCONCLUSIVE control_ok=%d bypass=%d hard_kill=%d full_sandbox_tested=0\n",control_ok,bypass,hard_kill);
    return 77;
}
C
SDKROOT="$probe_sdk" DEVELOPER_DIR="$probe_developer_dir" "$probe_clang" -O0 -Wall -Wextra -Werror -Wno-deprecated-declarations "$probe_root/cpu.c" -o "$probe_root/cpu"
/usr/bin/env -i PATH=/usr/bin:/bin "$probe_root/cpu" fd-selftest "$probe_root/cpu.c" < /dev/null
chmod a+rx "$probe_root/cpu"
chmod a+r "$probe_root/network-deny.sb"
/usr/bin/sw_vers
/usr/bin/uname -mrv
/usr/bin/shasum -a 256 "$probe_root/cpu.c" "$probe_root/cpu" "$probe_root/network-deny.sb"
printf 'CPU-PROBE v1 cases=2 soft=hard=1 guest-wall=9 supervisor-wall=10 workload-wait-max=20 cleanup-reap-wait-max=22\n'
printf 'PRIVILEGED EFFECTS: synthetic root/directory ownership, permanent UID/GID drops, limits, same-UID SIGKILL. No accounts/services/host ACL changes.\n'
set +e
sudo -n /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin "$probe_root/cpu" "$probe_root" < /dev/null
probe_status=$?
set -e
printf 'FIXTURE-RESULT status=%s retained=%s\n' "$probe_status" "$probe_root"
exit "$probe_status"
