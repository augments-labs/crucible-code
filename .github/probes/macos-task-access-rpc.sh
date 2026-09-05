#!/bin/sh
# Disposable own-process RPC classification; not a sandbox backend or full audit.
set -eu
test "$(uname -s)" = Darwin
test "$(id -u)" != 0
probe_root=$(mktemp -d /tmp/crucible-task-rpc.XXXXXX)
probe_root=$(cd "$probe_root" && pwd -P)
cat > "$probe_root/rpc.c" <<'C'
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <mach/mach.h>
#include <mach/ndr.h>
#include <mach/task_special_ports.h>
#include <sandbox.h>
extern int csops(pid_t, unsigned int, void *, size_t);
typedef struct { mach_msg_header_t header; NDR_record_t ndr; int32_t words[4]; } Request;
typedef union { struct { mach_msg_header_t header; NDR_record_t ndr; kern_return_t status; } reply; unsigned char bytes[1024]; } Response;
static void signature(const char *stage) {
    uint32_t flags=0; int result=csops(getpid(),0,&flags,sizeof flags);
    printf("OWN-SIGNATURE stage=%s result=%d flags=%u\n",stage,result,flags);
}
static int invoke(unsigned rpc) {
    mach_port_t endpoint=MACH_PORT_NULL,reply=MACH_PORT_NULL;
    kern_return_t acquired=task_get_special_port(mach_task_self(),TASK_ACCESS_PORT,&endpoint);
    if(acquired!=KERN_SUCCESS||!MACH_PORT_VALID(endpoint)) return 77;
    if(mach_port_allocate(mach_task_self(),MACH_PORT_RIGHT_RECEIVE,&reply)!=KERN_SUCCESS) {
        mach_port_deallocate(mach_task_self(),endpoint); return 77;
    }
    Request request={0}; request.header.msgh_bits=MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND,MACH_MSG_TYPE_MAKE_SEND_ONCE);
    request.header.msgh_remote_port=endpoint; request.header.msgh_local_port=reply;
    request.header.msgh_id=27000+(mach_msg_id_t)rpc; request.ndr=NDR_record;
    unsigned count;
    if(rpc==1) { request.words[0]=getpid(); count=1; }
    else { request.words[0]=getpid(); request.words[1]=(int32_t)getgid(); request.words[2]=getpid(); request.words[3]=TASK_FLAVOR_CONTROL; count=rpc==0?3:4; }
    request.header.msgh_size=(mach_msg_size_t)(offsetof(Request,words)+count*sizeof(int32_t));
    signature("before_rpc");
    mach_msg_return_t sent=mach_msg(&request.header,MACH_SEND_MSG|MACH_SEND_TIMEOUT|MACH_SEND_FILTER_NONFATAL,request.header.msgh_size,0,MACH_PORT_NULL,200,MACH_PORT_NULL);
    printf("TASK-RPC-SEND rpc=%u result=%u\n",rpc,(unsigned)sent);
    int outcome=0;
    if(sent==MACH_MSG_SUCCESS) {
        Response response={0};
        mach_msg_return_t received=mach_msg(&response.reply.header,MACH_RCV_MSG|MACH_RCV_TIMEOUT,0,sizeof response,reply,500,MACH_PORT_NULL);
        printf("TASK-RPC-RECEIVE rpc=%u result=%u\n",rpc,(unsigned)received);
        if(received!=MACH_MSG_SUCCESS) outcome=77;
        else if((response.reply.header.msgh_bits&MACH_MSGH_BITS_COMPLEX)||response.reply.header.msgh_size<sizeof response.reply||response.reply.header.msgh_size>sizeof response||response.reply.header.msgh_id!=27100+(mach_msg_id_t)rpc||response.reply.ndr.int_rep!=NDR_record.int_rep) {
            mach_msg_destroy(&response.reply.header); outcome=77;
        } else printf("TASK-RPC-SERVICE rpc=%u status=%d\n",rpc,response.reply.status);
    }
    signature("after_rpc");
    if(mach_port_destroy(mach_task_self(),reply)!=KERN_SUCCESS) outcome=77;
    if(mach_port_deallocate(mach_task_self(),endpoint)!=KERN_SUCCESS) outcome=77;
    printf("TASK-RPC-END rpc=%u status=%d full_effect_audit=0\n",rpc,outcome); return outcome;
}
int main(int argc,char **argv) {
    setvbuf(stdout,NULL,_IONBF,0); alarm(8);
    if(argc!=5) return 77;
    unsigned rpc=(unsigned)strtoul(argv[4],NULL,10); if(rpc>2) return 77;
    if(!strcmp(argv[1],"after")) return invoke(rpc);
    if(strcmp(argv[2],"unconfined")) {
        char profile[8192]; FILE *file=fopen(argv[3],"rb"); if(!file) return 77;
        size_t count=fread(profile,1,sizeof profile-1,file); int error=ferror(file),extra=fgetc(file),closed=fclose(file);
        if(error||extra!=EOF||closed||!count) return 77; profile[count]=0;
        char *diagnostic=NULL;
        if(sandbox_init(profile,0,&diagnostic)) {
            fprintf(stderr,"PROFILE-UNAVAILABLE %.256s\n",diagnostic?diagnostic:"unknown");
            if(diagnostic) sandbox_free_error(diagnostic); return 78;
        }
    }
    char *next[]={argv[0],"after",argv[2],argv[3],argv[4],NULL}; execv(argv[0],next); return 77;
}
C
python3 - "$probe_root" <<'PY'
import json, pathlib, sys
root = pathlib.Path(sys.argv[1]).resolve(strict=True)
base = ['(version 1)', '(deny default)', '(allow process-exec)', '(allow process-info* (target self))',
        '(allow file-read-data (literal "/"))',
        '(allow file-read-metadata (literal "/") (literal "/usr") (literal "/System") (literal "/private") (literal "/private/tmp"))',
        '(allow file-read* (subpath ' + json.dumps(str(root)) + ') (subpath "/usr/lib") (subpath "/System/Library"))']
for variant in ('default-deny','deny-send'):
    lines = base + (['(deny mach-message-send (global-name "com.apple.taskgated"))'] if variant == 'deny-send' else [])
    profile = '\n'.join(lines) + '\n'
    if len(profile.encode()) >= 8192:
        raise RuntimeError('profile bound')
    (root / (variant + '.sb')).write_text(profile)
PY
/usr/bin/xcrun clang -Wall -Wextra -Werror -Wno-deprecated-declarations "$probe_root/rpc.c" -o "$probe_root/rpc"
/usr/bin/sw_vers
/usr/bin/uname -mrv
/usr/bin/shasum -a 256 "$probe_root/rpc.c" "$probe_root/rpc" "$probe_root/default-deny.sb" "$probe_root/deny-send.sb"
for variant in unconfined default-deny deny-send; do
    for rpc in 0 1 2; do
        printf 'TASK-RPC-VARIANT-BEGIN variant=%s rpc=%s\n' "$variant" "$rpc"
        if /usr/bin/env -i PATH=/usr/bin:/bin "$probe_root/rpc" before "$variant" "$probe_root/$variant.sb" "$rpc" < /dev/null; then result=0; else result=$?; fi
        printf 'TASK-RPC-VARIANT-END variant=%s rpc=%s status=%s\n' "$variant" "$rpc" "$result"
        test "$result" -eq 0 || test "$result" -eq 78
    done
 done
