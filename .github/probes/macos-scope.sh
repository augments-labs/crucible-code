#!/bin/sh
# Disposable dimension probe, not a production sandbox profile.
# Writes only a newly created synthetic directory; retains it for evidence.
set -eu
test "$(uname -s)" = Darwin || { echo 'Requires native macOS' >&2; exit 77; }
test -x /usr/bin/sandbox-exec || { echo 'sandbox-exec missing' >&2; exit 77; }
probe_root=$(mktemp -d /tmp/crucible-macos-scope.XXXXXX)
probe_root=$(cd "$probe_root" && pwd -P)
mkdir "$probe_root/home" "$probe_root/tmp" "$probe_root/template"
printf 'Evidence retained at %s\n' "$probe_root"
cat > "$probe_root/scope.c" <<'C'
#define _DARWIN_C_SOURCE 1
#include <errno.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
#ifndef __APPLE__
#error Native macOS is required
#endif
#ifndef POSIX_SPAWN_SETSID
#error This SDK does not expose POSIX_SPAWN_SETSID
#endif
extern char **environ;

static int reap(pid_t pid) {
    int status;
    while (waitpid(pid, &status, 0) < 0) {
        if (errno != EINTR) { perror("waitpid"); return 77; }
    }
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    if (WIFSIGNALED(status)) {
        printf("CHILD-SIGNAL pid=%ld signal=%d\n", (long)pid, WTERMSIG(status));
        return 128 + WTERMSIG(status);
    }
    return 77;
}

static int changed(const char *name, pid_t sid, pid_t pgid) {
    pid_t now_sid = getsid(0), now_pgid = getpgrp();
    if (now_sid < 0 || now_pgid < 0) return 77;
    int differs = sid != now_sid || pgid != now_pgid;
    printf("IDENTITY case=%s pid=%ld old_sid=%ld sid=%ld old_pgid=%ld pgid=%ld changed=%d\n",
           name, (long)getpid(), (long)sid, (long)now_sid,
           (long)pgid, (long)now_pgid, differs);
    return differs ? 42 : 0;
}

static int attempt(const char *self, const char *name) {
    alarm(5);
    pid_t sid = getsid(0), pgid = getpgrp();
    if (sid < 0 || pgid < 0 || getpid() == pgid) return 77;
    printf("CASE-BEGIN case=%s pid=%ld\n", name, (long)getpid());
    if (!strncmp(name, "spawn", 5)) {
        posix_spawnattr_t attr;
        int error = posix_spawnattr_init(&attr);
        if (error) return 77;
        short flags = 0;
        if (strstr(name, "sid")) flags |= POSIX_SPAWN_SETSID;
        if (strstr(name, "pgid")) flags |= POSIX_SPAWN_SETPGROUP;
        if (strstr(name, "setexec")) flags |= POSIX_SPAWN_SETEXEC;
        if ((error = posix_spawnattr_setflags(&attr, flags)) == 0 &&
            (flags & POSIX_SPAWN_SETPGROUP))
            error = posix_spawnattr_setpgroup(&attr, 0);
        if (error) {
            printf("ATTR-ERROR case=%s error=%d\n", name, error);
            posix_spawnattr_destroy(&attr);
            return 77;
        }
        char expected_sid[32], expected_pgid[32];
        snprintf(expected_sid, sizeof expected_sid, "%ld", (long)sid);
        snprintf(expected_pgid, sizeof expected_pgid, "%ld", (long)pgid);
        char *args[] = {(char *)self, "report", (char *)name,
                        expected_sid, expected_pgid, NULL};
        pid_t pid;
        error = posix_spawn(&pid, self, NULL, &attr, args, environ);
        posix_spawnattr_destroy(&attr);
        printf("SPAWN-RETURN case=%s error=%d\n", name, error);
        if (error) return error == EPERM || error == EACCES ? 0 : 77;
        return reap(pid);
    }
    errno = 0;
    long result;
    if (!strcmp(name, "setsid")) result = setsid();
    else if (!strcmp(name, "setpgid")) result = setpgid(0, 0);
    else if (!strcmp(name, "raw-setsid")) result = syscall(SYS_setsid);
    else result = syscall(SYS_setpgid, 0, 0);
    int error = errno;
    printf("CALL-RETURN case=%s result=%ld errno=%d\n", name, result, error);
    int movement = changed(name, sid, pgid);
    if (movement) return movement;
    return result == -1 && (error == EPERM || error == EACCES) ? 0 : 77;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IONBF, 0);
    if (argc == 5 && !strcmp(argv[1], "report")) {
        alarm(5);
        return changed(argv[2], (pid_t)strtol(argv[3], NULL, 10),
                       (pid_t)strtol(argv[4], NULL, 10));
    }
    const char *cases[] = {"setsid", "setpgid", "raw-setsid", "raw-setpgid",
        "spawn-plain", "spawn-sid", "spawn-pgid", "spawn-setexec-sid",
        "spawn-setexec-pgid"};
    for (size_t i = 0; i < sizeof cases / sizeof cases[0]; i++) {
        pid_t pid = fork();
        if (pid < 0) { perror("fork"); return 77; }
        if (!pid) _exit(attempt(argv[0], cases[i]));
        int status = reap(pid);
        printf("CASE-END case=%s status=%d\n", cases[i], status);
    }
    return 0; /* Recording success is deliberately not a security verdict. */
}
C

cat > "$probe_root/compat.sh" <<'SH'
set -eu
cd "$1"
mkdir repo
cd repo
test "$(printf shell | /usr/bin/tr a-z A-Z)" = SHELL
printf payload > a
/bin/cat a > b
/usr/bin/cmp a b
/usr/bin/git init -q --template="$2/template"
/usr/bin/git -c core.hooksPath=/dev/null add a
test "$(/usr/bin/git diff --cached --name-only)" = a
test "$(/usr/bin/git -c alias.scopeprobe='!printf child-shell' scopeprobe)" = child-shell
/usr/bin/git status --porcelain
printf 'COMPAT-OK\n'
SH

cat > "$probe_root/control.sb" <<'SB'
(version 1)
(allow default)
(deny network*)
SB
cat > "$probe_root/direct.sb" <<'SB'
(version 1)
(allow default)
(deny network*)
(deny syscall-unix (syscall-number SYS_setsid SYS_setpgid))
SB
cat > "$probe_root/all-spawn.sb" <<'SB'
(version 1)
(allow default)
(deny network*)
(deny syscall-unix (syscall-number SYS_setsid SYS_setpgid SYS_posix_spawn))
SB

/usr/bin/xcrun clang -O0 -Wall -Wextra -Wno-deprecated-declarations \
    "$probe_root/scope.c" -o "$probe_root/scope"
{
    /usr/bin/sw_vers
    /usr/bin/uname -mrv
    /usr/bin/xcrun clang --version
    /usr/bin/git --version
    /usr/bin/shasum -a 256 /usr/bin/sandbox-exec "$probe_root/scope" \
        "$probe_root/scope.c" "$probe_root/compat.sh" "$probe_root/"*.sb
} > "$probe_root/environment.txt" 2>&1

/bin/cat "$probe_root/environment.txt"

for profile in control direct all-spawn; do
    mkdir "$probe_root/$profile-work"
    for workload in scope compat; do
        set +e
        if test "$workload" = scope; then
            /usr/bin/env -i HOME="$probe_root/home" TMPDIR="$probe_root/tmp/" \
                PATH=/usr/bin:/bin:/usr/sbin:/sbin \
                /usr/bin/sandbox-exec -f "$probe_root/$profile.sb" "$probe_root/scope" \
                > "$probe_root/$profile-$workload.log" 2>&1
        else
            /usr/bin/env -i HOME="$probe_root/home" TMPDIR="$probe_root/tmp/" \
                PATH=/usr/bin:/bin:/usr/sbin:/sbin \
                GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
                /usr/bin/sandbox-exec -f "$probe_root/$profile.sb" \
                /bin/sh "$probe_root/compat.sh" "$probe_root/$profile-work" "$probe_root" \
                > "$probe_root/$profile-$workload.log" 2>&1
        fi
        result=$?
        set -e
        printf '%s %s launch_status=%s\n' "$profile" "$workload" "$result" \
            >> "$probe_root/$profile-$workload.log"
        /bin/cat "$probe_root/$profile-$workload.log"
    done
done
printf 'Retained evidence: %s\n' "$probe_root"
