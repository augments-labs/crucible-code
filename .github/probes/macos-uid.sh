#!/bin/sh
# DISPOSABLE macOS VM ONLY. Numeric UID noncollision is a fixture assumption,
# not production identity reservation. No account/service/host ACL changes.
set -eu
test "$(uname -s)" = Darwin || { echo 'Requires native macOS' >&2; exit 77; }
test "$(id -u)" != 0 || { echo 'Start as the VM runner, not root' >&2; exit 77; }
probe_rustc=$(rustup which --toolchain stable rustc)
probe_toolchain=$(dirname "$(dirname "$probe_rustc")")
probe_clang=$(/usr/bin/xcrun --find clang)
probe_sdk=$(/usr/bin/xcrun --sdk macosx --show-sdk-path)
probe_python=$(command -v python3)
probe_root=$(mktemp -d /tmp/crucible-macos-uid.XXXXXX)
probe_root=$(cd "$probe_root" && pwd -P)
printf 'FIXTURE %s\n' "$probe_root"
mkdir "$probe_root/runtime" "$probe_root/fixtures" "$probe_root/runtime/rust"
chmod 0755 "$probe_root/runtime" "$probe_root/fixtures" "$probe_root/runtime/rust"
# Copy installed Rust executable/library files only; no runner home/config access.
cp -R "$probe_toolchain/bin" "$probe_toolchain/lib" "$probe_root/runtime/rust/"
chmod -R a+rX "$probe_root/runtime"
cat > "$probe_root/control.sb" <<'SB'
(version 1)
(allow default)
SB
cat > "$probe_root/network-deny.sb" <<'SB'
(version 1)
(allow default)
(deny network*)
SB
cat > "$probe_root/fixtures/hello.c" <<'C'
int main(void) { return 0; }
C
cat > "$probe_root/fixtures/hello.rs" <<'RS'
fn main() { println!("RUST-BINARY-RAN"); }
RS
cat > "$probe_root/fixtures/command.rs" <<'RS'
fn main() {
    assert!(std::process::Command::new("/usr/bin/true").status().unwrap().success());
    println!("RUST-COMMAND-RAN");
}
RS
cat > "$probe_root/fixtures/python.py" <<'PY'
import errno, os, socket, subprocess, sys
mode = sys.argv[1]
if mode == "network":
    for family, address in [(socket.AF_INET, ("127.0.0.1", 0)),
                            (socket.AF_INET6, ("::1", 0)),
                            (socket.AF_UNIX, "synthetic.socket")]:
        denied = False
        try:
            with socket.socket(family, socket.SOCK_STREAM) as s: s.bind(address)
        except OSError as e:
            if e.errno not in (errno.EPERM, errno.EACCES): raise
            denied = True
        print("NETWORK", family, "denied", denied, flush=True)
        assert denied == (sys.argv[2] == "network-deny")
elif mode == "python-spawn":
    p = os.posix_spawn("/usr/bin/true", ["true"], os.environ)
    assert os.waitstatus_to_exitcode(os.waitpid(p, 0)[1]) == 0
else:
    kwargs = {"close_fds": False} if mode == "python-close-fds-false" else {}
    if mode == "python-cwd": kwargs["cwd"] = "."
    subprocess.run(["/usr/bin/true"], check=True, timeout=5, **kwargs)
print("PYTHON-OK", mode, flush=True)
PY
cat > "$probe_root/run-case.sh" <<'SH'
set -eu
case_name=$1; profile=$2; fixture=$3
case "$case_name" in
    shell-git)
        test "$(printf shell | /usr/bin/tr a-z A-Z)" = SHELL
        mkdir repo empty-template; cd repo
        /usr/bin/git init -q --template=../empty-template
        printf payload > a
        /usr/bin/git -c core.hooksPath=/dev/null add a
        test "$(/usr/bin/git diff --cached --name-only)" = a
        test "$(/usr/bin/git -c alias.probe='!printf child-shell' probe)" = child-shell ;;
    rust-command) "$fixture/rust-command" ;;
    rustc-link) "$RUSTC" --edition=2021 "$fixture/hello.rs" -o rust-output; ./rust-output ;;
    clang-object) "$PROBE_CLANG" -c "$fixture/hello.c" -o c-output.o; test -s c-output.o ;;
    clang-link) "$PROBE_CLANG" "$fixture/hello.c" -o c-output; ./c-output ;;
    cargo-test)
        mkdir src
        printf '[package]\nname="uid-probe"\nversion="0.0.0"\nedition="2021"\n' > Cargo.toml
        cat > build.rs <<'RS'
fn main() { assert!(std::process::Command::new("/usr/bin/true").status().unwrap().success()); }
RS
        cat > src/lib.rs <<'RS'
#[test]
fn native_child() { assert!(std::process::Command::new("/usr/bin/true").status().unwrap().success()); }
RS
        "$PROBE_CARGO" test --offline --jobs 1 --target-dir target -- --nocapture ;;
    python-*|network) "$PROBE_PYTHON" -I "$fixture/python.py" "$case_name" "$profile" ;;
    *) exit 77 ;;
esac
printf 'CASE-OK %s\n' "$case_name"
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
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
extern char **environ;
static volatile sig_atomic_t stopped;
static void stop(int s) { (void)s; stopped = 1; }
static _Noreturn void die(const char *s) { perror(s); exit(77); }
static double now(void) { struct timespec t; if (clock_gettime(CLOCK_MONOTONIC,&t)) die("clock"); return t.tv_sec+t.tv_nsec/1e9; }
static void path(char *out, const char *a, const char *b) {
    if (snprintf(out,PATH_MAX,"%s/%s",a,b) >= PATH_MAX) die("path length");
}
static int scan(uid_t uid, int type, int verbose) {
    pid_t pids[65536]; errno=0;
    int bytes=proc_listpids(type,uid,pids,sizeof pids), error=errno;
    if (bytes<0 || (bytes==0 && error) || bytes%sizeof(pid_t) || bytes >= (int)sizeof pids) return -1;
    int n=bytes/(int)sizeof(pid_t);
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
    struct proc_bsdshortinfo p; gid_t groups[64];
    int n=getgroups(64,groups);
    if (proc_pidinfo(getpid(),PROC_PIDT_SHORTBSDINFO,0,&p,sizeof p)!=(int)sizeof p) die("self info");
    printf("CREDENTIALS label=%s pid=%d uid=%u euid=%u suid=%u gid=%u egid=%u sgid=%u groups=%d\n",
           label,getpid(),getuid(),geteuid(),p.pbsi_svuid,getgid(),getegid(),p.pbsi_svgid,n);
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
            alarm(2); closefrom(3); drop(uid);
            errno=0; int r=kill(-1,SIGKILL);
            _exit(r==0 || errno==ESRCH ? 0 : 77);
        }
        int status;
        if (!reap(worker,end,&status) || !WIFEXITED(status) || WEXITSTATUS(status)) break;
        usleep(100000);
    } while (now()<end);
    printf("QUARANTINE uid=%u reason=incomplete-uid-teardown\n",uid);
    scan(uid,PROC_UID_ONLY,1); scan(uid,PROC_RUID_ONLY,1); return 0;
}
static void heartbeat(uid_t uid, const char *name, pid_t oldsid, pid_t oldpgid) {
    verify(uid,name); alarm(20);
    printf("ESCAPED route=%s pid=%d sid=%d pgid=%d old_sid=%d old_pgid=%d\n",
           name,getpid(),getsid(0),getpgrp(),oldsid,oldpgid);
    if (getpgrp()==oldpgid || (strcmp(name,"spawn-pgid") && getsid(0)==oldsid)) exit(77);
    int fd=open(name,O_WRONLY|O_CREAT|O_EXCL,0600); if (fd<0) die("heartbeat");
    for (int i=0;i<150;i++) { if (write(fd,"x",1)!=1) die("heartbeat write"); usleep(100000); }
    close(fd); exit(0);
}
static int escapes(const char *self, uid_t uid) {
    const char *names[]={"spawn-sid","spawn-pgid","fork-sid"};
    char ids[32],sid[32],pgid[32];
    snprintf(ids,sizeof ids,"%u",uid); snprintf(sid,sizeof sid,"%d",getsid(0)); snprintf(pgid,sizeof pgid,"%d",getpgrp());
    for (int i=0;i<3;i++) {
        pid_t p;
        if (i==2) { p=fork(); if (!p) { if (setsid()<0) die("setsid"); heartbeat(uid,names[i],atoi(sid),atoi(pgid)); } }
        else {
            posix_spawnattr_t attr; int e=posix_spawnattr_init(&attr); if(e) return 77;
            e=posix_spawnattr_setflags(&attr,i==0?POSIX_SPAWN_SETSID:POSIX_SPAWN_SETPGROUP);
            if (!e && i==1) e=posix_spawnattr_setpgroup(&attr,0);
            char *args[]={(char *)self,"heartbeat",ids,(char *)names[i],sid,pgid,NULL};
            if (!e) e=posix_spawn(&p,self,NULL,&attr,args,environ);
            posix_spawnattr_destroy(&attr); if(e) { errno=e; die("escape spawn"); }
        }
        if(p<0) die("escape fork");
    }
    for(int n=0;n<50;n++) {
        int ready=0; struct stat st;
        for(int i=0;i<3;i++) if(!stat(names[i],&st) && st.st_size>0) ready++;
        if(ready==3) { puts("ESCAPE-READY count=3"); return 0; }
        usleep(100000);
    }
    return 77;
}
static void owned_dir(const char *name,uid_t uid) {
    if(mkdir(name,0700)||chown(name,uid,uid)) die("synthetic directory");
}
static int quiet_heartbeats(const char *work) {
    const char *names[]={"spawn-sid","spawn-pgid","fork-sid"}; off_t sizes[3];
    for(int round=0;round<2;round++) {
        for(int i=0;i<3;i++) {
            char file[PATH_MAX]; struct stat st; path(file,work,names[i]);
            if(lstat(file,&st)||!S_ISREG(st.st_mode)||st.st_size<1) return 0;
            if(round && sizes[i]!=st.st_size) return 0;
            sizes[i]=st.st_size;
        }
        if(!round) usleep(300000);
    }
    puts("HEARTBEATS-STOPPED count=3"); return 1;
}
int main(int argc,char **argv) {
    setvbuf(stdout,NULL,_IONBF,0);
    if(argc==6 && !strcmp(argv[1],"heartbeat")) heartbeat((uid_t)atoi(argv[2]),argv[3],atoi(argv[4]),atoi(argv[5]));
    if(argc==6 && !strcmp(argv[1],"guest")) {
        uid_t uid=(uid_t)atoi(argv[2]); verify(uid,"sandbox-guest"); alarm(60);
        if(!strcmp(argv[3],"escape")) return escapes(argv[0],uid);
        char script[PATH_MAX],fixtures[PATH_MAX]; path(script,argv[5],"run-case.sh"); path(fixtures,argv[5],"fixtures");
        execl("/bin/sh","sh",script,argv[3],argv[4],fixtures,(char *)NULL); die("guest exec");
    }
    if(argc!=5 || geteuid()!=0) { fprintf(stderr,"coordinator needs root and fixture/clang/python/sdk\n"); return 77; }
    closefrom(3); signal(SIGINT,stop); signal(SIGTERM,stop);
    const char *root=argv[1]; struct stat rst;
    if(lstat(root,&rst)||!S_ISDIR(rst.st_mode)||strncmp(root,"/private/tmp/crucible-macos-uid.",32)) die("fixture root");
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
    const char *cases[]={"shell-git","rust-command","rustc-link","clang-object","clang-link","cargo-test",
        "python-default","python-close-fds-false","python-cwd","python-spawn","network","escape"};
    const char *profiles[]={"control","network-deny"}; int failed=0;
    for(int pr=0;pr<2;pr++) for(int c=0;c<12;c++) {
        if(stopped) { cleanup(uid); return 77; }
        if(empty(uid)!=1) { puts("QUARANTINE reason=not-empty-before-guest"); return 77; }
        char work[PATH_MAX],name[80],prof[PATH_MAX],self[PATH_MAX],ids[32],home[PATH_MAX],tmp[PATH_MAX],cargo[PATH_MAX];
        snprintf(name,sizeof name,"%s-%s",profiles[pr],cases[c]); path(work,root,name); owned_dir(work,uid);
        path(home,work,"home"); path(tmp,work,"tmp"); path(cargo,work,"cargo-home");
        owned_dir(home,uid); owned_dir(tmp,uid); owned_dir(cargo,uid);
        snprintf(name,sizeof name,"%s.sb",profiles[pr]); path(prof,root,name); path(self,root,"uid"); snprintf(ids,sizeof ids,"%u",uid);
        pid_t p=fork(); if(p<0) { cleanup(uid); die("guest fork"); }
        if(!p) {
            signal(SIGINT,SIG_DFL); signal(SIGTERM,SIG_DFL); closefrom(3); drop(uid);
            if(chdir(work)) die("guest cwd");
            char rustc[PATH_MAX],rustdoc[PATH_MAX],cargobin[PATH_MAX],envpath[PATH_MAX];
            path(rustc,root,"runtime/rust/bin/rustc"); path(rustdoc,root,"runtime/rust/bin/rustdoc"); path(cargobin,root,"runtime/rust/bin/cargo");
            snprintf(envpath,sizeof envpath,"%s/runtime/rust/bin:/usr/bin:/bin:/usr/sbin:/sbin",root);
            char *clean[]={NULL}; environ=clean;
            if(setenv("HOME",home,1)||setenv("TMPDIR",tmp,1)||setenv("CARGO_HOME",cargo,1)||setenv("PATH",envpath,1)||
               setenv("RUSTC",rustc,1)||setenv("RUSTDOC",rustdoc,1)||setenv("PROBE_CARGO",cargobin,1)||
               setenv("PROBE_CLANG",argv[2],1)||setenv("PROBE_PYTHON",argv[3],1)||setenv("SDKROOT",argv[4],1)||
               setenv("GIT_CONFIG_NOSYSTEM","1",1)||setenv("GIT_CONFIG_GLOBAL","/dev/null",1)) die("environment");
            execl("/usr/bin/sandbox-exec","sandbox-exec","-f",prof,self,"guest",ids,cases[c],profiles[pr],root,(char *)NULL); die("sandbox exec");
        }
        int status=0,done=0; double end=now()+65;
        while(now()<end&&!stopped) { pid_t r=waitpid(p,&status,WNOHANG); if(r==p) { done=1; break; } if(r<0&&errno!=EINTR) break; usleep(50000); }
        int result=done&&WIFEXITED(status)?WEXITSTATUS(status):77;
        if(!strcmp(cases[c],"escape") && (scan(uid,PROC_UID_ONLY,1)<3 || scan(uid,PROC_RUID_ONLY,1)<3)) result=77;
        int clean=cleanup(uid);
        if(!done) {
            pid_t r=waitpid(p,&status,WNOHANG);
            if(r!=p && !(r<0&&errno==ECHILD) && !reap(p,now()+3,&status)) {
                puts("QUARANTINE reason=unreaped-coordinator-child"); return 77;
            }
        }
        if(!clean) return 77;
        if(!strcmp(cases[c],"escape") && !quiet_heartbeats(work)) result=77;
        printf("RESULT profile=%s case=%s status=%d uid_empty=1\n",profiles[pr],cases[c],result);
        if(result) failed=1;
    }
    puts(failed?"UID-PROBE-FAIL":"UID-PROBE-PASS compatibility-and-observed-UID-teardown-only");
    return failed?1:0;
}
C
SDKROOT="$probe_sdk" "$probe_rustc" --edition=2021 "$probe_root/fixtures/command.rs" -o "$probe_root/fixtures/rust-command"
/usr/bin/xcrun clang -O0 -Wall -Wextra -Werror -Wno-deprecated-declarations "$probe_root/uid.c" -o "$probe_root/uid"
chmod a+rx "$probe_root/uid" "$probe_root/fixtures/rust-command"
chmod a+r "$probe_root/"*.sb "$probe_root/run-case.sh" "$probe_root/fixtures/"*
/usr/bin/sw_vers
/usr/bin/uname -mrv
/usr/bin/shasum -a 256 "$probe_root/uid.c" "$probe_root/uid" "$probe_root/run-case.sh"
printf 'PRIVILEGED EFFECTS: synthetic root ownership/modes, synthetic per-case UID ownership, credential drops, limits, same-UID SIGKILL. No account/service installation.\n'
set +e
sudo -n /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin "$probe_root/uid" "$probe_root" "$probe_clang" "$probe_python" "$probe_sdk" < /dev/null
probe_status=$?
set -e
printf 'FIXTURE-RESULT status=%s retained=%s\n' "$probe_status" "$probe_root"
exit "$probe_status"
