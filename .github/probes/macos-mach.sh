#!/bin/sh
# macos-runtime-v3: exact SSL and page-size diagnostics, not a backend.
# No native execution by the author. Root reviews source/manifest before CI.
set -eu
umask 022
test "$(uname -s)" = Darwin || { echo 'Requires native macOS' >&2; exit 77; }
test "$(id -u)" != 0 || { echo 'Start as the fresh VM runner' >&2; exit 77; }
probe_rustc=$(rustup which --toolchain stable rustc)
probe_toolchain=$(dirname "$(dirname "$probe_rustc")")
python3 - "$probe_toolchain" <<'PY'
import os, pathlib, sys
total = 0
for name in ('bin', 'lib'):
    for root, directories, files in os.walk(pathlib.Path(sys.argv[1]) / name):
        for filename in files:
            total += (pathlib.Path(root) / filename).stat().st_size
            assert total <= 4 * 1024**3, 'runtime copy exceeds experiment bound'
print('RUNTIME-COPY-BOUND bytes=' + str(total))
PY
probe_developer=$(/usr/bin/xcode-select --print-path)
case "$probe_developer" in /*) ;; *) exit 77 ;; esac
probe_developer=$(cd "$probe_developer" && pwd -P)
probe_git=$(/usr/bin/env -i PATH=/usr/bin:/bin DEVELOPER_DIR="$probe_developer" /usr/bin/xcrun --find git)
probe_clang=$(/usr/bin/env -i PATH=/usr/bin:/bin DEVELOPER_DIR="$probe_developer" /usr/bin/xcrun --find clang)
probe_sdk=$(/usr/bin/env -i PATH=/usr/bin:/bin DEVELOPER_DIR="$probe_developer" /usr/bin/xcrun --sdk macosx --show-sdk-path)
for probe_path in "$probe_git" "$probe_clang" "$probe_sdk"; do case "$probe_path" in /*) ;; *) exit 77 ;; esac; done
test -x "$probe_git" && test -x "$probe_clang" && test -d "$probe_sdk"
test "$probe_git" != /usr/bin/git || { echo 'Git resolution still names shim'; exit 77; }
probe_root=$(mktemp -d /tmp/crucible-macos-mach.XXXXXX)
probe_root=$(cd "$probe_root" && pwd -P)
printf 'FIXTURE %s\n' "$probe_root"
mkdir "$probe_root/runtime" "$probe_root/runtime/rust"
cp -R "$probe_toolchain/bin" "$probe_toolchain/lib" "$probe_root/runtime/rust/"
chmod -R a+rX "$probe_root/runtime"
python3 - "$probe_root" "$probe_developer" <<'PY'
import json, os, pathlib, sys
root, developer = (pathlib.Path(value).resolve(strict=True) for value in sys.argv[1:])
trees = [root, developer, pathlib.Path('/System/Library'), pathlib.Path('/usr/lib'), pathlib.Path('/bin')]
files = [pathlib.Path(value) for value in ('/usr/bin/tr', '/usr/bin/true', '/dev/null')]
variant = os.environ['CRUCIBLE_RUNTIME_PROBE_VARIANT']
assert variant in ('baseline', 'ssl-config', 'ssl-pagesize')
if variant != 'baseline':
    files.append(pathlib.Path('/private/etc/ssl/openssl.cnf'))
quote = lambda value: json.dumps(str(value))
ancestors = sorted({parent for path in trees + files for parent in path.parents})
sysctls = ['hw.ncpu', 'hw.memsize', 'hw.pagesize', 'hw.cputype', 'hw.cpusubtype',
           'hw.cpufamily', 'kern.osrelease', 'kern.osversion', 'kern.argmax']
if variant == 'ssl-pagesize':
    sysctls.append('hw.pagesize_compat')
lines = ['(version 1)', '(deny default)', '(deny network*)', '(deny mach-lookup mach-register)',
         '(allow process-exec process-fork)', '(allow signal (target same-sandbox))',
         '(allow process-info* (target same-sandbox))', '(allow file-read-data (literal "/"))',
         '(allow file-read-metadata ' + ' '.join('(literal ' + quote(path) + ')' for path in ancestors) + ')',
         '(allow file-read* ' + ' '.join('(subpath ' + quote(path) + ')' for path in trees) +
         ' ' + ' '.join('(literal ' + quote(path) + ')' for path in files) + ')',
         '(allow file-write* (subpath ' + quote(root) + ') (literal "/dev/null"))',
         '(allow sysctl-read ' + ' '.join('(sysctl-name ' + quote(name) + ')' for name in sysctls) + ')']
profile = '\n'.join(lines) + '\n'
assert len(profile.encode()) < 16384 and '(allow default)' not in profile
(root / 'profile.sb').write_text(profile)
print('RUNTIME-PROFILE ' + json.dumps({'variant': variant, 'trees': [str(p) for p in trees], 'files': [str(p) for p in files], 'sysctls': sysctls}))
PY
cat > "$probe_root/hello.c" <<'C'
int main(void) { return 0; }
C
cat > "$probe_root/smoke.sh" <<'SH'
set -eu
case "$1" in
 shell) test "$(printf native | /usr/bin/tr a-z A-Z)" = NATIVE ;;
 git)
   mkdir repo empty-template; cd repo
   "$PROBE_GIT" init -q --template=../empty-template
   printf payload > a
   "$PROBE_GIT" -c core.hooksPath=/dev/null add a
   test "$("$PROBE_GIT" diff --cached --name-only)" = a
   test "$("$PROBE_GIT" -c alias.probe='!printf child-shell' probe)" = child-shell ;;
 clang) "$PROBE_CLANG" "$2/hello.c" -o hello; ./hello ;;
 cargo)
   mkdir src
   printf '[package]\nname="mach-probe"\nversion="0.0.0"\nedition="2021"\n' > Cargo.toml
   printf 'fn main(){assert!(std::process::Command::new("/usr/bin/true").status().unwrap().success());}\n' > build.rs
   printf '#[test] fn child(){assert!(std::process::Command::new("/usr/bin/true").status().unwrap().success());}\n' > src/lib.rs
   "$PROBE_CARGO" test --offline --jobs 1 --target-dir target -- --nocapture ;;
 *) exit 77 ;;
esac
printf 'SMOKE-OK %s\n' "$1"
SH
cat > "$probe_root/mach.c" <<'C'
#define _DARWIN_C_SOURCE 1
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <libproc.h>
#include <limits.h>
#include <mach/mach.h>
#include <mach/exception_types.h>
#include <mach/task_special_ports.h>
#include <mach/thread_status.h>
#include <pwd.h>
#include <sandbox.h>
#include <servers/bootstrap.h>
#include <signal.h>
#include <spawn.h>
#include <stdint.h>
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

/* Public Mach APIs; kernel source basis and limitations are in the packet.
 * Only task registered, task exception and task bootstrap anchors are tested.
 * No exception is raised and no host bootstrap-name request is sent. */
static uid_t guarded_uid;
static pid_t guarded_leader;
static int guarded;
/* Fatal setup/API failures in the parent still get the same bounded cleanup.
 * Never kill a borrowed numeric PID: cleanup may already have reaped it. */
static void emergency_cleanup(void) {
    if(!guarded || geteuid()!=0) return;
    int clean=cleanup(guarded_uid), status=0; errno=0;
    pid_t result=waitpid(guarded_leader,&status,WNOHANG);
    int reaped=result==guarded_leader || (result<0&&errno==ECHILD) || reap(guarded_leader,now()+3,&status);
    printf("EMERGENCY-CLEANUP uid_empty=%d leader_reaped=%d\n",clean,reaped);
    if(!clean||!reaped) puts("QUARANTINE emergency-cleanup-unconfirmed");
}
static void kr(kern_return_t code,const char *label) {
    if(code!=KERN_SUCCESS) { fprintf(stderr,"MACH-API-ERROR %s code=%d\n",label,code); exit(77); }
}
static void release(mach_port_t port) {
    if(MACH_PORT_VALID(port)) kr(mach_port_deallocate(mach_task_self(),port),"send reference release");
}
static void free_ports(mach_port_array_t ports,mach_msg_type_number_t count) {
    if(count) kr(vm_deallocate(mach_task_self(),(vm_address_t)ports,count*sizeof(*ports)),"port array release");
}
static mach_port_t slot(int which) {
    mach_port_t found=MACH_PORT_NULL;
    if(which==0) {
        mach_port_array_t ports=NULL; mach_msg_type_number_t count=0;
        kr(mach_ports_lookup(mach_task_self(),&ports,&count),"registered lookup");
        if(count>16) { fputs("INCONCLUSIVE registered array bound\n",stderr); exit(77); }
        for(mach_msg_type_number_t i=0;i<count;i++) if(MACH_PORT_VALID(ports[i])) {
            if(MACH_PORT_VALID(found)) { release(ports[i]); release(found); free_ports(ports,count); exit(77); }
            found=ports[i];
        }
        free_ports(ports,count);
    } else if(which==1) {
        exception_mask_t masks[32]; mach_port_t ports[32];
        exception_behavior_t behavior[32]; thread_state_flavor_t flavor[32]; mach_msg_type_number_t count=32;
        kr(task_get_exception_ports(mach_task_self(),EXC_MASK_ALL,masks,&count,ports,behavior,flavor),"exception lookup");
        if(count>32) exit(77);
        for(mach_msg_type_number_t i=0;i<count;i++) if(MACH_PORT_VALID(ports[i])) {
            if(MACH_PORT_VALID(found) || !(masks[i]&EXC_MASK_BREAKPOINT)) { release(ports[i]); release(found); exit(77); }
            found=ports[i];
        }
    } else kr(task_get_special_port(mach_task_self(),TASK_BOOTSTRAP_PORT,&found),"bootstrap lookup");
    return found;
}
/* Retain one owned reference while locating any replacement of the
 * inherited slot by trusted runtime initialization. Never seed an unknown port. */
static void check_seed(mach_port_t expected,const char *boundary) {
    mach_port_t actual=slot(0); mach_port_type_t expected_type=0,actual_type=0;
    kr(mach_port_type(mach_task_self(),expected,&expected_type),"expected seed type");
    kr(mach_port_type(mach_task_self(),actual,&actual_type),"actual seed type");
    printf("SEED-IDENTITY boundary=%s expected=%u actual=%u same=%d expected_type=%u actual_type=%u\n",
           boundary,expected,actual,expected==actual,expected_type,actual_type);
    int same=expected==actual; release(actual); if(!same) exit(77);
}
static void port_kind(mach_port_t port,const char *boundary) {
    unsigned kind=0;
    kern_return_t result=mach_port_kernel_object(mach_task_self(),port,&kind,NULL);
    printf("PORT-KIND boundary=%s result=%d type=%u\n",boundary,result,kind);
}
static void exception_boundary(mach_port_t service,const char *boundary) {
    kern_return_t result=task_set_exception_ports(mach_task_self(),EXC_MASK_BREAKPOINT,service,EXCEPTION_DEFAULT,THREAD_STATE_NONE);
    printf("EXCEPTION-BOUNDARY boundary=%s result=%d\n",boundary,result);
    if(result==KERN_SUCCESS) kr(task_set_exception_ports(mach_task_self(),EXC_MASK_BREAKPOINT,MACH_PORT_NULL,EXCEPTION_DEFAULT,THREAD_STATE_NONE),"diagnostic exception clear");
}
static void parent_registration(mach_port_t service) {
    mach_port_array_t ports=NULL; mach_msg_type_number_t count=0;
    kr(mach_ports_lookup(mach_task_self(),&ports,&count),"parent registration oracle");
    if(count>16) exit(77);
    int correct=count>0 && ports[0]==service;
    printf("PARENT-REGISTRATION owned=%u count=%u first=%u match=%d\n",service,count,count?ports[0]:0,correct);
    for(mach_msg_type_number_t i=0;i<count;i++) {
        if(i && MACH_PORT_VALID(ports[i])) correct=0;
        release(ports[i]);
    }
    free_ports(ports,count); if(!correct) exit(77);
}
static void no_user_reference(mach_port_t port) {
    mach_port_type_t type=0; kern_return_t result=mach_port_type(mach_task_self(),port,&type);
    if(result!=KERN_INVALID_NAME) {
        fprintf(stderr,"SEED-REFERENCE-REMAINS name=%u result=%d type=%u\n",port,result,type); exit(1);
    }
}
static void seed_slots(void) {
    mach_port_t service=slot(0); if(!MACH_PORT_VALID(service)) exit(77);
    kr(task_set_exception_ports(mach_task_self(),EXC_MASK_ALL,MACH_PORT_NULL,EXCEPTION_DEFAULT,THREAD_STATE_NONE),"clear original exceptions");
    kr(task_set_exception_ports(mach_task_self(),EXC_MASK_BREAKPOINT,service,EXCEPTION_DEFAULT,THREAD_STATE_NONE),"seed exception");
    kr(task_set_special_port(mach_task_self(),TASK_BOOTSTRAP_PORT,service),"seed bootstrap");
    /* This libc copy names the original parent bootstrap capability. Retire
     * it; the seeded service is held only by kernel slots, never a spare name. */
    mach_port_t original=bootstrap_port; bootstrap_port=MACH_PORT_NULL; release(original);
    release(service); no_user_reference(service);
    puts("SEEDED registered=1 task_exception=1 bootstrap=1 extra_seed_names=0");
}
static void stale_send(mach_port_t port) {
    mach_msg_header_t message={0};
    message.msgh_bits=MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND,0);
    message.msgh_size=sizeof message; message.msgh_remote_port=port; message.msgh_id=0x43524d31;
    mach_msg_return_t result=mach_msg(&message,MACH_SEND_MSG|MACH_SEND_TIMEOUT,sizeof message,0,MACH_PORT_NULL,0,MACH_PORT_NULL);
    if(result!=MACH_SEND_INVALID_DEST) { fprintf(stderr,"STALE-SEND-NOT-DENIED result=%d\n",result); exit(1); }
}
static void sanitize(void) {
    mach_port_t old[3]; for(int s=0;s<3;s++) { old[s]=slot(s); if(!MACH_PORT_VALID(old[s])) exit(77); }
    kr(mach_ports_register(mach_task_self(),NULL,0),"clear registered");
    kr(task_set_exception_ports(mach_task_self(),EXC_MASK_ALL,MACH_PORT_NULL,EXCEPTION_DEFAULT,THREAD_STATE_NONE),"clear exceptions");
    kr(task_set_special_port(mach_task_self(),TASK_BOOTSTRAP_PORT,MACH_PORT_NULL),"clear bootstrap");
    mach_port_t libc_copy=bootstrap_port; bootstrap_port=MACH_PORT_NULL; release(libc_copy);
    for(int s=0;s<3;s++) release(old[s]);
    /* Test stale known names before any lookup could allocate/reuse a name. */
    for(int s=0;s<3;s++) { no_user_reference(old[s]); stale_send(old[s]); }
    for(int s=0;s<3;s++) { mach_port_t p=slot(s); if(MACH_PORT_VALID(p)) { release(p); exit(1); } }
    puts("PREEXEC-SANITIZED anchors=0 stale_seed_names=0 stale_sends_denied=3");
}
#define PING_ID 0x43524d31
#define MAGIC 0x6d616368U
typedef struct { mach_msg_header_t h; uint32_t magic,route,slot; uint64_t nonce; } Message;
typedef union { Message m; unsigned char bytes[512]; } Buffer;
static uint64_t nonce_for(unsigned route,unsigned slot_number) { return UINT64_C(0x13579bdf24680000)+(route<<8)+slot_number; }
static int ping(mach_port_t service,unsigned route,unsigned slot_number) {
    mach_port_t reply=MACH_PORT_NULL;
    kr(mach_port_allocate(mach_task_self(),MACH_PORT_RIGHT_RECEIVE,&reply),"reply receive");
    Buffer b={0};
    b.m.h.msgh_bits=MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND,MACH_MSG_TYPE_MAKE_SEND_ONCE);
    b.m.h.msgh_size=sizeof(Message); b.m.h.msgh_remote_port=service; b.m.h.msgh_local_port=reply; b.m.h.msgh_id=PING_ID;
    b.m.magic=MAGIC; b.m.route=route; b.m.slot=slot_number; b.m.nonce=nonce_for(route,slot_number);
    mach_msg_return_t result=mach_msg(&b.m.h,MACH_SEND_MSG|MACH_SEND_TIMEOUT|MACH_RCV_MSG|MACH_RCV_TIMEOUT,
                                    sizeof(Message),sizeof b,reply,300,MACH_PORT_NULL);
    printf("PING-REPLY-HEADER result=%d size=%u id=%d bits=%u remote=%u local=%u\n",result,b.m.h.msgh_size,b.m.h.msgh_id,b.m.h.msgh_bits,b.m.h.msgh_remote_port,b.m.h.msgh_local_port);
    int ok=result==MACH_MSG_SUCCESS && b.m.h.msgh_size==sizeof(Message) && b.m.h.msgh_id==PING_ID+1 &&
        !(b.m.h.msgh_bits&MACH_MSGH_BITS_COMPLEX) && b.m.magic==(MAGIC^UINT32_C(0xa5a5a5a5)) && b.m.route==route && b.m.slot==slot_number && b.m.nonce==nonce_for(route,slot_number);
    kr(mach_port_destroy(mach_task_self(),reply),"reply destroy");
    printf("PING route=%u slot=%u acknowledged=%d result=%d\n",route,slot_number,ok,result);
    return ok;
}
/* Calibrate the exact message layout and owned queue without fork, slots,
 * credentials, a reply right, or sandbox policy. */
static void calibrate(mach_port_t service,const char *label) {
    Buffer sent={0},received={0};
    sent.m.h.msgh_bits=MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND,0);
    sent.m.h.msgh_size=sizeof(Message); sent.m.h.msgh_remote_port=service; sent.m.h.msgh_id=PING_ID;
    sent.m.magic=MAGIC; sent.m.route=17; sent.m.slot=2; sent.m.nonce=nonce_for(17,2);
    mach_msg_return_t tx=mach_msg(&sent.m.h,MACH_SEND_MSG|MACH_SEND_TIMEOUT,sizeof(Message),0,MACH_PORT_NULL,100,MACH_PORT_NULL);
    mach_msg_return_t rx=mach_msg(&received.m.h,MACH_RCV_MSG|MACH_RCV_TIMEOUT,0,sizeof received,service,100,MACH_PORT_NULL);
    int ok=tx==MACH_MSG_SUCCESS && rx==MACH_MSG_SUCCESS && received.m.h.msgh_size==sizeof(Message) &&
        received.m.h.msgh_id==PING_ID && !(received.m.h.msgh_bits&MACH_MSGH_BITS_COMPLEX) &&
        received.m.magic==MAGIC && received.m.route==17 && received.m.slot==2 && received.m.nonce==nonce_for(17,2);
    printf("QUEUE-CALIBRATION kind=%s tx=%d rx=%d size=%u id=%d exact_payload=%d\n",label,tx,rx,received.m.h.msgh_size,received.m.h.msgh_id,ok);
    if(!ok) exit(77);
}
static int receive_ping(mach_port_t service,unsigned route,unsigned *seen,int permitted) {
    Buffer b={0}; mach_msg_return_t result=mach_msg(&b.m.h,MACH_RCV_MSG|MACH_RCV_TIMEOUT,0,sizeof b,service,10,MACH_PORT_NULL);
    if(result==MACH_RCV_TIMED_OUT) return 1;
    if(result!=MACH_MSG_SUCCESS) { fprintf(stderr,"SERVER-ERROR result=%d\n",result); return 0; }
    printf("SERVER-RX size=%u id=%d bits=%u remote=%u local=%u magic=%u route=%u slot=%u nonce=%llu\n",b.m.h.msgh_size,b.m.h.msgh_id,b.m.h.msgh_bits,b.m.h.msgh_remote_port,b.m.h.msgh_local_port,b.m.magic,b.m.route,b.m.slot,(unsigned long long)b.m.nonce);
    if((!permitted && b.m.slot!=3) || b.m.h.msgh_size!=sizeof(Message) || (b.m.h.msgh_bits&MACH_MSGH_BITS_COMPLEX) ||
       b.m.h.msgh_id!=PING_ID || !MACH_PORT_VALID(b.m.h.msgh_remote_port) ||
       MACH_MSGH_BITS_REMOTE(b.m.h.msgh_bits)!=MACH_MSG_TYPE_PORT_SEND_ONCE ||
       b.m.magic!=MAGIC || b.m.route!=route || b.m.slot>=4 ||
       b.m.nonce!=nonce_for(route,b.m.slot) || (*seen&(1U<<b.m.slot))) {
        mach_msg_destroy(&b.m.h); fputs("UNEXPECTED-OWNED-SERVICE-MESSAGE\n",stderr); return 0;
    }
    *seen|=1U<<b.m.slot;
    b.m.h.msgh_bits=MACH_MSGH_BITS(MACH_MSG_TYPE_MOVE_SEND_ONCE,0);
    b.m.h.msgh_local_port=MACH_PORT_NULL; b.m.h.msgh_id=PING_ID+1; b.m.h.msgh_size=sizeof(Message);
    b.m.magic=MAGIC^UINT32_C(0xa5a5a5a5);
    result=mach_msg(&b.m.h,MACH_SEND_MSG|MACH_SEND_TIMEOUT,sizeof(Message),0,MACH_PORT_NULL,100,MACH_PORT_NULL);
    printf("SERVER-TX result=%d\n",result);
    if(result!=MACH_MSG_SUCCESS) { mach_msg_destroy(&b.m.h); return 0; }
    return 1;
}
static void guest_credentials(uid_t uid) {
    /* Directory-account getgroups may itself use bootstrap IPC. After seeding,
     * inspect only the kernel credential oracle already established by v5. */
    struct proc_bsdshortinfo p; gid_t groups[64];
    int n=(int)syscall(SYS_getgroups,64,groups);
    if(proc_pidinfo(getpid(),PROC_PIDT_SHORTBSDINFO,0,&p,sizeof p)!=(int)sizeof p) die("guest credentials");
    if(!uid||getuid()!=uid||geteuid()!=uid||p.pbsi_svuid!=uid||getgid()!=uid||getegid()!=uid||p.pbsi_svgid!=uid||n!=1||groups[0]!=uid) exit(77);
    printf("GUEST-CREDENTIALS uid=%u gid=%u kernel_groups=%d\n",uid,uid,n);
}
static int guest(uid_t uid,int sanitized,int route,const char *name,const char *root) {
    guest_credentials(uid); alarm(18);
    errno=0; int page_size=getpagesize(), page_errno=errno;
    printf("RUNTIME-PAGESIZE value=%d errno=%d\n",page_size,page_errno);
    for(int s=0;s<3;s++) {
        mach_port_t port=slot(s);
        printf("CAP-OBS mode=%s route=%s slot=%d present=%d\n",sanitized?"sanitized":"control",name,s,MACH_PORT_VALID(port));
        if(sanitized && MACH_PORT_VALID(port)) { release(port); return 1; }
        if(!sanitized && (!MACH_PORT_VALID(port)||!ping(port,(unsigned)route,(unsigned)s))) { release(port); return 77; }
        release(port);
    }
    if(route<3) { puts("CAP-CASE-OK"); return 0; }
    char script[PATH_MAX]; path(script,root,"smoke.sh");
    execl("/bin/sh","sh",script,name,root,(char *)NULL); die("smoke exec");
}
static void owned_dir(const char *name,uid_t uid) { if(mkdir(name,0700)||chown(name,uid,uid)) die("synthetic directory"); }
static int drain(int fd,size_t *count) {
    char bytes[1024];
    for(int i=0;i<65;i++) {
        ssize_t n=read(fd,bytes,sizeof bytes);
        if(n==0 || (n<0&&(errno==EAGAIN||errno==EWOULDBLOCK))) return 1;
        if(n<0) { if(errno==EINTR) continue; return 0; }
        if((size_t)n>65536-*count) { fputs("OUTPUT-BOUND\n",stderr); return 0; }
        *count+=(size_t)n; if(fwrite(bytes,1,(size_t)n,stdout)!=(size_t)n) return 0;
    }
    return 1;
}
int main(int argc,char **argv) {
    setvbuf(stdout,NULL,_IONBF,0);
    if(argc==3&&!strcmp(argv[1],"fd-selftest")) {
        int low=open(argv[2],O_RDONLY); if(low<3) die("fixture fd");
        int high=fcntl(low,F_DUPFD,128); if(high<128) die("high fixture fd"); close_extra_fds();
        errno=0; if(fcntl(low,F_GETFD)!=-1||errno!=EBADF) return 77;
        errno=0; if(fcntl(high,F_GETFD)!=-1||errno!=EBADF) return 77;
        puts("DARWIN-FD-CLOSE-SELFTEST-PASS"); return 0;
    }
    if(argc==7&&!strcmp(argv[1],"guest")) {
        uid_t uid=(uid_t)atoi(argv[2]); int sanitized=atoi(argv[3]),route=atoi(argv[4]);
        /* This program image was freshly exec'd under the final policy.
         * Fork now exercises descendants of the confined guest, never code
         * inside the privileged launcher's inherited runtime. */
        if(route==1) {
            pid_t child=fork(); if(child<0) die("confined guest fork");
            if(!child) _exit(guest(uid,sanitized,route,argv[5],argv[6]));
            int status=0; if(!reap(child,now()+18,&status)) return 77;
            return WIFEXITED(status)?WEXITSTATUS(status):77;
        }
        return guest(uid,sanitized,route,argv[5],argv[6]);
    }
    if(argc!=6||geteuid()!=0) { fputs("root coordinator requires fixture/clang/sdk/developer/git\n",stderr); return 77; }
    close_extra_fds(); signal(SIGINT,stop); signal(SIGTERM,stop);
    const char *root=argv[1]; struct stat st; const char *prefix="/private/tmp/crucible-macos-mach.";
    if(lstat(root,&st)||!S_ISDIR(st.st_mode)||strncmp(root,prefix,strlen(prefix))) die("fixture root");
    if(chown(root,0,0)||chmod(root,0755)||chdir(root)) die("fixture ownership");
    char profile[16384]; int profile_fd=open("profile.sb",O_RDONLY|O_NOFOLLOW);
    if(profile_fd<0 || fstat(profile_fd,&st) || !S_ISREG(st.st_mode) || st.st_size<=0 || st.st_size>=(off_t)sizeof profile) die("profile file");
    size_t profile_size=0;
    while(profile_size<(size_t)st.st_size) {
        ssize_t n=read(profile_fd,profile+profile_size,(size_t)st.st_size-profile_size);
        if(n<0&&errno==EINTR) continue;
        if(n<=0) die("profile read");
        profile_size+=(size_t)n;
    }
    profile[profile_size]=0; if(close(profile_fd)) die("profile close");
    uid_t uid=0;
    for(uid_t n=60000;n<60128;n++) {
        errno=0; struct passwd *pw=getpwuid(n); int pe=errno;
        errno=0; struct group *gr=getgrgid(n); int ge=errno;
        if(!pw&&!gr&&!pe&&!ge) { int a=absent(n); if(a<0) die("UID census"); if(a==1) { uid=n; break; } }
    }
    if(!uid) die("unused fixture UID");
    printf("UID-LEASE uid=%u assumption=exclusive-disposable-VM-only\n",uid);
    guarded_uid=uid; if(atexit(emergency_cleanup)) die("cleanup registration");
    mach_port_t service=MACH_PORT_NULL;
    /* Current XNU rejects ordinary immovable receive ports as exception
     * anchors. Use its designated exception port type for this fixture. */
    mach_port_options_t options={0}; options.flags=MPO_EXCEPTION_PORT;
    kr(mach_port_construct(mach_task_self(),&options,0,&service),"owned exception service receive");
    kr(mach_port_insert_right(mach_task_self(),service,service,MACH_MSG_TYPE_MAKE_SEND),"owned service send");
    mach_port_t ordinary=MACH_PORT_NULL;
    kr(mach_port_allocate(mach_task_self(),MACH_PORT_RIGHT_RECEIVE,&ordinary),"ordinary calibration receive");
    kr(mach_port_insert_right(mach_task_self(),ordinary,ordinary,MACH_MSG_TYPE_MAKE_SEND),"ordinary calibration send");
    calibrate(ordinary,"ordinary");
    kr(mach_port_destroy(mach_task_self(),ordinary),"ordinary calibration destroy");
    calibrate(service,"exception");
    port_kind(service,"parent_owned_queue");
    port_kind(mach_task_self(),"parent_task_control");
    kr(task_set_exception_ports(mach_task_self(),EXC_MASK_ALL,MACH_PORT_NULL,EXCEPTION_DEFAULT,THREAD_STATE_NONE),"clear coordinator exceptions");
    kr(task_set_exception_ports(mach_task_self(),EXC_MASK_BREAKPOINT,service,EXCEPTION_DEFAULT,THREAD_STATE_NONE),"install coordinator transport");
    const char *names[]={"exec","exec_then_fork","posix_spawn","exec","exec_then_fork","posix_spawn","shell","git","clang","cargo"};
    unsigned failed_cases=0;
    for(int c=0;c<10;c++) {
        if(stopped || empty(uid)!=1) { cleanup(uid); return 77; }
        int sanitized=c>=3, route=c<6?c%3:c;
        char work[PATH_MAX],label[80],self[PATH_MAX],ids[32],mode[8],route_text[8],home[PATH_MAX],tmp[PATH_MAX],cargo[PATH_MAX];
        snprintf(label,sizeof label,"%02d-%s",c,names[c]); path(work,root,label); owned_dir(work,uid);
        path(home,work,"home"); path(tmp,work,"tmp"); path(cargo,work,"cargo-home");
        owned_dir(home,uid); owned_dir(tmp,uid); owned_dir(cargo,uid); path(self,root,"mach");
        snprintf(ids,sizeof ids,"%u",uid); snprintf(mode,sizeof mode,"%d",sanitized); snprintf(route_text,sizeof route_text,"%d",route);
        int output[2]; if(pipe(output)||fcntl(output[0],F_SETFL,O_NONBLOCK)) die("bounded output pipe");
        pid_t p=fork();
        if(p>0) { guarded_leader=p; guarded=1; }
        if(p!=0) {
            close(output[1]);
        }
        if(p<0) { close(output[0]); cleanup(uid); die("launcher fork"); }
        if(!p) {
            /* Ordinary fork owns the runtime's registered slots. The owned
             * exception anchor is the calibrated transport; seed registered
             * slots explicitly only after the trusted child has arrived. */
            exception_mask_t transport_masks[32]; mach_port_t transport_ports[32];
            exception_behavior_t transport_behavior[32]; thread_state_flavor_t transport_flavor[32];
            mach_msg_type_number_t transport_count=32;
            kern_return_t transport_result=task_get_exception_ports(mach_task_self(),EXC_MASK_BREAKPOINT,transport_masks,&transport_count,transport_ports,transport_behavior,transport_flavor);
            printf("TRANSPORT-LOOKUP result=%d count=%u\n",transport_result,transport_count);
            if(transport_result || transport_count!=1 || !(transport_masks[0]&EXC_MASK_BREAKPOINT) || !MACH_PORT_VALID(transport_ports[0])) exit(77);
            mach_port_t expected_seed=transport_ports[0];
            kr(mach_ports_register(mach_task_self(),&expected_seed,1),"seed child registered");
            parent_registration(expected_seed);
            signal(SIGINT,SIG_DFL); signal(SIGTERM,SIG_DFL); alarm(18);
            if(dup2(output[1],1)<0||dup2(output[1],2)<0) die("guest output"); close_extra_fds();
            port_kind(expected_seed,"child_exception_transport");
            port_kind(mach_task_self(),"child_task_control");
            exception_boundary(expected_seed,"before_credentials");
            if(!ping(expected_seed,(unsigned)route,3)) exit(77);
            drop(uid); if(chdir(work)) die("guest cwd");
            exception_boundary(expected_seed,"after_credentials");
            check_seed(expected_seed,"credentials");
            char rustc[PATH_MAX],rustdoc[PATH_MAX],cargobin[PATH_MAX],envpath[PATH_MAX],linker[PATH_MAX+16];
            path(rustc,root,"runtime/rust/bin/rustc"); path(rustdoc,root,"runtime/rust/bin/rustdoc"); path(cargobin,root,"runtime/rust/bin/cargo");
            if(snprintf(envpath,sizeof envpath,"%s/runtime/rust/bin:/usr/bin:/bin:/usr/sbin:/sbin",root)>=(int)sizeof envpath ||
               snprintf(linker,sizeof linker,"-Clinker=%s",argv[2])>=(int)sizeof linker) die("environment path");
            char *clean[]={NULL}; environ=clean;
            if(setenv("HOME",home,1)||setenv("TMPDIR",tmp,1)||setenv("CARGO_HOME",cargo,1)||setenv("PATH",envpath,1)||
               setenv("RUSTC",rustc,1)||setenv("RUSTDOC",rustdoc,1)||setenv("PROBE_CARGO",cargobin,1)||
               setenv("PROBE_CLANG",argv[2],1)||setenv("SDKROOT",argv[3],1)||setenv("DEVELOPER_DIR",argv[4],1)||setenv("PROBE_GIT",argv[5],1)||
               setenv("CARGO_ENCODED_RUSTFLAGS",linker,1)||setenv("CARGO_ENCODED_RUSTDOCFLAGS",linker,1)||
               setenv("GIT_CONFIG_NOSYSTEM","1",1)||setenv("GIT_CONFIG_GLOBAL","/dev/null",1)||
               setenv("GIT_ATTR_NOSYSTEM","1",1)||setenv("OPENSSL_CONF","/dev/null",1)) die("clean environment");
            check_seed(expected_seed,"environment");
            char *error=NULL;
            if(sandbox_init(profile,0,&error)) { fprintf(stderr,"PROFILE-REFUSED %.256s\n",error?error:"no diagnostic"); if(error) sandbox_free_error(error); exit(77); }
            puts("PROFILE-APPLIED default=deny network=deny mach-lookup=deny mach-register=deny filesystem-isolation=untested");
            check_seed(expected_seed,"profile"); exception_boundary(expected_seed,"after_profile"); release(expected_seed);
            /* Compile/apply the profile before replacing the real bootstrap
             * context. Thereafter only direct kernel Mach APIs precede exec. */
            seed_slots(); if(sanitized) sanitize();
            char *args[]={self,"guest",ids,mode,route_text,(char *)names[c],(char *)root,NULL};
            if(route==2) {
                pid_t child; int e=posix_spawn(&child,self,NULL,NULL,args,environ); if(e) { errno=e; die("guest spawn"); }
                int status=0; if(!reap(child,now()+18,&status)) exit(77);
                exit(WIFEXITED(status)?WEXITSTATUS(status):77);
            }
            execv(self,args); die("guest exec");
        }
        int status=0,done=0,observed=1; size_t bytes=0; unsigned seen=0; double end=now()+20;
        while(now()<end&&!stopped) {
            if(!drain(output[0],&bytes)||!receive_ping(service,(unsigned)route,&seen,!sanitized)) { observed=0; break; }
            pid_t r=waitpid(p,&status,WNOHANG); if(r==p) { done=1; break; } if(r<0&&errno!=EINTR) { observed=0; break; }
        }
        int clean=cleanup(uid); guarded=0;
        if(!done) { pid_t r=waitpid(p,&status,WNOHANG); if(r!=p&&!(r<0&&errno==ECHILD)&&!reap(p,now()+3,&status)) { puts("QUARANTINE unreaped-launcher"); return 77; } }
        if(!drain(output[0],&bytes)) observed=0; close(output[0]);
        printf("RESULT case=%d mode=%s route=%s reaped_before_cleanup=%d status=%d server_mask=%u output_bytes=%zu uid_empty=%d\n",
               c,sanitized?"sanitized":"control",names[c],done,status,seen,bytes,clean);
        if(!clean) return 77;
        if(!done||!observed||!WIFEXITED(status)||WEXITSTATUS(status)||seen!=(sanitized?8U:15U)) failed_cases++;
    }
    kr(mach_port_destroy(mach_task_self(),service),"owned service destroy");
    printf("MACH-PREREQUISITE-RESULT cases=10 failed=%u named_lookup_tested=0 full_sandbox_tested=0\n",failed_cases);
    if(failed_cases) return 77;
    puts("MACH-PREREQUISITE-PASS cases=10 controls=3 sanitized=3 smoke=4 slots=3 named_lookup_tested=0 full_sandbox_tested=0");
    return 0;
}
C
SDKROOT="$probe_sdk" DEVELOPER_DIR="$probe_developer" "$probe_clang" -O0 -Wall -Wextra -Werror -Wno-deprecated-declarations "$probe_root/mach.c" -o "$probe_root/mach"
/usr/bin/env -i PATH=/usr/bin:/bin "$probe_root/mach" fd-selftest "$probe_root/hello.c" < /dev/null
chmod a+rx "$probe_root/mach"
chmod a+r "$probe_root/profile.sb" "$probe_root/smoke.sh" "$probe_root/hello.c"
/usr/bin/sw_vers
/usr/bin/uname -mrv
printf 'ADAPTER git=%s clang=%s sdk=%s developer=%s\n' "$probe_git" "$probe_clang" "$probe_sdk" "$probe_developer"
printf 'BOUNDS cases=10 wall_per_case=20 cleanup=8 reap=3 output_per_case=65536 protocol_bytes=512 controls_messages=3\n'
printf 'SCOPE kernel-slot inheritance/removal and adapted smoke only; profile is default deny with finite runtime reads; complete filesystem isolation and CPU enforcement not tested\n'
# Evidence is retained inside the unique fixture; native parent CI also archives it.
/usr/bin/shasum -a 256 "$probe_root/mach.c" "$probe_root/mach" "$probe_root/profile.sb" "$probe_root/smoke.sh" "$probe_root/hello.c" "$probe_git" "$probe_clang" > "$probe_root/manifest.sha256"
find "$probe_root/runtime" -type f -exec /usr/bin/shasum -a 256 {} + > "$probe_root/runtime.sha256"
/usr/bin/shasum -a 256 "$probe_root/manifest.sha256" "$probe_root/runtime.sha256"
cat "$probe_root/manifest.sha256"
printf 'PRIVILEGED EFFECTS: owned synthetic ports, fixture ownership, permanent UID/GID drops, rlimits and same-UID SIGKILL. No accounts, services, host ACLs or network.\n'
set +e
sudo -n /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin "$probe_root/mach" "$probe_root" "$probe_clang" "$probe_sdk" "$probe_developer" "$probe_git" < /dev/null
probe_status=$?
set -e
printf 'FIXTURE-RESULT status=%s retained=%s\n' "$probe_status" "$probe_root"
python3 .github/probes/macos-runtime-logs.py
exit "$probe_status"
