#!/bin/sh
# Disposable unprivileged policy-query experiment, not a sandbox backend.
set -eu
umask 022
test "$(uname -s)" = Darwin
test "$(id -u)" != 0
probe_root=$(mktemp -d /tmp/crucible-special-port.XXXXXX)
probe_root=$(cd "$probe_root" && pwd -P)
cat > "$probe_root/query.c" <<'C'
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <mach/mach.h>
#include <mach/task_special_ports.h>
#include <sandbox.h>
static void query(const char *stage) {
    const int slots[]={TASK_HOST_PORT,TASK_BOOTSTRAP_PORT,TASK_ACCESS_PORT,TASK_DEBUG_CONTROL_PORT};
    for(unsigned i=0;i<sizeof slots/sizeof slots[0];i++) {
        mach_port_t port=MACH_PORT_NULL; mach_port_type_t type=0;
        kern_return_t result=task_get_special_port(mach_task_self(),slots[i],&port);
        kern_return_t type_result=KERN_INVALID_NAME;
        if(result==KERN_SUCCESS&&MACH_PORT_VALID(port)) type_result=mach_port_type(mach_task_self(),port,&type);
        printf("SPECIAL-QUERY stage=%s slot=%d result=%d present=%d type_result=%d send=%d receive=%d\n",
               stage,slots[i],result,MACH_PORT_VALID(port),type_result,
               (type&MACH_PORT_TYPE_SEND)!=0,(type&MACH_PORT_TYPE_RECEIVE)!=0);
        if(MACH_PORT_VALID(port)&&mach_port_deallocate(mach_task_self(),port)!=KERN_SUCCESS) exit(77);
    }
}
int main(int argc,char **argv) {
    setvbuf(stdout,NULL,_IONBF,0); alarm(8);
    if(argc==2&&!strcmp(argv[1],"after")) {
        query("after_exec"); puts("QUERY-EXPERIMENT-COMPLETE full_sandbox_tested=0"); return 0;
    }
    if(argc!=3) return 77;
    char profile[8192]; FILE *file=fopen(argv[1],"rb"); if(!file) return 77;
    size_t count=fread(profile,1,sizeof(profile)-1,file);
    int error=ferror(file),extra=fgetc(file),closed=fclose(file);
    if(error||extra!=EOF||closed||count==0) return 77;
    profile[count]=0; query("before_policy");
    char *diagnostic=NULL;
    if(sandbox_init(profile,0,&diagnostic)) {
        fprintf(stderr,"PROFILE-UNAVAILABLE %.256s\n",diagnostic?diagnostic:"unknown");
        if(diagnostic) sandbox_free_error(diagnostic); return 77;
    }
    puts("PROFILE-APPLIED full_sandbox_tested=0"); query("after_policy");
    char *next[]={argv[2],"after",NULL}; execv(argv[2],next); perror("fresh exec"); return 77;
}
C
python3 - "$probe_root" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1]).resolve(strict=True)
quote = lambda value: json.dumps(str(value))
base = ['(version 1)', '(deny default)', '(allow process-exec)',
        '(allow file-read-data (literal "/"))',
        '(allow file-read-metadata (literal "/") (literal "/usr") (literal "/System") '
        '(literal "/private") (literal "/private/tmp"))',
        '(allow file-read* (subpath ' + quote(root) + ') (subpath "/usr/lib") (subpath "/System/Library"))']
for variant in ('baseline', 'deny-get'):
    lines = base + (['(deny mach-task-special-port-get)'] if variant == 'deny-get' else [])
    profile = '\n'.join(lines) + '\n'
    if len(profile.encode()) >= 8192:
        raise RuntimeError('profile bound')
    (root / (variant + '.sb')).write_text(profile)
PY
/usr/bin/xcrun clang -Wall -Wextra -Werror -Wno-deprecated-declarations "$probe_root/query.c" -o "$probe_root/query"
/usr/bin/sw_vers
/usr/bin/uname -mrv
/usr/bin/shasum -a 256 "$probe_root/query.c" "$probe_root/query" "$probe_root/baseline.sb" "$probe_root/deny-get.sb"
for variant in baseline deny-get; do
    printf 'QUERY-VARIANT-BEGIN %s\n' "$variant"
    if /usr/bin/env -i PATH=/usr/bin:/bin "$probe_root/query" "$probe_root/$variant.sb" "$probe_root/query" < /dev/null; then
        result=0
    else
        result=$?
    fi
    printf 'QUERY-VARIANT-END %s status=%s\n' "$variant" "$result"
    test "$result" -eq 0 || test "$result" -eq 77
 done
