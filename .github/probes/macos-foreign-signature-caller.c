/* One fixed cross-UID signature request; never a production sandbox. */
#include <bsm/libbsm.h>
#include <errno.h>
#include <libproc.h>
#include <limits.h>
#include <mach/mach.h>
#include <mach/ndr.h>
#include <mach/task_special_ports.h>
#include <sandbox.h>
#include <servers/bootstrap.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <unistd.h>
#include "nonce.h"
#if defined(_DARWIN_C_SOURCE) || defined(_DARWIN_UNLIMITED_GETGROUPS)
#error The diagnostic requires the ordinary kernel getgroups binding
#endif
typedef struct { mach_msg_header_t header; NDR_record_t ndr; int32_t pid; } Request;
typedef struct { mach_msg_header_t header; NDR_record_t ndr; kern_return_t status; } Reply;
static int foreign_task(pid_t target,const char *stage) {
    mach_port_t task=MACH_PORT_NULL;
    kern_return_t result=task_for_pid(mach_task_self(),target,&task);
    printf("FOREIGN-TASK stage=%s result=%d right=%d\n",stage,result,MACH_PORT_VALID(task));
    int valid=result!=KERN_SUCCESS&&!MACH_PORT_VALID(task);
    if(MACH_PORT_VALID(task)&&mach_port_deallocate(mach_task_self(),task)) valid=0;
    return valid?0:79;
}
int main(int argc,char **argv) {
    setvbuf(stdout,NULL,_IONBF,0);alarm(8);
    if(argc!=2) return 77;
    char *end=NULL;errno=0;long value=strtol(argv[1],&end,10);
    if(errno||!end||*end||value<=1||value>INT_MAX||value==getpid()) return 77;
    pid_t target=(pid_t)value;
    struct rlimit core={0,0},files={8192,8192};
    if(setrlimit(RLIMIT_CORE,&core)||setrlimit(RLIMIT_FSIZE,&files)) return 77;
    struct proc_bsdshortinfo info;gid_t groups[8];
    if(proc_pidinfo(getpid(),PROC_PIDT_SHORTBSDINFO,0,&info,sizeof info)!=(int)sizeof info) return 77;
    if(!getuid()||getuid()!=geteuid()||info.pbsi_svuid!=getuid()||getgid()!=getegid()||info.pbsi_svgid!=getgid()) return 77;
    if(getgroups(8,groups)!=1||groups[0]!=getgid()) return 77;
    errno=0;if(setuid(0)!=-1||errno!=EPERM) return 77;
    printf("CALLER nonce=%s uid=%u gid=%u permanent_nonroot=1\n",PROBE_NONCE,(unsigned)getuid(),(unsigned)getgid());
    mach_port_t endpoint=MACH_PORT_NULL,named=MACH_PORT_NULL,reply=MACH_PORT_NULL;int outcome=77;
    if(task_get_special_port(mach_task_self(),TASK_ACCESS_PORT,&endpoint)||!MACH_PORT_VALID(endpoint)) goto done;
    if(bootstrap_look_up(bootstrap_port,"com.apple.taskgated",&named)||endpoint!=named) goto done;
    char *diagnostic=NULL;
    /* Seatbelt is the selected diagnostic mechanism; its init/error-release pair is deprecated. */
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
    int applied=sandbox_init("(version 1)(deny default)(allow process-info* (target self))",0,&diagnostic);
    if(applied) {
        fprintf(stderr,"PROFILE-UNAVAILABLE %.256s\n",diagnostic?diagnostic:"unknown");
        if(diagnostic) sandbox_free_error(diagnostic);
        goto done;
    }
#pragma clang diagnostic pop
    puts("CALLER-PROFILE default_deny=1 inherited_endpoint_matches_bootstrap=1");
    if(foreign_task(target,"before")) goto done;
    if(mach_port_allocate(mach_task_self(),MACH_PORT_RIGHT_RECEIVE,&reply)) goto done;
    Request request={0};request.header.msgh_bits=MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND,MACH_MSG_TYPE_MAKE_SEND_ONCE);
    request.header.msgh_remote_port=endpoint;request.header.msgh_local_port=reply;request.header.msgh_size=sizeof request;
    request.header.msgh_id=27001;request.ndr=NDR_record;request.pid=target;
    mach_msg_return_t sent=mach_msg(&request.header,MACH_SEND_MSG|MACH_SEND_TIMEOUT|MACH_SEND_FILTER_NONFATAL,sizeof request,0,MACH_PORT_NULL,200,MACH_PORT_NULL);
    printf("FOREIGN-RPC send=%u\n",(unsigned)sent);if(sent) goto done;
    union { max_align_t aligned; mach_msg_header_t header; unsigned char bytes[1024]; } buffer={0};
    mach_msg_return_t received=mach_msg(&buffer.header,MACH_RCV_MSG|MACH_RCV_TIMEOUT|MACH_RCV_TRAILER_TYPE(MACH_MSG_TRAILER_FORMAT_0)|MACH_RCV_TRAILER_ELEMENTS(MACH_RCV_TRAILER_AUDIT),0,sizeof buffer,reply,500,MACH_PORT_NULL);
    printf("FOREIGN-RPC receive=%u\n",(unsigned)received);if(received) goto done;
    if((buffer.header.msgh_bits&MACH_MSGH_BITS_COMPLEX)||buffer.header.msgh_id!=27101||buffer.header.msgh_size!=sizeof(Reply)) {mach_msg_destroy(&buffer.header);goto done;}
    Reply response;memcpy(&response,buffer.bytes,sizeof response);if(response.ndr.int_rep!=NDR_record.int_rep) goto done;
    size_t offset=round_msg(response.header.msgh_size);
    if(offset>sizeof buffer-sizeof(mach_msg_audit_trailer_t)) goto done;
    mach_msg_audit_trailer_t trailer;memcpy(&trailer,buffer.bytes+offset,sizeof trailer);
    if(trailer.msgh_trailer_type!=MACH_MSG_TRAILER_FORMAT_0||trailer.msgh_trailer_size<sizeof trailer||trailer.msgh_trailer_size>sizeof buffer-offset) goto done;
    if(audit_token_to_euid(trailer.msgh_audit)!=0||audit_token_to_pid(trailer.msgh_audit)<=1) goto done;
    printf("FOREIGN-RPC service_status=%d audit_root_sender=1\n",response.status);
    if(foreign_task(target,"after")) goto done;
    outcome=0;
 done:
    if(MACH_PORT_VALID(reply)&&mach_port_mod_refs(mach_task_self(),reply,MACH_PORT_RIGHT_RECEIVE,-1)) outcome=77;
    if(MACH_PORT_VALID(named)&&mach_port_deallocate(mach_task_self(),named)) outcome=77;
    if(MACH_PORT_VALID(endpoint)&&mach_port_deallocate(mach_task_self(),endpoint)) outcome=77;
    printf("FOREIGN-CALLER-END status=%d full_backend=0\n",outcome);return outcome;
}
