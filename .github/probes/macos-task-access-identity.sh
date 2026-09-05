#!/bin/sh
# Read-only native endpoint provenance, not a sandbox acceptance test.
set -eu
test "$(uname -s)" = Darwin
test "$(id -u)" != 0
probe_root=$(mktemp -d /tmp/crucible-task-access.XXXXXX)
python3 - <<'PY'
import hashlib, json, pathlib, plistlib
path = pathlib.Path('/System/Library/LaunchDaemons/com.apple.taskgated.plist')
with path.open('rb') as f:
    data = f.read(65537)
if len(data) > 65536:
    raise RuntimeError('plist bound')
value = plistlib.loads(data)
if value.get('Label') != 'com.apple.taskgated':
    raise RuntimeError('unexpected native service label')
result = {k:value[k] for k in ('Label','Program','ProgramArguments','MachServices') if k in value}
encoded = json.dumps(result, sort_keys=True)
if len(encoded) > 4096:
    raise RuntimeError('selected metadata bound')
print('NATIVE-SERVICE-PLIST sha256=' + hashlib.sha256(data).hexdigest())
print('NATIVE-SERVICE-METADATA ' + encoded)
PY
cat > "$probe_root/identity.c" <<'C'
#include <stdio.h>
#include <unistd.h>
#include <mach/mach.h>
#include <mach/task_special_ports.h>
#include <servers/bootstrap.h>
int main(void) {
    setvbuf(stdout,NULL,_IONBF,0); alarm(8);
    mach_port_t inherited=MACH_PORT_NULL,named=MACH_PORT_NULL;
    kern_return_t first=task_get_special_port(mach_task_self(),TASK_ACCESS_PORT,&inherited);
    kern_return_t second=bootstrap_look_up(bootstrap_port,"com.apple.taskgated",&named);
    mach_port_type_t inherited_type=0,named_type=0;
    kern_return_t first_type=KERN_INVALID_NAME,second_type=KERN_INVALID_NAME;
    if(MACH_PORT_VALID(inherited)) first_type=mach_port_type(mach_task_self(),inherited,&inherited_type);
    if(MACH_PORT_VALID(named)) second_type=mach_port_type(mach_task_self(),named,&named_type);
    int valid=first==KERN_SUCCESS&&second==KERN_SUCCESS&&MACH_PORT_VALID(inherited)&&MACH_PORT_VALID(named);
    printf("TASK-ACCESS-IDENTITY inherited_result=%d named_result=%d inherited_present=%d named_present=%d same_endpoint=%d\n",
           first,second,MACH_PORT_VALID(inherited),MACH_PORT_VALID(named),valid&&inherited==named);
    printf("TASK-ACCESS-TYPES inherited_result=%d named_result=%d inherited_send=%d inherited_receive=%d named_send=%d named_receive=%d\n",
           first_type,second_type,!!(inherited_type&MACH_PORT_TYPE_SEND),!!(inherited_type&MACH_PORT_TYPE_RECEIVE),
           !!(named_type&MACH_PORT_TYPE_SEND),!!(named_type&MACH_PORT_TYPE_RECEIVE));
    if(MACH_PORT_VALID(inherited)&&mach_port_deallocate(mach_task_self(),inherited)!=KERN_SUCCESS) return 77;
    if(MACH_PORT_VALID(named)&&mach_port_deallocate(mach_task_self(),named)!=KERN_SUCCESS) return 77;
    puts("TASK-ACCESS-PROVENANCE-DIAGNOSTIC-COMPLETE service_rpc_sent=0 full_sandbox_tested=0"); return 0;
}
C
/usr/bin/xcrun clang -Wall -Wextra -Werror -Wno-deprecated-declarations "$probe_root/identity.c" -o "$probe_root/identity"
/usr/bin/sw_vers
/usr/bin/uname -mrv
/usr/bin/shasum -a 256 "$probe_root/identity.c" "$probe_root/identity"
/usr/bin/env -i PATH=/usr/bin:/bin "$probe_root/identity" < /dev/null
