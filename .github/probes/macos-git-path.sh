#!/bin/sh
# Three-case, 10-second Git launch-path diagnostic on an exclusive disposable VM.
# This is NOT the 24-case native compatibility battery or a sandbox approval.
# No accounts, services, host ACLs, user data, or global developer selection change.
# Exit 1 records a failed/timed-out case after all three comparisons; 77 is a
# fixture or cleanup failure. A cleanup failure immediately stops comparisons.
set -eu
test "$(uname -s)" = Darwin || { echo 'Requires native macOS' >&2; exit 77; }
test "$(id -u)" != 0 || { echo 'Start as the VM runner, not root' >&2; exit 77; }
# Apple TN2339 documents /usr/bin tool shims; TN3147 documents DEVELOPER_DIR.
# Resolve only installed tool paths as the ordinary VM runner, before UID drop.
probe_developer_dir=$(/usr/bin/xcode-select --print-path)
case "$probe_developer_dir" in /*) ;; *) echo 'Nonabsolute developer directory' >&2; exit 77 ;; esac
probe_developer_dir=$(cd "$probe_developer_dir" && pwd -P)
probe_git=$(/usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin DEVELOPER_DIR="$probe_developer_dir" /usr/bin/xcrun --find git)
probe_sdk=$(/usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin DEVELOPER_DIR="$probe_developer_dir" /usr/bin/xcrun --sdk macosx --show-sdk-path)
case "$probe_git" in /*) ;; *) echo 'Nonabsolute resolved Git' >&2; exit 77 ;; esac
case "$probe_sdk" in /*) ;; *) echo 'Nonabsolute SDK' >&2; exit 77 ;; esac
test -x "$probe_git" && test -d "$probe_sdk"
test "$probe_git" != /usr/bin/git || { echo 'Resolved Git still names shim' >&2; exit 77; }
probe_root=$(mktemp -d /tmp/crucible-macos-git-path.XXXXXX)
probe_root=$(cd "$probe_root" && pwd -P)
printf 'FIXTURE %s\nDEVELOPER-DIR %s\nRESOLVED-GIT %s\nSDK %s\n' "$probe_root" "$probe_developer_dir" "$probe_git" "$probe_sdk"
printf 'synthetic descriptor fixture\n' > "$probe_root/fd-fixture"
cat > "$probe_root/control.sb" <<'SB'
(version 1)
(allow default)
SB
cat > "$probe_root/run-case.sh" <<'SH'
set -eu
case_name=$1
echo "STAGE case=$case_name script-entered"
mkdir repo empty-template
cd repo
echo "STAGE case=$case_name git-init-before"
"$PROBE_GIT" init -q --template=../empty-template
echo "STAGE case=$case_name git-init-after"
test -d .git
echo "GIT-INIT-RETURNED case=$case_name"
SH
cat > "$probe_root/uid.c" <<'C'
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
    if(argc==6 && !strcmp(argv[1],"guest")) {
        uid_t uid=(uid_t)atoi(argv[2]); verify(uid,"sandbox-guest"); alarm(10);
        char script[PATH_MAX]; path(script,argv[5],"run-case.sh");
        printf("GUEST-SHELL-EXEC case=%s pid=%d\n",argv[3],getpid());
        execl("/bin/sh","sh",script,argv[3],(char *)NULL); die("guest exec");
    }
    if(argc!=5 || geteuid()!=0 || argv[2][0]!='/' || argv[3][0]!='/' || argv[4][0]!='/') {
        fprintf(stderr,"coordinator needs root and absolute fixture/developer-dir/real-git/sdk paths\n"); return 77;
    }
    close_extra_fds(); signal(SIGINT,stop); signal(SIGTERM,stop);
    const char *root=argv[1]; struct stat rst;
    const char prefix[]="/private/tmp/crucible-macos-git-path.";
    if(lstat(root,&rst)||!S_ISDIR(rst.st_mode)||strncmp(root,prefix,sizeof prefix-1)) die("fixture root");
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
    const char *cases[]={"shim-default","shim-developer-dir","resolved-developer-dir"};
    int failed=0;
    for(int c=0;c<3;c++) {
        if(stopped) { cleanup(uid); return 77; }
        if(empty(uid)!=1) { puts("QUARANTINE reason=not-empty-before-guest"); return 77; }
        char work[PATH_MAX],prof[PATH_MAX],self[PATH_MAX],ids[32],home[PATH_MAX],tmp[PATH_MAX];
        path(work,root,cases[c]); owned_dir(work,uid);
        path(home,work,"home"); path(tmp,work,"tmp");
        owned_dir(home,uid); owned_dir(tmp,uid);
        path(prof,root,"control.sb"); path(self,root,"uid"); snprintf(ids,sizeof ids,"%u",uid);
        const char *git=c==2?argv[3]:"/usr/bin/git";
        printf("DIAGNOSTIC-BEGIN case=%s executable=%s developer_dir=%s workload_deadline_seconds=10\n",
               cases[c],git,c?argv[2]:"<unset>");
        pid_t p=fork(); if(p<0) { cleanup(uid); die("guest fork"); }
        if(!p) {
            signal(SIGINT,SIG_DFL); signal(SIGTERM,SIG_DFL); close_extra_fds(); drop(uid);
            if(chdir(work)) die("guest cwd");
            char *clean[]={NULL}; environ=clean;
            if(setenv("HOME",home,1)||setenv("TMPDIR",tmp,1)||
               setenv("PATH","/usr/bin:/bin:/usr/sbin:/sbin",1)||
               setenv("SDKROOT",argv[4],1)||setenv("PROBE_GIT",git,1)||
               setenv("GIT_CONFIG_NOSYSTEM","1",1)||setenv("GIT_CONFIG_GLOBAL","/dev/null",1)||
               (c && setenv("DEVELOPER_DIR",argv[2],1))) die("environment");
            execl("/usr/bin/sandbox-exec","sandbox-exec","-f",prof,self,"guest",ids,cases[c],"control",root,(char *)NULL);
            die("sandbox exec");
        }
        int status=0,done=0; double end=now()+10;
        while(now()<end&&!stopped) {
            pid_t r=waitpid(p,&status,WNOHANG);
            if(r==p) { done=1; break; }
            if(r<0&&errno!=EINTR) break;
            usleep(50000);
        }
        printf("GUEST-WAIT case=%s reaped=%d raw_status=%d exit=%d signal=%d supervisor_deadline=%d\n",
               cases[c],done,status,
               done&&WIFEXITED(status)?WEXITSTATUS(status):-1,
               done&&WIFSIGNALED(status)?WTERMSIG(status):0,!done&&!stopped);
        int result=done&&WIFEXITED(status)?WEXITSTATUS(status):77;
        int clean=cleanup(uid);
        if(!done) {
            pid_t r=waitpid(p,&status,WNOHANG);
            if(r!=p && !(r<0&&errno==ECHILD) && !reap(p,now()+3,&status)) {
                puts("QUARANTINE reason=unreaped-coordinator-child"); return 77;
            }
        }
        if(!clean) return 77;
        printf("DIAGNOSTIC-RESULT case=%s status=%d uid_empty=1\n",cases[c],result);
        if(result) failed++;
    }
    printf("GIT-PATH-DIAGNOSTIC-COMPLETE cases=3 failed_cases=%d full_compatibility_tested=0\n",failed);
    return failed?1:0;
}
C
/usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin DEVELOPER_DIR="$probe_developer_dir" \
    /usr/bin/xcrun clang -O0 -Wall -Wextra -Werror -Wno-deprecated-declarations "$probe_root/uid.c" -o "$probe_root/uid"
/usr/bin/env -i PATH=/usr/bin:/bin "$probe_root/uid" fd-selftest "$probe_root/fd-fixture" < /dev/null
chmod a+rx "$probe_root/uid"
chmod a+r "$probe_root/control.sb" "$probe_root/run-case.sh"
/usr/bin/sw_vers
/usr/bin/uname -mrv
/usr/bin/shasum -a 256 /usr/bin/git "$probe_git" "$probe_root/uid.c" "$probe_root/uid" "$probe_root/run-case.sh"
printf 'PRIVILEGED EFFECTS: synthetic ownership/modes, credential drops, limits, same-UID SIGKILL only. No accounts/services/host ACL changes.\n'
set +e
sudo -n /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin "$probe_root/uid" \
    "$probe_root" "$probe_developer_dir" "$probe_git" "$probe_sdk" < /dev/null
probe_status=$?
set -e
printf 'DIAGNOSTIC-FIXTURE-RESULT status=%s retained=%s full_compatibility_tested=0\n' "$probe_status" "$probe_root"
exit "$probe_status"
