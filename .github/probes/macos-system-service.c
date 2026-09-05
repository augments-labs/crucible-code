/* Disposable direct-system-launchd endpoint identity diagnostic. */
#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
#include <sys/resource.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>
#include <limits.h>
#include <mach/mach.h>
#include <mach/ndr.h>
#include <mach/task_special_ports.h>
#include <servers/bootstrap.h>
#include <bsm/libbsm.h>
#include <Security/Security.h>

typedef struct { mach_msg_header_t header; NDR_record_t ndr; int32_t pid; } Request;
typedef struct { mach_msg_header_t header; NDR_record_t ndr; kern_return_t status; } Reply;
typedef union { max_align_t aligned; mach_msg_header_t header; unsigned char bytes[1024]; } Buffer;
static int authenticate(audit_token_t token) {
    CFDataRef data=NULL; CFDictionaryRef attributes=NULL; SecCodeRef code=NULL;
    SecRequirementRef requirement=NULL; CFURLRef url=NULL; int valid=0;
    if(audit_token_to_euid(token)!=0||audit_token_to_pid(token)<=1) return 77;
    data=CFDataCreate(NULL,(const UInt8 *)&token,sizeof token); if(!data) goto done;
    const void *keys[]={kSecGuestAttributeAudit},*values[]={data};
    attributes=CFDictionaryCreate(NULL,keys,values,1,&kCFTypeDictionaryKeyCallBacks,&kCFTypeDictionaryValueCallBacks);
    if(!attributes) goto done;
    OSStatus copied=SecCodeCopyGuestWithAttributes(NULL,attributes,kSecCSDefaultFlags,&code);
    printf("SERVICE-CODE audit_bound_copy_status=%d\n",(int)copied); if(copied) goto done;
    if(SecRequirementCreateWithString(CFSTR("anchor apple"),kSecCSDefaultFlags,&requirement)) goto done;
    OSStatus checked=SecCodeCheckValidity(code,kSecCSDefaultFlags,requirement);
    printf("SERVICE-CODE apple_dynamic_validity_status=%d\n",(int)checked); if(checked) goto done;
    if(SecCodeCopyPath(code,kSecCSDefaultFlags,&url)) goto done;
    UInt8 path[PATH_MAX];
    if(!CFURLGetFileSystemRepresentation(url,true,path,sizeof path)) goto done;
    valid=!strcmp((const char *)path,"/usr/libexec/taskgated");
    printf("SERVICE-CODE fixed_path_matches=%d root_sender=1 audit_token_bound=1\n",valid);
 done:
    if(url) CFRelease(url);if(requirement) CFRelease(requirement);if(code) CFRelease(code);
    if(attributes) CFRelease(attributes);if(data) CFRelease(data);return valid?0:77;
}
int main(void) {
    setvbuf(stdout,NULL,_IONBF,0);alarm(8);
    struct rlimit files={8192,8192},core={0,0};
    if(setrlimit(RLIMIT_FSIZE,&files)||setrlimit(RLIMIT_CORE,&core)) return 77;
    if(getuid()!=0||geteuid()!=0||getppid()!=1) return 77;
    puts("SYSTEM-LAUNCH uid=0 euid=0 parent=1");
    mach_port_t endpoint=MACH_PORT_NULL,named=MACH_PORT_NULL,reply=MACH_PORT_NULL;int outcome=77;
    if(task_get_special_port(mach_task_self(),TASK_ACCESS_PORT,&endpoint)||!MACH_PORT_VALID(endpoint)) goto done;
    if(bootstrap_look_up(bootstrap_port,"com.apple.taskgated",&named)||!MACH_PORT_VALID(named)||endpoint!=named) goto done;
    puts("SYSTEM-ENDPOINT inherited_matches_trusted_bootstrap=1");
    if(mach_port_allocate(mach_task_self(),MACH_PORT_RIGHT_RECEIVE,&reply)) goto done;
    Request request={0};request.header.msgh_bits=MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND,MACH_MSG_TYPE_MAKE_SEND_ONCE);
    request.header.msgh_remote_port=endpoint;request.header.msgh_local_port=reply;
    request.header.msgh_id=27001;request.header.msgh_size=sizeof request;request.ndr=NDR_record;request.pid=getpid();
    mach_msg_return_t sent=mach_msg(&request.header,MACH_SEND_MSG|MACH_SEND_TIMEOUT|MACH_SEND_FILTER_NONFATAL,sizeof request,0,MACH_PORT_NULL,200,MACH_PORT_NULL);
    printf("SYSTEM-RPC send=%u own_pid_only=1\n",(unsigned)sent);if(sent) goto done;
    Buffer buffer={0};
    mach_msg_return_t received=mach_msg(&buffer.header,MACH_RCV_MSG|MACH_RCV_TIMEOUT|MACH_RCV_TRAILER_TYPE(MACH_MSG_TRAILER_FORMAT_0)|MACH_RCV_TRAILER_ELEMENTS(MACH_RCV_TRAILER_AUDIT),0,sizeof buffer,reply,500,MACH_PORT_NULL);
    printf("SYSTEM-RPC receive=%u\n",(unsigned)received);if(received) goto done;
    if((buffer.header.msgh_bits&MACH_MSGH_BITS_COMPLEX)||buffer.header.msgh_id!=27101||buffer.header.msgh_size!=sizeof(Reply)) {mach_msg_destroy(&buffer.header);goto done;}
    Reply response;memcpy(&response,buffer.bytes,sizeof response);
    if(response.ndr.int_rep!=NDR_record.int_rep) goto done;
    size_t offset=round_msg(response.header.msgh_size);
    if(offset>sizeof buffer-sizeof(mach_msg_audit_trailer_t)) goto done;
    mach_msg_audit_trailer_t trailer;memcpy(&trailer,buffer.bytes+offset,sizeof trailer);
    if(trailer.msgh_trailer_type!=MACH_MSG_TRAILER_FORMAT_0||trailer.msgh_trailer_size<sizeof trailer||trailer.msgh_trailer_size>sizeof buffer-offset) goto done;
    printf("SYSTEM-RPC service_status=%d audit_trailer_valid=1\n",response.status);
    if(response.status) goto done;
    outcome=authenticate(trailer.msgh_audit);
 done:
    if(MACH_PORT_VALID(reply)&&mach_port_destroy(mach_task_self(),reply)) outcome=77;
    if(MACH_PORT_VALID(named)&&mach_port_deallocate(mach_task_self(),named)) outcome=77;
    if(MACH_PORT_VALID(endpoint)&&mach_port_deallocate(mach_task_self(),endpoint)) outcome=77;
    printf("SYSTEM-SERVICE-IDENTITY-END status=%d full_backend=0\n",outcome);return outcome;
}
